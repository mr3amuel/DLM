import SwiftUI

// Used at every window, sheet and independently hosted SwiftUI root. Unlike the
// system accent preference, explicit tint expresses DLM's control semantics.
private struct DLMBrandColors: ViewModifier {
    func body(content: Content) -> some View { content.tint(DLMDesign.controlActive) }
}

extension View {
    func dlmBrandColors() -> some View { modifier(DLMBrandColors()) }

    // Apple's native switch style, not a custom-drawn Toggle. Supply the native
    // style's explicit tint as well as the shared inherited environment.
    func dlmSwitchTint() -> some View {
        toggleStyle(SwitchToggleStyle(tint: DLMDesign.controlActive))
            .tint(DLMDesign.controlActive)
    }
}
