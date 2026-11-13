import AppKit
import SwiftUI

struct NativeToolbarCommand {
    let title: String
    var enabled = true
    var checked = false
    var destructive = false
    var children: [NativeToolbarCommand] = []
    var action: (() -> Void)?
}

// A real NSToolbar owns grouping, flexible space and overflow on macOS 13+.
// Custom content is limited to the teal primary button, not a simulated bar.
struct NativeDownloadToolbar: NSViewRepresentable {
    var downloads: Bool
    var addEnabled: Bool
    var resumeEnabled: Bool
    var pauseEnabled: Bool
    var removeEnabled: Bool
    var detailsTitle: String
    var detailsSymbol: String
    var commands: [NativeToolbarCommand]
    @Binding var search: String
    var searchFocusRequest: Int
    var add: () -> Void
    var resume: () -> Void
    var pause: () -> Void
    var remove: () -> Void
    var details: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeNSView(context: Context) -> Anchor {
        let view = Anchor()
        view.attached = { [weak coordinator = context.coordinator] window in coordinator?.attach(window) }
        return view
    }
    func updateNSView(_ view: Anchor, context: Context) { context.coordinator.update(self) }

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

    final class Coordinator: NSObject, NSToolbarDelegate, NSSearchFieldDelegate {
        private var state: NativeDownloadToolbar
        private let bar = NSToolbar(identifier: "DLM.Downloads.Toolbar")
        private var cached: [NSToolbarItem.Identifier: NSToolbarItem] = [:]
        private weak var window: NSWindow?
        private var focusRequest: Int
        private var lastDownloads: Bool?
        private static let add = NSToolbarItem.Identifier("dlm.add")
        private static let resume = NSToolbarItem.Identifier("dlm.resume")
        private static let pause = NSToolbarItem.Identifier("dlm.pause")
        private static let remove = NSToolbarItem.Identifier("dlm.remove")
        private static let more = NSToolbarItem.Identifier("dlm.more")
        private static let details = NSToolbarItem.Identifier("dlm.details")
        private static let search = NSToolbarItem.Identifier("dlm.search")

        init(_ state: NativeDownloadToolbar) {
            self.state = state
            focusRequest = state.searchFocusRequest
            super.init()
            bar.delegate = self
            bar.displayMode = .iconOnly
            bar.allowsUserCustomization = false
            bar.autosavesConfiguration = false
        }

        private var identifiers: [NSToolbarItem.Identifier] { Self.itemIdentifiers(downloads: state.downloads) }
        static func itemIdentifiers(downloads: Bool) -> [NSToolbarItem.Identifier] {
            downloads
                ? [Self.add, .space, Self.resume, Self.pause, Self.remove, Self.more, .flexibleSpace, Self.search, Self.details]
                : [Self.add, .flexibleSpace]
        }

        func attach(_ window: NSWindow) {
            guard self.window !== window else { return }
            self.window = window
            // Reserve one native title-bar region. The content must not overlap it.
            window.styleMask.remove(.fullSizeContentView)
            window.titleVisibility = .hidden
            window.toolbarStyle = .unified
            window.toolbar = bar
            refresh()
        }

        func toolbarDefaultItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { identifiers }
        func toolbarAllowedItemIdentifiers(_ toolbar: NSToolbar) -> [NSToolbarItem.Identifier] { identifiers }

        func toolbar(_ toolbar: NSToolbar, itemForItemIdentifier id: NSToolbarItem.Identifier,
                     willBeInsertedIntoToolbar flag: Bool) -> NSToolbarItem? {
            if let item = cached[id] { return item }
            let item: NSToolbarItem
            if id == Self.search {
                let search = NSSearchToolbarItem(itemIdentifier: id)
                search.searchField.delegate = self
                search.searchField.sendsSearchStringImmediately = true
                search.searchField.maximumRecents = 0
                search.visibilityPriority = .low
                item = search
            } else if id == Self.more {
                let menu = NSMenuToolbarItem(itemIdentifier: id)
                menu.showsIndicator = false
                item = menu
            } else {
                item = NSToolbarItem(itemIdentifier: id)
            }
            if id != Self.more && id != Self.search {
                item.target = self
                item.action = #selector(invoke(_:))
            }
            item.autovalidates = false
            item.isBordered = true
            item.visibilityPriority = id == Self.add ? .user : (id == Self.remove ? .low : .high)
            if id == Self.add {
                item.isNavigational = true
                item.view = NSHostingView(rootView: PrimaryButton(enabled: state.addEnabled, action: state.add))
                item.view?.widthAnchor.constraint(equalToConstant: 36).isActive = true
                item.view?.heightAnchor.constraint(equalToConstant: 36).isActive = true
            }
            cached[id] = item
            return item
        }

        func update(_ state: NativeDownloadToolbar) {
            self.state = state
            refresh()
            if focusRequest != state.searchFocusRequest {
                focusRequest = state.searchFocusRequest
                if let item = cached[Self.search] as? NSSearchToolbarItem {
                    item.beginSearchInteraction()
                }
            }
        }

        private func refresh() {
            guard window != nil else { return }
            if lastDownloads != state.downloads {
                lastDownloads = state.downloads
                if bar.items.map(\.itemIdentifier) != identifiers {
                    while !bar.items.isEmpty { bar.removeItem(at: 0) }
                    for (index, id) in identifiers.enumerated() { bar.insertItem(withItemIdentifier: id, at: index) }
                }
            }
            configure(Self.add, title: "Add Download", symbol: "plus", enabled: state.addEnabled)
            configure(Self.resume, title: "Resume", symbol: "play", enabled: state.resumeEnabled)
            configure(Self.pause, title: "Pause", symbol: "pause", enabled: state.pauseEnabled)
            configure(Self.remove, title: "Remove Records", symbol: "trash", enabled: state.removeEnabled)
            configure(Self.more, title: "Selection", symbol: "ellipsis.circle", enabled: state.commands.contains { $0.enabled })
            configure(Self.details, title: state.detailsTitle, symbol: state.detailsSymbol, enabled: true)
            if let item = cached[Self.more] as? NSMenuToolbarItem { item.menu = makeMenu(state.commands) }
            if let item = cached[Self.search] as? NSSearchToolbarItem {
                item.label = NativePreferences.text("Search downloads")
                item.searchField.placeholderString = item.label
                item.searchField.setAccessibilityLabel(item.label)
                if item.searchField.stringValue != state.search { item.searchField.stringValue = state.search }
            }
            if let view = cached[Self.add]?.view as? NSHostingView<PrimaryButton> {
                view.rootView = PrimaryButton(enabled: state.addEnabled, action: state.add)
            }
        }

        private func configure(_ id: NSToolbarItem.Identifier, title: String, symbol: String, enabled: Bool) {
            guard let item = cached[id] else { return }
            item.label = NativePreferences.text(title)
            item.toolTip = item.label
            item.image = NSImage(systemSymbolName: symbol, accessibilityDescription: item.label)
            item.isEnabled = enabled
            // Overflow dispatch must not assume its sender is an NSToolbarItem.
            if id != Self.more {
                let target = MenuAction { [weak self] in self?.run(id) }
                let overflow = NSMenuItem(title: item.label, action: #selector(MenuAction.runCommand), keyEquivalent: "")
                overflow.target = target
                overflow.representedObject = target
                overflow.isEnabled = enabled
                item.menuFormRepresentation = overflow
            }
        }

        @objc private func invoke(_ sender: NSToolbarItem) { run(sender.itemIdentifier) }

        private func run(_ identifier: NSToolbarItem.Identifier) {
            switch identifier {
            case Self.add: if state.addEnabled { state.add() }
            case Self.resume: if state.resumeEnabled { state.resume() }
            case Self.pause: if state.pauseEnabled { state.pause() }
            case Self.remove: if state.removeEnabled { state.remove() }
            case Self.details: state.details()
            default: break
            }
        }

        func controlTextDidChange(_ notification: Notification) {
            if let field = notification.object as? NSSearchField { state.search = field.stringValue }
        }

        private func makeMenu(_ commands: [NativeToolbarCommand]) -> NSMenu {
            let menu = NSMenu()
            menu.autoenablesItems = false
            for command in commands {
                let item = NSMenuItem(title: command.title, action: nil, keyEquivalent: "")
                item.isEnabled = command.enabled
                if !command.children.isEmpty { item.submenu = makeMenu(command.children) }
                if let action = command.action {
                    let target = MenuAction(action)
                    item.target = target; item.action = #selector(MenuAction.runCommand)
                    item.representedObject = target // NSMenuItem's target is weak.
                }
                menu.addItem(item)
            }
            return menu
        }
    }

    private struct PrimaryButton: View {
        let enabled: Bool
        let action: () -> Void
        var body: some View {
            Button(action: action) { Image(systemName: "plus") }
                .buttonStyle(DLMIconStyle(primary: true)).disabled(!enabled)
                .accessibilityLabel(Text(NativePreferences.text("Add Download")))
                .help(Text(NativePreferences.text("Add Download")))
                .frame(width: 36, height: 36)
                .dlmBrandColors()
        }
    }

    private final class MenuAction: NSObject {
        let action: () -> Void
        init(_ action: @escaping () -> Void) { self.action = action }
        @objc func runCommand() { action() }
    }
}

extension NativeToolbarCommand {
    static func priorityChoices(current: TransferPriority, enabled: Bool, change: @escaping (TransferPriority) -> Void) -> [Self] {
        TransferPriority.allCases.map { priority in
            Self(title: NativePreferences.text(priority.rawValue), enabled: enabled, checked: priority == current, action: { change(priority) })
        }
    }
}
