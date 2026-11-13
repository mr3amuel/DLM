import AppKit
import SwiftUI

// Own the navigation viewport instead of modifying SwiftUI's private scroll view
// after its title-bar/safe-area calculations. Header/footer remain outside it.
struct NativeSidebarScroll<Content: View>: NSViewRepresentable {
    @ViewBuilder var content: Content
    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        // AppKit selects overlay or legacy according to accessibility/device
        // preferences and updates the style when those preferences change.
        scroll.scrollerStyle = NSScroller.preferredScrollerStyle
        scroll.autohidesScrollers = true
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsetsZero
        let document = Document(rootView: root)
        document.isFlipped = true
        document.sizingOptions = [.intrinsicContentSize]
        document.translatesAutoresizingMaskIntoConstraints = false
        scroll.documentView = document
        // NSClipView manages the document origin and scrolling; constrain width only.
        document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor).isActive = true
        return scroll
    }
    func updateNSView(_ view: NSScrollView, context: Context) {
        (view.documentView as? Document)?.rootView = root
    }
    private var root: AnyView {
        AnyView(content.nativeLanguage().dlmBrandColors().fixedSize(horizontal: false, vertical: true))
    }
    typealias Document = NSHostingView<AnyView>
}
