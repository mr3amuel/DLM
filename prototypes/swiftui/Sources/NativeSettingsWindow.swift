import AppKit
import SwiftUI

// Settings is independently resizable; defaults and minimums fit its actual screen.
struct NativeSettingsWindow: NSViewRepresentable {
    @Environment(\.locale) private var locale
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeNSView(context: Context) -> Anchor {
        let view = Anchor()
        view.attached = { [weak coordinator = context.coordinator] in coordinator?.attach($0) }
        return view
    }
    func updateNSView(_ view: Anchor, context: Context) {
        view.window?.title = NativePreferences.text("Settings")
    }
    final class Anchor: NSView {
        var attached: ((NSWindow) -> Void)?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window else { return }
            DispatchQueue.main.async { [weak self, weak window] in
                if let window, self?.window === window { self?.attached?(window) }
            }
        }
    }
    final class Coordinator: NSObject {
        private weak var window: NSWindow?
        func attach(_ window: NSWindow) {
            window.title = NativePreferences.text("Settings")
            guard self.window !== window else { return }
            self.window = window
            window.styleMask.insert(.resizable)
            NotificationCenter.default.removeObserver(self)
            NotificationCenter.default.addObserver(self, selector: #selector(screenChanged),
                name: NSWindow.didChangeScreenNotification, object: window)
            NotificationCenter.default.addObserver(self, selector: #selector(screenChanged),
                name: NSApplication.didChangeScreenParametersNotification, object: nil)
            fit(initial: true)
        }
        @objc private func screenChanged() { fit(initial: false) }
        private func fit(initial: Bool) {
            guard let window, let screen = window.screen ?? NSScreen.main else { return }
            let visible = screen.visibleFrame
            let available = visible.insetBy(dx: visible.width * 0.03, dy: visible.height * 0.03)
            let content = window.contentRect(forFrameRect: available).size
            window.contentMinSize = NSSize(width: min(680, content.width), height: min(380, content.height))
            window.contentMaxSize = content
            let current = window.contentRect(forFrameRect: window.frame).size
            let desired = NSSize(width: min(initial ? 820 : current.width, content.width),
                                 height: min(initial ? 540 : current.height, content.height))
            var frame = window.frameRect(forContentRect: NSRect(origin: .zero, size: desired))
            if initial {
                frame.origin = NSPoint(x: available.midX - frame.width / 2, y: available.midY - frame.height / 2)
            } else {
                frame.origin = NSPoint(x: min(max(window.frame.minX, available.minX), available.maxX - frame.width),
                                       y: min(max(window.frame.minY, available.minY), available.maxY - frame.height))
            }
            window.setFrame(frame, display: false)
        }
    }
}
