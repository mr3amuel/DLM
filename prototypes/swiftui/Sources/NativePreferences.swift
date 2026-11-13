import AppKit
import SwiftUI
import UserNotifications

enum NativePreferences {
    static let chromeInvitationKey = "dlm.native.chromeInvitation.v1"
    static let languageKey = "dlm.native.language"
    static let connectionsKey = "dlm.native.connections"
    static let adaptiveKey = "dlm.native.adaptive"
    static let clipboardKey = "dlm.native.clipboardPrefill"
    static let destinationKey = "dlm.native.destination"
    private static let missedKey = "dlm.native.missedNotifications"
    static let notificationsKey = "dlm.native.notifications"

    static var language: String {
        UserDefaults.standard.string(forKey: languageKey) == "fa" ? "fa" : "en"
    }

    static var locale: Locale { locale(for: language) }
    static func locale(for language: String) -> Locale {
        Locale(identifier: language == "fa" ? "fa_IR@calendar=persian" : "en_US@calendar=gregorian")
    }

    static func text(_ key: String, language: String? = nil, bundle: Bundle = .main) -> String {
        guard let path = bundle.path(forResource: language ?? self.language, ofType: "lproj"),
              let resources = Bundle(path: path) else { return key }
        return resources.localizedString(forKey: key, value: key, table: nil)
    }

    static func format(_ key: String, _ arguments: CVarArg..., language: String? = nil, bundle: Bundle = .main) -> String {
        String(format: text(key, language: language, bundle: bundle),
               locale: locale(for: language ?? self.language), arguments: arguments)
    }

    static func number(_ value: UInt64) -> String {
        value.formatted(.number.grouping(.never).locale(locale))
    }

    static func date(_ milliseconds: UInt64) -> String {
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = Calendar(identifier: language == "fa" ? .persian : .gregorian)
        formatter.timeZone = .autoupdatingCurrent
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        return formatter.string(from: Date(timeIntervalSince1970: Double(milliseconds) / 1000))
    }

    static func notifyMissed(_ rules: [ScheduleEntry]) {
        let defaults = UserDefaults.standard
        guard defaults.bool(forKey: notificationsKey) else { return }
        let previous = Set(defaults.stringArray(forKey: missedKey) ?? [])
        let current = Set(rules.compactMap { rule -> String? in
            guard rule.notifyMissed, let time = rule.missedAtMs else { return nil }
            return "missed-\(rule.id)-\(time)"
        })
        guard previous != current else { return }
        if let identifier = current.subtracting(previous).sorted().first {
            notify("A scheduled download was missed.", identifier: identifier)
        }
        defaults.set(current.sorted(), forKey: missedKey)
    }

    static func notify(_ title: String, identifier: String) {
        guard UserDefaults.standard.bool(forKey: notificationsKey) else { return }
        let content = UNMutableNotificationContent()
        content.title = text(title)
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request) { error in
            if let error { NSLog("DLM notification failed (%@)", (error as NSError).domain) }
        }
    }
}

struct GeneralNativeSettings: View {
    @ObservedObject private var appearance = NativeAppearanceController.shared
    @AppStorage(NativePreferences.languageKey) private var language = "en"
    @AppStorage(NativePreferences.connectionsKey) private var connections = 4
    @AppStorage(NativePreferences.adaptiveKey) private var adaptive = true
    @AppStorage(NativePreferences.clipboardKey) private var clipboard = true
    @AppStorage(NativePreferences.notificationsKey) private var notifications = false
    @State private var notificationRequest = false
    @State private var denied = false

    var body: some View {
        DLMSettingsGroup {
            DLMSettingsRow(title: "Appearance", detail: "Choose Light, Dark or follow the system.", icon: "circle.lefthalf.filled") {
                Picker("Appearance", selection: Binding(get: { appearance.selection }, set: appearance.select)) {
                    ForEach(NativeAppearanceController.Selection.allCases) { item in
                        Text(LocalizedStringKey(item.rawValue)).tag(item)
                    }
                }.labelsHidden().frame(width: 150)
            }
            Divider()
            DLMSettingsRow(title: "Language", detail: "Choose the app language and calendar.", icon: "globe") {
                Picker("Language", selection: $language) {
                    Text(verbatim: "English").tag("en")
                    Text(verbatim: "فارسی").tag("fa")
                }.labelsHidden()
            }
            Divider()
            DLMSettingsRow(title: "Default connections", detail: "Connection count for new downloads.", icon: "point.3.connected.trianglepath.dotted") {
                Picker("Default connections", selection: $connections) {
                    ForEach([1, 2, 4, 8, 16, 32], id: \.self) { Text($0.formatted()).tag($0) }
                }.labelsHidden().frame(width: 100)
            }
            Divider()
            DLMSettingsRow(title: "Adaptive connections", detail: "Use adaptive connections by default", icon: "slider.horizontal.3") {
                Toggle("Use adaptive connections by default", isOn: $adaptive).labelsHidden().dlmSwitchTint()
            }
            Divider()
            DLMSettingsRow(title: "Clipboard", detail: "Prefill links from the clipboard", icon: "doc.on.clipboard") {
                Toggle("Prefill links from the clipboard", isOn: $clipboard).labelsHidden().dlmSwitchTint()
            }
            Divider()
            DLMSettingsRow(title: "Notifications", detail: "Download notifications", icon: "bell") {
                Toggle("Download notifications", isOn: Binding(get: { notifications }, set: setNotifications))
                    .labelsHidden().dlmSwitchTint().disabled(notificationRequest)
            }
            if denied { Text("Allow notifications for DLM in System Settings.").foregroundStyle(.secondary).padding(.bottom, 15) }
        }
    }

    private func setNotifications(_ enabled: Bool) {
        guard enabled else { notifications = false; denied = false; return }
        notificationRequest = true
        Task { @MainActor in
            let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert])) == true
            notifications = granted
            denied = !granted
            notificationRequest = false
        }
    }
}

private struct NativeLanguage: ViewModifier {
    @AppStorage(NativePreferences.languageKey) private var language = "en"

    func body(content: Content) -> some View {
        let persian = language == "fa"
        let locale = Locale(identifier: persian ? "fa_IR@calendar=persian" : "en_US@calendar=gregorian")
        var calendar = Calendar(identifier: persian ? .persian : .gregorian)
        calendar.locale = locale
        calendar.timeZone = .autoupdatingCurrent
        return content
            .environment(\.locale, locale)
            .environment(\.calendar, calendar)
            .environment(\.timeZone, calendar.timeZone)
            .environment(\.layoutDirection, persian ? .rightToLeft : .leftToRight)
    }
}

extension View {
    func nativeLanguage() -> some View { modifier(NativeLanguage()) }
}
