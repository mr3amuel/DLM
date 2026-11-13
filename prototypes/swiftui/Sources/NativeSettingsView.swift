import AppKit
import SwiftUI

private enum SettingsSection: String, CaseIterable, Identifiable {
    case general = "General", folders = "Folders", transfers = "Transfers"
    case organization = "Organization", browsers = "Browsers"
    var id: Self { self }
    var icon: String {
        switch self {
        case .general: "gearshape"
        case .folders: "folder"
        case .transfers: "speedometer"
        case .organization: "square.stack"
        case .browsers: "puzzlepiece.extension"
        }
    }
    var detail: LocalizedStringKey {
        switch self {
        case .general: "Customize language, download defaults and notifications."
        case .folders: "Choose where DLM saves your downloads."
        case .transfers: "Configure download performance, bandwidth and retry behavior."
        case .organization: "Manage categories and destination rules for new downloads."
        case .browsers: "Connect Chrome and manage its secure connection to DLM."
        }
    }
}

struct NativeSettingsView: View {
    @ObservedObject var model: DownloadModel
    @Environment(\.dismiss) private var dismiss
    @AppStorage(NativePreferences.destinationKey) private var destination = ""
    @State private var section: SettingsSection = .general
    @State private var queue: QueueConfiguration?
    @State private var retry: RetryConfiguration?
    @State private var limit = ""
    @State private var savedTransferState = ""
    @State private var error: String?
    @State private var saving = false
    @State private var categoryEditor: CategoryEditorRequest?
    @State private var removing: Organization.Category?

    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 9) {
                    DLMBrandMark(size: 30)
                    Text("Settings").font(.system(size: 15, weight: .semibold))
                }.padding(.horizontal, 6).padding(.vertical, 4)
                VStack(spacing: 4) {
                    ForEach(SettingsSection.allCases) { item in
                        Button { section = item } label: {
                            Label(LocalizedStringKey(item.rawValue), systemImage: item.icon)
                                .font(.system(size: 13, weight: section == item ? .semibold : .medium))
                                .labelStyle(.titleAndIcon)
                                .foregroundStyle(section == item ? DLMDesign.selectionForeground : .primary)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 9).padding(.vertical, 9)
                                .background(section == item ? DLMDesign.selection : .clear,
                                            in: RoundedRectangle(cornerRadius: 9))
                                .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(
                                    section == item ? DLMDesign.brandSecondary : .clear))
                        }.buttonStyle(.plain).accessibilityAddTraits(section == item ? .isSelected : [])
                    }
                }
                Spacer(minLength: 0)
            }.padding(12).frame(width: 170)
                .background(DLMDesign.sidebar)
            Rectangle().fill(DLMDesign.border).frame(width: 1)
            VStack(spacing: 0) {
                HStack(alignment: .top, spacing: 10) {
                    Image(systemName: section.icon).font(.system(size: 18, weight: .medium))
                        .foregroundStyle(DLMDesign.brandSecondary).frame(width: 24, height: 24)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 4) {
                        Text(LocalizedStringKey(section.rawValue)).font(.system(size: 20, weight: .semibold))
                            .accessibilityAddTraits(.isHeader)
                        Text(section.detail).font(.system(size: 12)).foregroundStyle(DLMDesign.textSecondary)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }.padding(18)
                ScrollView {
                    VStack(alignment: .leading, spacing: 12) {
                        switch section {
                        case .general: GeneralNativeSettings()
                        case .folders: folders
                        case .transfers: transfers
                        case .organization: organization
                        case .browsers:
                            NativeChromeView(model: model, embedded: true)
                        }
                        if let error { Text(error).font(.callout).foregroundStyle(DLMDesign.failure) }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 18).padding(.bottom, 16)
                        .background(SidebarOverlayScroller())
                }
                Divider().overlay(DLMDesign.border)
                HStack {
                    if section == .transfers {
                        Button("Cancel") { Task { await load() } }
                            .buttonStyle(DLMActionStyle()).disabled(saving || queue == nil)
                        Text("Transfer changes take effect when applied.")
                            .font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        Button { applyTransfers() } label: { Label("Apply", systemImage: "checkmark") }
                            .buttonStyle(DLMActionStyle(primary: true))
                            .disabled(saving || queue == nil || transferState == savedTransferState)
                            .keyboardShortcut(.defaultAction)
                    } else {
                        Text(LocalizedStringKey(section == .browsers ? "Chrome installation and pairing require your consent." : "Changes are saved automatically.")).font(.caption).foregroundStyle(.secondary)
                        Spacer()
                        Button("Done") { dismiss() }.buttonStyle(DLMActionStyle())
                            .keyboardShortcut(.cancelAction)
                    }
                }.padding(.horizontal, 18).padding(.vertical, 12)
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .onAppear { showRequestedBrowserSetup() }
        .onChange(of: model.browserSetupRequested) { _ in showRequestedBrowserSetup() }
        .frame(idealWidth: 820, idealHeight: 540)
        .background(NativeSettingsWindow())
        .background(DLMDesign.canvas)
        .task(id: model.ready) { if model.ready { await load() } }
        .sheet(item: $categoryEditor) { CategoryEditor(model: model, request: $0).nativeLanguage().dlmBrandColors() }
        .confirmationDialog("Delete custom category?", isPresented: Binding(
            get: { removing != nil }, set: { if !$0 { removing = nil } }
        ), presenting: removing) { entry in
            Button("Delete Category", role: .destructive) {
                Task { await model.changeOrganization(EngineCommand(operation: .deleteCategory, id: entry.id)) }
            }
        }
    }

    private func showRequestedBrowserSetup() {
        if model.browserSetupRequested { section = .browsers; model.browserSetupRequested = false }
    }

    private var folders: some View {
        DLMSettingsGroup {
            DLMSettingsRow(title: "Default Download Folder", detail: "Destination for new downloads.", icon: "folder") {
                Button("Choose Folder…") {
                    if let selected = model.chooseFolder() { destination = selected }
                }.disabled(!model.ready)
            }
            Text(destination.isEmpty ? NativePreferences.text("Native development Downloads") : destination)
                .font(.callout).foregroundStyle(.secondary).textSelection(.enabled)
                .environment(\.layoutDirection, .leftToRight)
                .frame(maxWidth: .infinity, alignment: .leading).padding(.bottom, 15)
            Divider()
            DLMSettingsRow(title: "Folder Access", detail: "Grant persistent access to a selected folder.", icon: "lock.shield") {
                Button("Authorize Folder…") { _ = model.chooseFolder() }
            }
            Divider()
            DLMSettingsRow(title: "App Download Folder", detail: "Restore the isolated native app destination.", icon: "arrow.uturn.backward") {
                Button("Use App Download Folder") { destination = "" }.disabled(destination.isEmpty)
            }
            if let issue = model.folderAccessError { Text(issue).foregroundStyle(.secondary).padding(.bottom, 15) }
        }
    }

    @ViewBuilder private var transfers: some View {
        if let queueBinding = Binding($queue), let retryBinding = Binding($retry) {
            DLMSettingsGroup {
                DLMSettingsRow(title: "Simultaneous Downloads", detail: "Maximum number of downloads that can run at the same time.", icon: "list.bullet") {
                    numberControl(queueBinding.maxDownloads, range: 1...8, label: "Simultaneous Downloads")
                }
                Divider()
                DLMSettingsRow(title: "Global Connections", detail: "Maximum total number of concurrent connections.", icon: "point.3.connected.trianglepath.dotted") {
                    numberControl(queueBinding.globalConnections, range: 1...128, label: "Global Connections")
                }
                Divider()
                DLMSettingsRow(title: "Connections per Host", detail: "Maximum number of connections to a single server.", icon: "server.rack") {
                    numberControl(queueBinding.perHostConnections, range: 1...32, label: "Connections per Host")
                }
                Divider()
                DLMSettingsRow(title: "Speed Limit", detail: "Maximum download bandwidth. Leave blank for unlimited.", icon: "speedometer") {
                    HStack {
                        TextField("Unlimited", text: $limit).textFieldStyle(.roundedBorder)
                            .accessibilityLabel("Global speed limit (kB/s)")
                        Text(verbatim: "kB/s").foregroundStyle(.secondary)
                    }
                }
                Divider()
                DLMSettingsRow(title: "Retry Attempts", detail: "Total attempts allowed for a download, including the first attempt.", icon: "arrow.triangle.2.circlepath") {
                    numberControl(retryBinding.maxAttempts, range: 1...100, label: "Retry Attempts")
                }
                Divider()
                DLMSettingsRow(title: "Initial Retry Delay", detail: "Wait time before the first retry attempt.", icon: "clock") {
                    secondsControl(retryBinding.initialDelayMs, label: "Initial Retry Delay")
                }
                Divider()
                DLMSettingsRow(title: "Maximum Retry Delay", detail: "Upper limit for exponential retry delays.", icon: "clock.arrow.circlepath") {
                    secondsControl(retryBinding.maxDelayMs, label: "Maximum Retry Delay")
                }
                Divider()
                DLMSettingsRow(title: "Retry Jitter", detail: "Random variation applied to retry delays.", icon: "waveform.path") {
                    HStack {
                        numberControl(retryBinding.jitterPercent, range: 0...100, label: "Retry Jitter")
                        Text(verbatim: "%").foregroundStyle(.secondary)
                    }
                }
            }.disabled(saving)
        } else {
            Button("Load Queue Settings") { Task { await load() } }.disabled(!model.ready || saving)
        }
    }

    private func numberControl(_ value: Binding<UInt32>, range: ClosedRange<UInt32>, label: LocalizedStringKey) -> some View {
        HStack(spacing: 6) {
            TextField(label, value: value, format: NativeIntegerFormat()).textFieldStyle(.roundedBorder)
            Stepper(label, value: value, in: range).labelsHidden()
        }.frame(width: 95).accessibilityLabel(label)
    }

    private func secondsControl(_ value: Binding<UInt64>, label: LocalizedStringKey) -> some View {
        HStack {
            TextField(label, value: value, format: NativeMillisecondsFormat())
                .textFieldStyle(.roundedBorder)
            Text("seconds").foregroundStyle(.secondary)
        }
    }

    private var organization: some View {
        VStack(alignment: .leading, spacing: 16) {
            DLMSettingsGroup {
                DLMSettingsRow(title: "Automatic Organization", detail: "Use category destination rules for new downloads. Existing files stay in place.", icon: "square.stack") {
                    Toggle("Organize new downloads into categories", isOn: Binding(
                        get: { model.organization?.enabled ?? false },
                        set: { enabled in Task { await model.changeOrganization(EngineCommand(operation: .setOrganization, enabled: enabled)) } }
                    )).labelsHidden().dlmSwitchTint().disabled(!model.ready || model.busy)
                }
            }
            HStack {
                Text("Categories").font(.headline)
                Spacer()
                Button("Add Category…") { categoryEditor = CategoryEditorRequest() }
                    .buttonStyle(DLMActionStyle()).disabled(!model.ready || model.busy)
            }
            DLMSettingsGroup {
                ForEach(model.organization?.categories ?? []) { category in
                    HStack(spacing: 12) {
                        Image(systemName: "folder").foregroundStyle(DLMDesign.uiAccent)
                        VStack(alignment: .leading, spacing: 3) {
                            Text(LocalizedStringKey(category.name)).font(.callout.weight(.semibold))
                            Text(category.directory ?? NativePreferences.text("Default category folder"))
                                .font(.caption).foregroundStyle(.secondary).lineLimit(2)
                        }.frame(maxWidth: .infinity, alignment: .leading)
                        Button("Edit…") { categoryEditor = CategoryEditorRequest(category: category) }
                        if !category.builtin { Button("Delete", role: .destructive) { removing = category } }
                    }.padding(.vertical, 12).disabled(model.busy)
                    Divider()
                }
            }
            if let message = model.error { Text(message).foregroundStyle(DLMDesign.failure) }
        }
    }

    private var transferState: String {
        guard let queue, let retry else { return "" }
        return "\(queue.maxDownloads)|\(queue.globalConnections)|\(queue.perHostConnections)|\(limit)|\(retry.maxAttempts)|\(retry.initialDelayMs)|\(retry.maxDelayMs)|\(retry.jitterPercent)"
    }

    private func load() async {
        guard !saving else { return }
        saving = true
        defer { saving = false }
        do {
            let settings = try await model.require(EngineCommand(operation: .queueSettings), as: QueueSettings.self)
            queue = settings.queue; retry = settings.retry; limit = speedLimitText(settings.limit)
            savedTransferState = transferState
            error = nil
        } catch { self.error = error.localizedDescription }
    }

    private func applyTransfers() {
        guard let queue, let retry, !saving, model.ready else { return }
        saving = true; error = nil
        Task {
            defer { saving = false }
            do {
                let cap = try parseSpeedLimit(limit)
                guard (1...8).contains(queue.maxDownloads), (1...128).contains(queue.globalConnections),
                      (1...32).contains(queue.perHostConnections), (1...100).contains(retry.maxAttempts),
                      retry.initialDelayMs > 0, retry.initialDelayMs <= retry.maxDelayMs,
                      retry.maxDelayMs <= 86_400_000, retry.jitterPercent <= 100 else {
                    throw EngineFailure(code: "invalid", message: NativePreferences.text("Check transfer limits and retry delays."))
                }
                try await model.apply(EngineCommand(operation: .setQueue, queue: queue))
                try await model.apply(EngineCommand(operation: .globalLimit, limit: cap))
                try await model.apply(EngineCommand(operation: .setRetry, policy: retry))
                savedTransferState = transferState
            } catch { self.error = error.localizedDescription }
        }
    }
}

private struct CategoryEditorRequest: Identifiable {
    let id = UUID()
    var category: Organization.Category?
}

private struct CategoryEditor: View {
    @ObservedObject var model: DownloadModel
    let request: CategoryEditorRequest
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var folder: String?
    @State private var saving = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: DLMDesign.gap) {
            DLMHeading(title: "Category", compact: true)
            DLMPanel(title: "Destination") {
                TextField("Category name", text: $name).disabled(request.category?.builtin == true)
                Text(folder ?? "Default category folder").textSelection(.enabled)
                HStack {
                    Button("Choose Folder…") { if let selected = model.chooseFolder() { folder = selected } }
                    if folder != nil { Button("Use Default") { folder = nil } }
                }
            }
            if let error { Text(error).foregroundStyle(DLMDesign.failure) }
            HStack {
                Button("Cancel") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Save") { save() }.keyboardShortcut(.defaultAction)
            }
        }.padding(24).frame(width: 460)
            .disabled(saving).interactiveDismissDisabled(saving)
            .onAppear {
                name = request.category?.name ?? ""
                folder = request.category?.directory
            }
    }

    private func save() {
        saving = true
        Task {
            defer { saving = false }
            do {
                _ = try await model.require(EngineCommand(operation: .saveCategory,
                    id: request.category?.id, directory: folder, name: name), as: String.self)
                try await model.refreshOrganization()
                dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}
