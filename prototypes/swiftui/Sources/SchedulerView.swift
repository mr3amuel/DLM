import SwiftUI

enum ScheduleKind: String, Codable, Sendable {
    case download = "Download", bandwidth = "Bandwidth"
}

struct ScheduleInput: Encodable, Sendable {
    struct LocalDate: Encodable, Sendable {
        let year: Int
        let month: Int
        let day: Int
    }
    var id = ""
    var name = ""
    var enabled = true
    var kind: ScheduleKind
    var downloadId: String?
    var date: LocalDate?
    var weekdays: UInt8
    var startMinute: Int
    var endMinute: Int
    var limit: UInt64?
    var atMs: Int64?
    var expectedRevision: UInt64?
}

struct MissedOccurrence: Encodable, Sendable {
    let id: String
    let timestamp: Int64
    func encode(to encoder: Encoder) throws {
        var values = encoder.unkeyedContainer()
        try values.encode(id)
        try values.encode(timestamp)
    }
}

struct ScheduleRule: Decodable, Equatable, Sendable {
    let id: String
    let name: String
    let enabled: Bool
    let kind: ScheduleKind
    let downloadId: String?
    let atMs: Int64?
    let weekdays: UInt8
    let startMinute: Int
    let endMinute: Int
    let bandwidthLimit: UInt64?
    let revision: UInt64
}

struct ScheduleEntry: Decodable, Equatable, Identifiable, Sendable {
    let rule: ScheduleRule
    let nextRunMs: Int64?
    let held: Bool
    let status: String
    let missedAtMs: Int64?
    let notifyMissed: Bool
    let pending: Bool
    var id: String { rule.id }
}

struct ScheduleSnapshot: Decodable, Equatable, Sendable {
    let rules: [ScheduleEntry]
    let effectiveLimit: UInt64?
    let manualOverride: Bool
    let error: String?

    var pendingDownloads: [ScheduleEntry] {
        rules.filter { $0.rule.kind == .download && $0.pending }.sorted {
            let a = $0.notifyMissed && $0.missedAtMs != nil
            let b = $1.notifyMissed && $1.missedAtMs != nil
            if a != b { return a }
            return ($0.nextRunMs ?? .max, $0.id) < ($1.nextRunMs ?? .max, $1.id)
        }
    }
    var missedDownloads: [ScheduleEntry] {
        pendingDownloads.filter { $0.notifyMissed && $0.missedAtMs != nil }
    }
    var missedOccurrences: [MissedOccurrence] {
        missedDownloads.compactMap { entry in
            entry.missedAtMs.map { MissedOccurrence(id: entry.id, timestamp: $0) }
        }
    }
    func editor(forDownload id: String) -> ScheduleEditorRequest {
        let rule = rules.first { $0.rule.kind == .download && $0.rule.downloadId == id }?.rule
        return ScheduleEditorRequest(kind: .download, rule: rule, downloadId: id)
    }
}

struct SchedulerView: View {
    @ObservedObject var model: DownloadModel
    @Environment(\.locale) private var locale
    @Environment(\.calendar) private var displayCalendar
    @State private var editor: ScheduleEditorRequest?
    @State private var removing: ScheduleEntry?
    @State private var choosingDownload = false
    @State private var pendingEditor: ScheduleEditorRequest?

    private var downloads: [ScheduleEntry] { model.schedules?.pendingDownloads ?? [] }
    private var bandwidth: [ScheduleEntry] { model.schedules?.rules.filter { $0.rule.kind == .bandwidth } ?? [] }

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                HStack(spacing: 16) {
                    DLMHeading(title: "Schedules", subtitle: "Automate your downloads and manage bandwidth rules")
                    Button { choosingDownload = true } label: { Label("New Schedule", systemImage: "calendar.badge.plus") }
                        .buttonStyle(DLMActionStyle(primary: true)).disabled(!model.ready || model.busy)
                }.padding(.bottom, 4)
                if let error = model.schedules?.error { Text(error).foregroundStyle(DLMDesign.failure) }
                if downloads.isEmpty {
                    VStack(spacing: 16) {
                        Image(systemName: "calendar.badge.clock")
                            .font(.system(size: 46, weight: .light)).foregroundStyle(DLMDesign.uiAccent)
                            .frame(width: 96, height: 96)
                            .background(DLMDesign.control.opacity(0.55), in: RoundedRectangle(cornerRadius: 24))
                            .accessibilityHidden(true)
                        Text("No schedules yet").font(.system(size: 23, weight: .semibold))
                        Text("Schedule downloads to start at a specific time or on selected days.")
                            .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center)
                            .frame(maxWidth: 460)
                        Button { choosingDownload = true } label: { Label("Create Schedule", systemImage: "calendar.badge.plus") }
                            .buttonStyle(DLMActionStyle(primary: true)).disabled(!model.ready || model.busy)
                    }.frame(maxWidth: .infinity).padding(.vertical, 32).padding(.horizontal, 24).dlmCard()
                } else {
                    VStack(spacing: 10) { ForEach(downloads) { scheduleCard($0) } }
                }
                Divider().overlay(DLMDesign.border)
                HStack(spacing: 16) {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Bandwidth Rules").font(.system(size: 20, weight: .semibold))
                        Text("Control download speed automatically using weekly time windows.")
                            .font(.callout).foregroundStyle(.secondary)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    Button { editor = ScheduleEditorRequest(kind: .bandwidth) } label: {
                        Label("Add Bandwidth Rule", systemImage: "speedometer")
                    }.buttonStyle(DLMActionStyle()).disabled(!model.ready || model.busy)
                }
                if model.schedules?.manualOverride == true {
                    Label("A manual bandwidth limit is overriding the current rule.", systemImage: "info.circle")
                        .font(.callout).foregroundStyle(.secondary)
                }
                if bandwidth.isEmpty {
                    HStack(spacing: 18) {
                        Image(systemName: "speedometer").font(.system(size: 25)).foregroundStyle(.secondary)
                            .frame(width: 48, height: 48)
                            .background(DLMDesign.control.opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 5) {
                            Text("No bandwidth rules").font(.headline)
                            Text("Limit download speed automatically during selected time windows.")
                                .font(.callout).foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 0)
                    }.padding(18).frame(maxWidth: .infinity, alignment: .leading).dlmCard()
                } else {
                    VStack(spacing: 10) { ForEach(bandwidth) { scheduleCard($0) } }
                }
            }.padding(DLMDesign.inset).padding(.top, 4)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(SidebarOverlayScroller())
        }
        .background(DLMDesign.canvas)
        .task(id: model.missedReviewRequest) {
            guard let request = model.missedReviewRequest else { return }
            guard let id = model.schedules?.missedDownloads.first?.id else {
                model.missedReviewRequest = nil
                return
            }
            await Task.yield()
            guard !Task.isCancelled, model.missedReviewRequest == request else { return }
            proxy.scrollTo(id, anchor: .top)
            model.missedReviewRequest = nil
        }
        }
        .sheet(isPresented: $choosingDownload, onDismiss: {
            if let request = pendingEditor { pendingEditor = nil; editor = request }
        }) {
            ScheduleDownloadPicker(model: model) { row in
                pendingEditor = model.schedules?.editor(forDownload: row.id)
                    ?? ScheduleEditorRequest(kind: .download, downloadId: row.id)
                choosingDownload = false
            }.nativeLanguage().dlmBrandColors()
        }
        .sheet(item: $editor) { ScheduleEditor(model: model, request: $0).nativeLanguage().dlmBrandColors() }
        .confirmationDialog("Delete schedule?", isPresented: Binding(
            get: { removing != nil }, set: { if !$0 { removing = nil } }
        ), presenting: removing) { entry in
            Button("Delete Schedule", role: .destructive) { send(EngineCommand(operation: .deleteSchedule, id: entry.id)) }
        } message: { _ in Text("The download and its files will be kept.") }
    }

    private func scheduleCard(_ entry: ScheduleEntry) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Image(systemName: entry.rule.kind == .bandwidth ? "speedometer" : "calendar")
                .font(.system(size: 23)).foregroundStyle(DLMDesign.uiAccent)
                .frame(width: 44, height: 44)
                .background(DLMDesign.control.opacity(0.5), in: RoundedRectangle(cornerRadius: 11))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                Text(entry.rule.name).font(.system(size: 15, weight: .semibold))
                if let id = entry.rule.downloadId, let row = model.rows.first(where: { $0.id == id }), row.filename != entry.rule.name {
                    Text(row.filename).font(.callout).lineLimit(2)
                }
                Text(recurrence(entry.rule)).font(.callout).foregroundStyle(.secondary)
                if entry.rule.kind == .bandwidth {
                    Text(entry.rule.bandwidthLimit.map { "\(speedLimitText($0)) kB/s" } ?? NativePreferences.text("Unlimited"))
                        .font(.callout).foregroundStyle(.secondary)
                }
                Text(LocalizedStringKey(entry.status)).font(.caption).foregroundStyle(.secondary)
                if entry.notifyMissed {
                    Label("Missed scheduled start", systemImage: "clock.badge.exclamationmark").font(.callout)
                }
                if let next = entry.nextRunMs {
                    HStack(spacing: 5) {
                        Text("Next start:")
                        Text(Date(timeIntervalSince1970: Double(next) / 1000), style: .date)
                        Text(Date(timeIntervalSince1970: Double(next) / 1000), style: .time)
                    }.font(.callout).foregroundStyle(.secondary)
                }
            }.frame(maxWidth: .infinity, alignment: .leading)
            Toggle("Enabled", isOn: Binding(get: { entry.rule.enabled }, set: {
                send(EngineCommand(operation: .enableSchedule, id: entry.id, enabled: $0))
            })).labelsHidden().dlmSwitchTint().disabled(model.busy)
            Menu {
                Button("Edit Schedule…") { editor = ScheduleEditorRequest(kind: entry.rule.kind, rule: entry.rule) }
                if let id = entry.rule.downloadId, model.rows.first(where: { $0.id == id })?.status.canResume == true {
                    Button("Start Now") { Task { await model.action(.resume, id: id) } }
                }
                if entry.held {
                    Button("Allow Scheduled Resume") { send(EngineCommand(operation: .rearmSchedule, id: entry.id)) }
                }
                if entry.notifyMissed, let timestamp = entry.missedAtMs {
                    Button("Dismiss Missed Start") {
                        send(EngineCommand(operation: .dismissMissed,
                            occurrences: [MissedOccurrence(id: entry.id, timestamp: timestamp)]))
                    }
                }
                Divider()
                Button("Delete Schedule", role: .destructive) { removing = entry }
            } label: { Image(systemName: "ellipsis").frame(width: 24, height: 28) }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .accessibilityLabel("Actions").disabled(model.busy)
        }.padding(18).dlmCard().id(entry.id)
    }

    private func recurrence(_ rule: ScheduleRule) -> String {
        if rule.atMs != nil { return NativePreferences.text("One-time start") }
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = locale
        let names = calendar.shortStandaloneWeekdaySymbols
        let days = (0..<7).filter { rule.weekdays & UInt8(1 << $0) != 0 }.map { names[($0 + 1) % 7] }.joined(separator: ", ")
        func clock(_ minute: Int) -> String {
            let date = displayCalendar.date(bySettingHour: minute / 60, minute: minute % 60, second: 0, of: .now) ?? .now
            let formatter = DateFormatter()
            formatter.locale = locale; formatter.calendar = displayCalendar; formatter.timeZone = .autoupdatingCurrent
            formatter.dateStyle = .none; formatter.timeStyle = .short
            return formatter.string(from: date)
        }
        return "\(days) · \(clock(rule.startMinute)) – \(clock(rule.endMinute))"
    }

    private func send(_ command: EngineCommand) { Task { await model.changeSchedule(command) } }
}

private struct ScheduleDownloadPicker: View {
    @ObservedObject var model: DownloadModel
    let choose: (Transfer) -> Void
    @Environment(\.dismiss) private var dismiss
    private var eligible: [Transfer] {
        model.rows.filter { [.ready, .paused, .queued, .failed].contains($0.status) }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            DLMHeading(title: "New Schedule", subtitle: "Choose a download to schedule.", compact: true)
            if eligible.isEmpty {
                Label("No downloads available to schedule", systemImage: "calendar.badge.exclamationmark")
                Text("Add a download first, or pause an active download before scheduling it.")
                    .foregroundStyle(.secondary)
            } else {
                ScrollView {
                    VStack(spacing: 8) {
                        ForEach(eligible) { row in
                            Button { choose(row) } label: {
                                HStack(spacing: 12) {
                                    DLMFileTile(filename: row.filename, size: 36)
                                    Text(row.filename).lineLimit(2)
                                    Spacer()
                                    Image(systemName: "chevron.forward").foregroundStyle(.secondary)
                                }.padding(10).dlmCard()
                            }.buttonStyle(.plain).disabled(model.busy)
                        }
                    }.background(SidebarOverlayScroller())
                }.frame(maxHeight: 300)
            }
            HStack { Spacer(); Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction) }
        }.padding(24).frame(width: 460).background(DLMDesign.canvas)
    }
}

struct ScheduleEditorRequest: Identifiable {
    let id = UUID()
    let kind: ScheduleKind
    var rule: ScheduleRule?
    var downloadId: String?
}

struct ScheduleEditor: View {
    @ObservedObject var model: DownloadModel
    let request: ScheduleEditorRequest
    @Environment(\.dismiss) private var dismiss
    @State private var draft = NativeScheduleDraft()
    @State private var error: String?
    @State private var saving = false
    @State private var loaded = false

    private var blockingReason: String? {
        if let original = request.rule {
            guard let current = model.schedules?.rules.first(where: { $0.id == original.id })?.rule else {
                return "This schedule was deleted. Close this editor and choose a download again."
            }
            if current.revision != original.revision || current.kind != original.kind || current.downloadId != original.downloadId {
                return "This schedule changed. Close and reopen it to load the current values."
            }
        }
        if request.kind == .download {
            guard let row = model.rows.first(where: { $0.id == (request.rule?.downloadId ?? request.downloadId) }),
                  row.status != .completed, row.status != .cancelled else {
                return "Choose an unfinished download from its menu."
            }
            if row.status == .running || row.status == .finalizing {
                return "Pause this download before scheduling; its current transfer will continue"
            }
        }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: DLMDesign.gap) {
            DLMHeading(title: request.kind == .bandwidth ? "Bandwidth Rule" : (request.rule == nil ? "Schedule Download" : "Edit Schedule…"), compact: true)
            DLMPanel(title: "Schedule") {
                TextField("Name", text: $draft.name)
                if request.kind == .bandwidth { Toggle("Enabled", isOn: $draft.enabled) }
                if request.kind == .download { Toggle("One-time start", isOn: Binding(
                    get: { draft.once }, set: { draft.setOneTime($0) })) }
                if request.kind == .download && draft.once {
                    DatePicker("Start", selection: $draft.start, displayedComponents: [.date, .hourAndMinute])
                } else {
                    NativeScheduleWeekdays(weekdays: $draft.weekdays)
                    DatePicker("From", selection: $draft.start, displayedComponents: [.hourAndMinute])
                    DatePicker("Until", selection: $draft.end, displayedComponents: [.hourAndMinute])
                }
                if request.kind == .bandwidth { TextField("Speed limit (kB/s)", text: $draft.limit, prompt: Text("Unlimited")) }
            }.disabled(blockingReason != nil)
            if let reason = blockingReason { Text(LocalizedStringKey(reason)).foregroundStyle(DLMDesign.failure) }
            if let error { Text(error).foregroundStyle(DLMDesign.failure) }
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Save") { save() }.keyboardShortcut(.defaultAction).disabled(blockingReason != nil)
            }
        }.padding(DLMDesign.inset).frame(width: 480).disabled(saving)
            .background(DLMDesign.canvas)
            .interactiveDismissDisabled(saving)
            .onReceive(NotificationCenter.default.publisher(for: .NSSystemTimeZoneDidChange)) { _ in
                draft.followSystemTimezone(kind: request.kind)
            }
            .onAppear {
                guard !loaded else { return }
                loaded = true
                draft = NativeScheduleDraft(rule: request.rule)
            }
    }

    private func save() {
        guard !saving, blockingReason == nil else { return }
        saving = true
        error = nil
        Task {
            defer { saving = false }
            do {
                let input = try draft.input(kind: request.kind, downloadID: request.downloadId, original: request.rule)
                _ = try await model.saveSchedule(input)
                dismiss()
            } catch { self.error = NativePreferences.text(error.localizedDescription) }
        }
    }
}
