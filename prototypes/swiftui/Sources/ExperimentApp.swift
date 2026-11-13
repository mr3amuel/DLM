import AppKit
import SwiftUI

@MainActor
final class ExperimentDelegate: NSObject, NSApplicationDelegate {
    weak var model: DownloadModel?
    private var terminating = false
    var reopenMain: (() -> Void)?

    func applicationWillFinishLaunching(_ notification: Notification) {
        NativeAppearanceController.shared.apply()
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NotificationCenter.default.addObserver(self, selector: #selector(timezoneChanged), name: .NSSystemTimeZoneDidChange, object: nil)
        // Match the bundle icon explicitly for this process, without global cache resets.
        if let url = Bundle.main.url(forResource: "DLMIcon", withExtension: "icns"),
           let icon = NSImage(contentsOf: url) {
            NSApp.applicationIconImage = icon
        } else {
            NSLog("DLM bundled application icon could not be decoded.")
        }
    }

    @objc private func timezoneChanged() { Task { await model?.timezoneChanged() } }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { MenuBarLifecycle.terminateAfterLastWindow }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        reopenMain?()
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let model else { return .terminateNow }
        guard !terminating else { return .terminateLater }
        terminating = true
        Task {
            let outcome = await model.shutdown()
            if case .failedConsumed(let cause) = outcome {
                // The handle is gone. Cancelling termination would leave a dead session.
                let alert = NSAlert()
                alert.alertStyle = .critical
                alert.messageText = NativePreferences.text("DLM could not confirm a safe shutdown.")
                alert.informativeText = NativePreferences.text("The engine session has closed and cannot continue. DLM will quit. Restart it to recover saved downloads. Recent progress may not have been saved.") + "\n\n" + cause
                alert.addButton(withTitle: NativePreferences.text("Quit DLM"))
                alert.runModal()
            }
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

@main
struct ExperimentApp: App {
    @NSApplicationDelegateAdaptor(ExperimentDelegate.self) private var delegate
    @StateObject private var model: DownloadModel
    @StateObject private var center: MenuBarPresentation
    init() {
        let shared = DownloadModel()
        _model = StateObject(wrappedValue: shared)
        _center = StateObject(wrappedValue: MenuBarPresentation(model: shared))
    }
    @AppStorage(NativePreferences.languageKey) private var language = "en"

    var body: some Scene {
        Window(NativePreferences.text(NativeIdentity.name), id: "downloads") {
            DownloadsView(model: model)
                .nativeLanguage().dlmBrandColors()
                .background(MainWindowReopenRegistration { delegate.reopenMain = $0 })
                .task {
                    delegate.model = model
                    do {
                        await model.open(root: try isolatedRoot(), desktopServices: true)
                    } catch {
                        model.error = error.localizedDescription

                    }
                }
        }
        .windowStyle(.titleBar)
        .windowToolbarStyle(.unified)
        .defaultSize(width: 1200, height: 730)
        .commands {
            CommandMenu(NativePreferences.text("Downloads")) {
                NativeAddDownloadCommand(model: model)
                Button(NativePreferences.text("Pause Selected Download")) {
                    if let id = model.selection { Task { await model.batch(.pause, ids: [id]) } }
                }
                .disabled(model.busy || model.selected?.status.canPause != true)
                Button(NativePreferences.text("Resume Selected Download")) {
                    if let id = model.selection { Task { await model.action(.resume, id: id) } }
                }
                .disabled(model.busy || model.selected?.status.canResume != true)
            }
        }
        MenuBarExtra {
            MenuBarDownloadCenter(model: model, presentation: center).nativeLanguage().dlmBrandColors()
        } label: {
            MenuBarStatusLabel(presentation: center).nativeLanguage()
        }.menuBarExtraStyle(.window)
        Window(NativePreferences.text("Add Download"), id: "add-download") {
            AddDownloadView(model: model, onOpenExisting: { delegate.reopenMain?() }).nativeLanguage().dlmBrandColors()
                .background(StandaloneAddWindowLifecycle { model.closeStandaloneAdd() })
        }.windowResizability(.contentSize)
        Settings {
            NativeSettingsView(model: model)
                .nativeLanguage().dlmBrandColors()
        }.windowResizability(.contentMinSize)
    }

    private func isolatedRoot() throws -> URL {
        try NativeIdentity.storageRoot()
    }
}

private struct MenuBarStatusLabel: View {
    @ObservedObject var presentation: MenuBarPresentation
    var body: some View {
        if let template = MenuBarBrandTemplate.image {
            Image(nsImage: template).renderingMode(.template).accessibilityLabel("DLM")
        } else {
            Image(systemName: presentation.summary.symbol).renderingMode(.template)
                .help("DLM logo unavailable").accessibilityLabel("DLM logo unavailable")
        }
        if !presentation.summary.statusText.isEmpty {
            Text(presentation.summary.statusText).monospacedDigit()
        }
    }
}
