import Foundation

@main
struct NativeUnitTests {
    static func main() throws {
        let examples: [(String, UInt64?)] = [
            ("", nil), ("۱۲٫۳۴۵", 12_345), ("٠.٠٠١", 1), (" 4 ", 4000),
            ("18446744073709551.615", UInt64.max),
        ]
        for (input, expected) in examples {
            let actual = try parseSpeedLimit(input)
            precondition(actual == expected, "Unexpected parsed value for \(input)")
            let roundTrip = try parseSpeedLimit(speedLimitText(actual))
            precondition(roundTrip == expected, "Speed limit failed to round-trip")
        }
        for input in ["0", "-1", "1.2345", "1e5", "12.", "18446744073709551.616"] {
            do {
                _ = try parseSpeedLimit(input)
                fatalError("Accepted an invalid speed limit: \(input)")
            } catch is EngineFailure {}
        }
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        let wire = try encoder.encode(EngineCommand(operation: .dismissMissed,
            occurrences: [MissedOccurrence(id: "schedule", timestamp: 123)]))
        let object = try JSONSerialization.jsonObject(with: wire) as? [String: Any]
        precondition(object?["operation"] as? String == "dismiss_missed")
        let occurrences = object?["occurrences"] as? [[Any]]
        precondition(occurrences?.first?.first as? String == "schedule")
        precondition(occurrences?.first?.last as? Int == 123)

        let disabled = LatencyTrace(mode: "")
        precondition(disabled.measure("sample") { 42 } == 42 && disabled.drain().isEmpty)
        let trace = LatencyTrace(mode: "test")
        DispatchQueue.concurrentPerform(iterations: 10_010) { _ in
            trace.record("sample", milliseconds: 1)
        }
        precondition(trace.drain()["sample"]?.count == 10_000)
        precondition(trace.drain().isEmpty)
        print("Native Swift unit checks passed")
    }
}
