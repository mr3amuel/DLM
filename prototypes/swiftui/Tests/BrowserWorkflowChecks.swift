import Foundation

@MainActor
enum BrowserWorkflowChecks {
    static func run(model: DownloadModel) async throws {
        let suite = "dlm-native-browser-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        precondition(defaults.string(forKey: NativePreferences.chromeInvitationKey) == nil)
        for decision in ["later", "setup"] {
            defaults.set(decision, forKey: NativePreferences.chromeInvitationKey)
            precondition(UserDefaults(suiteName: suite)!.string(forKey: NativePreferences.chromeInvitationKey) == decision)
        }
        let key = try String(contentsOfFile: "chrome-public-key.txt", encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let id = NativeChromeSetup.extensionID(publicKey: Data(base64Encoded: key)!)
        precondition(id.count == 32 && id.utf8.allSatisfy { (97...112).contains($0) })
        let productionKey = try String(contentsOfFile: "../../browser-extension/chrome-public-key.txt", encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        precondition(id != NativeChromeSetup.extensionID(publicKey: Data(base64Encoded: productionKey)!))
        var calls: [[String]] = []
        var status = "pending"
        let attempt = String(repeating: "a", count: 32)
        let setup = NativeBrowserSetupSession { arguments in
            calls.append(arguments)
            if arguments.first == "register-browser" { return attempt }
            return status
        }
        try await setup.register(browser: .chrome, app: "/fixture/DLM.app", inbox: "/fixture/browser-integration", id: id)
        precondition(setup.attempt == attempt && !setup.connected)
        var done = try await setup.check(attempt)
        precondition(!done && !setup.connected)
        status = "failed"
        done = try await setup.check(attempt)
        precondition(!done && setup.notice != nil && !setup.connected)
        status = "ready"
        done = try await setup.check(attempt)
        precondition(done && setup.connected)
        setup.invalidate()
        _ = try await setup.check(attempt)
        precondition(!setup.connected && setup.attempt == nil, "Stale result revived a revoked attempt")
        try await setup.register(browser: .edge, app: "/moved/DLM.app", inbox: "/fixture/browser-integration", id: id)
        precondition(calls.last?[1] == "edge" && calls.last?[2] == "/moved/DLM.app")
        setup.timedOut(attempt)
        precondition(!setup.connected && setup.notice != nil)
        let invalid = NativeBrowserSetupSession { _ in "not-an-attempt" }
        do { try await invalid.register(browser: .chrome, app: "/app", inbox: "/inbox", id: id); fatalError("Accepted invalid attempt") }
        catch is EngineFailure {}

        var began: CheckedContinuation<Void, Never>?
        var reply: CheckedContinuation<String, Never>?
        let racing = NativeBrowserSetupSession { _ in
            await withCheckedContinuation { continuation in
                reply = continuation
                began?.resume()
            }
        }
        let pendingRegistration = Task {
            try await racing.register(browser: .chrome, app: "/fixture/DLM.app", inbox: "/fixture/browser-integration", id: id)
        }
        await withCheckedContinuation { began = $0 }
        racing.invalidate()
        reply!.resume(returning: attempt)
        do { try await pendingRegistration.value; fatalError("Revived a cancelled registration") }
        catch is CancellationError {}
        precondition(racing.attempt == nil && !racing.connected)

        try await model.configureBrowser(true)
        let payload = "{\"data\":{\"id\":\"browser-request\",\"url\":\"https://example.invalid/selected\",\"filename\":\"selected.bin\"}}"
        payload.withCString { lifecycle_browser_json($0) }
        await model.refreshBrowserDraft()
        precondition(model.browserDraft?.id == "browser-request" && !model.adding)
        model.adding = true
        model.draftURL = "already editing"
        model.reviewBrowserDraft()
        precondition(model.draftURL == "already editing", "Review interrupted another sheet")
        model.adding = false
        model.reviewBrowserDraft()
        precondition(model.adding && model.draftURL == "https://example.invalid/selected")
        let taken = lifecycle_count(Int32(TEST_BROWSER_TAKE))
        await model.refreshBrowserDraft()
        precondition(lifecycle_count(Int32(TEST_BROWSER_TAKE)) == taken, "Fetched another request during a sheet")
        model.adding = false
        model.browserReviewClosed() // Cancel leaves the same request available.
        precondition(model.browserDraft?.id == "browser-request")
        model.reviewBrowserDraft()
        lifecycle_fail(Int32(TEST_BROWSER_ACK), 1)
        await model.finishBrowserReview()
        precondition(model.browserDraft == nil && model.browserError != nil)
        model.adding = false
        model.browserReviewClosed()
        await model.refreshBrowserDraft()
        precondition(model.browserDraft == nil && lifecycle_count(Int32(TEST_BROWSER_ACK)) == 2)
        payload.withCString { lifecycle_browser_json($0) }
        await model.refreshBrowserDraft()
        precondition(model.browserDraft != nil)
        try await model.resetBrowserPairing()
        precondition(model.browserDraft == nil)
        try await model.configureBrowser(false)
        print("Browser workflow checks passed: persistent invitation, setup generations/fresh receipt, repair arguments, review/cancel retention, deferred ack and revocation")
    }
}
