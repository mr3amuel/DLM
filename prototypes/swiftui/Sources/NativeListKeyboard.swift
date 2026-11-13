import AppKit
import SwiftUI

enum DownloadListKey: Equatable {
    case selectAll, clear, move(Int)
    static func textEditing(marked: Bool, selectionLocation: Int) -> Bool {
        marked || selectionLocation != NSNotFound
    }
    static func resolve(key: String, code: UInt16, command: Bool, shift: Bool,
                        otherModifiers: Bool, editing: Bool, blocked: Bool, selected: Bool = true) -> Self? {
        guard !blocked, !editing, !otherModifiers else { return nil }
        if command { return !shift && key.lowercased() == "a" ? .selectAll : nil }
        if code == 53 && !shift { return selected ? .clear : nil }
        if code == 125 { return .move(1) }
        if code == 126 { return .move(-1) }
        return nil
    }
}

// Window-scoped, removed with the list. Text editing and modal interactions retain
// their native key handling. No global shortcuts, first-responder stealing or key synthesis.
struct NativeListKeyboard: NSViewRepresentable {
    var blocked: Bool
    var hasSelection: Bool
    var action: (DownloadListKey) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        context.coordinator.view = view
        return view
    }
    func updateNSView(_ view: NSView, context: Context) { context.coordinator.state = self }
    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) { coordinator.stop() }

    final class Coordinator {
        var state: NativeListKeyboard
        weak var view: NSView?
        private var monitor: Any?
        init(_ state: NativeListKeyboard) {
            self.state = state
            monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
                guard let self, let window = self.view?.window, event.window === window,
                      window.isKeyWindow else { return event }
                let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
                let blocked = self.state.blocked || window.attachedSheet != nil || NSApp.modalWindow != nil
                var responder = window.firstResponder as? NSView
                func activeInput(_ responder: NSResponder?) -> Bool {
                    guard let client = responder as? NSTextInputClient else { return false }
                    // Hosting views can implement the protocol without an active editor.
                    return DownloadListKey.textEditing(marked: client.hasMarkedText(), selectionLocation: client.selectedRange().location)
                }
                var editing = activeInput(window.firstResponder)
                while let current = responder {
                    // Action buttons (including list rows) must not disable list commands.
                    // Editable/adjustable native controls keep their own key handling.
                    if activeInput(current) || current is NSTextView || current is NSTextField || current is NSDatePicker
                        || current is NSStepper || current is NSSlider || current is NSPopUpButton
                        || current is NSSegmentedControl || current is NSSwitch {
                        editing = true; break
                    }
                    responder = current.superview
                }
                guard let key = DownloadListKey.resolve(key: event.charactersIgnoringModifiers ?? "",
                    code: event.keyCode, command: flags.contains(.command), shift: flags.contains(.shift),
                    otherModifiers: !flags.intersection([.option, .control]).isEmpty,
                    editing: editing, blocked: blocked, selected: self.state.hasSelection) else { return event }
                self.state.action(key)
                return nil
            }
        }
        func stop() {
            if let monitor { NSEvent.removeMonitor(monitor); self.monitor = nil }
        }
        deinit { stop() }
    }
}

enum NativeClipboard {
    static func copy(_ text: String, write: (String) -> Bool = { value in
        NSPasteboard.general.clearContents()
        return NSPasteboard.general.setString(value, forType: .string)
    }) -> Bool { write(text) }
}
