#if os(macOS)
import AppKit

typealias PlatformView = NSView
typealias PlatformColor = NSColor
typealias PlatformFont = NSFont
typealias PlatformImage = NSImage
typealias PlatformEdgeInsets = NSEdgeInsets
#else
import UIKit

typealias PlatformView = UIView
typealias PlatformColor = UIColor
typealias PlatformFont = UIFont
typealias PlatformImage = UIImage
typealias PlatformEdgeInsets = UIEdgeInsets
#endif

/// The reader's colours, named for what they are used for.
extension PlatformColor {
    #if os(macOS)
    static var readerBackground: NSColor { .textBackgroundColor }
    static var readerText: NSColor { .labelColor }
    static var readerSecondary: NSColor { .secondaryLabelColor }
    static var readerTertiary: NSColor { .tertiaryLabelColor }
    static var readerAccent: NSColor { .controlAccentColor }
    static var readerSelection: NSColor { .selectedTextBackgroundColor }
    #else
    static var readerBackground: UIColor { .systemBackground }
    static var readerText: UIColor { .label }
    static var readerSecondary: UIColor { .secondaryLabel }
    static var readerTertiary: UIColor { .tertiaryLabel }
    static var readerAccent: UIColor { .tintColor }
    static var readerSelection: UIColor { UIColor.tintColor.withAlphaComponent(0.25) }
    #endif
}

extension PlatformView {
    func redraw() {
        #if os(macOS)
        needsDisplay = true
        #else
        setNeedsDisplay()
        #endif
    }

    func relayoutSoon() {
        #if os(macOS)
        needsLayout = true
        #else
        setNeedsLayout()
        #endif
    }
}

extension PlatformImage {
    /// A symbol in one flat colour, to draw into the page.
    @MainActor
    static func readerSymbol(_ name: String, pointSize: CGFloat, tint: PlatformColor) -> PlatformImage? {
        #if os(macOS)
        let config = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
        return NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(config)?.tinted(tint)
        #else
        let config = UIImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
        return UIImage(systemName: name, withConfiguration: config)?.withTintColor(tint, renderingMode: .alwaysOriginal)
        #endif
    }
}
