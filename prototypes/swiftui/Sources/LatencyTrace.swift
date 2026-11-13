import Foundation

/// In-memory instrumentation. Enabled only for the disposable Release fixture.
final class LatencyTrace: @unchecked Sendable {
    static let shared = LatencyTrace(mode: "")
    let mode: String
    var enabled: Bool { !mode.isEmpty }
    private let lock = NSLock()
    private var values: [String: [Double]] = [:]

    init(mode: String) { self.mode = mode }

    static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    func record(_ label: String, since start: UInt64) {
        record(label, milliseconds: Double(Self.now() - start) / 1_000_000)
    }

    func record(_ label: String, milliseconds: Double) {
        guard enabled else { return }
        lock.lock()
        if values[label, default: []].count < 10_000 { values[label, default: []].append(milliseconds) }
        lock.unlock()
    }

    func drain() -> [String: [Double]] {
        lock.lock()
        defer { lock.unlock() }
        let result = values
        values.removeAll(keepingCapacity: true)
        return result
    }

    func measure<T>(_ label: String, _ operation: () throws -> T) rethrows -> T {
        guard enabled else { return try operation() }
        let start = Self.now()
        defer { record(label, since: start) }
        return try operation()
    }
}
