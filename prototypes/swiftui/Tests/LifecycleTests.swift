import Foundation

// Sleeps advance only when the test requests it. Cancellation always releases a sleeper.
private actor TestClock {
    private var sleepers: [UUID: CheckedContinuation<Void, Error>] = [:]
    private(set) var delays: [Duration] = []

    func sleep(_ delay: Duration) async throws {
        let id = UUID()
        try await withTaskCancellationHandler {
            try Task.checkCancellation()
            delays.append(delay)
            try await withCheckedThrowingContinuation { sleepers[id] = $0 }
        } onCancel: {
            Task { await self.cancel(id) }
        }
    }

    private func cancel(_ id: UUID) {
        sleepers.removeValue(forKey: id)?.resume(throwing: CancellationError())
    }

    func advance() {
        precondition(sleepers.count == 1, "Missing or duplicate observation task")
        let id = sleepers.keys.first!
        sleepers.removeValue(forKey: id)?.resume()
    }

    func waitForSleeps(_ count: Int) async {
        for _ in 0..<100_000 {
            if delays.count >= count { return }
            await Task.yield()
        }
        fatalError("Observation did not reach the expected sleep")
    }
}

@main
struct LifecycleTests {
    @MainActor
    static func main() async throws {
        // Initial observation failure must not strand an otherwise open engine.
        lifecycle_reset()
        lifecycle_fail(Int32(TEST_SNAPSHOT), 1)
        let clock = TestClock()
        let model = DownloadModel(sleep: { try await clock.sleep($0) })
        await model.open(root: URL(fileURLWithPath: "/unused-fixture"))
        precondition(model.ready && model.error?.hasSuffix("Injected lifecycle failure") == true)
        await clock.waitForSleeps(1)
        let initialDelays = await clock.delays
        precondition(initialDelays == [.milliseconds(500)])
        await clock.advance()
        await clock.waitForSleeps(2)
        precondition(model.organization != nil && model.schedules != nil)
        precondition(lifecycle_count(Int32(TEST_SNAPSHOT)) == 2)

        // Scheduler failure retries with a capped backoff, preserves state and deduplicates alerts.
        model.error = nil
        lifecycle_fail(Int32(TEST_SCHEDULES), 8)
        // Tick 1 recovered above; ticks 2, 3 and 4 reach the normal scheduler read.
        for count in 3...5 {
            await clock.advance()
            await clock.waitForSleeps(count)
        }
        precondition(model.error?.hasSuffix("Injected lifecycle failure") == true)
        let previous = model.schedules
        let previousRows = model.rows
        precondition(previousRows.first?.downloaded == 123)
        model.error = nil
        for count in 6...12 {
            await clock.advance()
            await clock.waitForSleeps(count)
            precondition(model.error == nil, "Repeated the same alert during an outage")
            precondition(model.schedules == previous, "Discarded last valid state")
        }
        let delays = await clock.delays
        precondition(Array(delays.suffix(8)) == [500, 1000, 2000, 4000, 8000, 8000, 8000, 8000].map { .milliseconds($0) })
        await clock.advance()
        await clock.waitForSleeps(13)
        let recoveredDelays = await clock.delays
        precondition(recoveredDelays.last == .milliseconds(250))

        // A committed add must return its ID even if the refresh fails. Only reads retry.
        lifecycle_fail(Int32(TEST_SNAPSHOT), 1)
        let id = try await model.add(url: "https://example.invalid/file", filename: "file")
        precondition(id == "created-id" && lifecycle_count(Int32(TEST_ADD)) == 1)
        precondition(model.rows == previousRows, "Lost valid rows after a failed refresh")
        await clock.advance()
        await clock.waitForSleeps(14)
        precondition(lifecycle_count(Int32(TEST_ADD)) == 1 && model.selection == id)

        try await DownloadWorkflowChecks.run(model: model)
        try await DownloadListChecks.run(model: model)
        try await ScheduleWorkflowChecks.run(model: model)
        try await BrowserWorkflowChecks.run(model: model)
        precondition(lifecycle_count(Int32(TEST_ADD)) == 1, "A read/navigation executed a mutation")

        model.error = nil
        precondition(model.requestMenuBarAdd(mainVisible: false) == .standalone)
        precondition(model.adding && model.standaloneAdding)
        precondition(model.requestMenuBarAdd(mainVisible: false) == .unavailable)
        model.closeStandaloneAdd()
        model.closeStandaloneAdd()
        precondition(!model.adding && !model.standaloneAdding)
        precondition(model.requestMenuBarAdd(mainVisible: true) == .main)
        precondition(model.adding && !model.standaloneAdding)
        precondition(model.requestMenuBarAdd(mainVisible: true) == .unavailable)
        model.adding = false
        model.browserReviewClosed()

        let opensBeforeReopen = lifecycle_count(Int32(TEST_OPEN))
        await model.open(root: URL(fileURLWithPath: "/unused-fixture"))
        precondition(lifecycle_count(Int32(TEST_OPEN)) == opensBeforeReopen, "Window recreation opened a second engine")

        // Normal close stops observation, consumes once, and coalesces repeat shutdown requests.
        async let first = model.shutdown()
        async let second = model.shutdown()
        let closed = await (first, second)
        precondition(closed.0 == .closed && closed.1 == .closed)
        precondition(!model.ready && lifecycle_live_handles() == 0 && lifecycle_count(Int32(TEST_CLOSE)) == 1)
        let repeatedClose = await model.shutdown()
        precondition(repeatedClose == .closed)
        precondition(lifecycle_count(Int32(TEST_CLOSE)) == 1)

        // Failed close is terminal and sticky, including at the actual Swift FFI wrapper.
        lifecycle_reset()
        let bridge = EngineBridge()
        let failedClock = TestClock()
        let failed = DownloadModel(bridge: bridge, sleep: { try await failedClock.sleep($0) })
        await failed.open(root: URL(fileURLWithPath: "/unused-fixture"))
        await failedClock.waitForSleeps(1)
        lifecycle_fail(Int32(TEST_CLOSE), 1)
        let outcome = await failed.shutdown()
        precondition(outcome == .failedConsumed("Injected lifecycle failure"))
        precondition(!failed.ready && lifecycle_live_handles() == 0)
        let repeatedFailure = await failed.shutdown()
        precondition(repeatedFailure == outcome)
        do {
            try await bridge.close()
            fatalError("A failed close was reported successful on retry")
        } catch is EngineFailure {}
        precondition(lifecycle_count(Int32(TEST_CLOSE)) == 1)
        do {
            _ = try await bridge.call(EngineCommand(operation: .snapshot), as: [Transfer].self)
            fatalError("Used a consumed handle")
        } catch let error as EngineFailure { precondition(error.code == "closed") }
        do {
            try await bridge.open(root: URL(fileURLWithPath: "/unused-fixture"), timezone: "UTC")
            fatalError("Reopened a terminal session")
        } catch let error as EngineFailure { precondition(error.code == "closed") }
        precondition(lifecycle_count(Int32(TEST_OPEN)) == 1)
        print("Lifecycle checks passed: recovery/backoff, mutation outcome, normal and consumed-error close")
    }
}
