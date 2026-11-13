import AppKit
import SwiftUI

struct DownloadsView: View {
    @ObservedObject var model: DownloadModel
    @Environment(\.layoutDirection) private var layoutDirection
    @AppStorage(NativePreferences.chromeInvitationKey) private var chromeInvitation = ""
    @State private var section: SectionKind? = .all
    @State private var cancelling: Transfer?
    @State private var editing: Transfer?
    @State private var replacingLink: Transfer?
    @State private var scheduleEditor: ScheduleEditorRequest?
    @State private var search = ""
    @State private var category: String?
    @State private var listOptions = DownloadListOptions()
    @State private var showingSizeFilter = false
    @State private var inspectedPartial: PartialFileInfo?
    @State private var inspectedPartialID: String?
    @State private var inspectedPartialStatus: Transfer.Status?
    @State private var inspectingPartial = false
    @State private var batchConfirmation: BatchOperation?
    @State private var batchIDs: Set<String> = []

    @State private var inspectorVisible = true
    @State private var showingInspector = false
    @State private var inspectorSchedule: ScheduleEditorRequest?
    @State private var searchFocusRequest = 0

    enum SectionKind: String, CaseIterable, Identifiable {
        case all = "All Downloads", active = "Active", paused = "Paused", completed = "Completed", failed = "Failed"
        case schedules = "Schedules"
        var id: Self { self }
        var icon: String {
            switch self {
            case .all: "square.stack"
            case .active: "play.circle"
            case .paused: "pause.circle"
            case .completed: "checkmark.circle"
            case .failed: "exclamationmark.triangle"
            case .schedules: "calendar"
            }
        }

        var isDownloads: Bool { self != .schedules }

        func includes(_ row: Transfer) -> Bool {
            switch self {
            case .all: true
            case .active: row.status.active
            case .paused: row.status == .paused
            case .completed: row.status == .completed
            case .failed: row.status == .failed
            case .schedules: false
            }
        }
    }

    private var currentSection: SectionKind { section ?? .all }

    private var rows: [Transfer] {
        listOptions.select(model.rows, search: search, category: category, including: currentSection.includes)
    }

    private var visibleSelection: Set<String> {
        model.selectedIDs.intersection(rows.map(\.id))
    }

    var body: some View {
        LatencyTrace.shared.measure("downloads_body_ms") { content }
    }

    private var content: some View {
        GeometryReader { geometry in
            let sidebarWidth = min(DLMDesign.sidebarRange.upperBound,
                                   max(DLMDesign.sidebarRange.lowerBound, geometry.size.width * 0.22))
            let inspectorWidth = min(DLMDesign.inspectorRange.upperBound,
                                     max(DLMDesign.inspectorRange.lowerBound, geometry.size.width * 0.298))
            let wide = geometry.size.width >= DLMDesign.inspectorBreakpoint
            HStack(spacing: 0) {
                sidebar.frame(width: sidebarWidth)
                verticalDivider
                VStack(spacing: 0) {
                    if chromeInvitation.isEmpty && model.ready && model.browserState?.enabled == false
                        && model.browserDraft == nil && !model.busy && !model.adding && model.error == nil
                        && editing == nil && scheduleEditor == nil && replacingLink == nil
                        && !showingInspector && !showingSizeFilter && cancelling == nil && batchConfirmation == nil
                        && !model.rows.contains(where: { [.running, .queued, .finalizing].contains($0.status) || $0.notice != nil }) {
                        HStack(spacing: DLMDesign.gap) {
                            Text("Connect Chrome optionally. You can also set it up later in Settings → Browsers.")
                            Spacer(minLength: 0)
                            chromeSetupLink
                            Button("Not Now") { chromeInvitation = "later" }
                        }.font(.callout)
                            .padding(.horizontal, DLMDesign.inset).padding(.vertical, 9)
                            .background(DLMDesign.selection)
                    }
                    if let draft = model.browserDraft {
                        HStack(spacing: DLMDesign.gap) {
                            Label("A browser download link is ready to review.", systemImage: "link")
                            Spacer()
                            Button("Dismiss") { Task { await model.dismissBrowserDraft(id: draft.id) } }
                                .disabled(model.busy || model.adding)
                            Button("Review Link") { model.reviewBrowserDraft() }
                                .disabled(model.busy || model.adding || editing != nil || scheduleEditor != nil
                                    || showingInspector || showingSizeFilter || replacingLink != nil || cancelling != nil || batchConfirmation != nil || model.error != nil)
                        }
                        .padding(.horizontal, DLMDesign.inset).padding(.vertical, 9)
                        .background(DLMDesign.selection)
                    }
                    if let missed = model.schedules?.missedDownloads, !missed.isEmpty {
                        HStack(spacing: DLMDesign.gap) {
                            Label(NativePreferences.format("%@ downloads missed their scheduled start.", NativePreferences.number(UInt64(missed.count))),
                                  systemImage: "clock.badge.exclamationmark")
                                .lineLimit(2)
                            Spacer(minLength: 0)
                            Button("Review") {
                                model.missedReviewRequest = UUID()
                                section = .schedules
                            }
                            Button("Dismiss") {
                                let keys = missed.compactMap { entry in entry.missedAtMs.map { MissedOccurrence(id: entry.id, timestamp: $0) } }
                                Task { await model.dismissMissedSchedules(keys) }
                            }
                        }
                        .font(.callout)
                        .help("They will not start automatically. Choose Start Now or a new time.")
                        .padding(.horizontal, DLMDesign.inset).padding(.vertical, 9)
                        .background(DLMDesign.selection)
                        .disabled(model.busy || model.adding || model.error != nil || editing != nil || replacingLink != nil
                            || scheduleEditor != nil || showingInspector || showingSizeFilter || cancelling != nil || batchConfirmation != nil)
                    }
                    if currentSection.isDownloads {
                        HStack(spacing: 0) {
                            transferWorkspace(wide: wide)
                            if wide && inspectorVisible {
                                verticalDivider
                                inspector.frame(width: inspectorWidth)
                            }
                        }
                    } else if currentSection == .schedules {
                        SchedulerView(model: model)
                    }
                }
            }
            .background(nativeToolbar(wide: wide))
            .background {
                if currentSection.isDownloads {
                    Button("Search downloads") { searchFocusRequest += 1 }
                        .keyboardShortcut("f", modifiers: .command).hidden().accessibilityHidden(true)
                    Button("Download Details") { toggleDetails(wide: wide) }
                        .keyboardShortcut("i", modifiers: [.command, .option]).hidden().accessibilityHidden(true)
                }
            }
            .onChange(of: wide) { if $0 { showingInspector = false } }
        }
        .background(DLMDesign.canvas)
        .frame(minWidth: 760, minHeight: 480)
        .onChange(of: model.requestedDownloadID) { id in
            if id != nil {
                section = .all; category = nil; search = ""
                listOptions.clearFilters()
            }
        }
        .onAppear { applyMenuBarSection() }
        .onChange(of: model.menuBarSection) { _ in applyMenuBarSection() }
        .onChange(of: section) { _ in reconcileSelection() }
        .onChange(of: category) { _ in reconcileSelection() }
        .onChange(of: search) { _ in reconcileSelection() }
        .onChange(of: listOptions) { _ in reconcileSelection() }
        .onChange(of: rows.map(\.id)) { _ in reconcileSelection() }
        .sheet(isPresented: $showingInspector, onDismiss: {
            if let request = inspectorSchedule {
                inspectorSchedule = nil
                scheduleEditor = request
            }
        }) {
            VStack(alignment: .leading, spacing: 0) {
                HStack {
                    Text("Download Details").font(.headline)
                    Spacer()
                    Button("Close") { showingInspector = false }.keyboardShortcut(.cancelAction)
                }.padding(DLMDesign.inset)
                Divider()
                inspector
            }
            .frame(width: 400, height: 500)
            .nativeLanguage().dlmBrandColors()
        }
        .dropDestination(for: URL.self) { items, _ in
            guard model.ready, !model.busy, !model.adding, items.count == 1,
                  let url = items.first, ["http", "https"].contains(url.scheme?.lowercased() ?? "") else { return false }
            model.draftURL = url.absoluteString
            model.adding = true
            return true
        }
        .sheet(isPresented: Binding(get: { model.adding && !model.standaloneAdding }, set: { if !model.standaloneAdding { model.adding = $0 } }), onDismiss: { model.browserReviewClosed() }) { AddDownloadView(model: model).nativeLanguage().dlmBrandColors() }
        .sheet(item: $editing) { TransferSettingsView(model: model, transfer: $0).nativeLanguage().dlmBrandColors() }
        .sheet(item: $scheduleEditor) { ScheduleEditor(model: model, request: $0).nativeLanguage().dlmBrandColors() }
        .sheet(item: $replacingLink) { ReplacementLinkView(model: model, transfer: $0).nativeLanguage().dlmBrandColors() }
        .confirmationDialog("Apply to selected downloads?", isPresented: Binding(
            get: { batchConfirmation != nil },
            set: { if !$0 { batchConfirmation = nil } }
        ), presenting: batchConfirmation) { action in
            Button(LocalizedStringKey(action == .pause ? "Pause Downloads" : "Confirm"), role: action == .pause ? nil : .destructive) {
                let ids = batchIDs
                Task { await model.batch(action, ids: ids, confirmed: true) }
            }
        } message: { action in
            switch action {
            case .pause: Text("Some selected downloads may not resume safely.")
            case .cancel: Text("Cancelling removes partial files.")
            case .remove: Text("Remove eligible records from the list. Completed files stay on disk.")
            default: Text("Apply this change to the selection.")
            }
        }
        .alert("Download Engine", isPresented: Binding(
            get: { model.error != nil },
            set: { if !$0 { model.error = nil } }
        )) {
            Button("OK") { model.error = nil }
        } message: {
            Text(model.error ?? "")
        }
        .confirmationDialog("Cancel download?", isPresented: Binding(
            get: { cancelling != nil },
            set: { if !$0 { cancelling = nil } }
        ), presenting: cancelling) { row in
            Button("Cancel Download", role: .destructive) { invoke(.cancel, row) }
        } message: { row in
            Text("The partial file for “\(row.filename)” will be removed.")
        }
    }

    private var verticalDivider: some View {
        Rectangle().fill(DLMDesign.border).frame(width: 1).accessibilityHidden(true)
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            HStack(spacing: 13) {
                DLMBrandMark(size: 42)
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: "DLM").font(.system(size: 23, weight: .bold))
                    Text("Download Manager").font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 16)
            .padding(.top, 9)
            .padding(.bottom, 19)
            .fixedSize(horizontal: false, vertical: true)

            NativeSidebarScroll {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(SectionKind.allCases.filter(\.isDownloads)) { item in
                        sidebarButton(LocalizedStringKey(item.rawValue), icon: item.icon,
                                      count: model.rows.lazy.filter(item.includes).count,
                                      selected: currentSection == item && category == nil) {
                            section = item
                            category = nil
                            listOptions.status = nil
                        }
                    }
                    if let categories = model.organization?.categories, !categories.isEmpty {
                        sidebarDivider
                        Text("Categories").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                            .padding(.leading, 10).padding(.bottom, 3)
                        ForEach(categories) { item in
                            sidebarButton(LocalizedStringKey(item.name), icon: categoryIcon(item.id),
                                          count: model.rows.lazy.filter { ($0.categoryId ?? "other") == item.id }.count,
                                          selected: currentSection.isDownloads && category == item.id) {
                                section = .all
                                category = item.id
                            }
                        }
                    }
                    sidebarDivider
                    ForEach([SectionKind.schedules]) { item in
                        sidebarButton(LocalizedStringKey(item.rawValue), icon: item.icon,
                                      selected: currentSection == item) { section = item }
                    }
                    settingsButton
                }
                .padding(.horizontal, 13)
                .padding(.vertical, 8)
            }
            .frame(maxHeight: .infinity)
            .clipped()
            TransferSpeedCard(speed: model.rows.reduce(0) { sum, row in
                sum + (row.status == .running && row.speed.isFinite && row.speed > 0 ? row.speed : 0)
            }).padding(14).fixedSize(horizontal: false, vertical: true)
        }
        .background(DLMDesign.sidebar)
    }

    private func requestChromeSetup() {
        chromeInvitation = "setup"
        model.browserSetupRequested = true
    }
    @ViewBuilder private var chromeSetupLink: some View {
        if #available(macOS 14, *) {
            BrowserSetupSettingsLink(prepare: requestChromeSetup)
        } else {
            Button("Set Up Chrome") {
                requestChromeSetup()
                NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }

    @ViewBuilder private var settingsButton: some View {
        if #available(macOS 14, *) {
            SettingsLink { settingsLabel }.buttonStyle(.plain)
        } else {
            Button {
                NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
                NSApp.activate(ignoringOtherApps: true)
            } label: { settingsLabel }.buttonStyle(.plain)
        }
    }

    private var settingsLabel: some View {
        Label("Settings", systemImage: "gearshape")
            .font(.system(size: 13, weight: .medium))
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 10).padding(.vertical, 9)
    }

    private var sidebarDivider: some View {
        Divider().overlay(DLMDesign.border).padding(.vertical, 8)
    }

    private func sidebarButton(_ title: LocalizedStringKey, icon: String, count: Int? = nil,
                               selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: icon).frame(width: 18).accessibilityHidden(true)
                Text(title).lineLimit(2).fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading).layoutPriority(1)
                if let count, count > 0 {
                    Text(NativePreferences.number(UInt64(count)))
                        .font(.caption.monospacedDigit()).fixedSize()
                        .padding(.horizontal, 6).padding(.vertical, 2)
                        .background(selected ? DLMDesign.selection : DLMDesign.badge, in: Capsule())
                }
            }
            .font(.system(size: 13, weight: selected ? .semibold : .medium))
            .foregroundStyle(selected ? DLMDesign.selectionForeground : .primary)
            .padding(.horizontal, 10).padding(.vertical, 6).frame(minHeight: 34)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .background(selected ? DLMDesign.selection : .clear, in: RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .help(Text(title))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func categoryIcon(_ id: String) -> String {
        switch id {
        case "videos": "film"
        case "music": "music.note"
        case "images": "photo"
        case "documents": "doc"
        case "archives": "archivebox"
        case "applications": "app"
        default: "shippingbox"
        }
    }

    private func toggleDetails(wide: Bool) {
        if wide { inspectorVisible.toggle() }
        else { showingInspector.toggle() }
    }

    private func nativeToolbar(wide: Bool) -> some View {
        let selected = rows.filter { model.selectedIDs.contains($0.id) }
        let available = !model.busy && !visibleSelection.isEmpty
        let canResume = available && selected.contains { $0.status.canResume }
        let canPause = available && selected.contains { $0.status.canPause }
        let canRemoveRecords = available && selected.contains(where: canRemove)
        let commands = [
            NativeToolbarCommand(title: NativePreferences.text("Pause"), enabled: canPause, action: { requestBatch(.pause) }),
            NativeToolbarCommand(title: NativePreferences.text("Resume"), enabled: canResume, action: { requestBatch(.resume) }),
            NativeToolbarCommand(title: NativePreferences.text("Assign Category"), enabled: available,
                children: (model.organization?.categories ?? []).map { entry in
                    NativeToolbarCommand(title: NativePreferences.text(entry.name), action: {
                        let ids = visibleSelection
                        Task { await model.batch(.category, ids: ids, category: entry.id) }
                    })
                }),
            NativeToolbarCommand(title: NativePreferences.text("Cancel Downloads"), enabled: available, action: { requestBatch(.cancel) }),
            NativeToolbarCommand(title: NativePreferences.text("Remove Records"), enabled: canRemoveRecords, action: { requestBatch(.remove) })
        ]
        return NativeDownloadToolbar(downloads: currentSection.isDownloads,
            addEnabled: model.ready && !model.busy && !model.adding,
            resumeEnabled: canResume, pauseEnabled: canPause, removeEnabled: canRemoveRecords,
            detailsTitle: wide && inspectorVisible ? "Hide Download Details" : "Show Download Details",
            detailsSymbol: layoutDirection == .rightToLeft ? "sidebar.left" : "sidebar.right",
            commands: commands, search: $search, searchFocusRequest: searchFocusRequest,
            add: { model.adding = true }, resume: {
                if let row = selected.first, selected.count == 1 {
                    invoke(row.status == .failed ? .retry : .resume, row)
                } else { requestBatch(.resume) }
            }, pause: { requestBatch(.pause) }, remove: { requestBatch(.remove) },
            details: { toggleDetails(wide: wide) })
    }

    private func transferWorkspace(wide: Bool) -> some View {
        let visibleRows = rows
        let active = visibleRows.filter { $0.status.active }.count
        let paused = visibleRows.filter { $0.status == .paused }.count
        return VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: DLMDesign.gap) {
                DLMHeading(title: LocalizedStringKey(listOptions.status?.rawValue ?? currentSection.rawValue),
                           subtitle: LocalizedStringKey(NativePreferences.format("%@ items · %@ active, %@ paused",
                               NativePreferences.number(UInt64(visibleRows.count)), NativePreferences.number(UInt64(active)),
                               NativePreferences.number(UInt64(paused)))))
                Picker("Category", selection: $category) {
                    Text("All Categories").tag(nil as String?)
                    ForEach(model.organization?.categories ?? []) { entry in
                        Text(LocalizedStringKey(entry.name)).tag(Optional(entry.id))
                    }
                }
                .labelsHidden().frame(maxWidth: 155)
                Menu {
                    Menu("Download Status") {
                        ForEach(SectionKind.allCases.filter(\.isDownloads)) { item in
                            Button(LocalizedStringKey(item.rawValue)) { section = item; listOptions.status = nil }
                        }
                        Divider()
                        ForEach(Transfer.Status.allCases, id: \.self) { status in
                            Button {
                                section = .all
                                listOptions.status = status
                            } label: {
                                if listOptions.status == status { Label(LocalizedStringKey(status.rawValue), systemImage: "checkmark") }
                                else { Text(LocalizedStringKey(status.rawValue)) }
                            }
                        }
                    }
                    Button { showingSizeFilter = true } label: {
                        if listOptions.minimumBytes != nil || listOptions.maximumBytes != nil {
                            Label("File Size…", systemImage: "checkmark")
                        } else { Text("File Size…") }
                    }
                    Button("Clear Filters") {
                        section = .all; category = nil; search = ""
                        listOptions.clearFilters()
                    }
                } label: {
                    Label("Filter Downloads", systemImage: "slider.horizontal.3").labelStyle(.iconOnly)
                }
                .menuIndicator(.hidden)
                .help("Filter Downloads")
                .fixedSize()
                .popover(isPresented: $showingSizeFilter) {
                    DownloadSizeFilter(options: $listOptions).nativeLanguage().dlmBrandColors()
                }
            }
            .padding(DLMDesign.inset)
            NativeDownloadTable(rows: visibleRows, selection: $model.selectedIDs, options: $listOptions,
                blocked: keyboardBlocked, revealID: model.requestedDownloadID,
                didReveal: { id in if model.requestedDownloadID == id { model.finishLocatingDownload() } },
                showDetails: { row in
                    model.selection = row.id
                    if wide { inspectorVisible = true } else { showingInspector = true }
                }, actions: { tableActions(for: $0, wide: wide) })
                .overlay(alignment: .topLeading) {
                    if visibleRows.isEmpty { emptyState.padding(.top, 28) }
                }

        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(LocalizedStringKey(model.ready ? "No downloads" : "Opening download engine…"))
                .font(.system(size: 17, weight: .semibold))
            if model.ready {
                Text(LocalizedStringKey(search.isEmpty && category == nil && currentSection == .all
                     && listOptions.status == nil && listOptions.minimumBytes == nil && listOptions.maximumBytes == nil
                     ? "Add a link to start your first download."
                     : "No downloads match the current view."))
                    .font(.callout).foregroundStyle(DLMDesign.textSecondary)
                Button { model.adding = true } label: { Label("Add Download", systemImage: "plus") }
                    .buttonStyle(DLMActionStyle(primary: true)).disabled(model.busy)
            } else {
                ProgressView().controlSize(.small)
            }
        }
        .padding(DLMDesign.inset)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var keyboardBlocked: Bool {
        model.adding || model.error != nil || editing != nil || replacingLink != nil || scheduleEditor != nil
            || showingInspector || showingSizeFilter || cancelling != nil || batchConfirmation != nil
            || !currentSection.isDownloads
    }

    private func applyMenuBarSection() {
        guard let requested = model.menuBarSection, let target = SectionKind(rawValue: requested) else { return }
        section = target; category = nil; search = ""; listOptions.clearFilters()
        model.menuBarSection = nil
    }

    private func reconcileSelection() {
        guard model.requestedDownloadID == nil else { return }
        let retained = DownloadListSelection.visible(model.selectedIDs, in: rows)
        if retained != model.selectedIDs { model.selectedIDs = retained }
    }

    private func canRemove(_ row: Transfer) -> Bool {
        row.status == .cancelled || row.status == .completed
    }

    private func tableActions(for row: Transfer, wide: Bool) -> [NativeToolbarCommand] {
        var commands: [NativeToolbarCommand] = []
        func add(_ title: String, enabled: Bool = true, destructive: Bool = false, action: @escaping () -> Void) {
            commands.append(NativeToolbarCommand(title: NativePreferences.text(title), enabled: enabled, destructive: destructive, action: action))
        }
        add("Show Download Details") {
            model.selection = row.id
            if wide { inspectorVisible = true } else { showingInspector = true }
        }
        add("Copy Source URL") { copy(row.url) }
        add("Open File Location") {
            if let target = DownloadFileLocation.target(row) { NSWorkspace.shared.activateFileViewerSelecting([target]) }
            else { model.error = NativePreferences.text("The download location is missing or inaccessible.") }
        }
        if row.status.canPause { add("Pause", enabled: !model.busy) { invoke(.pause, row) } }
        if row.status.canResume {
            add(row.status == .failed ? "Retry" : "Resume", enabled: !model.busy) {
                invoke(row.status == .failed ? .retry : .resume, row)
            }
        }
        if row.status == .cancelled {
            add("Restart from Beginning", enabled: !model.busy) { invoke(.restartCancelled, row) }
        }
        if row.status != .completed && row.status != .cancelled && row.status != .finalizing {
            let request = model.schedules?.editor(forDownload: row.id)
                ?? ScheduleEditorRequest(kind: .download, downloadId: row.id)
            add(request.rule == nil ? "Schedule Download…" : "Edit Schedule…", enabled: !model.busy) { scheduleEditor = request }
            add("Edit Download…", enabled: !model.busy) { editing = row }
            commands.append(NativeToolbarCommand(title: NativePreferences.text("Priority"), enabled: !model.busy && row.status != .running,
                children: NativeToolbarCommand.priorityChoices(current: row.priority, enabled: !model.busy && row.status != .running) { priority in
                    Task { await model.action(EngineCommand(operation: .priority, id: row.id, priority: priority)) }
                }))
            if row.status == .queued {
                add("Move Earlier in Queue", enabled: !model.busy) { reorder(row, direction: -1) }
                add("Move Later in Queue", enabled: !model.busy) { reorder(row, direction: 1) }
            }
        }
        if row.status == .paused || row.status == .failed || row.status == .cancelled {
            add("Update Download Link…", enabled: !model.busy) { replacingLink = row }
        }
        if row.status.canCancel { add("Cancel Download", enabled: !model.busy, destructive: true) { cancelling = row } }
        if row.status == .completed {
            add("Move to Folder…", enabled: !model.busy) {
                if let folder = model.chooseFolder() { Task { await model.move(id: row.id, directory: folder) } }
            }
            add("Reveal in Finder") { reveal(row) }
        }
        if canRemove(row) { add("Remove Record", enabled: !model.busy) { requestBatch(.remove, ids: [row.id]) } }
        return commands
    }

    private func reveal(_ row: Transfer) {
        let file = URL(fileURLWithPath: row.destination)
        let target = FileManager.default.fileExists(atPath: file.path) ? file : file.deletingLastPathComponent()
        NSWorkspace.shared.activateFileViewerSelecting([target])
    }

    private var inspector: some View {
        ScrollView {
            if let row = model.selected, visibleSelection.contains(row.id) {
                details(row).id(row.id)
            } else {
                Label("Select a download to see its details.", systemImage: "sidebar.right")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(DLMDesign.inset)
            }
        }
        .background(DLMDesign.canvas)
    }

    private func details(_ row: Transfer) -> some View {
        let file = DLMFileTile(filename: row.filename, size: 44)
        let values = DownloadTableValues(row)
        let schedule = model.schedules?.rules.first { $0.rule.downloadId == row.id && $0.rule.kind == .download }
        return VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .top, spacing: 10) {
                file.fixedSize()
                VStack(alignment: .leading, spacing: 4) {
                    Text(row.filename).font(.system(size: 15, weight: .semibold))
                        .lineLimit(2).truncationMode(.middle).fixedSize(horizontal: false, vertical: true)
                        .help(row.filename).textSelection(.enabled)
                        .contextMenu { Button("Copy Filename") { copy(row.filename) } }
                    HStack(spacing: 4) {
                        Text(file.fileType)
                        Text("·")
                        Text(values.size)
                    }.font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    Text(verbatim: values.status).font(.caption.weight(.medium))
                        .foregroundStyle(DLMDesign.statusColor(row.status))
                }.frame(maxWidth: .infinity, alignment: .leading).layoutPriority(1)
            }.padding(.vertical, 4)

            VStack(spacing: 12) {
                HStack(spacing: 8) {
                    ProgressView(value: values.fraction ?? 0).progressViewStyle(DLMProgress(height: 7))
                        .tint(DLMDesign.progress).accessibilityLabel("Download progress")
                    Text(values.percentage).font(.caption.weight(.semibold)).monospacedDigit()
                        .help(values.fraction == nil ? Text("Progress unknown") : Text(values.percentage))
                }
                HStack(spacing: 0) {
                    inspectorMetric("Speed", value: values.speed, icon: "arrow.down.to.line", accented: true)
                    verticalDivider.frame(height: 40)
                    inspectorMetric("Remaining", value: row.status == .running ? (row.etaSeconds.map { remaining($0) } ?? "—") : "—", icon: "clock")
                    verticalDivider.frame(height: 40)
                    inspectorMetric("Connections", value: NativePreferences.number(UInt64(row.activeConnections)), icon: "point.3.connected.trianglepath.dotted")
                }
            }.padding(.vertical, 4)
            Divider().overlay(DLMDesign.border)
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    Image(systemName: "folder").frame(width: 18).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Destination").font(.callout.weight(.medium))
                        Text((row.destination as NSString).abbreviatingWithTildeInPath)
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                            .help(row.destination).environment(\.layoutDirection, .leftToRight)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    Button("Reveal") { reveal(row) }.controlSize(.small)
                }
                inspectorMenuRow("Category", icon: "tag") {
                    Menu {
                        ForEach(model.organization?.categories ?? []) { item in
                            Button(LocalizedStringKey(item.name)) {
                                Task { await model.batch(.category, ids: [row.id], category: item.id) }
                            }
                        }
                    } label: {
                        Text(LocalizedStringKey(model.organization?.categories.first { $0.id == (row.categoryId ?? "other") }?.name ?? "None"))
                            .lineLimit(1)
                    }.disabled(model.busy || model.organization?.categories.isEmpty != false)
                }
                inspectorMenuRow("Schedule", icon: "calendar") {
                    if row.status != .completed && row.status != .cancelled && row.status != .finalizing {
                        Menu {
                            Button(LocalizedStringKey(schedule == nil ? "Schedule Download…" : "Edit Schedule…")) {
                                let request = model.schedules?.editor(forDownload: row.id)
                                    ?? ScheduleEditorRequest(kind: .download, downloadId: row.id)
                                if showingInspector {
                                    inspectorSchedule = request
                                    showingInspector = false
                                } else {
                                    scheduleEditor = request
                                }
                            }
                        } label: {
                            Text(LocalizedStringKey(schedule?.rule.name ?? "None")).lineLimit(1)
                        }.disabled(model.busy)
                    } else {
                        Text(LocalizedStringKey(schedule?.rule.name ?? "None")).foregroundStyle(.secondary)
                    }
                }
                if let next = schedule?.nextRunMs {
                    HStack(spacing: 4) {
                        Text("Next start:")
                        Text(Date(timeIntervalSince1970: Double(next) / 1000), style: .date)
                        Text(Date(timeIntervalSince1970: Double(next) / 1000), style: .time)
                    }.font(.caption).foregroundStyle(.secondary)
                }
            }
            Divider().overlay(DLMDesign.border)
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text("Source URL").font(.callout.weight(.medium))
                    Spacer(minLength: 4)
                    Button("Copy Source URL") { copy(row.url) }.controlSize(.small)
                }
                Text(row.url).font(.caption).textSelection(.enabled).lineLimit(2).truncationMode(.middle)
                    .help(row.url).environment(\.layoutDirection, .leftToRight)
            }
            Divider().overlay(DLMDesign.border)
            DisclosureGroup("More Details") {
                VStack(alignment: .leading, spacing: 6) {
                    Text(row.byteSummary)
                    Text(verbatim: row.statusLabel)
                    Text(LocalizedStringKey(row.resumeCapability.label))
                    Text(verbatim: NativePreferences.format("%@ saved · %@ priority", Transfer.bytes(row.durableBytes), NativePreferences.text(row.priority.rawValue)))
                    Text(verbatim: NativePreferences.format("Speed limit: %@", row.bandwidthLimit.map { Transfer.bytes($0) + "/s" } ?? NativePreferences.text("Unlimited")))
                    if let created = row.createdAtMs { metadata("Created", value: NativePreferences.date(created)) }
                    if let type = row.contentType { metadata("Content Type", value: type) }
                    if let position = row.queuePosition {
                        Text(verbatim: NativePreferences.format("Queue position %@", NativePreferences.number(UInt64(position))))
                    }
                    if let state = row.scheduleState { Text(LocalizedStringKey(state)) }
                    if let label = row.errorKindLabel { Text(verbatim: label) }
                    if let error = row.error { Text(error).textSelection(.enabled) }
                    if let notice = row.notice { Text(notice).textSelection(.enabled) }
                    Button("Inspect Partial File") { inspectPartial(row) }.disabled(inspectingPartial || model.busy)
                    if inspectedPartialID == row.id, inspectedPartialStatus == row.status, let partial = inspectedPartial {
                        Text(LocalizedStringKey(partial.exists ? "Partial file present" : "No partial file retained"))
                        if let length = partial.fileLength {
                            metadata("Partial file length", value: Transfer.bytes(length))
                            Text("Logical length may include sparse or unverified bytes.")
                        }
                        Text(verbatim: NativePreferences.format("%@ verified bytes retained", Transfer.bytes(partial.retainedVerifiedBytes)))
                    }
                    DisclosureGroup("Technical Details") {
                        VStack(alignment: .leading, spacing: 6) {
                            metadata("Download ID", value: row.id)
                            if let path = row.partialPath {
                                metadata("Partial path", value: path).environment(\.layoutDirection, .leftToRight)
                                if inspectedPartialID == row.id, inspectedPartialStatus == row.status, inspectedPartial?.exists == true {
                                    Button("Copy temporary path") { copy(path) }
                                    Button("Reveal partial in Finder") { revealPartial(path) }
                                    if row.status == .cancelled {
                                        Button("Delete partial data", role: .destructive) {
                                            inspectedPartialID = nil
                                            cancelling = row // Existing confirmed Cancel retries core cleanup only.
                                        }.disabled(model.busy)
                                    }
                                }
                            }
                            if let etag = row.etag { metadata("ETag", value: etag) }
                            if let modified = row.lastModified { metadata("Last Modified", value: modified) }
                            if let sha = row.sha256 { metadata("SHA-256", value: sha) }
                            if let resolved = row.resolvedUrl {
                                metadata("Resolved URL", value: resolved).environment(\.layoutDirection, .leftToRight)
                            }
                            if let explanation = row.resumeExplanation { Text(verbatim: explanation) }
                        }.padding(.top, 7)
                    }
                }.font(.caption).foregroundStyle(.secondary).padding(.top, 7)
                Button("Copy Destination Path") { copy(row.destination) }
                Button("Copy destination folder") { copy(URL(fileURLWithPath: row.destination).deletingLastPathComponent().path) }
                Divider().overlay(DLMDesign.border)
                DisclosureGroup("Connection Info") {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(verbatim: NativePreferences.format("%@ of %@ connections", NativePreferences.number(UInt64(row.activeConnections)), NativePreferences.number(UInt64(row.targetConnections))))
                        ForEach(row.workers) { worker in
                            ProgressView(value: worker.fraction) { Text(LocalizedStringKey(worker.phase)) }
                                .tint(worker.requestPermitHeld ? DLMDesign.progress : .secondary)
                        }
                    }.font(.caption).foregroundStyle(.secondary).padding(.top, 7)
                }
            }.font(.callout)
        }.padding(14)
    }

    private func metadata(_ title: LocalizedStringKey, value: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title)
            Text(value).textSelection(.enabled)
        }
    }

    private func copy(_ value: String) {
        if !NativeClipboard.copy(value) { model.error = NativePreferences.text("The text could not be copied.") }
    }

    private func revealPartial(_ path: String) {
        guard FileManager.default.fileExists(atPath: path) else {
            model.error = NativePreferences.text("The partial file is no longer available.")
            return
        }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
    }

    private func inspectPartial(_ row: Transfer) {
        guard !inspectingPartial else { return }
        inspectingPartial = true
        inspectedPartialID = nil
        Task {
            defer { inspectingPartial = false }
            do {
                let info = try await model.require(EngineCommand(operation: .partialInfo, id: row.id), as: PartialFileInfo.self)
                inspectedPartialID = row.id
                inspectedPartialStatus = row.status
                inspectedPartial = info
            } catch { model.error = error.localizedDescription }
        }
    }

    private func inspectorMetric(_ title: LocalizedStringKey, value: String, icon: String,
                                 accented: Bool = false) -> some View {
        VStack(spacing: 5) {
            Image(systemName: icon).font(.system(size: 17))
                .foregroundStyle(accented ? DLMDesign.uiAccent : .primary).accessibilityHidden(true)
            Text(value).font(.callout.weight(.semibold)).monospacedDigit().lineLimit(1)
                .minimumScaleFactor(0.75)
            Text(title).font(.caption).foregroundStyle(.secondary).lineLimit(1)
        }.frame(maxWidth: .infinity)
        .accessibilityElement(children: .combine)
    }

    private func inspectorMenuRow<Content: View>(_ title: LocalizedStringKey, icon: String,
                                                 @ViewBuilder content: () -> Content) -> some View {
        HStack(spacing: 10) {
            Label(title, systemImage: icon).font(.callout.weight(.medium))
            Spacer(minLength: 4)
            content().controlSize(.small).frame(maxWidth: 126, alignment: .trailing)
        }
    }

    private func invoke(_ command: EngineOperation, _ row: Transfer) {
        if command == .pause && row.status == .running && !row.safePause {
            batchIDs = [row.id]
            batchConfirmation = .pause
        } else if command == .pause {
            Task { await model.batch(.pause, ids: [row.id]) }
        } else {
            Task { await model.action(command, id: row.id) }
        }
    }

    private func requestBatch(_ action: BatchOperation, ids requested: Set<String>? = nil) {
        let selection = requested ?? visibleSelection
        let ids = action == .remove ? Set(rows.filter { selection.contains($0.id) && canRemove($0) }.map(\.id)) : selection
        guard !ids.isEmpty else { return }
        let unsafePause = action == .pause && rows.contains {
            ids.contains($0.id) && $0.status == .running && !$0.safePause
        }
        if action == .cancel || action == .remove || unsafePause {
            batchIDs = ids
            batchConfirmation = action
        } else {
            Task { await model.batch(action, ids: ids) }
        }
    }

    private func reorder(_ row: Transfer, direction: Int) {
        var ids = model.rows.filter { $0.status == .queued }
            .sorted { ($0.queuePosition ?? .max, $0.id) < ($1.queuePosition ?? .max, $1.id) }.map(\.id)
        guard let index = ids.firstIndex(of: row.id), ids.indices.contains(index + direction) else { return }
        ids.swapAt(index, index + direction)
        Task { await model.action(EngineCommand(operation: .reorder, ids: ids)) }
    }
}

func remaining(_ seconds: UInt64) -> String {
    let formatter = DateComponentsFormatter()
    var calendar = Calendar(identifier: .gregorian)
    calendar.locale = NativePreferences.locale
    formatter.calendar = calendar
    formatter.allowedUnits = [.hour, .minute, .second]
    formatter.unitsStyle = .abbreviated
    formatter.maximumUnitCount = 2
    return formatter.string(from: Double(seconds)) ?? "—"
}

private struct TransferSpeedCard: View {
    let speed: Double
    @State private var samples: [Double] = []

    var body: some View {
        HStack(spacing: 10) {
            Canvas { context, size in
                let maximum = max(samples.max() ?? 0, 1)
                guard samples.count > 1 else { return }
                var line = Path()
                for (index, sample) in samples.enumerated() {
                    let x = size.width * CGFloat(index) / CGFloat(samples.count - 1)
                    let y = size.height - 3 - (size.height - 6) * CGFloat(sample / maximum)
                    if index == 0 { line.move(to: CGPoint(x: x, y: y)) }
                    else { line.addLine(to: CGPoint(x: x, y: y)) }
                }
                var area = line
                area.addLine(to: CGPoint(x: size.width, y: size.height))
                area.addLine(to: CGPoint(x: 0, y: size.height))
                area.closeSubpath()
                context.fill(area, with: .color(DLMDesign.progress.opacity(0.13)))
                context.stroke(line, with: .color(DLMDesign.progress), style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
            }
            .frame(height: 34)
            .accessibilityHidden(true)
            HStack(spacing: 4) {
                Image(systemName: "arrow.down").foregroundStyle(DLMDesign.uiAccent).accessibilityHidden(true)
                Text(Transfer.bytes(UInt64(min(speed, Double(Int64.max / 2)))) + "/s")
                    .font(.caption.weight(.semibold)).monospacedDigit().lineLimit(1)
            }
            .fixedSize()
        }
        .padding(10).dlmCard()
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Download speed")
        .accessibilityValue(Text(Transfer.bytes(UInt64(min(speed, Double(Int64.max / 2)))) + "/s"))
        .onChange(of: speed) { value in
            samples.append(value)
            if samples.count > 30 { samples.removeFirst() }
        }
        .onAppear { if samples.isEmpty { samples = [speed] } }
    }
}

// Keep the SwiftUI content and AppKit's native thumb, while preventing a
// legacy scroller from reserving part of the sidebar's fixed width.
struct SidebarOverlayScroller: NSViewRepresentable {
    var managesContentInsets = false
    func makeNSView(context: Context) -> Probe {
        let view = Probe(frame: .zero)
        view.managesContentInsets = managesContentInsets
        return view
    }
    func updateNSView(_ view: Probe, context: Context) {}


    final class Probe: NSView {
        var managesContentInsets = false
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            NotificationCenter.default.removeObserver(self)
            guard window != nil else { return }
            NotificationCenter.default.addObserver(self, selector: #selector(preferenceChanged),
                name: NSScroller.preferredScrollerStyleDidChangeNotification, object: nil)
            preferenceChanged()
        }

        @objc private func preferenceChanged() {
            DispatchQueue.main.async { [weak self] in self?.useOverlay() }
        }

        private func useOverlay() {
            guard let scrollView = enclosingScrollView else { return }
            if scrollView.scrollerStyle != .overlay { scrollView.scrollerStyle = .overlay }
            if managesContentInsets {
                scrollView.automaticallyAdjustsContentInsets = false
                scrollView.contentInsets = NSEdgeInsetsZero
            }
        }
    }
}

@available(macOS 14, *)
private struct BrowserSetupSettingsLink: View {
    let prepare: () -> Void
    @Environment(\.openSettings) private var openSettings
    var body: some View {
        Button("Set Up Chrome") { prepare(); openSettings() }
    }
}
