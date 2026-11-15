#if os(macOS)
import AppKit

/// The reader's macOS input: mouse, keys, scroll, menus, cursors, and the
/// AppKit accessibility protocol. What they do lives in `ReaderCanvas`.
extension ReaderCanvas: NSMenuItemValidation {
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var isOpaque: Bool { true }

    func platformSetUp() {
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
    }

    var isResizingLive: Bool { inLiveResize }
    /// How far outside a paragraph button a click still counts.
    var controlSlop: CGFloat { 4 }
    var flipLayer: CALayer? { layer }

    override func layout() {
        super.layout()
        sizeChanged()
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        drawContents(dirtyRect, in: ctx)
    }

    func controlsMoved() {
        window?.invalidateCursorRects(for: self)
    }

    func pageChanged() {
        NSAccessibility.post(element: self, notification: .valueChanged)
    }

    func takeKeys() {
        window?.makeFirstResponder(self)
    }

    /// Keys must reach the reader as soon as it shows; a request made before it
    /// had a window would have been dropped.
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else { return }
        DispatchQueue.main.async { [weak self, weak window] in
            guard let self, let window, self.window === window else { return }
            let current = window.firstResponder
            if current == nil || current === window || !(current is NSView) || (current as? NSView)?.window == nil {
                window.makeFirstResponder(self)
            }
        }
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()
        if isProvisional { relayout(keeping: sticky ?? currentAnchor()) }
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        dragged = false
        // ⌘- or ⌃-click translates afresh, skipping the cache (as in the web app).
        if activateControl(at: point, fresh: !event.modifierFlags.intersection([.command, .control]).isEmpty) {
            dragAnchor = nil
            return
        }
        textSelection = nil
        dragAnchor = offset(at: point)
    }

    override func mouseDragged(with event: NSEvent) {
        guard let anchor = dragAnchor, let current = offset(at: convert(event.locationInWindow, from: nil)) else {
            return
        }
        dragged = true
        textSelection = NSRange(location: min(anchor, current), length: abs(current - anchor))
    }

    override func mouseUp(with event: NSEvent) {
        defer { dragAnchor = nil }
        guard dragAnchor != nil, !dragged else { return }
        // A click without a drag: reveal the word's translation and select it.
        selectWord(at: convert(event.locationInWindow, from: nil))
    }

    @objc func copy(_ sender: Any?) {
        guard let s = selectedText else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        item.action == #selector(copy(_:)) ? selectedText != nil : true
    }

    @objc private func lookUpSelection(_ sender: NSMenuItem) {
        guard let range = textSelection, let point = sender.representedObject as? CGPoint else { return }
        showDefinition(for: text.storage.attributedSubstring(from: range), at: point)
    }

    /// The paragraph buttons are drawn, not views: give them a pointer and a tooltip.
    override func resetCursorRects() {
        super.resetCursorRects()
        removeAllToolTips()
        for (_, rect) in visibleControlRects() {
            addCursorRect(rect, cursor: .pointingHand)
            addToolTip(rect, owner: self, userData: nil)
        }
    }

    @objc func view(_ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint, userData: UnsafeMutableRawPointer?) -> String {
        guard let (pid, _) = visibleControlRects().first(where: { $0.1.contains(point) }) else { return "" }
        if delegate?.readerTranslationProgress(self, paragraphId: pid) != nil { return "Translating…" }
        return delegate?.readerCanTranslate(self) ?? true
            ? "Translate paragraph (⌘-click to translate without the cache)" : "Waiting for chapter summaries…"
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let point = convert(event.locationInWindow, from: nil)
        if let selected = selectedText {
            let menu = NSMenu()
            let short = selected.count > 24 ? String(selected.prefix(24)) + "…" : selected
            menu.addItem(withTitle: "Look Up “\(short)”", action: #selector(lookUpSelection(_:)), keyEquivalent: "")
                .representedObject = point
            menu.addItem(withTitle: "Copy", action: #selector(copy(_:)), keyEquivalent: "")
            for item in menu.items { item.target = self }
            return menu
        }
        guard let hit = hit(point) else { return nil }
        let menu = NSMenu()
        if let w = hit.wordIndex, let p = text.paragraphs[hit.paragraphId], w < p.words.count {
            let word = p.words[w]
            let surface = (p.text as NSString).substring(with: NSRange(location: word.start, length: word.len))
            menu.addItem(withTitle: "Look Up “\(surface)”", action: #selector(lookUpMenu(_:)), keyEquivalent: "")
                .representedObject = LookUp(paragraphId: hit.paragraphId, wordIndex: w, point: point)
            menu.addItem(withTitle: "Copy “\(surface)”", action: #selector(copyString(_:)), keyEquivalent: "")
                .representedObject = surface
        }
        if let p = text.paragraphs[hit.paragraphId] {
            menu.addItem(withTitle: "Copy Paragraph", action: #selector(copyString(_:)), keyEquivalent: "")
                .representedObject = p.text.replacingOccurrences(of: "\u{2028}", with: "\n")
            menu.addItem(.separator())
            let title = p.translated ? "Retranslate Paragraph" : "Translate Paragraph"
            menu.addItem(withTitle: title, action: #selector(translateMenu(_:)), keyEquivalent: "")
                .representedObject = (hit.paragraphId, !p.translated)
        }
        for item in menu.items { item.target = self }
        return menu
    }

    private struct LookUp {
        let paragraphId: Int
        let wordIndex: Int
        let point: CGPoint
    }

    @objc private func lookUpMenu(_ sender: NSMenuItem) {
        guard let lookUp = sender.representedObject as? LookUp,
            let range = text.wordRange(paragraphId: lookUp.paragraphId, wordIndex: lookUp.wordIndex)
        else { return }
        let attributed = text.storage.attributedSubstring(from: range)
        showDefinition(for: attributed, at: lookUp.point)
    }

    @objc private func copyString(_ sender: NSMenuItem) {
        guard let s = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(s, forType: .string)
    }

    @objc private func translateMenu(_ sender: NSMenuItem) {
        guard let (pid, useCache) = sender.representedObject as? (Int, Bool) else { return }
        delegate?.reader(self, translate: pid, useCache: useCache)
    }

    // MARK: Accessibility

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .staticText }
    override func accessibilityRoleDescription() -> String? { "book page" }

    override func accessibilityLabel() -> String? { pageLabel }
    override func accessibilityValue() -> Any? { visibleText }

    override func accessibilityPerformIncrement() -> Bool {
        nextSpread()
        return true
    }

    override func accessibilityPerformDecrement() -> Bool {
        previousSpread()
        return true
    }

    override func keyDown(with event: NSEvent) {
        switch event.specialKey {
        case .rightArrow?, .pageDown?, .downArrow?: nextSpread()
        case .leftArrow?, .pageUp?, .upArrow?: previousSpread()
        case .home?: go(toPage: 0)
        case .end?: go(toPage: text.pages.count - 1)
        default:
            let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
            if key == " " {
                event.modifierFlags.contains(.shift) ? previousSpread() : nextSpread()
            } else if event.modifierFlags.intersection([.command, .control, .option]).isEmpty,
                delegate?.readerToggle(self, key: key) == true
            {
                return
            } else {
                super.keyDown(with: event)
            }
        }
    }

    override func scrollWheel(with event: NSEvent) {
        // A wheel, or a device that reports no gesture phases: one spread per burst.
        guard event.hasPreciseScrollingDeltas, !(event.phase.isEmpty && event.momentumPhase.isEmpty) else {
            guard event.timestamp - lastWheelFlip > 0.25 else { return }
            lastWheelFlip = event.timestamp
            if event.scrollingDeltaY < 0 || event.scrollingDeltaX < 0 { nextSpread() } else { previousSpread() }
            return
        }
        if event.phase == .began {
            swipeAccumulator = 0
            swipeConsumed = false
        }
        guard !swipeConsumed else { return }
        let delta = abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) ? event.scrollingDeltaX : event.scrollingDeltaY
        swipeAccumulator += delta
        if swipeAccumulator < -60 {
            swipeConsumed = true
            nextSpread()
        } else if swipeAccumulator > 60 {
            swipeConsumed = true
            previousSpread()
        }
    }
}

extension NSImage {
    func tinted(_ color: NSColor) -> NSImage {
        let image = NSImage(size: size, flipped: false) { rect in
            self.draw(in: rect)
            // Source-in replaces the symbol's colour (keeping its shape) with the
            // tint, alpha included; source-atop would blend a translucent tint
            // over the black template and leave it near black.
            color.set()
            rect.fill(using: .sourceIn)
            return true
        }
        image.isTemplate = false
        return image
    }
}
#endif
