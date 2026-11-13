import AppKit
import SwiftUI

func parseSpeedLimit(_ text: String) throws -> UInt64? {
    do { return try NativeNumber.scaled(text, places: 3, allowZero: false) }
    catch {
        throw EngineFailure(code: "invalid", message: NativePreferences.text("Enter a positive speed limit in kB/s with up to three decimal places, or leave it blank."))
    }
}

func speedLimitText(_ bytes: UInt64?) -> String {
    NativeNumber.text(bytes, places: 3)
}

struct AddDownloadView: View {
    @ObservedObject var model: DownloadModel
    var onOpenExisting: (() -> Void)? = nil
    @Environment(\.dismiss) private var dismiss
    @State private var draft = Draft(url: "", filename: "")
    @State private var limit = ""
    @State private var error: String?
    @State private var review: DraftReview?
    @State private var preview: DraftReview?
    @State private var previewDraft: Draft?
    @State private var previewError: String?
    @State private var sourceNotice: String?
    private var prepared: Draft { draft.prepared }
    @State private var submitted: Draft?
    @State private var busy = false
    @State private var scheduled = false
    @State private var scheduleDraft = NativeScheduleDraft()
    @State private var schedule: ScheduleInput?
    @State private var advanced = false
    @State private var customSpeed = false
    @FocusState private var urlFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 14) {
                DLMBrandMark(size: 42)
                DLMHeading(title: review == nil ? "Add Download" : "Possible duplicates",
                           subtitle: review == nil ? "Add a new download to DLM" : nil, compact: true)
            }
            .padding(.bottom, 3)

            if let review, let submitted {
                AddDownloadSection(title: "Destination", icon: "folder") {
                    Text(submitted.filename).font(.headline).lineLimit(2)
                    Text(review.directory).font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                        .environment(\.layoutDirection, .leftToRight)
                }
                AddDownloadSection(title: "Possible duplicates", icon: "square.on.square") {
                    ScrollView {
                        ForEach(Array(review.duplicates.enumerated()), id: \.offset) { _, entry in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(entry.filename).font(.callout.weight(.semibold))
                                Text(entry.reasons.joined(separator: ", ")).foregroundStyle(.secondary)
                                if !entry.id.isEmpty {
                                    Button("Open existing") {
                                        busy = true
                                        Task {
                                            defer { busy = false }
                                            do { try await model.locateExistingDownload(entry.id); onOpenExisting?(); await model.finishBrowserReview(); dismiss() }
                                            catch { self.error = error.localizedDescription }
                                        }
                                    }.disabled(busy)
                                }
                            }.frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }.frame(maxHeight: 92)
                }
            } else {
                ViewThatFits(in: .vertical) {
                    editor.fixedSize(horizontal: false, vertical: true)
                    ScrollView { editor }
                }
            }
            if let error = error ?? previewError { Text(error).font(.callout).foregroundStyle(DLMDesign.failure) }
            Divider().overlay(DLMDesign.border).padding(.top, 4)
            footer
        }
        .padding(DLMDesign.inset)
        .frame(width: 650)
        .frame(maxHeight: 630)
        .background(DLMDesign.canvas)
        .interactiveDismissDisabled(busy)
        .task(id: prepared) {
            let candidate = prepared
            preview = nil
            previewDraft = nil
            previewError = nil
            guard URL(string: candidate.url)?.host != nil else { return }
            do {
                let result = try await model.inspect(candidate)
                try Task.checkCancellation()
                preview = result
                previewDraft = candidate
            } catch is CancellationError {} catch {
                if !Task.isCancelled { previewError = error.localizedDescription }
            }
        }
        .task(id: draft.url.trimmingCharacters(in: .whitespacesAndNewlines)) {
            draft.source = nil
            sourceNotice = nil
            let candidate = prepared
            guard let url = URLComponents(string: candidate.url),
                  ["http", "https"].contains(url.scheme?.lowercased() ?? ""), url.host != nil,
                  url.user == nil, url.password == nil else { return }
            do {
                try await Task.sleep(for: .milliseconds(600))
                sourceNotice = NativePreferences.text("Inspecting source…")
                let source = try await model.inspectSource(url: candidate.url, filename: candidate.filename)
                try Task.checkCancellation()
                guard prepared.url == source.url else { return }
                sourceNotice = nil
                // Freeze the inspected destination/settings during a submission or duplicate choice.
                guard !busy, review == nil else { return }
                draft.source = source
            } catch is CancellationError {} catch {
                if !Task.isCancelled {
                    sourceNotice = NativePreferences.text("Source details unavailable; you can still download.")
                }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .NSSystemTimeZoneDidChange)) { _ in
            scheduleDraft.followSystemTimezone()
        }
        .onAppear {
            let defaults = UserDefaults.standard
            let connections = defaults.integer(forKey: NativePreferences.connectionsKey)
            draft.connections = [1, 2, 4, 8, 16, 32].contains(connections) ? UInt32(connections) : 4
            draft.optimize = defaults.object(forKey: NativePreferences.adaptiveKey) as? Bool ?? true
            draft.directory = defaults.string(forKey: NativePreferences.destinationKey).flatMap { $0.isEmpty ? nil : $0 }
            draft.url = model.draftURL
            if draft.url.isEmpty, defaults.object(forKey: NativePreferences.clipboardKey) as? Bool ?? true,
               let text = NSPasteboard.general.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines),
               let url = URLComponents(string: text), ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
               url.host != nil, url.user == nil, url.password == nil {
                draft.url = text
            }
            draft.filename = model.draftFilename
            draft.route = model.organization?.enabled == true
        }
    }

    private var editor: some View {
        VStack(spacing: 12) {
            AddDownloadSection(title: "Source", icon: "link") {
                HStack(spacing: 9) {
                    TextField("URL", text: $draft.url, prompt: Text("https://example.com/file.zip"))
                        .textFieldStyle(.plain)
                        .accessibilityLabel("Download URL")
                        .focused($urlFocused)
                    if !draft.url.isEmpty {
                        Button {
                            draft.url = ""
                            urlFocused = true
                        } label: {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain).accessibilityLabel("Clear URL")
                    }
                }
                .environment(\.layoutDirection, .leftToRight)
                .modifier(AddDownloadField(focused: urlFocused))
                HStack(spacing: 9) {
                    Image(systemName: "doc").foregroundStyle(.secondary).accessibilityHidden(true)
                    TextField("Filename (optional)", text: $draft.filename).textFieldStyle(.plain)
                }.modifier(AddDownloadField())
                if let sourceNotice { Text(sourceNotice).font(.caption).foregroundStyle(.secondary) }
                else if let source = prepared.source, source.total != nil || source.contentType != nil {
                    Text([source.total.map(Transfer.bytes), source.contentType].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            AddDownloadSection(title: "Destination", icon: "folder") {
                fieldRow("Category") {
                    fieldMenu {
                        categoryChoice("Automatic", id: nil)
                        ForEach(model.organization?.categories ?? []) { category in
                            categoryChoice(LocalizedStringKey(category.name), id: category.id)
                        }
                    } label: {
                        Text(LocalizedStringKey(model.organization?.categories
                            .first { $0.id == draft.category }?.name ?? "Automatic"))
                    }.accessibilityLabel("Category")
                }
                fieldRow("Save to") {
                    HStack(spacing: 10) {
                        HStack(spacing: 8) {
                            Image(systemName: "folder").foregroundStyle(DLMDesign.uiAccent).accessibilityHidden(true)
                            Text(previewDraft == prepared ? (preview?.directory ?? "") : NativePreferences.text("Resolving destination…"))
                                .lineLimit(1).truncationMode(.middle)
                            Spacer(minLength: 0)
                        }
                        .environment(\.layoutDirection, .leftToRight)
                        .modifier(AddDownloadField())
                        Button("Choose Folder…") {
                            if let folder = model.chooseFolder() {
                                draft.directory = folder
                                draft.route = false
                            }
                        }.buttonStyle(AddDownloadButton()).fixedSize()
                    }
                }
                if model.organization?.enabled == true {
                    fieldRow("Category routing") {
                        Toggle("Use category destination", isOn: $draft.route).toggleStyle(.checkbox)
                    }
                }
            }
            AddDownloadSection(title: "Transfer Options", icon: "slider.horizontal.3") {
                Toggle(isOn: $scheduled) {
                    Label("Start later", systemImage: "calendar")
                }.toggleStyle(.checkbox)
                if scheduled {
                    fieldRow("Schedule type") {
                        fieldMenu {
                            Button("One-time start") { scheduleDraft.setOneTime(true) }
                            Button("Weekly window") { scheduleDraft.setOneTime(false) }
                        } label: {
                            Text(LocalizedStringKey(scheduleDraft.once ? "One-time start" : "Weekly window"))
                        }
                    }
                    if scheduleDraft.once {
                        fieldRow("Start at") {
                            HStack(spacing: 10) {
                                AddDownloadDatePicker(date: $scheduleDraft.start, elements: .yearMonthDay)
                                    .accessibilityLabel("Date")
                                AddDownloadDatePicker(date: $scheduleDraft.start, elements: .hourMinute)
                                    .accessibilityLabel("Time")
                                Spacer(minLength: 0)
                            }
                        }
                    } else {
                        fieldRow("Days") { NativeScheduleWeekdays(weekdays: $scheduleDraft.weekdays) }
                        fieldRow("From") { AddDownloadDatePicker(date: $scheduleDraft.start, elements: .hourMinute) }
                        fieldRow("Until") { AddDownloadDatePicker(date: $scheduleDraft.end, elements: .hourMinute) }
                        if let spec = try? scheduleDraft.input(), spec.endMinute < spec.startMinute {
                            Text("Ends the next day · Local time").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                Divider().overlay(DLMDesign.border)
                DisclosureGroup("Advanced", isExpanded: $advanced) {
                    VStack(alignment: .leading, spacing: 10) {
                        fieldRow("Connections") {
                            fieldMenu {
                                ForEach([1, 2, 4, 8, 16, 32] as [UInt32], id: \.self) { count in
                                    Button(count.formatted()) { draft.connections = count }
                                }
                            } label: {
                                Text(draft.connections.formatted())
                            }.accessibilityLabel("Connections")
                        }
                        Toggle("Adaptive connections", isOn: $draft.optimize).toggleStyle(.checkbox)
                        Text("Adjust the number of connections based on network conditions.")
                            .font(.caption).foregroundStyle(.secondary).padding(.leading, 20)
                        fieldRow("Speed limit") {
                            fieldMenu {
                                Button("Unlimited") { customSpeed = false; limit = "" }
                                Button("Custom…") { customSpeed = true }
                            } label: {
                                Text(LocalizedStringKey(customSpeed ? "Custom…" : "Unlimited"))
                            }.accessibilityLabel("Speed limit")
                        }
                        if customSpeed {
                            fieldRow("Speed limit (kB/s)") {
                                TextField("Speed limit (kB/s)", text: $limit, prompt: Text("Unlimited"))
                                    .textFieldStyle(.plain).modifier(AddDownloadField())
                            }
                        }
                    }.padding(.top, 8)
                }
            }
        }.disabled(busy)
    }

    private var footer: some View {
        HStack {
            if review != nil, let submitted {
                Button("Back") { self.review = nil; error = nil }
                    .buttonStyle(AddDownloadButton()).disabled(busy)
                Spacer()
                Button("Create Separate Copy") {
                    submit(submitted, separateCopy: true)
                }.buttonStyle(AddDownloadButton(primary: true)).keyboardShortcut(.defaultAction).disabled(busy)
            } else {
                Button("Cancel") { dismiss() }
                    .buttonStyle(AddDownloadButton()).keyboardShortcut(.cancelAction).disabled(busy)
                Spacer()
                Button { createDownload() } label: {
                    HStack(spacing: 10) {
                        Text("Download")
                        Image(systemName: "arrow.forward").accessibilityHidden(true)
                    }
                }
                .buttonStyle(AddDownloadButton(primary: true))
                .keyboardShortcut(.defaultAction)
                .disabled(previewDraft != prepared || preview == nil || busy)
            }
        }
    }

    private func fieldRow<Content: View>(_ title: LocalizedStringKey,
                                         @ViewBuilder content: () -> Content) -> some View {
        HStack(spacing: 12) {
            Text(title).font(.callout.weight(.medium)).frame(width: 112, alignment: .leading)
            content().frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func fieldMenu<Options: View, LabelView: View>(@ViewBuilder options: () -> Options,
                                                            @ViewBuilder label: () -> LabelView) -> some View {
        Menu(content: options) {
            HStack {
                label().lineLimit(1)
                Spacer(minLength: 4)
                Image(systemName: "chevron.down").font(.caption.weight(.semibold))
                    .accessibilityHidden(true)
            }.modifier(AddDownloadField())
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden)
    }

    private func categoryChoice(_ label: LocalizedStringKey, id: String?) -> some View {
        Button { draft.category = id } label: {
            if draft.category == id { Label(label, systemImage: "checkmark") }
            else { Text(label) }
        }
    }

    private func createDownload() {
        guard !busy else { return }
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                var candidate = prepared
                candidate.limit = try parseSpeedLimit(limit)
                schedule = scheduled ? try scheduleDraft.input() : nil
                let inspected = try await model.inspect(candidate)
                if inspected.duplicates.isEmpty {
                    try await create(candidate, separateCopy: false)
                } else {
                    review = inspected
                    submitted = candidate
                }
            } catch { self.error = error.localizedDescription }
        }
    }

    private func create(_ candidate: Draft, separateCopy: Bool) async throws {
        _ = try await model.add(candidate, separateCopy: separateCopy, schedule: schedule)
        await model.finishBrowserReview()
        dismiss()
    }

    private func submit(_ candidate: Draft, separateCopy: Bool) {
        guard !busy else { return }
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await create(candidate, separateCopy: separateCopy)
            } catch {
                review = nil
                self.error = error.localizedDescription
            }
        }
    }
}

private struct AddDownloadDatePicker: NSViewRepresentable {
    @Binding var date: Date
    let elements: NSDatePicker.ElementFlags
    @Environment(\.locale) private var locale
    @Environment(\.calendar) private var calendar
    @Environment(\.timeZone) private var timeZone

    func makeCoordinator() -> Coordinator { Coordinator(date: $date) }

    func makeNSView(context: Context) -> NSDatePicker {
        let picker = NSDatePicker()
        picker.datePickerStyle = .textFieldAndStepper
        picker.datePickerElements = elements
        picker.presentsCalendarOverlay = elements == .yearMonthDay
        picker.target = context.coordinator
        picker.action = #selector(Coordinator.changed(_:))
        return picker
    }

    func updateNSView(_ picker: NSDatePicker, context: Context) {
        context.coordinator.date = $date
        if picker.locale != locale { picker.locale = locale }
        if picker.calendar != calendar { picker.calendar = calendar }
        if picker.timeZone != timeZone { picker.timeZone = timeZone }
        if picker.dateValue != date { picker.dateValue = date }
    }

    func sizeThatFits(_ proposal: ProposedViewSize, nsView: NSDatePicker, context: Context) -> CGSize? {
        nsView.fittingSize
    }

    final class Coordinator: NSObject {
        var date: Binding<Date>

        init(date: Binding<Date>) { self.date = date }

        @objc func changed(_ sender: NSDatePicker) { date.wrappedValue = sender.dateValue }
    }
}

private struct AddDownloadSection<Content: View>: View {
    let title: LocalizedStringKey
    let icon: String
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 11) {
            Label {
                Text(title).font(.system(size: 16, weight: .semibold)).accessibilityAddTraits(.isHeader)
            } icon: {
                Image(systemName: icon).font(.system(size: 18, weight: .medium))
                    .foregroundStyle(DLMDesign.uiAccent).frame(width: 24).accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 9) { content }
                .padding(.leading, 23)
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .dlmCard()
    }
}

private struct AddDownloadField: ViewModifier {
    var focused = false

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: 8)
        content
            .padding(.horizontal, 10)
            .frame(minHeight: 30)
            .background(DLMDesign.control.opacity(0.68), in: shape)
            .overlay {
                shape.strokeBorder(focused ? DLMDesign.focus : DLMDesign.border,
                                   lineWidth: focused ? 1.5 : 1)
                    .allowsHitTesting(false)
            }
    }
}

private struct AddDownloadButton: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    var primary = false
    @State private var hovered = false

    func makeBody(configuration: Configuration) -> some View {
        let shape = RoundedRectangle(cornerRadius: 8)
        configuration.label
            .font(.callout.weight(.semibold))
            .foregroundStyle(primary ? DLMDesign.actionForeground : .primary)
            .padding(.horizontal, primary ? 16 : 12)
            .frame(minHeight: 31)
            .background(primary ? (hovered ? DLMDesign.actionHover : DLMDesign.actionPrimary)
                        : (hovered ? DLMDesign.hoveredControl : DLMDesign.control), in: shape)
            .overlay {
                shape.strokeBorder(primary ? DLMDesign.uiAccent.opacity(0.65) : DLMDesign.border)
                    .allowsHitTesting(false)
            }
            .opacity(enabled ? (configuration.isPressed ? 0.76 : 1) : 0.46)
            .onHover { hovered = $0 }
    }
}

struct TransferOptions: View {
    @Binding var connections: UInt32
    @Binding var optimize: Bool
    @Binding var limit: String

    var body: some View {
        Picker("Connections", selection: $connections) {
            ForEach([1, 2, 4, 8, 16, 32] as [UInt32], id: \.self) { Text(String($0)).tag($0) }
        }
        Toggle("Adaptive connections", isOn: $optimize)
        TextField("Speed limit (kB/s)", text: $limit, prompt: Text("Unlimited"))
    }
}

struct TransferSettingsView: View {
    @ObservedObject var model: DownloadModel
    let transfer: Transfer
    @Environment(\.dismiss) private var dismiss
    @State private var filename = ""
    @State private var connections: UInt32 = 4
    @State private var optimize = true
    @State private var limit = ""
    @State private var customLimit = false
    @FocusState private var filenameFocused: Bool
    @State private var error: String?
    @State private var saving = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            DLMHeading(title: "Edit Download", compact: true)
            DLMPanel(title: "File") {
                LabeledContent("Filename") {
                    TextField("Filename", text: $filename).labelsHidden().focused($filenameFocused)
                        .disabled(![Transfer.Status.ready, .paused, .failed].contains(transfer.status))
                }
            }.disabled(saving)
            DLMPanel(title: "Transfer Options") {
                LabeledContent("Connections") {
                    Picker("Connections", selection: $connections) {
                        ForEach([1, 2, 4, 8, 16, 32], id: \.self) { value in Text(NativePreferences.number(UInt64(value))).tag(UInt32(value)) }
                    }.labelsHidden()
                }
                Toggle("Adaptive connections", isOn: $optimize)
                LabeledContent("Speed Limit") {
                    VStack(alignment: .leading, spacing: 6) {
                        Picker("Speed Limit", selection: $customLimit) {
                            Text("Unlimited").tag(false)
                            Text("Custom…").tag(true)
                        }.labelsHidden()
                        if customLimit {
                            HStack(spacing: 6) {
                                TextField("Speed Limit", text: $limit).environment(\.layoutDirection, .leftToRight)
                                Text("KB/s").foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }.disabled(saving)
            if let error { Text(error).foregroundStyle(DLMDesign.failure) }
            HStack {
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction).disabled(saving)
                Spacer()
                Button("Save Changes") { save() }.keyboardShortcut(.defaultAction).disabled(saving)
            }
        }.padding(DLMDesign.inset).frame(width: 470)
            .background(DLMDesign.canvas)
            .interactiveDismissDisabled(saving)
            .onAppear {
                filename = transfer.filename
                connections = transfer.connections
                optimize = transfer.optimize
                limit = speedLimitText(transfer.bandwidthLimit)
                customLimit = transfer.bandwidthLimit != nil
                filenameFocused = [.ready, .paused, .failed].contains(transfer.status)
            }
    }

    private func save() {
        guard !saving else { return }
        saving = true
        error = nil
        Task {
            defer { saving = false }
            do {
                let speed = try EditDownloadSpeed.bytes(custom: customLimit, text: limit)
                if filename != transfer.filename {
                    try await model.apply(EngineCommand(operation: .rename, id: transfer.id, filename: filename))
                }
                if connections != transfer.connections || optimize != transfer.optimize {
                    try await model.apply(EngineCommand(operation: .configure, id: transfer.id,
                        connections: connections, optimize: optimize))
                }
                if speed != transfer.bandwidthLimit {
                    try await model.apply(EngineCommand(operation: .speedLimit, id: transfer.id, limit: speed))
                }
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct ReplacementLinkView: View {
    @ObservedObject var model: DownloadModel
    let transfer: Transfer
    @Environment(\.dismiss) private var dismiss
    @State private var address = ""
    @State private var verification: LinkVerification?
    @State private var error: String?
    @State private var busy = false
    @State private var confirmRestart = false
    @State private var partial: PartialFileInfo?
    @State private var partialError = false
    @State private var committed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            DLMHeading(title: "Update Download Link", compact: true)
            DLMPanel(title: "Source") {
                Text(transfer.filename).font(.headline)
                Text("\(Transfer.bytes(transfer.durableBytes)) saved").foregroundStyle(.secondary)
                if let partial {
                    if partial.exists {
                        Text("\(Transfer.bytes(partial.retainedVerifiedBytes)) verified bytes retained").foregroundStyle(.secondary)
                    } else {
                        Text("No partial file retained. Only an explicit restart is available.").foregroundStyle(.secondary)
                    }
                } else if partialError {
                    Text("The partial file could not be inspected. Verify will report whether recovery is possible.").foregroundStyle(.secondary)
                }
                TextField("Replacement URL", text: $address)
                    .environment(\.layoutDirection, .leftToRight)
                    .onChange(of: address) { _ in verification = nil }
                    .disabled(busy || committed)
            }
            if let verification {
                Text(LocalizedStringKey(verification.outcome.label)).font(.headline)
                Text(verification.reason)
                Text("\(Transfer.bytes(verification.comparedBytes)) compared").foregroundStyle(.secondary)
            }
            if let error { Text(error).foregroundStyle(DLMDesign.failure) }
            HStack {
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Verify Link") { verify() }.disabled(busy || address.isEmpty || committed)
                if committed {
                    Button("Resume") { resumeCommitted() }.disabled(busy)
                } else if let verification, let token = verification.token {
                    if verification.safeToResume {
                        Button("Apply and Resume") { apply(token, restart: false) }.disabled(busy)
                    } else {
                        Button("Restart With Link", role: .destructive) { confirmRestart = true }.disabled(busy)
                    }
                }
            }.disabled(busy)
        }.padding(DLMDesign.inset).frame(width: 500)
            .background(DLMDesign.canvas)
            .interactiveDismissDisabled(busy)
            .task {
                if UserDefaults.standard.object(forKey: NativePreferences.clipboardKey) as? Bool ?? true,
                   address.isEmpty, let text = NSPasteboard.general.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines),
                   let url = URLComponents(string: text), ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
                   url.host != nil, url.user == nil, url.password == nil { address = text }
                do {
                    partial = try await model.require(EngineCommand(operation: .partialInfo, id: transfer.id), as: PartialFileInfo.self)
                } catch { partialError = true }
            }
            .confirmationDialog("Discard saved bytes and restart?", isPresented: $confirmRestart) {
                Button("Discard and Restart", role: .destructive) {
                    if let token = verification?.token { apply(token, restart: true) }
                }
            } message: {
                Text("This discards retained partial data and restarts from zero. The destination file will not be overwritten. This cannot be undone.")
            }
    }

    private func verify() {
        guard !busy, !committed else { return }
        verification = nil
        address = address.trimmingCharacters(in: .whitespacesAndNewlines)
        perform {
            verification = try await model.require(
                EngineCommand(operation: .verifyLink, id: transfer.id, url: address),
                as: LinkVerification.self
            )
        }
    }

    private func apply(_ token: String, restart: Bool) {
        perform {
            do {
                try await model.apply(EngineCommand(operation: .applyLink, id: transfer.id, token: token, restart: restart))
                committed = true
            } catch {
                verification = nil
                throw error
            }
            try await model.apply(EngineCommand(operation: .resume, id: transfer.id))
            dismiss()
        }
    }

    private func resumeCommitted() {
        perform {
            try await model.apply(EngineCommand(operation: .resume, id: transfer.id))
            dismiss()
        }
    }

    private func perform(_ action: @escaping @MainActor () async throws -> Void) {
        guard !busy else { return }
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do { try await action() }
            catch { self.error = error.localizedDescription }
        }
    }
}

// Edit uses the existing decimal KB/s contract: three decimal places preserve
// every integer byte/second, including previously stored sub-KB values.
enum EditDownloadSpeed {
    static func bytes(custom: Bool, text: String) throws -> UInt64? {
        guard custom else { return nil }
        do {
            if let value = try NativeNumber.scaled(text, places: 3, allowZero: false) { return value }
        } catch {}
        throw EngineFailure(code: "invalid", message: NativePreferences.text("Enter a positive speed limit in KB/s or choose Unlimited."))
    }
}
