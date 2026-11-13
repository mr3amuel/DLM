import AppKit
import SwiftUI

@MainActor
final class DownloadModel: ObservableObject {
    @Published private(set) var rows: [Transfer] = []
    @Published private(set) var ready = false
    @Published private(set) var busy = false
    @Published var error: String?
    @Published var adding = false
    @Published private(set) var standaloneAdding = false
    enum AddRoute: Equatable { case main, standalone, unavailable }
    func requestMenuBarAdd(mainVisible: Bool) -> AddRoute {
        guard ready, !busy, !adding, error == nil else { return .unavailable }
        standaloneAdding = !mainVisible
        adding = true
        return mainVisible ? .main : .standalone
    }
    func closeStandaloneAdd() {
        guard standaloneAdding else { return }
        adding = false
        standaloneAdding = false
        browserReviewClosed()
    }
    @Published var menuBarSection: String?
    @Published private(set) var requestedDownloadID: String?
    private var sourceInspection: Task<SourceMetadata, Error>?

    func locateExistingDownload(_ id: String) async throws {
        try await snapshot()
        guard !id.isEmpty, rows.contains(where: { $0.id == id }) else {
            throw EngineFailure(code: "not_found", message: "This download record is no longer available.")
        }
        selection = id
        requestedDownloadID = id
    }

    func finishLocatingDownload() { requestedDownloadID = nil }

    func inspectSource(url: String, filename: String) async throws -> SourceMetadata {
        // At most one bounded HTTP inspection. Superseded callers never queue more HEADs.
        while let existing = sourceInspection {
            _ = await existing.result
            try Task.checkCancellation()
        }
        guard ready, !closing else { throw EngineFailure(code: "closed", message: "The engine is closed.") }
        let task = Task<SourceMetadata, Error> {
            defer { sourceInspection = nil }
            guard let result = try await bridge.call(
                EngineCommand(operation: .inspectSource, filename: filename, url: url), as: SourceMetadata.self
            ) else { throw EngineFailure(code: "internal", message: "The engine returned no result.") }
            return result
        }
        sourceInspection = task
        return try await task.value
    }
    var draftURL = ""
    var draftFilename = ""
    private var desktopServices = false
    private var browserEpoch = 0
    @Published var browserSetupRequested = false
    private var browserReviewID: String?
    private var browserAcknowledgments: Set<String> = []
    @Published private(set) var browserState: NativeBrowserState?
    @Published private(set) var browserDraft: NativeBrowserDraft?
    @Published private(set) var browserError: String?

    func configureBrowser(_ enabled: Bool) async throws {
        browserEpoch += 1
        _ = try await bridge.call(EngineCommand(operation: .browserEnable, enabled: enabled), as: Bool.self)
        if !enabled { browserDraft = nil }
        do { try await refreshBrowser() } catch { browserError = error.localizedDescription }
    }

    func resetBrowserPairing() async throws {
        browserEpoch += 1
        _ = try await bridge.call(EngineCommand(operation: .browserReset), as: Bool.self)
        browserDraft = nil
    }

    func browserPairingCode() async throws -> String {
        guard let code = try await bridge.call(EngineCommand(operation: .browserPairingCode), as: String.self) else {
            throw EngineFailure(code: "unavailable", message: "Chrome pairing is unavailable.")
        }
        return code
    }

    func reviewBrowserDraft() {
        guard ready, !closing, !busy, !adding, let browserDraft else { return }
        draftURL = browserDraft.url
        draftFilename = browserDraft.filename ?? ""
        browserReviewID = browserDraft.id
        adding = true
    }

    func browserReviewClosed() {
        browserReviewID = nil
        draftURL = ""
        draftFilename = ""
    }

    func finishBrowserReview() async {
        guard let id = browserReviewID else { return }
        await dismissBrowserDraft(id: id)
    }

    func dismissBrowserDraft(id: String) async {
        // Ack failures are observation/inbox failures, never a failed download mutation.
        browserAcknowledgments.insert(id)
        if browserDraft?.id == id { browserDraft = nil }
        await refreshBrowserDraft()
    }

    func refreshBrowserDraft() async {
        let epoch = browserEpoch
        do {
            try await refreshBrowser()
            for id in browserAcknowledgments.sorted() {
                _ = try await bridge.call(EngineCommand(operation: .browserAcknowledge, id: id), as: Bool.self)
                browserAcknowledgments.remove(id)
            }
            guard browserState?.enabled == true, browserDraft == nil, !adding, !closing else { return }
            let draft = try await bridge.call(EngineCommand(operation: .browserTake), as: NativeBrowserDraft.self)
            if epoch == browserEpoch, browserState?.enabled == true, !closing {
                browserDraft = draft
            }
        } catch { browserError = error.localizedDescription }
    }

    private func refreshBrowser() async throws {
        browserState = try await bridge.call(EngineCommand(operation: .browserState), as: NativeBrowserState.self)
        browserError = nil
    }
    @Published var selectedIDs: Set<String> = []
    @Published private(set) var organization: Organization?
    @Published private(set) var schedules: ScheduleSnapshot?
    @Published var missedReviewRequest: UUID?

    func saveSchedule(_ input: ScheduleInput) async throws -> String {
        let id = try await require(EngineCommand(operation: .saveSchedule, schedule: input), as: String.self)
        // Committed save remains successful even if observation is temporarily unavailable.
        do { try await refreshSchedules() } catch { observationFailed(error) }
        return id
    }

    func dismissMissedSchedules(_ occurrences: [MissedOccurrence]) async {
        guard !occurrences.isEmpty else { return }
        await changeSchedule(EngineCommand(operation: .dismissMissed, occurrences: occurrences))
    }
    var selection: String? {
        get { selectedIDs.count == 1 ? selectedIDs.first : nil }
        set { selectedIDs = newValue.map { [$0] } ?? [] }
    }

    private var folderAccess: FolderAccess?
    @Published private(set) var folderAccessError: String?

    func chooseFolder() -> String? {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let selected = panel.url else { return nil }
        do {
            return try folderAccess?.remember(selected) ?? selected.path
        } catch {
            self.error = error.localizedDescription
            return nil
        }
    }

    private let bridge: EngineBridge
    // A cancellable clock seam keeps failure-path tests deterministic (no wall-clock waits).
    private let sleep: @Sendable (Duration) async throws -> Void
    private var observationFailures = 0
    private var reportedObservationErrors: Set<String> = []
    private var organizationNeedsRefresh = true
    private var shutdownTask: Task<ShutdownOutcome, Never>?

    init(bridge: EngineBridge = EngineBridge(),
         sleep: @escaping @Sendable (Duration) async throws -> Void = { try await Task.sleep(for: $0) }) {
        self.bridge = bridge
        self.sleep = sleep
    }
    private var polling: Task<Void, Never>?
    private var opening: Task<Void, Never>?
    private var closing = false

    var selected: Transfer? { rows.first { $0.id == selection } }

    func open(root: URL, desktopServices: Bool = false) async {
        guard opening == nil, !closing else { return }
        self.desktopServices = desktopServices
        let task = Task {
            do {
                if Bundle.main.object(forInfoDictionaryKey: "DLMScopedFolders") as? Bool == true {
                    do {
                        folderAccess = try FolderAccess(root: root)
                        try folderAccess?.restore()
                    } catch {
                        folderAccessError = error.localizedDescription
                        throw error
                    }
                }
                try await bridge.open(root: root, timezone: TimeZone.autoupdatingCurrent.identifier)
                if closing { return }
                // Opening succeeded. Observation failures must not strand an open engine.
                ready = true
                do {
                    try await snapshot()
                    try await refreshOrganization()
                    organizationNeedsRefresh = false
                    try await refreshSchedules()
                } catch { observationFailed(error) }
                if desktopServices {
                    do { try await refreshBrowser() }
                    catch { browserError = error.localizedDescription }
                }
                if !closing { polling = Task { await poll() } }
            } catch { self.error = error.localizedDescription }
        }
        opening = task
        await task.value
    }

    // Retry indefinitely, with a bounded 0.5–8 second delay. Only reads are retried.
    private var observationDelay: Duration {
        observationFailures == 0 ? .milliseconds(250)
            : .milliseconds(500 * (1 << min(observationFailures - 1, 4)))
    }

    private func observationFailed(_ failure: Error) {
        guard !closing, !(failure is CancellationError) else { return }
        observationFailures = min(observationFailures + 1, 5)
        let message = failure.localizedDescription
        // Keep last valid state; report each distinct cause only once per outage.
        // Do not replace an outstanding command error with an observation alert.
        if error == nil, reportedObservationErrors.insert(message).inserted {
            error = NativePreferences.text("Download updates are temporarily unavailable. DLM will retry automatically.")
                + "\n\n" + message
        }
    }

    private func poll() async {
        var ticks = 0
        while !Task.isCancelled, !closing {
            do {
                try await sleep(observationDelay)
                try Task.checkCancellation()
                guard !closing else { return }
                let recovering = observationFailures > 0
                try await snapshot()
                ticks += 1
                if organizationNeedsRefresh {
                    try await refreshOrganization()
                    organizationNeedsRefresh = false
                }
                if recovering || ticks % 4 == 0 {
                    try await refreshSchedules()
                    if desktopServices { await refreshBrowserDraft() }
                }
                guard !closing else { return }
                observationFailures = 0
                reportedObservationErrors.removeAll()
            } catch is CancellationError {
                return
            } catch {
                observationFailed(error)
            }
        }
    }

    private func snapshot() async throws {
        let trace = LatencyTrace.shared
        let start = trace.enabled ? LatencyTrace.now() : 0
        let changed = try await bridge.call(EngineCommand(operation: .snapshot), as: [Transfer].self)
        if trace.enabled { trace.record("snapshot_round_trip_ms", since: start) }
        trace.measure("publish_rows_ms") {
            if let changed, changed != rows {
                if desktopServices, ready, UserDefaults.standard.bool(forKey: NativePreferences.notificationsKey) {
                    let previous = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0.status) })
                    for row in changed where previous[row.id] != nil && previous[row.id] != row.status {
                        if row.status == .completed {
                            NativePreferences.notify("A download is complete.", identifier: "complete-\(row.id)")
                        } else if row.status == .failed {
                            NativePreferences.notify("A download needs attention.", identifier: "failed-\(row.id)")
                        }
                    }
                }
                rows = changed
            }
        }
    }

    func add(url: String, filename: String, connections: UInt32 = 4) async throws -> String {
        try await add(Draft(url: url, filename: filename, connections: connections, optimize: connections > 1))
    }

    func add(_ draft: Draft, separateCopy: Bool = false, schedule: ScheduleInput? = nil) async throws -> String {
        let id = try await require(EngineCommand(operation: .add, draft: draft, separateCopy: separateCopy, schedule: schedule), as: String.self)
        selection = id
        return id
    }

    func inspect(_ draft: Draft) async throws -> DraftReview {
        guard let result = try await bridge.call(EngineCommand(operation: .inspectDraft, draft: draft), as: DraftReview.self)
        else { throw EngineFailure(code: "internal", message: "The engine returned no result.") }
        return result
    }

    func action(_ operation: EngineOperation, id: String) async {
        await action(EngineCommand(operation: operation, id: id))
    }

    func action(_ command: EngineCommand) async {
        do { _ = try await execute(command, as: String.self) }
        catch is CancellationError {}
        catch { self.error = error.localizedDescription }
    }

    func move(id: String, directory: String) async {
        do {
            let kept = try await require(EngineCommand(operation: .move, id: id, directory: directory), as: Bool.self)
            if kept { error = "The download moved. The original file was kept." }
        } catch { self.error = error.localizedDescription }
    }

    func batch(_ operation: BatchOperation, ids: Set<String>, category: String? = nil, confirmed: Bool = false) async {
        do {
            let labels = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0.filename) })
            let results = try await require(EngineCommand(operation: .batch, ids: ids.sorted(),
                action: operation, category: category, allowUnsafePause: confirmed), as: [BatchResult].self)
            let failures = results.compactMap { row in row.error.map { "\(labels[row.id] ?? row.id): \($0)" } }
            if !failures.isEmpty { error = failures.joined(separator: "\n") }
        } catch { self.error = error.localizedDescription }
    }

    func refreshSchedules() async throws {
        let update = try await bridge.call(EngineCommand(operation: .schedules), as: ScheduleSnapshot.self)
        if schedules != update { schedules = update }
        if desktopServices { NativePreferences.notifyMissed(update?.missedDownloads ?? []) }
    }

    func changeSchedule(_ command: EngineCommand) async {
        do {
            try await apply(command)
            do { try await refreshSchedules() }
            catch { observationFailed(error) }
        } catch { self.error = error.localizedDescription }
    }

    func refreshOrganization() async throws {
        organization = try await bridge.call(EngineCommand(operation: .organization), as: Organization.self)
    }

    func changeOrganization(_ command: EngineCommand) async {
        do {
            _ = try await execute(command, as: String.self)
            organizationNeedsRefresh = true
            do {
                try await refreshOrganization()
                organizationNeedsRefresh = false
            } catch { observationFailed(error) }
        } catch { self.error = error.localizedDescription }
    }

    func apply(_ command: EngineCommand) async throws {
        _ = try await execute(command, as: String.self)
    }

    func require<T: Decodable>(_ command: EngineCommand, as type: T.Type) async throws -> T {
        guard let result = try await execute(command, as: type) else {
            throw EngineFailure(code: "internal", message: "The engine returned no result.")
        }
        return result
    }

    func timezoneChanged() async {
        guard ready, !closing else { return }
        do {
            _ = try await bridge.call(
                EngineCommand(operation: .timezone, timezone: TimeZone.autoupdatingCurrent.identifier), as: String.self
            )
        } catch { self.error = error.localizedDescription }
    }

    private func execute<T: Decodable>(_ command: EngineCommand, as type: T.Type) async throws -> T? {
        guard ready, !busy, !closing else {
            throw EngineFailure(code: "busy", message: "Wait for the current download command to finish.")
        }
        busy = true
        defer { busy = false }
        let result = try await bridge.call(command, as: type)
        if !closing {
            do { try await snapshot() }
            catch { observationFailed(error) }
        }
        // A failed read cannot undo or change the result of a committed mutation.
        return result
    }

    @discardableResult
    func shutdown() async -> ShutdownOutcome {
        if let shutdownTask { return await shutdownTask.value }
        closing = true
        ready = false
        polling?.cancel()
        let task = Task<ShutdownOutcome, Never> {
            await opening?.value
            // Drain the cancelled reader before consuming the serial bridge handle.
            await polling?.value
            defer { folderAccess?.release() }
            do {
                try await bridge.close()
                return .closed
            } catch {
                // dlm_close consumes ownership even on error. No reopen or retry is safe here.
                return .failedConsumed(error.localizedDescription)
            }
        }
        shutdownTask = task
        return await task.value
    }
}

struct NativeBrowserState: Decodable, Sendable {
    let enabled: Bool
    let directory: String
}

struct NativeBrowserDraft: Decodable, Sendable {
    let id: String
    let url: String
    let filename: String?
}

// A failed close is terminal for this session, not a cancellable pause.
enum ShutdownOutcome: Equatable, Sendable {
    case closed
    case failedConsumed(String)
}
