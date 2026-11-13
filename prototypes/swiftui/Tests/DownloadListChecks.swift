import AppKit
import SwiftUI

@MainActor
enum DownloadListChecks {
    private static func row(_ id: String, _ name: String, _ created: UInt64?, _ total: UInt64?,
                            _ speed: Double, _ status: String, _ category: String?, _ url: String, downloaded: UInt64 = 0, priority: TransferPriority = .normal, partialPath: String? = nil) throws -> Transfer {
        let object: [String: Any] = ["id": id, "filename": name, "created_at_ms": created as Any? ?? NSNull(),
            "total": total as Any? ?? NSNull(), "speed": speed, "status": status,
            "category_id": category as Any? ?? NSNull(), "url": url, "destination": "/unused/\(name)",
            "downloaded": downloaded, "durable_bytes": 0, "resume_capability": "unknown", "workers": [],
            "priority": priority.rawValue, "partial_path": partialPath as Any? ?? NSNull(), "connections": 4, "optimize": true, "safe_pause": false,
            "active_connections": 0, "target_connections": 1, "segmented": false]
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(Transfer.self, from: JSONSerialization.data(withJSONObject: object))
    }

    static func run(model: DownloadModel) async throws {
        let source = try [row("a", "beta", 100, 100, 20, "Queued", "videos", "https://example.invalid/a"),
            row("b", "Alpha", 300, 300, 10, "Paused", "documents", "https://example.invalid/b"),
            row("c", "alpha", 100, 100, 20, "Running", nil, "https://example.invalid/SEARCH?token=private"),
            row("d", "Zulu", nil, nil, 0, "Cancelled", nil, "https://example.invalid/d")]
        var options = DownloadListOptions()
        let expected: [(DownloadListOptions.Sort, [String], [String])] = [
            (.creationDate, ["a", "c", "b", "d"], ["b", "a", "c", "d"]),
            (.filename, ["b", "c", "a", "d"], ["d", "a", "b", "c"]),
            (.size, ["a", "c", "b", "d"], ["b", "a", "c", "d"]),
            (.speed, ["d", "b", "a", "c"], ["a", "c", "b", "d"]),
            (.priority, ["a", "b", "c", "d"], ["a", "b", "c", "d"]),
            (.status, ["c", "a", "b", "d"], ["d", "b", "a", "c"]),
            (.progress, ["a", "b", "c", "d"], ["a", "b", "c", "d"]),
        ]
        for (sort, ascending, descending) in expected {
            options.sort = sort
            options.ascending = true
            precondition(options.select(source).map(\.id) == ascending)
            options.ascending = false
            precondition(options.select(source).map(\.id) == descending)
            // Input order never decides ties.
            precondition(options.select(Array(source.reversed())).map(\.id) == descending)
        }
        try tableChecks(source)
        options.sort = .speed
        options.ascending = false
        options.status = .running
        try options.setSizeBounds(minimum: "۰٫۰۰۰۱", maximum: "٠٫٠٠٠١")
        precondition(options.select(source, search: "  search?TOKEN  ", category: "other").map(\.id) == ["c"])
        precondition(options.select(source, search: "search", category: "videos").isEmpty)
        let before = options
        do { try options.setSizeBounds(minimum: "2", maximum: "1"); fatalError("Accepted reversed bounds") }
        catch is EngineFailure { precondition(options == before) }
        options.clearFilters()
        precondition(options.select(source, search: "ALPHA").map(\.id) == ["c", "b"])
        options.status = .queued
        precondition(options.select(source).map(\.id) == ["a"])
        options.clearFilters()
        options.maximumBytes = 100
        precondition(Set(options.select(source).map(\.id)) == ["a", "c"], "Unknown size passed a bound")
        let selected: Set<String> = ["a", "c", "removed"]
        precondition(DownloadListSelection.visible(selected, in: options.select(source)) == ["a", "c"])
        options.sort = .filename
        options.clearFilters()
        options.ascending = true
        let ordered = options.select(source)
        precondition(DownloadListSelection.range(from: "b", to: "a", in: ordered) == ["a", "b", "c"])
        precondition(DownloadListSelection.next("c", delta: 1, in: ordered) == "a")
        precondition(DownloadListSelection.next(nil, delta: -1, in: ordered) == "d")
        precondition(DownloadListSelection.next(nil, delta: 1, in: []) == nil)

        // Open Existing resets every hiding filter while preserving the chosen sort.
        options.status = .failed
        options.minimumBytes = .max
        precondition(options.select(model.rows, search: "hidden", category: "videos").isEmpty)
        try await model.locateExistingDownload("existing-id")
        options.clearFilters()
        let revealed = options.select(model.rows, search: "", category: nil)
        precondition(revealed.contains { $0.id == model.requestedDownloadID })
        precondition(options.sort == .filename && options.ascending && model.selection == "existing-id")
        model.finishLocatingDownload()

        precondition(!DownloadListKey.textEditing(marked: false, selectionLocation: NSNotFound))
        precondition(DownloadListKey.textEditing(marked: true, selectionLocation: NSNotFound))
        precondition(DownloadListKey.textEditing(marked: false, selectionLocation: 0))
        for editing in [false, true] {
            for blocked in [false, true] {
                let allowed = !editing && !blocked
                precondition(DownloadListKey.resolve(key: "a", code: 0, command: true, shift: false,
                    otherModifiers: false, editing: editing, blocked: blocked) == (allowed ? .selectAll : nil))
                precondition(DownloadListKey.resolve(key: "", code: 53, command: false, shift: false,
                    otherModifiers: false, editing: editing, blocked: blocked) == (allowed ? .clear : nil))
                precondition(DownloadListKey.resolve(key: "", code: 125, command: false, shift: true,
                    otherModifiers: false, editing: editing, blocked: blocked) == (allowed ? .move(1) : nil))
            }
        }
        precondition(DownloadListKey.resolve(key: "a", code: 0, command: true, shift: false,
            otherModifiers: true, editing: false, blocked: false) == nil)
        precondition(DownloadListKey.resolve(key: "f", code: 3, command: true, shift: false,
            otherModifiers: false, editing: false, blocked: false) == nil)
        precondition(DownloadListKey.resolve(key: "", code: 53, command: false, shift: false,
            otherModifiers: false, editing: false, blocked: false, selected: false) == nil,
            "Swallowed Escape with no selection (native window handling must remain available)")
        let all = DownloadListSelection.visible(Set(ordered.map(\.id)), in: ordered)
        precondition(all.count == ordered.count)

        var copied = ""
        let literal = "https://example.invalid/file?token=private&name=نام"
        precondition(NativeClipboard.copy(literal, write: { copied = $0; return true }) && copied == literal)
        precondition(NativeClipboard.copy("/not-yet-created/نام file.zip", write: { copied = $0; return true }))
        precondition(copied == "/not-yet-created/نام file.zip")
        precondition(!NativeClipboard.copy(literal, write: { _ in false }))
        precondition(lifecycle_count(Int32(TEST_PARTIAL)) == 0, "Partial info leaked into observation")
        let partial = try await model.require(EngineCommand(operation: .partialInfo, id: "existing-id"), as: PartialFileInfo.self)
        precondition(partial.exists && partial.fileLength == 123 && partial.retainedVerifiedBytes == 120)
        precondition(lifecycle_count(Int32(TEST_PARTIAL)) == 1)

        let byte = try NativeNumber.scaled("۰٫۰۰۰۰۰۱", places: 6, allowZero: true)
        let maximum = try NativeNumber.scaled("18446744073709.551615", places: 6, allowZero: true)
        let integer = try NativeIntegerFormat.Strategy().parse("٤٢٩٤٩٦٧٢٩٥")
        let milliseconds = try NativeMillisecondsFormat.Strategy().parse("۱۲٫۳۴۵")
        precondition(byte == 1 && maximum == UInt64.max && integer == UInt32.max && milliseconds == 12_345)
        for bad in ["-1", "+1", "1e3", "۱٬۰۰۰", "1,000", "1.", ".1", "1.0000001", "18446744073709551616"] {
            do { _ = try NativeNumber.scaled(bad, places: 6, allowZero: true); fatalError("Accepted invalid numeric input") }
            catch is EngineFailure {}
        }
        let editedUnlimited = try EditDownloadSpeed.bytes(custom: false, text: "1")
        let editedKB = try EditDownloadSpeed.bytes(custom: true, text: "1")
        let editedByte = try EditDownloadSpeed.bytes(custom: true, text: "۰٫۰۰۱")
        precondition(editedUnlimited == nil && editedKB == 1000 && editedByte == 1)
        for stored in [UInt64(1), 999, 1000, 1_000_000, UInt64.max] {
            let edited = try EditDownloadSpeed.bytes(custom: true, text: speedLimitText(stored))
            precondition(edited == stored)
        }
        for invalid in ["", " ", "0", "-1", "1e3", "1,000", "۱٫۲۳۴۵"] {
            do { _ = try EditDownloadSpeed.bytes(custom: true, text: invalid); fatalError("Accepted invalid edit speed") }
            catch is EngineFailure {}
        }
        let unlimited = try parseSpeedLimit("")
        let speed = try parseSpeedLimit("٠.٠٠١")
        let maxSpeed = try parseSpeedLimit("18446744073709551.615")
        precondition(unlimited == nil && speed == 1 && maxSpeed == UInt64.max)
        let roundTrip = try parseSpeedLimit(speedLimitText(maxSpeed))
        precondition(roundTrip == maxSpeed)
        for bad in ["0", "-1", "1.2345", "1e5", "12.", "18446744073709551.616"] {
            do { _ = try parseSpeedLimit(bad); fatalError("Accepted invalid speed limit") }
            catch is EngineFailure {}
        }
        do { _ = try NativeIntegerFormat.Strategy().parse("4294967296"); fatalError("Accepted overflow") }
        catch is EngineFailure {}
        // Foundation date parsing accepts these digits without changing calendar/instant semantics.
        let dates = DateFormatter()
        dates.locale = NativePreferences.locale(for: "en")
        dates.calendar = Calendar(identifier: .gregorian)
        dates.timeZone = TimeZone(secondsFromGMT: 0)
        dates.dateFormat = "yyyy-MM-dd HH:mm"
        let expectedDate = dates.date(from: "2026-11-12 09:30")
        precondition(expectedDate != nil && dates.date(from: "۲۰۲۶-١١-۱۲ ٠٩:۳۰") == expectedDate)
        try localization()
        print("Download list checks passed: fourteen header sort orders, table mapping/live updates, combined filters, selection/reveal, key routing, copy/partial info, localization/numeric input")
    }

    private static func tableChecks(_ source: [Transfer]) throws {
        let menu = MenuBarSummary(rows: source)
        precondition(!MenuBarLifecycle.terminateAfterLastWindow)
        precondition(menu.active.count == 1 && menu.queued.count == 1 && menu.speed == 20)
        precondition(menu.pauseIDs == ["a", "c"] && menu.resumeIDs == ["b"])
        precondition(MenuBarSummary(rows: []).statusText.isEmpty)
        precondition(MenuBarSummary(rows: [source[1]]).symbol == menu.symbol)
        precondition(menu.relevant.map(\.id) == ["c", "b"])
        let failure = try row("failure", "failed.zip", nil, nil, 0, "Failed", nil, "https://example.invalid/failed")
        let completed = try row("complete", "done.zip", 10, 100, 0, "Completed", nil, "https://example.invalid/done", downloaded: 100)
        precondition(MenuBarSummary(rows: [failure]).symbol == menu.symbol)
        precondition(MenuBarSummary(rows: [completed]).statusText.isEmpty)
        precondition(MenuBarSummary(rows: [completed]).resumeIDs.isEmpty)
        precondition(MenuBarSummary(rows: []).symbol == menu.symbol)
        precondition(MenuBarSummary(rows: [source[1]]).statusText.isEmpty)
        precondition(menu.statusText.hasPrefix("↓ "))
        precondition(MenuCenterLayout.actionCellWidth(360) == 72)
        precondition(MenuCenterLayout.actionCellWidth(335) * 5 == 335)
        precondition(MenuCenterLayout.contentHeight(0) == 0)
        precondition(MenuCenterLayout.contentHeight(55) == 55 && MenuCenterLayout.contentHeight(310) == 310)
        precondition(MenuCenterLayout.contentHeight(900) == 420)
        let artwork = NSImage(systemSymbolName: "chevron.forward.2", accessibilityDescription: nil)!
        artwork.isTemplate = false
        let originalSize = artwork.size
        let template = MenuBarBrandTemplate.makeTemplate(artwork)!
        precondition(template.isTemplate && !artwork.isTemplate && artwork.size == originalSize)
        precondition(abs(max(template.size.width, template.size.height) - 20) < 0.001)
        let activity = MenuBarSummary(rows: [completed, failure])
        precondition(activity.recent(completed: [completed]).map(\.id) == ["complete", "failure"])
        precondition(MenuBarSummary(rows: []).recent(completed: [completed]).isEmpty)
        precondition(MenuBarSummary(rows: [failure]).recent(completed: [completed]).map(\.id) == ["failure"])

        var options = DownloadListOptions()
        for mode in DownloadListOptions.Sort.allCases {
            let ascending = NSSortDescriptor(key: mode.rawValue, ascending: true)
            options.applyHeaderSort(mode, ascending: ascending.ascending)
            precondition(options.sort == mode && options.ascending)
            let reversed = ascending.reversedSortDescriptor as! NSSortDescriptor
            options.applyHeaderSort(mode, ascending: reversed.ascending)
            precondition(options.sort == mode && !options.ascending)
        }
        let running = try row("live", "very-long-name.zip", 1_800_000_000_000, 200, 1200,
            "Running", nil, "https://example.invalid/live", downloaded: 50)
        let update = try row("live", "very-long-name.zip", 1_800_000_000_000, 200, 2400,
            "Running", nil, "https://example.invalid/live", downloaded: 150)
        let before = DownloadTableValues(running), after = DownloadTableValues(update)
        precondition(before.filename == running.filename && before.status == running.statusLabel)
        precondition(before.fraction == 0.25 && after.fraction == 0.75 && before.percentage != after.percentage)
        precondition(before.speed != after.speed && before.size == Transfer.bytes(200))
        precondition(before.created == NativePreferences.date(1_800_000_000_000))
        precondition(DownloadTableValues(source[1]).speed == "—", "Displayed stale speed for an inactive record")
        precondition(DownloadTableValues(source[3]).created == "—" && DownloadTableValues(source[3]).size == "—")
        precondition(DownloadTableValues(source[3]).fraction == nil && DownloadTableValues(source[3]).percentage == "—")
        for mode in DownloadListOptions.Sort.allCases {
            precondition(DownloadTableValues.changed(mode, from: running, to: update) == [.speed, .progress].contains(mode))
        }
        final class State {
            var options = DownloadListOptions()
            var selection: Set<String> = ["a", "c"]
        }
        let box = State()
        func view() -> NativeDownloadTable {
            NativeDownloadTable(rows: box.options.select(source), selection: Binding(get: { box.selection }, set: { box.selection = $0 }),
                options: Binding(get: { box.options }, set: { box.options = $0 }), blocked: false, revealID: nil,
                didReveal: { _ in }, showDetails: { _ in }, actions: { _ in [] })
        }
        let adapter = view()
        let coordinator = adapter.makeCoordinator()
        let scroll = adapter.makeScrollView(coordinator: coordinator, persistColumns: false)
        let native = scroll.documentView as! NSTableView
        scroll.setFrameSize(NSSize(width: 1100, height: 180)) // Offscreen fixture size only.
        let high = try row("high", "high.zip", nil, 100, 0, "Paused", nil, "https://example.invalid/high", priority: .high)
        let low = try row("low", "low.zip", nil, 100, 0, "Paused", nil, "https://example.invalid/low", priority: .low)
        precondition(high.priority.menuIndicatorColor == .systemRed && low.priority.menuIndicatorColor == .systemOrange)
        precondition(source[0].priority.menuIndicatorColor == .systemGreen)
        var ranked = DownloadListOptions(); ranked.sort = .priority; ranked.ascending = true
        precondition(ranked.select([low, source[0], high]).map(\.id) == ["high", "a", "low"])
        precondition(DownloadTableValues.changed(.priority, from: high, to: low))
        for selected in TransferPriority.allCases {
            let choices = NativeToolbarCommand.priorityChoices(current: selected, enabled: true, change: { _ in })
            let menu = coordinator.makeMenu(choices)
            precondition(menu.items.filter { $0.state == .on }.count == 1)
            precondition(menu.items.first { $0.state == .on }?.title == NativePreferences.text(selected.rawValue))
        }
        let destructive = coordinator.makeMenu([NativeToolbarCommand(title: "Cancel Download", destructive: true, action: {}), NativeToolbarCommand(title: "Pause", action: {})])
        precondition(destructive.items[0].attributedTitle?.attribute(.foregroundColor, at: 0, effectiveRange: nil) as? NSColor == .systemRed)
        precondition(destructive.items[1].attributedTitle == nil)
        let incomplete = try row("partial", "file.zip", nil, 100, 0, "Paused", nil, "https://example.invalid/file", partialPath: "/partial/bytes.part")
        let complete = try row("complete", "file.zip", nil, 100, 0, "Completed", nil, "https://example.invalid/file")
        precondition(DownloadFileLocation.target(complete, readable: { url, _ in url.path == "/unused/file.zip" })?.path == "/unused/file.zip")
        precondition(DownloadFileLocation.target(incomplete, readable: { url, _ in url.path == "/partial/bytes.part" })?.path == "/partial/bytes.part")
        precondition(DownloadFileLocation.target(incomplete, readable: { url, directory in directory && url.path == "/unused" })?.path == "/unused")
        precondition(DownloadFileLocation.target(incomplete, readable: { _, _ in false }) == nil)
        for rtl in [false, true] {
            let centers = (0..<5).map { MenuQuickActionLayout.center(index: $0, count: 5, width: 360, rtl: rtl) }
            precondition(centers == (rtl ? [324,252,180,108,36] : [36,108,180,252,324]))
        }

        // Simulate column configuration restored from the pre-fix application.
        for column in native.tableColumns { column.sortDescriptorPrototype = NSSortDescriptor(key: column.identifier.rawValue, ascending: true) }
        coordinator.update(view())
        precondition(native.tableColumns.allSatisfy { $0.sortDescriptorPrototype == nil })
        var incoming = source
        func refreshedView() -> NativeDownloadTable {
            NativeDownloadTable(rows: box.options.select(incoming), selection: Binding(get: { box.selection }, set: { box.selection = $0 }),
                options: Binding(get: { box.options }, set: { box.options = $0 }), blocked: false, revealID: nil,
                didReveal: { _ in }, showDetails: { _ in }, actions: { _ in [] })
        }
        for cycle in 0..<4 {
            for key in NativeDownloadTable.Coordinator.columns {
                let column = native.tableColumns[native.column(withIdentifier: NSUserInterfaceItemIdentifier(key.rawValue))]
                for click in 0..<2 {
                    native.delegate?.tableView?(native, didClick: column)
                    precondition(box.options.sort == key && box.options.ascending == (click == 0))
                    coordinator.update(refreshedView()) // The real SwiftUI binding/snapshot round trip.
                    let expected = box.options.select(incoming)
                    precondition(native.numberOfRows == expected.count)
                    for index in expected.indices {
                        let cell = native.view(atColumn: 0, row: index, makeIfNecessary: true) as! NSTableCellView
                        let priorityColumn = native.column(withIdentifier: NSUserInterfaceItemIdentifier("Priority"))
                        let dot = native.view(atColumn: priorityColumn, row: index, makeIfNecessary: true) as! NSTableCellView
                        precondition(dot.toolTip == NativePreferences.text(expected[index].priority.rawValue))
                        precondition(dot.textField?.textColor == expected[index].priority.menuIndicatorColor)
                        precondition(cell.textField?.stringValue == expected[index].filename, "Cached row mismatch: \(key) \(click) row \(index): \(cell.textField?.stringValue ?? "nil") != \(expected[index].filename)")
                    }
                    let arrows = native.tableColumns.filter { native.indicatorImage(in: $0) != nil }
                    precondition(arrows.count == 1 && arrows[0] === column)
                    precondition(native.highlightedTableColumn === column)
                    precondition(native.sortDescriptors.count == 1 && native.sortDescriptors[0].key == key.rawValue
                        && native.sortDescriptors[0].ascending == box.options.ascending)
                    let image = NSImage(named: box.options.ascending ? "NSAscendingSortIndicator" : "NSDescendingSortIndicator")
                    precondition(native.indicatorImage(in: column) === image)
                    precondition(Set(native.selectedRowIndexes.map { expected[$0].id }) == box.selection)
                }
            }
            incoming[2] = try row("c", "alpha", 100, 100, Double(90 - cycle), "Running", nil,
                "https://example.invalid/live", downloaded: UInt64(cycle * 20), priority: cycle % 2 == 0 ? .high : .low)
            coordinator.update(refreshedView())
        }
        // Reproduce the old contract mismatch: native state contains two keys,
        // with the former key first. It must not steal authority/leave a stale arrow.
        let authoritative = box.options
        native.sortDescriptors = [NSSortDescriptor(key: "Filename", ascending: true), NSSortDescriptor(key: "File Size", ascending: false)]
        precondition(box.options == authoritative)
        precondition(native.sortDescriptors.count == 1 && native.sortDescriptors[0].key == authoritative.sort.rawValue)
        coordinator.update(refreshedView())
        precondition(native.numberOfRows == source.count)
        precondition(native.delegate?.responds(to: #selector(NSTableViewDelegate.tableView(_:didClick:))) == true)
        print("Native sorting integration passed: 56 header callbacks, bound options, real data-source cells, arrows, live snapshots and selection")


        // Repeated header permutations and live values must keep a complete,
        // index-synchronized snapshot and preserve selection by ID.
        var displayed = source
        let selected: Set<String> = ["a", "c"]
        for iteration in 0..<12 {
            let live = try row("c", "alpha", 100, 100, Double(iteration * 10), "Running", nil,
                "https://example.invalid/live", downloaded: UInt64(iteration * 7))
            let current = [source[0], source[1], live, source[3]]
            for mode in DownloadListOptions.Sort.allCases {
                options.applyHeaderSort(mode, ascending: iteration % 2 == 0)
                let next = options.select(current)
                let oldIDs = displayed.map(\.id), newIDs = next.map(\.id)
                let plan = DownloadTableUpdate.plan(from: oldIDs, to: newIDs)
                var mapped = oldIDs
                switch plan {
                case .moves(let moves):
                    for move in moves { mapped.insert(mapped.remove(at: move.from), at: move.to) }
                case .cells: break
                case .reload: fatalError("Reloaded a pure sorting permutation")
                }
                precondition(mapped == newIDs && Set(mapped) == Set(source.map(\.id)))
                let indexes = next.indices.filter { selected.contains(next[$0].id) }
                precondition(Set(indexes.map { next[$0].id }) == selected)
                displayed = next
            }
        }
        precondition(DownloadTableUpdate.plan(from: displayed.map(\.id), to: displayed.map(\.id)) == .cells)
        precondition(DownloadTableUpdate.plan(from: ["a", "b"], to: ["b"]) == .reload)
        precondition(DownloadTableUpdate.plan(from: ["b"], to: ["a", "b"]) == .reload)
        precondition(DownloadListSelection.visible(selected, in: [source[0], source[3]]) == ["a"])

        // Offscreen AppKit geometry only: no NSApplication/window/event automation.
        let clip = NSClipView(frame: NSRect(x: 0, y: 0, width: 200, height: 300))
        clip.documentView = NSView(frame: NSRect(x: 0, y: 0, width: 200, height: 330))
        let desired = NSRect(x: 0, y: 297, width: 200, height: 300) // Anchor moved to final row.
        clip.scroll(to: desired.origin)
        let rawOrigin = clip.bounds.minY
        let safe = clip.constrainBoundsRect(desired)
        precondition(safe.minY >= 0 && safe.maxY <= 330)
        DownloadTableUpdate.restoreViewport(desired, in: clip)
        precondition(clip.bounds.maxY <= 330)
        print("Table clip check: raw origin \(rawOrigin), constrained origin \(safe.minY)")
        clip.documentView?.setFrameSize(NSSize(width: 200, height: 100)) // Filter removed the old anchor.
        let expected = clip.constrainBoundsRect(desired).origin
        DownloadTableUpdate.restoreViewport(desired, in: clip)
        precondition(clip.bounds.origin == expected)
        let toolbar = NativeDownloadToolbar.Coordinator.itemIdentifiers(downloads: true).map(\.rawValue)
        precondition(toolbar.last == "dlm.details" && toolbar[toolbar.count - 2] == "dlm.search")
        precondition(NativeDownloadToolbar.Coordinator.itemIdentifiers(downloads: false).count == 2)
        let longName = String(repeating: "نام-file-", count: 20) + ".zip"
        let longRow = try row("long", longName, nil, nil, 0, "Paused", nil, "https://example.invalid/long")
        precondition(DownloadTableValues(longRow).filename == longName)

        for direction in [true, false] {
            options.applyHeaderSort(.progress, ascending: direction)
            let rows = options.select([source[3], update, running])
            precondition(rows.last?.id == "d")
            precondition(rows.first?.downloaded == (direction ? 50 : 150))
        }
    }

    private static func localization() throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("dlm-list-strings-\(UUID().uuidString)")
        defer { try? files.removeItem(at: root) }
        let bundleURL = root.appendingPathComponent("Strings.bundle")
        let contents = bundleURL.appendingPathComponent("Contents")
        let resources = contents.appendingPathComponent("Resources/fa.lproj")
        try files.createDirectory(at: resources, withIntermediateDirectories: true)
        try files.copyItem(at: URL(fileURLWithPath: "Resources/fa.lproj/Localizable.strings"),
                           to: resources.appendingPathComponent("Localizable.strings"))
        let info: [String: String] = ["CFBundleIdentifier": "com.dlm.test.strings", "CFBundlePackageType": "BNDL"]
        try PropertyListSerialization.data(fromPropertyList: info, format: .xml, options: 0)
            .write(to: contents.appendingPathComponent("Info.plist"))
        let bundle = Bundle(url: bundleURL)!
        let translated = NativePreferences.format("%@ of %@", "۱", "۲", language: "fa", bundle: bundle)
        // Foundation inserts directional isolates around interpolated arguments in RTL.
        // Keep these in the product; ignore only those marks when comparing visible wording.
        func wording(_ text: String) -> String {
            String(String.UnicodeScalarView(text.unicodeScalars.filter { ![0x2068, 0x2069].contains($0.value) }))
        }
        precondition(wording(translated) == "۱ از ۲")
        precondition(wording(NativePreferences.format("%@ of %@", "1", "2", language: "en", bundle: bundle)) == "1 of 2")
        for key in ["Open File Location", "The download location is missing or inaccessible.", "File", "Speed Limit", "Enter a positive speed limit in KB/s or choose Unlimited.", "Total Speed", "Recent Activity", "Show", "Details", "Open DLM", "Pause All", "Resume All", "No active downloads", "Completed this session", "More Details", "Source URL", "Copy Filename", "Status", "Progress", "Progress unknown", "Filter Downloads", "Right-click or press Shift-F10 for download actions.",
                    "Creation Date", "File Size…", "Copy Source URL", "Inspect Partial File", "Queued #%@",
                    "%@ items · %@ active, %@ paused", "Queue position %@", "Retry in %@ s",
                    "A browser download link is ready to review.", "Browser connection verified",
                    "Register or Repair Connection", "Retry Connection Test", "Not Now", "Set Up Chrome",
                    "Schedule type", "Weekly window", "Ends the next day · Local time",
                    "%@ downloads missed their scheduled start.", "Review", "Dismiss",
                    "They will not start automatically. Choose Start Now or a new time.",
                    "This schedule changed. Close and reopen it to load the current values.",
                    "This schedule was deleted. Close this editor and choose a download again."] {
            precondition(NativePreferences.text(key, language: "fa", bundle: bundle) != key, "Untranslated key: \(key)")
        }
    }
}
