import AppKit
import CryptoKit
import SwiftUI

enum NativeChromeSetup {
    private static let chromeID = "com.google.Chrome"
    private static let chromePage = URL(string: "chrome://extensions/")!

    @MainActor static var chrome: URL? {
        NSRunningApplication.runningApplications(withBundleIdentifier: chromeID).first?.bundleURL
            ?? NSWorkspace.shared.urlForApplication(withBundleIdentifier: chromeID)
            ?? ["/Applications/Google Chrome.app", NSHomeDirectory() + "/Applications/Google Chrome.app"]
                .map { URL(fileURLWithPath: $0) }.first { Bundle(url: $0)?.bundleIdentifier == chromeID }
    }

    @MainActor static func openExtensions() async throws -> Bool {
        guard let chrome else { throw failure("Google Chrome could not be found.") }
        if NSRunningApplication.runningApplications(withBundleIdentifier: chromeID).isEmpty {
            if await openApplication(chrome, arguments: [chromePage.absoluteString]) { return true }
        } else {
            let chromePath = chrome.path
            if (try? await Task.detached(priority: .utility, operation: {
                let process = Process()
                process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
                process.arguments = ["-a", chromePath, chromePage.absoluteString]
                process.standardInput = FileHandle.nullDevice
                process.standardOutput = FileHandle.nullDevice
                process.standardError = FileHandle.nullDevice
                try process.run()
                process.waitUntilExit()
                return process.terminationStatus == 0
            }).value) == true { return true }
        }
        guard await openApplication(chrome, arguments: []) else {
            throw failure("Google Chrome could not be opened.")
        }
        return false
    }

    @MainActor private static func openApplication(_ url: URL, arguments: [String]) async -> Bool {
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.arguments = arguments
        return await withCheckedContinuation { continuation in
            NSWorkspace.shared.openApplication(at: url, configuration: configuration) { app, error in
                continuation.resume(returning: app != nil && error == nil)
            }
        }
    }

    static func extensionID() throws -> String {
        guard let url = Bundle.main.url(forResource: "manifest", withExtension: "json", subdirectory: "DLMChromeExtension"),
              let dictionary = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any],
              let key = dictionary["key"] as? String, let bytes = Data(base64Encoded: key) else {
            throw failure("The development extension is missing from this app.")
        }
        return extensionID(publicKey: bytes)
    }

    static func extensionID(publicKey bytes: Data) -> String {
        let alphabet = Array("abcdefghijklmnop")
        return SHA256.hash(data: bytes).prefix(16).map {
            String([alphabet[Int($0 >> 4)], alphabet[Int($0 & 15)]])
        }.joined()
    }

    static func command(_ arguments: [String]) async throws -> String {
        let helper = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/dlm-native-chrome-host")
        return try await Task.detached(priority: .utility) {
            let process = Process()
            let output = Pipe()
            process.executableURL = helper
            process.arguments = ["chrome-setup"] + arguments
            process.standardInput = FileHandle.nullDevice
            process.standardOutput = output
            process.standardError = FileHandle.nullDevice
            try process.run()
            output.fileHandleForWriting.closeFile()
            let result = try output.fileHandleForReading.readToEnd() ?? Data()
            process.waitUntilExit()
            guard process.terminationStatus == 0, result.count <= 4096,
                  let value = String(data: result, encoding: .utf8) else {
                throw failure("The native browser helper could not complete this step. Reopen DLM from its installed location and try again.")
            }
            return value.trimmingCharacters(in: .newlines)
        }.value
    }

    static func failure(_ message: String) -> EngineFailure {
        EngineFailure(code: "chrome_setup", message: NativePreferences.text(message))
    }
}

enum NativeSetupBrowser: String, CaseIterable, Identifiable {
    case chrome = "Google Chrome", edge = "Microsoft Edge", firefox = "Firefox"
    var id: Self { self }
    var argument: String { switch self { case .chrome: "chrome"; case .edge: "edge"; case .firefox: "firefox" } }
}

struct NativeRegistrationStatus: Decodable {
    let registration: String
    let folder: Bool
    let directory: String?
}

// One setup attempt, bounded polling and fresh authenticated receipt. No engine policy.
@MainActor
final class NativeBrowserSetupSession: ObservableObject {
    @Published private(set) var attempt: String?
    @Published private(set) var connected = false
    @Published private(set) var notice: String?
    private var generation = UUID()
    let command: ([String]) async throws -> String
    init(command: @escaping ([String]) async throws -> String = NativeChromeSetup.command) { self.command = command }

    func invalidate() { generation = UUID(); attempt = nil; connected = false; notice = nil }

    func register(browser: NativeSetupBrowser, app: String, inbox: String, id: String) async throws {
        invalidate()
        let current = generation
        let value = try await command(["register-browser", browser.argument, app, inbox, id])
        guard generation == current, !Task.isCancelled else { throw CancellationError() }
        guard value.count == 32, value.allSatisfy({ $0.isHexDigit }) else {
            throw NativeChromeSetup.failure("Browser registration could not be verified.")
        }
        attempt = value
    }

    func check(_ generation: String) async throws -> Bool {
        let result = try await command(["status", generation])
        guard attempt == generation, !Task.isCancelled else { return true }
        connected = result == "ready"
        notice = result == "failed" ? "The connection was not verified. Copy the current code and try again in the browser." : nil
        return connected
    }

    func timedOut(_ generation: String) {
        guard attempt == generation else { return }
        connected = false
        notice = "Still waiting for the browser. Open the extension options and test the connection, or Retry."
    }
}

struct NativeChromeView: View {
    @ObservedObject var model: DownloadModel
    var embedded = false
    @StateObject private var setup = NativeBrowserSetupSession()
    @State private var browser = NativeSetupBrowser.chrome
    @State private var folder: String?
    @State private var working = false
    @State private var notice: String?
    @State private var error: String?
    @State private var registration = "missing"
    @State private var advanced = false
    @State private var manualID = ""
    @State private var confirmingRemoval = false
    @State private var confirmingReset = false

    private func extensionID() throws -> String {
        if browser == .firefox { return NativeIdentity.firefoxID }
        let id = manualID.trimmingCharacters(in: .whitespacesAndNewlines)
        if id.isEmpty { return try NativeChromeSetup.extensionID() }
        guard id.count == 32, id.utf8.allSatisfy({ (97...112).contains($0) }) else {
            throw NativeChromeSetup.failure("Enter the 32-letter ID from the extension card.")
        }
        return id
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DLMDesign.gap) {
            if !embedded { DLMHeading(title: NativeIdentity.production ? "Connect Chrome to DLM" : "Connect Chrome to DLM Native Development", compact: true) }
            DLMSettingsGroup {
                DLMSettingsRow(title: "Browser Integration", detail: "Review browser links in DLM before downloading.", icon: "puzzlepiece.extension") {
                    Toggle("Enable browser integration", isOn: Binding(
                        get: { model.browserState?.enabled == true },
                        set: { enabled in run { setup.invalidate(); try await model.configureBrowser(enabled) } }
                    )).labelsHidden().dlmSwitchTint()
                        .help("Disabling integration discards pending browser links; downloads are unchanged.")
                }
                Picker("Browser", selection: $browser) {
                    ForEach(NativeSetupBrowser.allCases) { Text($0.rawValue).tag($0) }
                }
                Label(LocalizedStringKey(setup.connected ? "Browser connection verified" :
                    (model.browserState?.enabled == true ? "Connection enabled" : "Not connected")),
                      systemImage: setup.connected ? "checkmark.circle" : "link")
                    .font(.callout).foregroundStyle(.secondary)
            }
            Text("This development connection is separate from production DLM. Installation and browser-side consent remain explicit.")
                .fixedSize(horizontal: false, vertical: true)
            DLMPanel(title: "Extension Setup") {
                if browser == .chrome || browser == .edge {
                    Text("In Extensions, enable Developer mode and choose Load unpacked. Press Shift-Command-G and paste the copied folder path.")
                    HStack {
                        Button("Prepare Extension") { run { try await prepare(); try await register() } }
                        if let folder {
                            Button("Copy Path and Show Folder") { showFolder(folder) }
                        }
                    }
                    if browser == .chrome {
                        Button("Open Chrome Extensions") {
                            run {
                                if folder == nil { try await prepare() }
                                if let folder { showFolder(folder) }
                                if try await !NativeChromeSetup.openExtensions() {
                                    notice = "Chrome opened. Enter chrome://extensions/ in its address bar."
                                }
                            }
                        }
                    } else { Text("In Microsoft Edge, open edge://extensions. The unpacked extension uses the same folder.").font(.callout) }
                } else {
                    Text("Firefox 140 or later: open about:debugging, choose This Firefox, then Load Temporary Add-on and select manifest.json in the extension folder. Temporary installation ends when Firefox exits.")
                    Button("Show Firefox Extension") {
                        guard let source = Bundle.main.url(forResource: "DLMFirefoxExtension", withExtension: nil) else {
                            error = NativePreferences.text("The development extension is missing from this app."); return
                        }
                        showFolder(source.path)
                    }
                }
                HStack {
                    Button("Register or Repair Connection") { run { try await register() } }
                    Button("Retry Connection Test") { run { try await register() } }
                }
                if registration == "stale" {
                    Text("This registration points to an older location or inbox. Register or Repair Connection updates only the native candidate.").font(.callout)
                } else if registration == "unsafe" {
                    Text("The native registration is not recognized. It was left unchanged; check the troubleshooting guide.").font(.callout)
                }
            }
            if setup.attempt != nil || model.browserState?.enabled == true {
                DLMPanel(title: "Pairing") {
                    Text("Open the extension options, paste the pairing code, and explicitly allow the connection. If already paired, choose Test Connection.")
                        .fixedSize(horizontal: false, vertical: true)
                    HStack {
                        Button("Copy Pairing Code") {
                            run {
                                copy(try await model.browserPairingCode())
                                let change = NSPasteboard.general.changeCount
                                Task { @MainActor in
                                    try? await Task.sleep(for: .seconds(120))
                                    if NSPasteboard.general.changeCount == change { NSPasteboard.general.clearContents() }
                                }
                            }
                        }.disabled(model.browserState?.enabled != true)
                        Button("Reset pairing") { confirmingReset = true }
                    }
                }
            }
            DisclosureGroup("Troubleshooting", isExpanded: $advanced) {
                if browser != .firefox {
                    TextField("Extension ID (optional)", text: $manualID)
                        .environment(\.layoutDirection, .leftToRight)
                }
                Text("After moving DLM, reopen it from its new location and repair each browser connection. Remove the extension or forget its code in the browser separately.")
                if let path = model.browserState?.directory {
                    Text(path).font(.caption).textSelection(.enabled).environment(\.layoutDirection, .leftToRight)
                }
            }
            if working { ProgressView().controlSize(.small) }
            if let notice = notice ?? setup.notice { Text(LocalizedStringKey(notice)).foregroundStyle(.secondary) }
            if let error = error ?? model.browserError { Text(error).foregroundStyle(DLMDesign.failure) }
            Button("Disconnect and Revoke Pairing", role: .destructive) { confirmingRemoval = true }
        }
        .disabled(!model.ready || working)
        .padding(embedded ? 0 : 20)
        .task(id: browser) { setup.invalidate(); await inspect() }
        .task(id: setup.attempt) {
            guard let attempt = setup.attempt else { return }
            do {
                for _ in 0..<120 {
                    if try await setup.check(attempt) { return }
                    try await Task.sleep(for: .seconds(1))
                }
                setup.timedOut(attempt)
            } catch is CancellationError {} catch { self.error = error.localizedDescription }
        }
        .onChange(of: model.browserState?.enabled) { enabled in if enabled != true { setup.invalidate() } }
        .confirmationDialog("Reset browser pairing?", isPresented: $confirmingReset) {
            Button("Reset pairing", role: .destructive) { run { setup.invalidate(); try await model.resetBrowserPairing() } }
        } message: { Text("Previously paired native extensions need the new code. Pending browser links are discarded; downloads and production DLM are unchanged.") }
        .confirmationDialog("Disconnect and revoke browser pairing?", isPresented: $confirmingRemoval) {
            Button("Disconnect and Revoke Pairing", role: .destructive) {
                run {
                    setup.invalidate()
                    try await model.configureBrowser(false)
                    try await model.resetBrowserPairing()
                    _ = try await NativeChromeSetup.command(["remove"])
                    folder = nil; registration = "missing"
                }
            }
        } message: { Text("All native candidate browser connections will be removed. Downloads and production DLM are unchanged. Remove the extensions in their browsers separately.") }
    }

    private func run(_ operation: @escaping @MainActor () async throws -> Void) {
        guard !working else { return }
        working = true; error = nil; notice = nil
        Task {
            defer { working = false }
            do { try await operation() } catch { self.error = error.localizedDescription }
        }
    }
    private func prepare() async throws {
        _ = try NativeChromeSetup.extensionID()
        folder = try await NativeChromeSetup.command(["prepare", Bundle.main.bundlePath])
        UserDefaults.standard.set("setup", forKey: NativePreferences.chromeInvitationKey)
    }
    private func inspect() async {
        guard let state = model.browserState else { return }
        let selected = browser
        do {
            let result = try await NativeChromeSetup.command(["inspect", browser.argument, Bundle.main.bundlePath, state.directory, try extensionID()])
            let status = try JSONDecoder().decode(NativeRegistrationStatus.self, from: Data(result.utf8))
            guard !Task.isCancelled, selected == browser else { return }
            registration = status.registration
            folder = status.folder ? status.directory : nil
            // Preparation is idempotent but explicit; inspection never repairs or installs.
        } catch { self.error = error.localizedDescription }
    }
    private func register() async throws {
        guard let state = model.browserState else { throw NativeChromeSetup.failure("The Chrome inbox is unavailable.") }
        if browser != .chrome && Bundle.main.object(forInfoDictionaryKey: "DLMScopedFolders") as? Bool == true {
            throw NativeChromeSetup.failure("Edge and Firefox setup requires the nonsandboxed development candidate. Sandbox permissions are deferred.")
        }
        UserDefaults.standard.set("setup", forKey: NativePreferences.chromeInvitationKey)
        if !state.enabled { try await model.configureBrowser(true) }
        try await setup.register(browser: browser, app: Bundle.main.bundlePath, inbox: state.directory, id: extensionID())
        registration = "current"
    }
    private func showFolder(_ path: String) {
        copy(path)
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }
    private func copy(_ value: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }
}
