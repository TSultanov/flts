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
    @MainActor private static var symbols: [String: PlatformImage] = [:]

    /// A symbol in one flat colour, to draw into the page. Cached: the page
    /// draws one per untranslated paragraph on every redraw.
    @MainActor
    static func readerSymbol(_ name: String, pointSize: CGFloat, tint: PlatformColor) -> PlatformImage? {
        #if os(macOS)
        // The tinted image redraws itself for the current appearance.
        let key = "\(name)-\(pointSize)-\(tint)"
        if let cached = symbols[key] { return cached }
        let config = NSImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
        let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)?.withSymbolConfiguration(config)?.tinted(tint)
        #else
        let resolved = tint.resolvedColor(with: .current)
        let key = "\(name)-\(pointSize)-\(resolved)"
        if let cached = symbols[key] { return cached }
        let config = UIImage.SymbolConfiguration(pointSize: pointSize, weight: .regular)
        let image = UIImage(systemName: name, withConfiguration: config)?.withTintColor(resolved, renderingMode: .alwaysOriginal)
        #endif
        symbols[key] = image
        return image
    }
}
