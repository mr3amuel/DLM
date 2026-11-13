import AppKit
import SwiftUI

enum DLMDesign {
    // Original DLM artwork endpoints and the native named accent catalog.
    static let brandTurquoiseHex: UInt32 = 0x0CDECA
    static let brandBlueHex: UInt32 = 0x0755FF
    static let iconBackgroundHex: UInt32 = 0x10223C
    static let brandPrimary = adaptive(light: brandTurquoiseHex, dark: brandTurquoiseHex)
    static let brandSecondary = adaptive(light: brandBlueHex, dark: brandBlueHex)
    // Interactive accents are independent of the unchanged two-color logo.
    static let uiAccentHex: UInt32 = brandBlueHex
    static let uiAccent = adaptive(light: uiAccentHex, dark: uiAccentHex)
    static let actionPrimary = uiAccent
    static let controlActive = uiAccent
    static let progress = uiAccent
    static let focus = uiAccent
    static let selection = brandSecondary.opacity(0.12)
    static let selectionForeground = adaptive(light: brandBlueHex, dark: 0xF2F3F5)
    static let actionForeground = adaptive(light: 0xFFFFFF, dark: 0xFFFFFF)
    static let actionHover = actionPrimary.opacity(0.86)
    static let success = Color(nsColor: .systemGreen)
    static let warning = Color(nsColor: .systemOrange)
    static let failure = Color(nsColor: .systemRed)
    static let canvas = adaptive(light: 0xFFFFFF, dark: 0x171B23)
    static let sidebar = adaptive(light: 0xF4F6FB, dark: 0x272B3A)
    static let toolbar = adaptive(light: 0xFFFFFF, dark: 0x252831)
    static let surface = adaptive(light: 0xFFFFFF, dark: 0x222730)
    static let control = adaptive(light: 0xF3F4F6, dark: 0x343943)
    static let border = adaptive(light: 0xE5E7EB, dark: 0x3A404B)
    static let badge = adaptive(light: 0xEEF2F6, dark: 0x45495B)
    // Neutral light surfaces remain separate from DLM's arrow accents.
    static let textPrimary = adaptive(light: 0x111827, dark: 0xF2F3F5)
    static let textSecondary = adaptive(light: 0x6B7280, dark: 0xB4BAC5)
    static let track = adaptive(light: 0xE5E7EB, dark: 0x343943)
    static let hoveredSurface = adaptive(light: 0xF9FAFB, dark: 0x282E38)
    static let hoveredControl = adaptive(light: 0xE5E7EB, dark: 0x404652)
    static let inset: CGFloat = 22
    static let gap: CGFloat = 12
    static let radius: CGFloat = 11
    static let toolbarHeight: CGFloat = 56
    static let sidebarRange: ClosedRange<CGFloat> = 220...280
    static let inspectorRange: ClosedRange<CGFloat> = 280...356
    static let inspectorBreakpoint: CGFloat = 1010

    static func adaptive(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let hex = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: Double((hex >> 16) & 255) / 255,
                           green: Double((hex >> 8) & 255) / 255,
                           blue: Double(hex & 255) / 255, alpha: 1)
        })
    }

    static func statusColor(_ status: Transfer.Status) -> Color {
        switch status {
        case .running: progress
        case .completed: success
        case .failed: failure
        case .paused: textSecondary
        default: textSecondary
        }
    }
}

struct DLMHeading: View {
    let title: LocalizedStringKey
    var subtitle: LocalizedStringKey?
    var compact = false

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.system(size: compact ? 21 : 29, weight: .semibold))
                .foregroundStyle(DLMDesign.textPrimary)
                .accessibilityAddTraits(.isHeader)
            if let subtitle {
                Text(subtitle).font(.subheadline).foregroundStyle(DLMDesign.textSecondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct DLMPanel<Content: View>: View {
    let title: LocalizedStringKey
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: DLMDesign.gap) {
            Text(title).font(.headline).accessibilityAddTraits(.isHeader)
            VStack(alignment: .leading, spacing: DLMDesign.gap) { content }
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .dlmCard()
        }
    }
}

struct DLMFileTile: View {
    let filename: String

    var size: CGFloat = 52
    var brandTint = false

    private var kind: (symbol: String, label: LocalizedStringKey, color: Color) {
        switch (filename as NSString).pathExtension.lowercased() {
        case "7z", "rar": ("archivebox.fill", "Archive", .purple)
        case "zip", "tar", "gz", "xz", "xip": ("archivebox.fill", "Archive", .orange)
        case "dmg", "iso", "img": ("internaldrive.fill", "Disk Image", .gray)
        case "pkg", "app", "exe": ("shippingbox.fill", "Application", .blue)
        case "mov", "mp4", "mkv", "webm": ("play.rectangle.fill", "Video", .red)
        case "mp3", "wav", "m4a", "flac": ("waveform", "Audio", .purple)
        case "jpg", "jpeg", "png", "gif", "webp", "heic": ("photo.fill", "Image", .blue)
        case "pdf", "txt", "md", "docx": ("doc.text.fill", "Document", .blue)
        default: ("doc.fill", "File", .gray)
        }
    }

    var fileType: LocalizedStringKey { kind.label }

    var body: some View {
        Image(systemName: kind.symbol)
            .font(.system(size: size * 0.48, weight: .medium))
            .symbolRenderingMode(.hierarchical)
            .foregroundStyle(brandTint ? DLMDesign.uiAccent : kind.color)
            .frame(width: size, height: size)
            .background(brandTint ? DLMDesign.selection : kind.color.opacity(0.14), in: RoundedRectangle(cornerRadius: DLMDesign.radius))
            .accessibilityHidden(true)
    }
}

struct DLMIconStyle: ButtonStyle {
    var primary = false
    var boxed = false
    @Environment(\.isEnabled) private var enabled
    @State private var hovered = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 15, weight: .medium))
            .frame(width: 32, height: 32)
            .foregroundStyle(primary ? DLMDesign.actionForeground : Color.primary)
            .background {
                if primary || boxed || configuration.isPressed || hovered {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(primary ? (hovered ? DLMDesign.actionHover : DLMDesign.actionPrimary) : DLMDesign.control)
                }
            }
            .opacity(enabled ? (configuration.isPressed ? 0.75 : 1) : 0.45)
            .contentShape(Rectangle())
            .onHover { hovered = $0 }
    }
}

struct DLMProgress: ProgressViewStyle {
    var height: CGFloat = 8
    func makeBody(configuration: Configuration) -> some View {
        GeometryReader { geometry in
            Capsule().fill(DLMDesign.track)
                .overlay(alignment: .leading) {
                    Capsule().fill(.tint)
                        .frame(width: geometry.size.width * (configuration.fractionCompleted ?? 0))
                }
        }
        .frame(height: height)
        .accessibilityElement(children: .ignore)
        .accessibilityValue(Text((configuration.fractionCompleted ?? 0).formatted(.percent.precision(.fractionLength(0)))))
    }
}

private struct DLMCard: ViewModifier {
    var selected: Bool
    @State private var hovered = false

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: DLMDesign.radius)
        content
            .background(selected ? DLMDesign.selection : (hovered ? DLMDesign.hoveredSurface : DLMDesign.surface), in: shape)
            .overlay {
                shape.strokeBorder(selected ? DLMDesign.brandSecondary : DLMDesign.border.opacity(hovered ? 1 : 0.63),
                                   lineWidth: selected ? 1.5 : 1)
                    .allowsHitTesting(false)
            }
            .onHover { hovered = $0 }
    }
}

extension View {
    func dlmCard(selected: Bool = false) -> some View { modifier(DLMCard(selected: selected)) }
}

// Shared by configuration windows and workflow pages.
struct DLMActionStyle: ButtonStyle {
    var primary = false
    @Environment(\.isEnabled) private var enabled
    @State private var hovered = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13, weight: .semibold))
            .padding(.horizontal, 16).frame(minHeight: 34)
            .foregroundStyle(primary ? DLMDesign.actionForeground : .primary)
            .background(primary ? (hovered ? DLMDesign.actionHover : DLMDesign.actionPrimary) : (hovered ? DLMDesign.hoveredControl : DLMDesign.control),
                        in: RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(
                primary ? DLMDesign.actionPrimary.opacity(0.6) : DLMDesign.border))
            .opacity(enabled ? (configuration.isPressed ? 0.75 : 1) : 0.45)
            .onHover { hovered = $0 }
    }
}

struct DLMSettingsRow<Content: View>: View {
    let title: LocalizedStringKey
    let detail: LocalizedStringKey
    let icon: String
    @ViewBuilder var content: Content
    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: icon).font(.system(size: 15, weight: .medium))
                .foregroundStyle(DLMDesign.brandSecondary).frame(width: 22)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.system(size: 13, weight: .semibold)).foregroundStyle(DLMDesign.textPrimary)
                Text(detail).font(.system(size: 11)).foregroundStyle(DLMDesign.textSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }.frame(maxWidth: .infinity, alignment: .leading)
            content.controlSize(.regular).frame(width: 180, alignment: .trailing)
        }.padding(.vertical, 10)
    }
}

struct DLMSettingsGroup<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        VStack(spacing: 0) { content }.padding(.horizontal, 14).dlmCard()
    }
}

// Resolve a loose Resources PNG explicitly; do not depend on named-image caches.
@MainActor
private enum DLMBrandResource {
    static let image: NSImage? = {
        let bundle = Bundle.main
        let version = bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown"
        guard let url = bundle.url(forResource: "DLMBrand", withExtension: "png"),
              let image = NSImage(contentsOf: url), image.isValid,
              image.size.width > 0, image.size.height > 0 else {
            NSLog("DLM brand PNG missing or undecodable (bundle=%@, build=%@).", bundle.bundleURL.path, version)
            return nil
        }
        NSLog("DLM brand PNG loaded (bundle=%@, build=%@).", bundle.bundleURL.path, version)
        return image
    }()
}

// Main sidebar, Settings and Add Download share this original, unframed artwork.
struct DLMBrandMark: View {
    var size: CGFloat = 36
    var body: some View {
        if let image = DLMBrandResource.image {
            Image(nsImage: image).resizable().renderingMode(.original).interpolation(.high)
                .scaledToFit().frame(width: size, height: size).accessibilityHidden(true)
        } else {
            Text("Logo unavailable").font(.caption2).multilineTextAlignment(.center)
                .foregroundStyle(DLMDesign.textSecondary)
                .frame(width: size, height: size)
                .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(DLMDesign.warning))
                .help("DLMBrand.png could not be loaded from this application bundle.")
                .accessibilityLabel("DLM logo unavailable")
        }
    }
}
