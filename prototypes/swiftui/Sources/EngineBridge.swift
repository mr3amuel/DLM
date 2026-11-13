import Foundation

struct EngineFailure: LocalizedError, Decodable, Sendable {
    let code: String
    let message: String
    var errorDescription: String? { message }
}

struct EngineReply<T: Decodable>: Decodable {
    let data: T?
    let error: EngineFailure?
}

// The serial queue is the sole owner of the handle. Callers await Data, never pointers.
final class EngineBridge: @unchecked Sendable {
    private let queue = DispatchQueue(label: "com.dlm.swift-experiment.engine", qos: .userInitiated)
    private var handle: OpaquePointer?
    private var closeResponse: Result<Data, Error>?

    func open(root: URL, timezone: String) async throws {
        guard dlm_abi_version() == 1 else {
            throw EngineFailure(code: "incompatible", message: "The native engine interface is incompatible.")
        }
        let input = try JSONEncoder().encode(OpenRequest(root: root.path, timezone: timezone))
        let response = try await perform {
            guard self.closeResponse == nil else {
                throw EngineFailure(code: "closed", message: "The engine session has been closed. Restart DLM to open a new session.")
            }
            guard self.handle == nil else { throw EngineFailure(code: "busy", message: "The engine is already open.") }
            return try input.withUnsafeBytes { buffer in
                try self.take(dlm_open(buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count, &self.handle))
            }
        }
        _ = try decode(response, as: String.self)
    }

    func call<T: Decodable>(_ command: EngineCommand, as type: T.Type) async throws -> T? {
        try Task.checkCancellation()
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let input = try encoder.encode(command)
        let trace = LatencyTrace.shared
        let issued = trace.enabled ? LatencyTrace.now() : 0
        let span = switch command.operation {
        case .snapshot: "ffi_transfer_ms"
        case .schedules: "ffi_schedules_ms"
        default: "ffi_command_ms"
        }
        let response = try await perform {
            if trace.enabled { trace.record("bridge_queue_ms", since: issued) }
            guard let handle = self.handle else { throw EngineFailure(code: "closed", message: "The engine is closed.") }
            return try trace.measure(span) {
                try input.withUnsafeBytes { buffer in
                    try self.take(dlm_call(handle, buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count))
                }
            }
        }
        return try trace.measure("swift_decode_ms") { try decode(response, as: type) }
    }

    func close() async throws {
        let response = try await perform {
            if let previous = self.closeResponse { return try previous.get() }
            let handle = self.handle
            self.handle = nil // dlm_close consumes this even if it fails or returns invalid JSON.
            let response = Result { try self.take(dlm_close(handle)) }
            self.closeResponse = response
            return try response.get()
        }
        _ = try decode(response, as: String.self)
    }

    private func decode<T: Decodable>(_ bytes: Data, as type: T.Type) throws -> T? {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let result = try decoder.decode(EngineReply<T>.self, from: bytes)
        if let error = result.error { throw error }
        return result.data
    }

    private func take(_ json: UnsafeMutablePointer<CChar>?) throws -> Data {
        guard let json else { throw EngineFailure(code: "internal", message: "The engine returned an empty response.") }
        defer { dlm_free_json(json) }
        return Data(bytes: json, count: strlen(json))
    }

    private func perform(_ work: @escaping @Sendable () throws -> Data) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                continuation.resume(with: Result(catching: work))
            }
        }
    }
}

private struct OpenRequest: Encodable {
    let root: String
    let timezone: String
}

enum EngineOperation: String, Encodable, Sendable {
    case browserState = "browser_state", browserEnable = "browser_enable"
    case browserPairingCode = "browser_pairing_code", browserReset = "browser_reset", browserTake = "browser_take", browserAcknowledge = "browser_acknowledge"
    case snapshot, add, start, pause, resume, cancel, retry, timezone, rename, move, priority
    case reorder, configure, organization, batch, schedules
    case saveSchedule = "save_schedule", enableSchedule = "enable_schedule"
    case deleteSchedule = "delete_schedule", rearmSchedule = "rearm_schedule"
    case dismissMissed = "dismiss_missed"
    case inspectDraft = "inspect_draft", speedLimit = "speed_limit", queueSettings = "queue_settings"
    case setRetry = "set_retry"
    case setQueue = "set_queue", globalLimit = "global_limit", setOrganization = "set_organization"
    case saveCategory = "save_category", deleteCategory = "delete_category"
    case verifyLink = "verify_link", applyLink = "apply_link"
    case restartCancelled = "restart_cancelled", inspectSource = "inspect_source", partialInfo = "partial_info"
}

enum TransferPriority: String, Codable, CaseIterable, Sendable {
    case low = "Low", normal = "Normal", high = "High"
}

enum BatchOperation: String, Encodable, Sendable {
    case pause, resume, cancel, remove, category
}

struct Draft: Encodable, Equatable, Sendable {
    var url: String
    var filename: String
    var directory: String? = nil
    var category: String? = nil
    var route = false
    var connections: UInt32 = 4
    var optimize = true
    var limit: UInt64? = nil
    var source: SourceMetadata? = nil

    var prepared: Draft {
        var result = self
        result.url = url.trimmingCharacters(in: .whitespacesAndNewlines)
        result.filename = filename.trimmingCharacters(in: .whitespacesAndNewlines)
        if result.filename.isEmpty {
            let name = URL(string: result.url)?.lastPathComponent ?? ""
            result.filename = name.isEmpty || name == "/" ? "download" : name
        }
        if result.source?.url != result.url { result.source = nil }
        return result
    }
}

struct SourceMetadata: Codable, Equatable, Sendable {
    let url: String
    let categoryId: String?
    let contentType: String?
    let total: UInt64?
    let etag: String?
}

struct DraftReview: Decodable, Sendable {
    struct Duplicate: Decodable, Sendable {
        let id: String
        let filename: String
        let destination: String
        let reasons: [String]
    }
    let directory: String
    let categoryId: String?
    let duplicates: [Duplicate]
}

struct Organization: Decodable, Sendable {
    struct Category: Identifiable, Decodable, Sendable {
        let id: String
        let name: String
        let directory: String?
        let builtin: Bool
    }
    let enabled: Bool
    let categories: [Category]
}

struct QueueConfiguration: Codable, Sendable {
    var maxDownloads: UInt32
    var globalConnections: UInt32
    var perHostConnections: UInt32
}

struct RetryConfiguration: Codable, Sendable {
    var maxAttempts: UInt32
    var initialDelayMs: UInt64
    var maxDelayMs: UInt64
    var jitterPercent: UInt32
}

struct QueueSettings: Decodable, Sendable {
    let retry: RetryConfiguration
    var queue: QueueConfiguration
    var limit: UInt64?
}

struct BatchResult: Decodable, Sendable {
    let id: String
    let error: String?
}

struct LinkVerification: Decodable, Sendable {
    enum Outcome: String, Decodable, CaseIterable, Sendable {
        case safeToResume = "safe_to_resume", incompatible, cannotVerify = "cannot_verify"
        case rangeUnsupported = "range_unsupported", temporaryFailure = "temporary_failure"
        var label: String {
            switch self {
            case .safeToResume: "Safe to Resume"
            case .incompatible: "Incompatible"
            case .cannotVerify: "Cannot Verify"
            case .rangeUnsupported: "Range Unsupported"
            case .temporaryFailure: "Temporary Failure"
            }
        }
    }
    let outcome: Outcome
    var safeToResume: Bool { outcome == .safeToResume }
    let reason: String
    let token: String?
    let comparedBytes: UInt64
}

struct PartialFileInfo: Decodable, Sendable {
    let exists: Bool
    let fileLength: UInt64?
    let retainedVerifiedBytes: UInt64
}

struct EngineCommand: Encodable, Sendable {
    let operation: EngineOperation
    var id: String?
    var ids: [String]?
    var draft: Draft?
    var separateCopy: Bool?
    var action: BatchOperation?
    var category: String?
    var allowUnsafePause: Bool?
    var directory: String?
    var filename: String?
    var name: String?
    var priority: TransferPriority?
    var queue: QueueConfiguration?
    var policy: RetryConfiguration?
    var connections: UInt32?
    var optimize: Bool?
    var limit: UInt64?
    var enabled: Bool?
    var timezone: String?
    var url: String?
    var token: String?
    var restart: Bool?
    var schedule: ScheduleInput?
    var occurrences: [MissedOccurrence]?
}

struct Transfer: Identifiable, Equatable, Decodable, Sendable {
    enum Resume: String, Decodable, Sendable {
        case unknown, supported, unsupported

        var label: String {
            switch self {
            case .unknown: "Resume availability unknown"
            case .supported: "Resume supported"
            case .unsupported: "Resume unavailable"
            }
        }
    }

    struct Worker: Identifiable, Equatable, Decodable, Sendable {
        let workerId: UInt64
        let transferGeneration: String
        let rangeStart: UInt64
        let rangeEnd: UInt64
        let writtenBytes: UInt64
        let phase: String
        let requestPermitHeld: Bool
        var id: String { "\(transferGeneration)-\(workerId)" }
        var fraction: Double {
            guard rangeEnd > rangeStart else { return 0 }
            return min(1, Double(writtenBytes) / Double(rangeEnd - rangeStart))
        }
    }
    enum Status: String, Decodable, CaseIterable, Sendable {
        case ready = "Ready", queued = "Queued", running = "Running", paused = "Paused"
        case cancelled = "Cancelled", finalizing = "Finalizing", completed = "Completed", failed = "Failed"

        var active: Bool { self == .queued || self == .running || self == .finalizing }
        var canPause: Bool { self == .running || self == .queued }
        var canResume: Bool { self == .paused || self == .ready || self == .failed }
        var canCancel: Bool { canPause || canResume }
    }

    let id: String
    let createdAtMs: UInt64?
    let contentType: String?
    let etag: String?
    let lastModified: String?
    let sha256: String?
    let resolvedUrl: String?
    let partialPath: String?
    let errorKind: String?
    let notice: String?
    let resumeReason: String?
    let filename: String
    let destination: String
    let status: Status
    let downloaded: UInt64
    let durableBytes: UInt64
    let total: UInt64?
    let speed: Double
    let url: String
    let etaSeconds: UInt64?
    let retryRemainingSeconds: UInt64?
    let resumeCapability: Resume
    let workers: [Worker]
    let priority: TransferPriority
    let categoryId: String?
    let queuePosition: UInt32?
    let connections: UInt32
    let optimize: Bool
    let bandwidthLimit: UInt64?
    let safePause: Bool
    let activeConnections: UInt32
    let targetConnections: UInt32
    let segmented: Bool
    let error: String?
    let scheduleState: String?

    var fraction: Double? {
        guard let total, total > 0 else { return nil }
        return min(1, Double(downloaded) / Double(total))
    }

    var byteSummary: String {
        let current = Self.bytes(downloaded)
        return total.map { NativePreferences.format("%@ of %@", current, Self.bytes($0)) } ?? current
    }

    var speedSummary: String {
        guard speed.isFinite, speed > 0 else { return "—" }
        return Self.bytes(UInt64(min(speed, Double(Int64.max / 2)))) + "/s"
    }

    var statusLabel: String {
        if status == .queued {
            return NativePreferences.format("Queued #%@", queuePosition.map { NativePreferences.number(UInt64($0)) } ?? "…")
        }
        if let retryRemainingSeconds, status == .running {
            return NativePreferences.format("Retry in %@ s", NativePreferences.number(retryRemainingSeconds))
        }
        return NativePreferences.text(status.rawValue)
    }

    var resumeExplanation: String? {
        let explanations = [
            "NotObserved": "Range support has not been verified for this resource.",
            "RevalidationRequired": "The resource must be checked again before continuing.",
            "ValidatedRange": "The engine verified byte ranges and a strong resource identity.",
            "RangeIgnored": "The server ignored an unconditional byte-range request.",
            "MissingStrongValidator": "The response lacks the strong identity required for safe resume.",
            "ConditionalFullResponse": "The server returned a full response to a conditional range; support is inconclusive.",
            "IdentityChanged": "The remote resource identity changed.",
            "InvalidRangeResponse": "The range response could not be safely validated.",
            "AuthenticationRequired": "Authentication is required before capability can be checked.",
            "UnknownTotal": "The range response did not establish a known resource size.",
        ]
        return resumeReason.flatMap { explanations[$0] }.map { NativePreferences.text($0) }
    }

    var errorKindLabel: String? {
        let labels = ["Network": "Network failure", "Http": "HTTP failure", "RateLimited": "Server rate limited",
            "Authentication": "Authentication or expired URL", "Disk": "Disk failure", "Storage": "Metadata failure",
            "Integrity": "Integrity failure", "Invalid": "Invalid request", "Internal": "Internal failure"]
        return errorKind.flatMap { labels[$0] }.map { NativePreferences.text($0) }
    }

    static func bytes(_ count: UInt64) -> String {
        let units = ["B", "KB", "MB", "GB", "TB", "PB", "EB"]
        var value = Double(count)
        var index = 0
        while value >= 1000 && index < units.count - 1 { value /= 1000; index += 1 }
        let number = value.formatted(.number.grouping(.never).precision(.fractionLength(0...(index < 3 ? 1 : 2)))
            .locale(NativePreferences.locale))
        return "\(number) \(units[index])"
    }
}
