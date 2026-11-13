import AppKit
import SwiftUI

// One application-wide override also covers native panels, sheets and toolbars.
@MainActor
final class NativeAppearanceController: ObservableObject {
    enum Selection: String, CaseIterable, Identifiable {
        case light = "Light", dark = "Dark", system = "System"
        var id: Self { self }
    }
    static let shared = NativeAppearanceController()
    private static let key = "dlm.native.appearance"
    @Published private(set) var selection: Selection

    private init() {
        selection = UserDefaults.standard.string(forKey: Self.key).flatMap(Selection.init(rawValue:)) ?? .system
    }

    func select(_ selection: Selection) {
        self.selection = selection
        UserDefaults.standard.set(selection.rawValue, forKey: Self.key)
        apply()
    }

    func apply() {
        switch selection {
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        case .system: NSApp.appearance = nil
        }
    }
}
