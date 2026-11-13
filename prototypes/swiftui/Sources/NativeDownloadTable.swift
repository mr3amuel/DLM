import AppKit
import SwiftUI
import UniformTypeIdentifiers

// Formatting only: the DTO and Phase 2B helper remain authoritative for data/sorting.
struct DownloadTableValues {
    private let row: Transfer
    init(_ row: Transfer) { self.row = row }
    var filename: String { row.filename }
    var status: String { row.statusLabel }
    var statusHelp: String {
        [row.statusLabel, row.scheduleState.map { NativePreferences.text($0) }, row.error]
            .compactMap { $0 }.joined(separator: " · ")
    }
    var size: String { row.total.map(Transfer.bytes) ?? "—" }
    var fraction: Double? { row.fraction }
    var percentage: String {
        row.fraction.map { $0.formatted(.percent.precision(.fractionLength(0...1)).locale(NativePreferences.locale)) } ?? "—"
    }
    var speed: String { row.status == .running ? row.speedSummary : "—" }
    var created: String { row.createdAtMs.map(NativePreferences.date) ?? "—" }

    static func changed(_ key: DownloadListOptions.Sort, from old: Transfer, to new: Transfer) -> Bool {
        switch key {
        case .filename: old.filename != new.filename
        case .status: old.status != new.status || old.queuePosition != new.queuePosition
            || old.retryRemainingSeconds != new.retryRemainingSeconds || old.scheduleState != new.scheduleState || old.error != new.error
        case .size: old.total != new.total
        case .progress: old.downloaded != new.downloaded || old.total != new.total
        case .speed: old.speed != new.speed || old.status != new.status
        case .creationDate: old.createdAtMs != new.createdAtMs
        case .priority: old.priority != new.priority
        }
    }
}

// Each move uses the current order after the preceding move. Publish that same
// intermediate snapshot before notifying AppKit, so cached row views stay aligned.
enum DownloadTableUpdate: Equatable {
    struct Move: Equatable { let from: Int; let to: Int }
    case reload, cells, moves([Move])
    static func restoreViewport(_ desired: NSRect, in clip: NSClipView) {
        clip.scroll(to: clip.constrainBoundsRect(desired).origin)
    }
    static func plan(from old: [String], to new: [String]) -> Self {
        if old == new { return .cells }
        guard old.count == new.count, Set(old).count == old.count, Set(old) == Set(new) else { return .reload }
        var order = old
        var moves: [Move] = []
        for (position, id) in new.enumerated() where order[position] != id {
            let source = order.firstIndex(of: id)!
            order.insert(order.remove(at: source), at: position)
            moves.append(Move(from: source, to: position))
        }
        return .moves(moves)
    }
}

struct NativeDownloadTable: NSViewRepresentable {
    let rows: [Transfer]
    @Binding var selection: Set<String>
    @Binding var options: DownloadListOptions
    let blocked: Bool
    let revealID: String?
    let didReveal: (String) -> Void
    let showDetails: (Transfer) -> Void
    let actions: (Transfer) -> [NativeToolbarCommand]
    @Environment(\.layoutDirection) private var direction
    @Environment(\.locale) private var locale

    func makeCoordinator() -> Coordinator { Coordinator(self) }
    func makeNSView(context: Context) -> NSScrollView { makeScrollView(coordinator: context.coordinator) }
    // Same factory for the application and offscreen integration checks; tests disable preference writes.
    func makeScrollView(coordinator: Coordinator, persistColumns: Bool = true) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = NSScroller.preferredScrollerStyle
        scroll.borderType = .noBorder
        scroll.drawsBackground = true
        scroll.backgroundColor = NSColor(DLMDesign.canvas)
        let table = DownloadTableView()
        table.allowsMultipleSelection = true
        table.allowsEmptySelection = true
        table.allowsColumnReordering = false
        table.allowsColumnResizing = true
        table.columnAutoresizingStyle = .noColumnAutoresizing
        table.style = .plain
        table.rowHeight = 32
        table.intercellSpacing = NSSize(width: 8, height: 1)
        table.gridStyleMask = .solidHorizontalGridLineMask
        table.gridColor = .separatorColor
        table.backgroundColor = NSColor(DLMDesign.canvas)
        table.target = coordinator
        table.doubleAction = #selector(Coordinator.openDetails)
        table.blocked = { [weak coordinator = coordinator] in coordinator?.state.blocked ?? true }
        table.rowMenu = { [weak coordinator = coordinator] index in coordinator?.menu(for: index) }
        table.setAccessibilityLabel(NativePreferences.text("Downloads"))
        table.setAccessibilityHelp(NativePreferences.text("Right-click or press Shift-F10 for download actions."))
        for key in Coordinator.columns {
            let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(key.rawValue))
            let sizes = Coordinator.widths(key)
            column.minWidth = sizes.0; column.width = sizes.1; column.maxWidth = sizes.2
            column.resizingMask = .userResizingMask
            // The delegate owns single-key sorting. A prototype would also enable
            // AppKit's independent multi-descriptor header tracking.
            column.sortDescriptorPrototype = nil
            table.addTableColumn(column)
        }
        if persistColumns {
            table.autosaveName = "DLM.Native.DownloadTable.v1"
            table.autosaveTableColumns = true
        }
        table.delegate = coordinator
        table.dataSource = coordinator
        scroll.documentView = table
        coordinator.table = table
        coordinator.update(self)
        return scroll
    }
    func updateNSView(_ scroll: NSScrollView, context: Context) { context.coordinator.update(self) }
    static func dismantleNSView(_ view: NSScrollView, coordinator: Coordinator) {
        coordinator.table?.delegate = nil
        coordinator.table?.dataSource = nil
        coordinator.table?.rowMenu = nil
        coordinator.table?.blocked = nil
    }

    final class Coordinator: NSObject, NSTableViewDelegate, NSTableViewDataSource {
        static let columns: [DownloadListOptions.Sort] = [.filename, .priority, .status, .size, .progress, .speed, .creationDate]
        static func widths(_ key: DownloadListOptions.Sort) -> (CGFloat, CGFloat, CGFloat) {
            switch key {
            case .filename: (180, 240, 1000)
            case .priority: (32, 44, 90)
            case .status: (110, 135, 400)
            case .size: (85, 95, 220)
            case .progress: (150, 170, 450)
            case .speed: (100, 105, 240)
            case .creationDate: (135, 150, 400)
            }
        }
        var state: NativeDownloadTable
        weak var table: DownloadTableView?
        private var updating = false
        private var synchronizingSort = false
        private var loaded = false
        private var renderedRows: [Transfer] = []
        init(_ state: NativeDownloadTable) { self.state = state }

        func update(_ next: NativeDownloadTable) {
            guard let table else { state = next; return }
            let old = state
            let oldRows = renderedRows
            let plan = loaded ? DownloadTableUpdate.plan(from: oldRows.map(\.id), to: next.rows.map(\.id)) : .reload
            let first = table.rows(in: table.visibleRect).location
            let anchor = oldRows.indices.contains(first) ? oldRows[first].id : nil
            let offset = oldRows.indices.contains(first) ? table.visibleRect.minY - table.rect(ofRow: first).minY : 0
            let x = table.enclosingScrollView?.contentView.bounds.minX ?? 0
            state = next
            updating = true
            defer { updating = false }
            table.setAccessibilityLabel(NativePreferences.text("Downloads"))
            table.setAccessibilityHelp(NativePreferences.text("Right-click or press Shift-F10 for download actions."))
            let rtl = next.direction == .rightToLeft
            table.userInterfaceLayoutDirection = rtl ? .rightToLeft : .leftToRight
            table.enclosingScrollView?.userInterfaceLayoutDirection = table.userInterfaceLayoutDirection
            let columns = rtl ? Array(Self.columns.reversed()) : Self.columns
            for (position, key) in columns.enumerated() {
                let id = NSUserInterfaceItemIdentifier(key.rawValue)
                let current = table.column(withIdentifier: id)
                if current != position { table.moveColumn(current, toColumn: position) }
                let column = table.tableColumns[position]
                column.title = NativePreferences.text(key.rawValue)
                column.headerToolTip = column.title
                column.headerCell.alignment = rtl ? .right : .left
            }
            synchronizeSortIndicator()
            switch plan {
            case .reload:
                renderedRows = next.rows
                table.reloadData() // Initial load, insertion/removal or filtering only.
            case .moves(let moves):
                NSAnimationContext.runAnimationGroup { context in
                    context.duration = 0
                    context.allowsImplicitAnimation = false
                    for move in moves {
                        renderedRows.insert(renderedRows.remove(at: move.from), at: move.to)
                        table.moveRow(at: move.from, to: move.to)
                    }
                    renderedRows = next.rows
                }
            case .cells:
                renderedRows = next.rows
            }
            loaded = true
            if plan != .cells, let scroll = table.enclosingScrollView {
                let clip = scroll.contentView
                let y = anchor.flatMap { id in renderedRows.firstIndex(where: { $0.id == id }) }
                    .map { table.rect(ofRow: $0).minY + offset } ?? clip.bounds.minY
                let desired = NSRect(origin: NSPoint(x: x, y: y), size: clip.bounds.size)
                // A record can move to the final row. Raw scroll(to:) accepts an
                // out-of-document origin; use the clip view's native bounds policy.
                DownloadTableUpdate.restoreViewport(desired, in: clip)
                scroll.reflectScrolledClipView(clip)
            }
            if plan != .reload {
                let previous = Dictionary(oldRows.map { ($0.id, $0) }, uniquingKeysWith: { _, latest in latest })
                let visible = table.rows(in: table.visibleRect)
                if visible.location != NSNotFound {
                    let end = min(renderedRows.count, visible.location + visible.length)
                    for (columnIndex, column) in table.tableColumns.enumerated() {
                        guard let key = DownloadListOptions.Sort(rawValue: column.identifier.rawValue) else { continue }
                        let changed = IndexSet((visible.location..<end).filter { index in
                            old.locale != next.locale || old.direction != next.direction
                                || previous[renderedRows[index].id].map { DownloadTableValues.changed(key, from: $0, to: renderedRows[index]) } != false
                        })
                        if !changed.isEmpty { table.reloadData(forRowIndexes: changed, columnIndexes: IndexSet(integer: columnIndex)) }
                    }
                }
            }
            let selected = IndexSet(next.rows.indices.filter { next.selection.contains(next.rows[$0].id) })
            if table.selectedRowIndexes != selected { table.selectRowIndexes(selected, byExtendingSelection: false) }
            if let id = next.revealID, let index = next.rows.firstIndex(where: { $0.id == id }) {
                table.scrollRowToVisible(index)
                // Never mutate SwiftUI bindings synchronously during updateNSView.
                DispatchQueue.main.async { [weak self] in
                    guard self?.state.revealID == id else { return }
                    self?.state.didReveal(id)
                }
            }
        }
        func numberOfRows(in tableView: NSTableView) -> Int { renderedRows.count }
        func tableViewSelectionDidChange(_ notification: Notification) {
            guard !updating, let table else { return }
            let ids = Set(table.selectedRowIndexes.compactMap { renderedRows.indices.contains($0) ? renderedRows[$0].id : nil })
            if ids != state.selection { state.selection = ids }
        }
        func tableView(_ tableView: NSTableView, didClick tableColumn: NSTableColumn) {
            guard !updating, !state.blocked,
                  let key = DownloadListOptions.Sort(rawValue: tableColumn.identifier.rawValue) else { return }
            let ascending = state.options.sort == key ? !state.options.ascending : true
            state.options.applyHeaderSort(key, ascending: ascending)
            synchronizeSortIndicator()
            // SwiftUI recomputes the filtered/ordered snapshot with the same helper,
            // then updateNSView applies ID moves and changed-cell updates.
        }
        func tableView(_ tableView: NSTableView, sortDescriptorsDidChange oldDescriptors: [NSSortDescriptor]) {
            guard !updating, !synchronizingSort else { return }
            // Restored/external native descriptors are presentation, not another
            // source of sort state. In particular, never adopt an arbitrary first key.
            synchronizeSortIndicator()
        }
        private func synchronizeSortIndicator() {
            guard let table, !synchronizingSort else { return }
            synchronizingSort = true
            defer { synchronizingSort = false }
            let descriptor = NSSortDescriptor(key: state.options.sort.rawValue, ascending: state.options.ascending)
            if table.sortDescriptors != [descriptor] { table.sortDescriptors = [descriptor] }
            let index = table.column(withIdentifier: NSUserInterfaceItemIdentifier(state.options.sort.rawValue))
            guard index >= 0 else { return }
            let active = table.tableColumns[index]
            if table.highlightedTableColumn !== active { table.highlightedTableColumn = active }
            let arrow = NSImage(named: state.options.ascending ? "NSAscendingSortIndicator" : "NSDescendingSortIndicator")
            for column in table.tableColumns {
                // AppKit archives prototypes with autosaved column configuration.
                if column.sortDescriptorPrototype != nil { column.sortDescriptorPrototype = nil }
                let desired = column === active ? arrow : nil
                if table.indicatorImage(in: column) !== desired { table.setIndicatorImage(desired, in: column) }
            }
        }
        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard renderedRows.indices.contains(row), let key = tableColumn.flatMap({ DownloadListOptions.Sort(rawValue: $0.identifier.rawValue) }) else { return nil }
            let id = NSUserInterfaceItemIdentifier(key.rawValue)
            let cell = tableView.makeView(withIdentifier: id, owner: self) as? DownloadTableCell ?? DownloadTableCell(key: key)
            cell.identifier = id
            cell.configure(renderedRows[row], key: key, rtl: state.direction == .rightToLeft)
            return cell
        }
        @objc func openDetails() {
            guard !state.blocked, let table, renderedRows.indices.contains(table.clickedRow) else { return }
            state.showDetails(renderedRows[table.clickedRow])
        }
        func menu(for index: Int) -> NSMenu? {
            guard !state.blocked, renderedRows.indices.contains(index) else { return nil }
            return makeMenu(state.actions(renderedRows[index]))
        }
        func makeMenu(_ commands: [NativeToolbarCommand]) -> NSMenu {
            let menu = NSMenu()
            menu.autoenablesItems = false
            for command in commands {
                let item = NSMenuItem(title: command.title, action: nil, keyEquivalent: "")
                item.isEnabled = command.enabled
                item.state = command.checked ? .on : .off
                if command.destructive && command.enabled {
                    // NSMenuItem has no public destructive-role property on macOS 13.
                    item.attributedTitle = NSAttributedString(string: command.title, attributes: [.foregroundColor: NSColor.systemRed, .font: NSFont.menuFont(ofSize: 0)])
                }
                if !command.children.isEmpty { item.submenu = makeMenu(command.children) }
                if let action = command.action {
                    let target = TableMenuAction(action)
                    item.target = target; item.action = #selector(TableMenuAction.run)
                    item.representedObject = target
                }
                menu.addItem(item)
            }
            return menu
        }
    }
}

private final class TableMenuAction: NSObject {
    let action: () -> Void
    init(_ action: @escaping () -> Void) { self.action = action }
    @objc func run() { action() }
}

final class DownloadTableView: NSTableView {
    var blocked: (() -> Bool)?
    var rowMenu: ((Int) -> NSMenu?)?
    override func mouseDown(with event: NSEvent) {
        guard blocked?() != true else { return }
        super.mouseDown(with: event)
    }
    override func selectAll(_ sender: Any?) {
        guard blocked?() != true else { return }
        super.selectAll(sender)
    }
    override func keyDown(with event: NSEvent) {
        guard blocked?() != true else { super.keyDown(with: event); return }
        if event.keyCode == 53, selectedRowIndexes.count > 0 {
            deselectAll(nil)
        } else if event.keyCode == 109, event.modifierFlags.contains(.shift), selectedRow >= 0 {
            let point = NSPoint(x: visibleRect.midX, y: rect(ofRow: selectedRow).midY)
            rowMenu?(selectedRow)?.popUp(positioning: nil, at: point, in: self)
        } else { super.keyDown(with: event) }
    }
    override func menu(for event: NSEvent) -> NSMenu? {
        guard blocked?() != true else { return nil }
        let index = row(at: convert(event.locationInWindow, from: nil))
        guard index >= 0 else { return nil }
        if !selectedRowIndexes.contains(index) { selectRowIndexes(IndexSet(integer: index), byExtendingSelection: false) }
        return rowMenu?(index)
    }
}

private final class DownloadTableCell: NSTableCellView {
    private let label = NSTextField(labelWithString: "")
    private let fileIcon = NSImageView()
    private let progress = NSProgressIndicator()
    private var priorityColor: NSColor?
    override var backgroundStyle: NSView.BackgroundStyle {
        didSet { label.textColor = priorityColor ?? (backgroundStyle == .emphasized ? .selectedControlTextColor : .labelColor) }
    }
    init(key: DownloadListOptions.Sort) {
        super.init(frame: .zero)
        label.font = .systemFont(ofSize: 13)
        label.lineBreakMode = .byTruncatingMiddle
        label.maximumNumberOfLines = 1
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        textField = label
        label.translatesAutoresizingMaskIntoConstraints = false
        addSubview(label)
        if key == .filename {
            imageView = fileIcon
            fileIcon.translatesAutoresizingMaskIntoConstraints = false
            addSubview(fileIcon)
            NSLayoutConstraint.activate([
                fileIcon.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
                fileIcon.widthAnchor.constraint(equalToConstant: 20), fileIcon.heightAnchor.constraint(equalToConstant: 20),
                fileIcon.centerYAnchor.constraint(equalTo: centerYAnchor),
                label.leadingAnchor.constraint(equalTo: fileIcon.trailingAnchor, constant: 7)
            ])
        } else if key == .progress {
            progress.style = .bar; progress.controlSize = .small
            progress.isIndeterminate = false; progress.minValue = 0; progress.maxValue = 1
            progress.translatesAutoresizingMaskIntoConstraints = false
            addSubview(progress)
            label.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
            NSLayoutConstraint.activate([
                progress.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6),
                progress.trailingAnchor.constraint(equalTo: label.leadingAnchor, constant: -7),
                progress.centerYAnchor.constraint(equalTo: centerYAnchor),
                label.widthAnchor.constraint(equalToConstant: 46)
            ])
        } else { label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 6).isActive = true }
        NSLayoutConstraint.activate([
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -6),
            label.centerYAnchor.constraint(equalTo: centerYAnchor)
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
    // Cells have no interactive subcontrols: leave selection, drag/range clicks and menus to NSTableView.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    func configure(_ row: Transfer, key: DownloadListOptions.Sort, rtl: Bool) {
        userInterfaceLayoutDirection = rtl ? .rightToLeft : .leftToRight
        let values = DownloadTableValues(row)
        priorityColor = nil
        switch key {
        case .filename:
            label.stringValue = values.filename
            fileIcon.image = NSWorkspace.shared.icon(for: UTType(filenameExtension: URL(fileURLWithPath: row.filename).pathExtension) ?? .data)
        case .status: label.stringValue = values.status
        case .size: label.stringValue = values.size
        case .progress:
            label.stringValue = values.percentage
            progress.doubleValue = values.fraction ?? 0
            progress.setAccessibilityValue(values.fraction == nil ? NativePreferences.text("Progress unknown") : values.percentage)
            progress.setAccessibilityLabel(NativePreferences.text("Progress"))
        case .speed: label.stringValue = values.speed
        case .creationDate: label.stringValue = values.created
        case .priority:
            label.stringValue = "●"
            priorityColor = row.priority.menuIndicatorColor
            label.setAccessibilityLabel(NativePreferences.text(row.priority.rawValue))
        }
        label.alignment = key == .priority ? .center : ([.size, .speed, .progress].contains(key) ? .right : .natural)
        label.textColor = priorityColor ?? (backgroundStyle == .emphasized ? .selectedControlTextColor : .labelColor)
        toolTip = key == .priority ? NativePreferences.text(row.priority.rawValue) : key == .status ? values.statusHelp : (key == .progress && values.fraction == nil ? NativePreferences.text("Progress unknown") : label.stringValue)
        setAccessibilityLabel(NativePreferences.text(key.rawValue))
        setAccessibilityValue(key == .priority ? NativePreferences.text(row.priority.rawValue) : key == .progress && values.fraction == nil ? NativePreferences.text("Progress unknown") : label.stringValue)
    }
}

extension TransferPriority {
    var menuIndicatorColor: NSColor { switch self { case .high: .systemRed; case .normal: .systemGreen; case .low: .systemOrange } }
}
