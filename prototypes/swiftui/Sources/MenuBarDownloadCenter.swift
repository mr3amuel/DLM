import AppKit
import Combine
import SwiftUI

enum MenuBarLifecycle { static let terminateAfterLastWindow = false }

struct MenuBarSummary: Equatable {
    let rows: [Transfer]
    var active: [Transfer] { rows.filter { $0.status == .running || $0.status == .finalizing } }
    var queued: [Transfer] { rows.filter { $0.status == .queued }.sorted { ($0.queuePosition ?? .max, $0.id) < ($1.queuePosition ?? .max, $1.id) } }
    var speed: Double { active.reduce(0) { $0 + ($1.speed.isFinite ? max(0, $1.speed) : 0) } }
    var speedText: String { Transfer.bytes(UInt64(min(speed, Double(Int64.max / 2)))) + "/s" }
    var pauseIDs: Set<String> { Set(rows.filter { $0.status.canPause }.map(\.id)) }
    var resumeIDs: Set<String> { Set(rows.filter { $0.status.canResume }.map(\.id)) }
    var relevant: [Transfer] { active + rows.filter { $0.status == .paused } + rows.filter { $0.status == .ready } }
    var failed: [Transfer] { rows.filter { $0.status == .failed } }
    // Identity is stable in all states; failures remain visible in Recent Activity.
    var symbol: String { "chevron.forward.2" } // Diagnostic fallback if the bundled template cannot load.
    func recent(completed: [Transfer]) -> [Transfer] {
        Array(completed.filter { done in rows.contains { $0.id == done.id && $0.status == .completed } }.prefix(1)) + Array(failed.prefix(1))
    }
    var statusText: String {
        guard !active.isEmpty else { return "" }
        let count = active.count > 999 ? "999+" : NativePreferences.number(UInt64(active.count))
        return "↓ " + count + " · " + speedText
    }
}

@MainActor
final class MenuBarPresentation: ObservableObject {
    @Published private(set) var summary = MenuBarSummary(rows: [])
    @Published private(set) var completed: [Transfer] = []
    @Published private(set) var next: ScheduleEntry?
    @Published private(set) var controlsEnabled = false
    @Published private(set) var error: String?
    private var observations = Set<AnyCancellable>()
    private var prior: [String: Transfer.Status] = [:]
    init(model: DownloadModel) {
        Publishers.CombineLatest3(model.$ready, model.$busy, model.$adding)
            .map { $0 && !$1 && !$2 }.removeDuplicates()
            .sink { [weak self] in self?.controlsEnabled = $0 }.store(in: &observations)
        model.$error.removeDuplicates().sink { [weak self] in self?.error = $0 }.store(in: &observations)
        model.$rows.combineLatest(model.$schedules)
            .removeDuplicates { $0.0 == $1.0 && $0.1 == $1.1 }
            .throttle(for: .seconds(1), scheduler: DispatchQueue.main, latest: true)
            .sink { [weak self] rows, schedules in
                guard let self else { return }
                let newlyCompleted = rows.filter { $0.status == .completed && self.prior[$0.id] != nil && self.prior[$0.id] != .completed }
                self.prior = Dictionary(rows.map { ($0.id, $0.status) }, uniquingKeysWith: { _, latest in latest })
                if !newlyCompleted.isEmpty { self.completed = Array((newlyCompleted + self.completed.filter { old in !newlyCompleted.contains { $0.id == old.id } }).prefix(3)) }
                let live = Set(rows.map(\.id))
                self.completed = self.completed.filter { live.contains($0.id) }
                let updated = MenuBarSummary(rows: rows)
                if self.summary != updated { self.summary = updated }
                let next = schedules?.pendingDownloads.filter { $0.rule.enabled && $0.nextRunMs != nil && !$0.notifyMissed }
                    .min { $0.nextRunMs! < $1.nextRunMs! }
                if self.next != next { self.next = next }
            }.store(in: &observations)
    }
}

struct MenuBarDownloadCenter: View {
    let model: DownloadModel
    @ObservedObject var presentation: MenuBarPresentation
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismiss) private var dismiss
    @Environment(\.layoutDirection) private var layoutDirection
    @State private var confirmingPause: Set<String> = []
    private var enabled: Bool { presentation.controlsEnabled && presentation.error == nil }
    @State private var contentHeight: CGFloat = 0
    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                DLMBrandMark(size: 32)
                VStack(alignment: .leading, spacing: 1) {
                    Text("DLM").font(.system(size: 18, weight: .semibold))
                    Text("Download Manager").font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button { openMain() } label: { Label("Open DLM", systemImage: "arrow.up.right") }
                    .controlSize(.small)
            }.padding(16)
            Divider()
            HStack(spacing: 0) {
                metric("Total Speed", value: presentation.summary.active.isEmpty ? "—" : presentation.summary.speedText, icon: "arrow.down.to.line")
                Divider().frame(height: 32)
                metric("Active", value: NativePreferences.number(UInt64(presentation.summary.active.count)), icon: "play.fill")
                Divider().frame(height: 32)
                metric("Queued", value: NativePreferences.number(UInt64(presentation.summary.queued.count)), icon: "square.3.layers.3d")
            }.padding(.horizontal, 12).padding(.vertical, 12)
            Divider().padding(.horizontal, 16)
            ScrollView {
                activityContent.padding(16)
                    .background(GeometryReader { proxy in Color.clear.preference(key: MenuCenterHeight.self, value: proxy.size.height) })
            }
            .frame(height: MenuCenterLayout.contentHeight(contentHeight))
            .onPreferenceChange(MenuCenterHeight.self) { height in
                if abs(contentHeight - height) > 0.5 { contentHeight = height }
            }
            Divider().padding(.horizontal, 16)
            quickActions.padding(.horizontal, 10).padding(.vertical, 10)
        }.frame(width: 380).controlSize(.small)
        .confirmationDialog("Pause downloads that cannot safely resume?", isPresented: Binding(get: { !confirmingPause.isEmpty }, set: { if !$0 { confirmingPause = [] } })) {
            Button("Pause") {
                let ids = confirmingPause; confirmingPause = []
                Task { await model.batch(.pause, ids: ids, confirmed: true) }
            }
        } message: { Text("Some transfers may need to restart from the beginning after pausing.") }
    }

    private var activityContent: some View {
        let summary = presentation.summary
        let recent = summary.recent(completed: presentation.completed)
        return VStack(alignment: .leading, spacing: 12) {
            if summary.relevant.isEmpty {
                Text("No active downloads").font(.callout).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 4)
            } else {
                HStack {
                    Text(LocalizedStringKey(summary.active.isEmpty ? "Downloads" : "Active Downloads")).font(.headline)
                    Spacer()
                }
                ForEach(Array(summary.relevant.prefix(5))) { row in downloadRow(row) }
                if summary.relevant.count > 5 { Button("View All Downloads") { navigate("All Downloads") } }
            }
            if !recent.isEmpty {
                Divider()
                Text("Recent Activity").font(.subheadline.weight(.semibold))
                ForEach(recent) { row in
                    HStack(spacing: 8) {
                        Image(systemName: row.status == .completed ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                            .foregroundStyle(row.status == .completed ? DLMDesign.success : DLMDesign.failure)
                            .accessibilityHidden(true)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(row.filename).lineLimit(1).truncationMode(.middle).help(row.filename)
                            Text(LocalizedStringKey(row.status == .completed ? "Completed this session" : "Failed"))
                                .font(.caption).foregroundStyle(.secondary)
                        }.frame(maxWidth: .infinity, alignment: .leading)
                        if row.status == .completed {
                            Button("Show") { showCompleted(row) }.help("Reveal in Finder")
                        } else { Button("Details") { select(row) } }
                    }.font(.callout)
                }
            }
            if let queued = summary.queued.first {
                Button { select(queued) } label: { Label(queued.filename, systemImage: "list.bullet").lineLimit(1) }
                    .buttonStyle(.borderless).help("Next in Queue")
            }
            if let next = presentation.next, let time = next.nextRunMs {
                Button {
                    if let id = next.rule.downloadId, let row = model.rows.first(where: { $0.id == id }) { select(row) }
                } label: {
                    HStack { Image(systemName: "calendar"); Text(next.rule.name).lineLimit(1); Text(Date(timeIntervalSince1970: Double(time) / 1000), style: .time) }
                }.buttonStyle(.borderless).help("Next scheduled download")
            }
            if let error = presentation.error { Text(error).font(.caption).foregroundStyle(DLMDesign.failure).lineLimit(3) }
        }
    }
    private func downloadRow(_ row: Transfer) -> some View {
        HStack(alignment: .center, spacing: 8) {
            DLMFileTile(filename: row.filename, size: 28, brandTint: true)
            Button { select(row) } label: {
                VStack(alignment: .leading, spacing: 3) {
                    Text(row.filename).font(.callout.weight(.medium)).lineLimit(1).truncationMode(.middle).help(row.filename)
                    Text([row.status == .running ? row.speedSummary : row.statusLabel,
                          row.status == .running ? row.etaSeconds.map { remaining($0) } : nil].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    if let fraction = row.fraction {
                        HStack(spacing: 7) {
                            ProgressView(value: fraction).progressViewStyle(DLMProgress(height: 4)).tint(DLMDesign.progress)
                            Text(DownloadTableValues(row).percentage).font(.caption2.monospacedDigit())
                        }
                    }
                }.frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
            }.buttonStyle(.plain)
            if row.status.canPause {
                Button { pause([row.id]) } label: { Image(systemName: "pause.fill").frame(width: 18, height: 22) }
                    .help("Pause").accessibilityLabel("Pause").disabled(!enabled)
            } else if row.status.canResume {
                Button { Task { await model.batch(.resume, ids: [row.id]) } } label: { Image(systemName: "play.fill").frame(width: 18, height: 22) }
                    .help("Resume").accessibilityLabel("Resume").disabled(!enabled)
            }
        }
    }
    private func metric(_ title: LocalizedStringKey, value: String, icon: String) -> some View {
        HStack(spacing: 7) {
            Image(systemName: icon).font(.system(size: 17, weight: .semibold)).foregroundStyle(DLMDesign.uiAccent).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(value).font(.callout.weight(.semibold)).monospacedDigit().lineLimit(1)
                Text(title).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
            }
        }.frame(maxWidth: .infinity)
    }
    private var quickActions: some View {
        GeometryReader { geometry in
        MenuQuickActionLayout(rtl: layoutDirection == .rightToLeft) {
                Button { addDownload() } label: { MenuCenterActionLabel(title: "Add Download", icon: "plus", primary: true) }
                    .foregroundStyle(DLMDesign.actionPrimary).disabled(!enabled)
                Button { pause(presentation.summary.pauseIDs) } label: { MenuCenterActionLabel(title: "Pause All", icon: "pause.fill") }
                    .disabled(!enabled || presentation.summary.pauseIDs.isEmpty)
                Button { Task { await model.batch(.resume, ids: presentation.summary.resumeIDs) } } label: { MenuCenterActionLabel(title: "Resume All", icon: "play.fill") }
                    .disabled(!enabled || presentation.summary.resumeIDs.isEmpty)
                if #available(macOS 14, *) { CenterSettingsButton(close: { dismiss() }) }
                else {
                    Button { dismiss(); NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil); NSApp.activate(ignoringOtherApps: true) }
                        label: { MenuCenterActionLabel(title: "Settings", icon: "gearshape") }
                }
                Button { dismiss(); NSApp.terminate(nil) } label: { MenuCenterActionLabel(title: "Quit DLM", icon: "power") }
        }.buttonStyle(.plain)
            .overlay {
                    ForEach(1..<5) { index in
                        Rectangle().fill(Color(nsColor: .separatorColor))
                            .frame(width: 1, height: 40)
                            .position(x: MenuCenterLayout.actionCellWidth(geometry.size.width) * CGFloat(index), y: geometry.size.height / 2)
                    }
                    .allowsHitTesting(false).accessibilityHidden(true)
            }
        }.frame(height: 48)
    }
    private func addDownload() {
        NativeAddWindowAction.present(model: model, openWindow: openWindow, closePopover: { dismiss() })
    }
    private func showCompleted(_ row: Transfer) {
        let file = URL(fileURLWithPath: row.destination)
        NSWorkspace.shared.activateFileViewerSelecting([FileManager.default.fileExists(atPath: file.path) ? file : file.deletingLastPathComponent()])
    }
    private func pause(_ ids: Set<String>) {
        guard enabled else { return }
        if model.rows.contains(where: { ids.contains($0.id) && $0.status == .running && !$0.safePause }) { confirmingPause = ids }
        else { Task { await model.batch(.pause, ids: ids) } }
    }
    private func openMain() { dismiss(); openWindow(id: "downloads"); NSApp.activate(ignoringOtherApps: true) }
    private func navigate(_ section: String) { model.menuBarSection = section; openMain() }
    private func select(_ row: Transfer) {
        openMain()
        Task {
            do {
                try await model.locateExistingDownload(row.id)
                model.menuBarSection = row.status == .failed ? "Failed" : "All Downloads"
            } catch { model.error = error.localizedDescription }
        }
    }
}

@available(macOS 14, *)
private struct CenterSettingsButton: View {
    let close: () -> Void
    @Environment(\.openSettings) private var openSettings
    var body: some View { Button { close(); openSettings(); NSApp.activate(ignoringOtherApps: true) } label: { MenuCenterActionLabel(title: "Settings", icon: "gearshape") } }
}

struct MainWindowReopenRegistration: View {
    let register: (@escaping () -> Void) -> Void
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        MainWindowCapture().onAppear { register { openWindow(id: "downloads"); NSApp.activate(ignoringOtherApps: true) } }
    }
}

private struct MenuCenterActionLabel: View {
    let title: LocalizedStringKey
    let icon: String
    var primary = false
    @Environment(\.isEnabled) private var enabled
    @State private var hovered = false
    var body: some View {
        VStack(spacing: 5) {
            Image(systemName: icon).font(.system(size: 16, weight: .medium)).frame(width: 20, height: 20).accessibilityHidden(true)
            Text(title).font(.system(size: 10)).multilineTextAlignment(.center).lineLimit(2)
        }.foregroundStyle(primary ? DLMDesign.actionPrimary : .primary)
            .frame(maxWidth: .infinity).frame(height: 48).contentShape(Rectangle())
            .background(hovered && enabled ? DLMDesign.hoveredControl : .clear, in: RoundedRectangle(cornerRadius: 5))
            .opacity(enabled ? 1 : 0.35).onHover { hovered = $0 }
    }
}

enum MenuCenterLayout {
    static func actionCellWidth(_ available: CGFloat) -> CGFloat { max(0, available) / 5 }
    static func contentHeight(_ measured: CGFloat) -> CGFloat { min(420, max(0, measured)) }
}
private struct MenuCenterHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}

// Template copy only: the colored bundled PNG and Dock icon remain untouched.
enum MenuBarBrandTemplate {
    static let image: NSImage? = {
        guard let url = Bundle.main.url(forResource: "DLMBrand", withExtension: "png"),
              let source = NSImage(contentsOf: url) else { return nil }
        return makeTemplate(source)
    }()
    static func makeTemplate(_ source: NSImage) -> NSImage? {
        guard source.isValid, source.size.width > 0, source.size.height > 0,
              let template = source.copy() as? NSImage else { return nil }
        let scale = 20 / max(source.size.width, source.size.height)
        template.size = NSSize(width: source.size.width * scale, height: source.size.height * scale)
        template.isTemplate = true
        return template
    }
}

@MainActor
final class NativeMainWindowRegistry {
    static let shared = NativeMainWindowRegistry()
    weak var window: NSWindow?
}
private struct MainWindowCapture: NSViewRepresentable {
    func makeNSView(context: Context) -> Capture { Capture() }
    func updateNSView(_ view: Capture, context: Context) {}
    final class Capture: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let window { NativeMainWindowRegistry.shared.window = window }
        }
    }
}

struct StandaloneAddWindowLifecycle: NSViewRepresentable {
    let closed: () -> Void
    func makeNSView(context: Context) -> Anchor { Anchor(closed: closed) }
    func updateNSView(_ view: Anchor, context: Context) { view.closed = closed }
    final class Anchor: NSView {
        var closed: () -> Void
        private var observation: NSObjectProtocol?
        init(closed: @escaping () -> Void) { self.closed = closed; super.init(frame: .zero) }
        required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let observation { NotificationCenter.default.removeObserver(observation) }
            observation = nil
            if let window {
                observation = NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { [weak self] _ in self?.closed() }
            }
        }
        deinit { if let observation { NotificationCenter.default.removeObserver(observation) } }
    }
}

@MainActor
enum NativeAddWindowAction {
    static func present(model: DownloadModel, openWindow: OpenWindowAction, closePopover: () -> Void = {}) {
        let main = NativeMainWindowRegistry.shared.window
        switch model.requestMenuBarAdd(mainVisible: main?.isVisible == true || main?.isMiniaturized == true) {
        case .main:
            closePopover(); main?.deminiaturize(nil); openWindow(id: "downloads"); NSApp.activate(ignoringOtherApps: true)
        case .standalone:
            closePopover(); openWindow(id: "add-download"); NSApp.activate(ignoringOtherApps: true)
        case .unavailable: break
        }
    }
}
struct NativeAddDownloadCommand: View {
    @ObservedObject var model: DownloadModel
    @Environment(\.openWindow) private var openWindow
    var body: some View {
        Button(NativePreferences.text("Add Download…")) { NativeAddWindowAction.present(model: model, openWindow: openWindow) }
            .keyboardShortcut("n", modifiers: .command)
            .disabled(!model.ready || model.busy || model.adding || model.error != nil)
    }
}

struct MenuQuickActionLayout: Layout {
    var rtl: Bool
    static func center(index: Int, count: Int, width: CGFloat, rtl: Bool) -> CGFloat {
        width / CGFloat(count) * (CGFloat(rtl ? count - 1 - index : index) + 0.5)
    }
    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        CGSize(width: proposal.width ?? 360, height: 48)
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        guard !subviews.isEmpty else { return }
        for (index, view) in subviews.enumerated() {
            view.place(at: CGPoint(x: bounds.minX + Self.center(index: index, count: subviews.count, width: bounds.width, rtl: rtl), y: bounds.midY),
                anchor: .center, proposal: ProposedViewSize(width: bounds.width / CGFloat(subviews.count), height: bounds.height))
        }
    }
}
