import AppKit
import QuartzCore

/// A reading position: the synced (paragraph, pages into it) pair, plus the
/// exact top line when this device knows it.
struct ReadingAnchor: Equatable {
    var paragraphId: Int
    var pageOffset: Int
    var line: LineAnchor?
}

@MainActor
protocol ReaderCanvasDelegate: AnyObject {
    func reader(_ canvas: ReaderCanvas, selected paragraphId: Int, wordIndex: Int)
    func reader(_ canvas: ReaderCanvas, translate paragraphId: Int, useCache: Bool)
    func reader(_ canvas: ReaderCanvas, movedTo page: Int, of count: Int)
    func readerTranslationProgress(_ canvas: ReaderCanvas, paragraphId: Int) -> Double?
    /// A plain key the reader does not use itself; true if handled.
    func readerToggle(_ canvas: ReaderCanvas, key: String) -> Bool
    /// False until the chapter's translation context (summaries) exists.
    func readerCanTranslate(_ canvas: ReaderCanvas) -> Bool
    func readerDeselected(_ canvas: ReaderCanvas)
}

/// Draws a spread of pages from one `ChapterTextLayout` and turns clicks,
/// keys and swipes into reader actions. Flipping only changes which bands are
/// drawn; text is never re-laid out for it.
@MainActor
final class ReaderCanvas: NSView, NSMenuItemValidation {
    /// What is drawn: the full chapter, or while that is laid out off the
    /// main thread, a provisional layout starting at the reading position.
    private(set) var text = ChapterTextLayout()
    private(set) var isProvisional = false
    private var order: [Int] = []
    private var paragraphs: [Int: ParagraphText] = [:]
    private var typography = ReaderTypography()
    private var generation = 0
    /// Set when the full layout lands, for tests and timing.
    var onFullLayout: (() -> Void)?
    /// Lets tests simulate a window drag.
    var inLiveResizeOverride = false
    weak var delegate: ReaderCanvasDelegate?

    private(set) var spreadStart = 0
    private(set) var columns = 1
    private var columnWidth: CGFloat = 400
    private var laidOutSize: CGSize = .zero
    private var pendingAnchor: ReadingAnchor?
    /// Where the reader is, as last set by the reader (opening, flipping).
    /// Relayouts restore to it rather than to whatever line they snapped to,
    /// so resizes and sidebar animations cannot walk the position.
    private var sticky: ReadingAnchor?
    /// Full layouts started off the main thread, for tests.
    private(set) var fullLayoutsStarted = 0
    private var swipeAccumulator: CGFloat = 0

    /// A dragged text selection, as a storage range of the current layout.
    private(set) var textSelection: NSRange? {
        didSet { needsDisplay = true }
    }
    private var dragAnchor: Int?
    private var dragged = false

    var selected: (paragraphId: Int, wordIndex: Int)? {
        didSet { needsDisplay = true }
    }
    /// Words whose translation is shown above them.
    var revealed: Set<WordKey> = [] {
        didSet { needsDisplay = true }
    }

    struct WordKey: Hashable {
        let paragraphId: Int
        let wordIndex: Int
    }

    var margins = NSEdgeInsets(top: 40, left: 56, bottom: 44, right: 56)
    var columnGap: CGFloat = 64

    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var isOpaque: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layerContentsRedrawPolicy = .onSetNeedsDisplay
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Content

    func setContent(order: [Int], paragraphs: [Int: ParagraphText], typography: ReaderTypography,
                    anchor: ReadingAnchor?)
    {
        self.order = order
        self.paragraphs = paragraphs
        self.typography = typography
        pendingAnchor = anchor
        sticky = anchor
        laidOutSize = .zero
        needsLayout = true
        needsDisplay = true
    }

    /// Changed paragraphs not yet applied; applied a few per run-loop turn so
    /// a burst (sync, a finished chapter) never blocks a frame for long.
    private var pendingChanges: [Int: ParagraphText] = [:]
    private var drainScheduled = false
    static let changesPerTurn = 5
    /// Set when queued changes are all applied, for tests and timing.
    var onChangesApplied: (() -> Void)?

    func replace(_ changed: [ParagraphText]) {
        for p in changed {
            paragraphs[p.id] = p
            pendingChanges[p.id] = p
        }
        // During the provisional layout they wait for the full one (restarting
        // it on every change could keep it from ever landing).
        guard !isProvisional else { return }
        scheduleDrain()
    }

    private func scheduleDrain() {
        guard !drainScheduled else { return }
        drainScheduled = true
        DispatchQueue.main.async { [weak self] in
            self?.drainChanges()
        }
    }

    private func drainChanges() {
        drainScheduled = false
        guard !isProvisional else { return }
        let ids = pendingChanges.keys.sorted().prefix(Self.changesPerTurn)
        let batch = ids.compactMap { pendingChanges.removeValue(forKey: $0) }
        if !batch.isEmpty {
            textSelection = nil
            let keep = text.lineAnchor(forPage: spreadStart)
            text.replace(batch)
            text.layout(width: columnWidth, pageHeight: text.pageHeight, breakAt: keep)
            spreadStart = keep.flatMap { text.page(of: $0) } ?? min(spreadStart, text.pages.count - 1)
            needsDisplay = true
            delegate?.reader(self, movedTo: spreadStart, of: text.pages.count)
        }
        if pendingChanges.isEmpty {
            onChangesApplied?()
        } else {
            scheduleDrain()
        }
    }

    func setTypography(_ typography: ReaderTypography) {
        guard typography != self.typography else { return }
        self.typography = typography
        relayout(keeping: sticky ?? currentAnchor())
    }

    func currentAnchor() -> ReadingAnchor? {
        if text.pages.isEmpty || order.isEmpty { return pendingAnchor }
        guard let a = text.anchor(forPage: spreadStart) else { return nil }
        return ReadingAnchor(paragraphId: a.paragraphId, pageOffset: a.pageOffset, line: text.lineAnchor(forPage: spreadStart))
    }

    var pageCount: Int { text.pages.count }
    var contentOrder: [Int] { order }

    /// The paragraph as the reader has it (including changes not yet laid out).
    func contentParagraph(_ id: Int) -> ParagraphText? { paragraphs[id] }

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

    // MARK: Layout

    override func layout() {
        super.layout()
        if bounds.size != laidOutSize {
            relayout(keeping: laidOutSize == .zero ? pendingAnchor : (sticky ?? currentAnchor()))
        }
    }

    private func relayout(keeping anchor: ReadingAnchor?) {
        let size = bounds.size
        guard size.width > 0, size.height > 0, !order.isEmpty else { return }
        laidOutSize = size
        let fontSize = typography.fontSize
        let available = size.width - margins.left - margins.right
        let minColumn = fontSize * 24
        columns = available >= minColumn * 2 + columnGap ? 2 : 1
        let maxColumn = fontSize * 40
        columnWidth = min((available - columnGap * CGFloat(columns - 1)) / CGFloat(columns), maxColumn)
        let pageHeight = size.height - margins.top - margins.bottom
        pendingAnchor = nil

        // Provisional: the anchor paragraph onward, just enough to fill the view.
        let anchorId = anchor?.paragraphId ?? order.first!
        let from = order.firstIndex(of: anchorId) ?? 0
        let charsPerPage = Int((columnWidth / (fontSize * 0.5)) * (pageHeight / typography.lineHeight))
        let lead = anchor?.line.map { $0.start / max(charsPerPage, 1) + 1 } ?? (anchor?.pageOffset ?? 0)
        let wanted = charsPerPage * (lead + columns + 1)
        var subset: [Int] = []
        var chars = 0
        for id in order[from...] {
            subset.append(id)
            chars += (paragraphs[id]?.text as NSString?)?.length ?? 0
            if chars > wanted { break }
        }
        textSelection = nil
        let provisional = ChapterTextLayout()
        provisional.setContent(order: subset, paragraphs: paragraphs, typography: typography)
        provisional.layout(width: columnWidth, pageHeight: pageHeight, breakAt: anchor?.line)
        text = provisional
        isProvisional = subset.count < order.count || from > 0
        if let line = anchor?.line, let page = provisional.page(of: line) {
            spreadStart = page
        } else if let anchor, let page = provisional.page(forParagraph: anchor.paragraphId, pageOffset: anchor.pageOffset) {
            spreadStart = page
        } else {
            spreadStart = 0
        }
        needsDisplay = true
        delegate?.reader(self, movedTo: spreadStart, of: text.pages.count)

        generation += 1
        let gen = generation
        // During a live resize only the cheap layout runs; the full one follows the drag.
        if inLiveResize || inLiveResizeOverride, isProvisional { return }
        guard isProvisional else {
            onFullLayout?()
            return
        }
        // Wait for relayouts to settle (sidebar animations, resizes) so one
        // full layout runs, not one per frame.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.12) { [weak self] in
            guard let self, gen == self.generation else { return }
            self.startFullLayout(generation: gen, pageHeight: pageHeight)
        }
    }

    private func startFullLayout(generation gen: Int, pageHeight: CGFloat) {
        fullLayoutsStarted += 1
        let (order, paragraphs, typography, width) = (self.order, self.paragraphs, self.typography, columnWidth)
        Task.detached(priority: .userInitiated) {
            let full = ChapterTextLayout()
            full.setContent(order: order, paragraphs: paragraphs, typography: typography)
            full.layout(width: width, pageHeight: pageHeight)
            await MainActor.run { [weak self] in
                self?.adopt(full, generation: gen)
            }
        }
    }

    override func viewDidEndLiveResize() {
        super.viewDidEndLiveResize()
        if isProvisional { relayout(keeping: sticky ?? currentAnchor()) }
    }

    /// Swaps in the full layout, re-cut so the visible line still starts the page.
    private func adopt(_ full: ChapterTextLayout, generation gen: Int) {
        guard gen == generation else { return }
        let keep = text.lineAnchor(forPage: spreadStart)
        full.paginate(breakAt: keep)
        let selectedBefore = selected
        textSelection = nil
        text = full
        isProvisional = false
        spreadStart = keep.flatMap { full.page(of: $0) } ?? 0
        selected = selectedBefore
        needsDisplay = true
        delegate?.reader(self, movedTo: spreadStart, of: text.pages.count)
        // Changes that arrived after this layout started.
        if !pendingChanges.isEmpty { scheduleDrain() }
        onFullLayout?()
    }

    /// Left edge of a column in view coordinates (the spread is centered).
    private func columnX(_ column: Int) -> CGFloat {
        let spreadWidth = columnWidth * CGFloat(columns) + columnGap * CGFloat(columns - 1)
        let left = max(margins.left, (bounds.width - spreadWidth) / 2)
        return left + CGFloat(column) * (columnWidth + columnGap)
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        NSColor.textBackgroundColor.setFill()
        dirtyRect.fill()
        guard let ctx = NSGraphicsContext.current?.cgContext, !text.pages.isEmpty else { return }
        for column in 0..<columns {
            let index = spreadStart + column
            guard index < text.pages.count else { break }
            drawPage(text.pages[index], at: CGPoint(x: columnX(column), y: margins.top), in: ctx)
        }
        drawFooter()
    }

    private func drawPage(_ page: ReaderPage, at origin: CGPoint, in ctx: CGContext) {
        let dy = origin.y - page.top
        let band = CGRect(x: origin.x - columnGap / 2, y: origin.y, width: columnWidth + columnGap, height: page.bottom - page.top)

        ctx.saveGState()
        // Descenders hang below a line's box (its extra leading sits above the
        // glyphs), so the text band is shifted down by their depth.
        let descent = (text.typography.fontSize * 0.4).rounded()
        ctx.clip(to: band.offsetBy(dx: 0, dy: descent))

        if let range = textSelection, range.length > 0 {
            NSColor.selectedTextBackgroundColor.setFill()
            for frame in text.frames(of: range) {
                highlightRect(frame).offsetBy(dx: origin.x, dy: dy).fill()
            }
        }

        if let sel = selected, let range = text.wordRange(paragraphId: sel.paragraphId, wordIndex: sel.wordIndex) {
            NSColor.controlAccentColor.withAlphaComponent(0.22).setFill()
            for frame in text.frames(of: range) {
                let r = highlightRect(frame).offsetBy(dx: origin.x, dy: dy).insetBy(dx: -2, dy: 0)
                NSBezierPath(roundedRect: r, xRadius: 3, yRadius: 3).fill()
            }
        }

        for fragment in text.fragments(in: page) {
            let f = fragment.layoutFragmentFrame
            fragment.draw(at: CGPoint(x: f.minX + origin.x, y: f.minY + dy), in: ctx)
        }

        ctx.restoreGState()
        ctx.saveGState()
        ctx.clip(to: band)
        drawTranslations(in: page, origin: origin, dy: dy)
        drawParagraphControls(in: page, origin: origin, dy: dy)
        ctx.restoreGState()
    }

    /// The glyph box inside a line-height-tall segment frame; extra leading sits above.
    private func glyphRect(_ frame: CGRect) -> CGRect {
        let fontSize = text.typography.fontSize
        let glyphHeight = (fontSize * 1.3).rounded()
        return CGRect(x: frame.minX, y: frame.maxY - glyphHeight, width: frame.width, height: glyphHeight)
    }

    /// The glyph box without the headroom above tall letters, where a word's
    /// translation is drawn: a highlight must not reach under it.
    private func highlightRect(_ frame: CGRect) -> CGRect {
        let glyph = glyphRect(frame)
        let headroom = (text.typography.fontSize * 0.2).rounded()
        return CGRect(x: glyph.minX, y: glyph.minY + headroom, width: glyph.width, height: glyph.height - headroom)
    }

    private func drawTranslations(in page: ReaderPage, origin: CGPoint, dy: CGFloat) {
        guard !revealed.isEmpty else { return }
        let font = text.typography.translationFont
        let style = NSMutableParagraphStyle()
        style.alignment = .center
        style.lineBreakMode = .byClipping
        for key in revealed {
            guard let p = text.paragraphs[key.paragraphId], key.wordIndex < p.words.count,
                let translation = p.words[key.wordIndex].translation, !translation.isEmpty,
                let range = text.wordRange(paragraphId: key.paragraphId, wordIndex: key.wordIndex),
                let first = text.frames(of: range).first,
                first.minY >= page.top - 1, first.maxY <= page.bottom + 1
            else { continue }
            let glyph = glyphRect(first)
            var attrs: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: NSColor.controlAccentColor, .paragraphStyle: style,
            ]
            var size = (translation as NSString).size(withAttributes: attrs)
            // Shrink to the word's width, down to 60%, then let it overflow centered.
            if size.width > glyph.width, glyph.width > 0 {
                let scale = max(glyph.width / size.width, 0.6)
                let shrunk = NSFont.systemFont(ofSize: font.pointSize * scale)
                attrs[.font] = shrunk
                size = (translation as NSString).size(withAttributes: attrs)
            }
            let width = max(size.width, glyph.width)
            let rect = CGRect(
                x: glyph.midX - width / 2 + origin.x, y: glyph.minY - size.height + 2 + dy, width: width,
                height: size.height)
            (translation as NSString).draw(in: rect, withAttributes: attrs)
        }
    }

    private static let translateSymbol: NSImage? = {
        let config = NSImage.SymbolConfiguration(pointSize: 13, weight: .regular)
        return NSImage(systemSymbolName: "character.bubble", accessibilityDescription: "Translate paragraph")?
            .withSymbolConfiguration(config)
    }()

    private func drawParagraphControls(in page: ReaderPage, origin: CGPoint, dy: CGFloat) {
        for (pid, rect) in controlRects(in: page, origin: origin, dy: dy) {
            if let progress = delegate?.readerTranslationProgress(self, paragraphId: pid) {
                drawProgress(progress, in: rect)
            } else if let symbol = Self.translateSymbol {
                let enabled = delegate?.readerCanTranslate(self) ?? true
                let tinted = symbol.tinted(enabled ? NSColor.secondaryLabelColor : NSColor.tertiaryLabelColor.withAlphaComponent(0.35))
                tinted.draw(in: rect.insetBy(dx: (rect.width - symbol.size.width) / 2, dy: (rect.height - symbol.size.height) / 2))
            }
        }
    }

    private func drawProgress(_ progress: Double, in rect: CGRect) {
        let ring = rect.insetBy(dx: 4, dy: 4)
        NSColor.tertiaryLabelColor.setStroke()
        let track = NSBezierPath(ovalIn: ring)
        track.lineWidth = 2.5
        track.stroke()
        NSColor.controlAccentColor.setStroke()
        let arc = NSBezierPath()
        let center = CGPoint(x: ring.midX, y: ring.midY)
        let end = -90 + 360 * min(max(progress, 0.03), 1)
        arc.appendArc(withCenter: center, radius: ring.width / 2, startAngle: -90, endAngle: end, clockwise: false)
        arc.lineWidth = 2.5
        arc.stroke()
    }

    /// Gutter buttons for untranslated or translating paragraphs that start on this page.
    private func controlRects(in page: ReaderPage, origin: CGPoint, dy: CGFloat) -> [(Int, CGRect)] {
        var out: [(Int, CGRect)] = []
        let size: CGFloat = 22
        for line in text.lines where line.top >= page.top && line.top < page.bottom {
            guard line.paragraphIndex < text.order.count else { continue }
            let pid = text.order[line.paragraphIndex]
            guard text.firstLineTop(of: pid) == line.top else { continue }
            let translating = delegate?.readerTranslationProgress(self, paragraphId: pid) != nil
            guard translating || !(text.paragraphs[pid]?.translated ?? true) else { continue }
            // Centred on the first line's lowercase letters.
            let y = line.bottom - text.typography.fontSize * 0.55 - size / 2 + dy
            out.append((pid, CGRect(x: origin.x - size - 10, y: y, width: size, height: size)))
        }
        return out
    }

    private func drawFooter() {
        guard !text.pages.isEmpty else { return }
        let last = min(spreadStart + columns, text.pages.count)
        var label =
            columns > 1 && last - spreadStart > 1
            ? "\(spreadStart + 1)–\(last) of \(text.pages.count)" : "\(spreadStart + 1) of \(text.pages.count)"
        if isProvisional { label = "…" }
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: NSColor.tertiaryLabelColor,
        ]
        let size = (label as NSString).size(withAttributes: attrs)
        let point = CGPoint(x: (bounds.width - size.width) / 2, y: bounds.height - margins.bottom / 2 - size.height / 2)
        (label as NSString).draw(at: point, withAttributes: attrs)
    }

    // MARK: Hit testing

    private func locate(_ viewPoint: CGPoint) -> (column: Int, point: CGPoint)? {
        for column in 0..<columns {
            let index = spreadStart + column
            guard index < text.pages.count else { break }
            let page = text.pages[index]
            let x0 = columnX(column)
            let local = CGPoint(x: viewPoint.x - x0, y: viewPoint.y - margins.top + page.top)
            if viewPoint.x >= x0 - columnGap / 2, viewPoint.x <= x0 + columnWidth + columnGap / 2,
                local.y >= page.top, local.y < page.bottom
            {
                return (column, local)
            }
        }
        return nil
    }

    private func controlHit(_ viewPoint: CGPoint) -> Int? {
        for column in 0..<columns {
            let index = spreadStart + column
            guard index < text.pages.count else { break }
            let page = text.pages[index]
            let origin = CGPoint(x: columnX(column), y: margins.top)
            for (pid, rect) in controlRects(in: page, origin: origin, dy: origin.y - page.top)
            where rect.insetBy(dx: -4, dy: -4).contains(viewPoint) {
                return pid
            }
        }
        return nil
    }

    /// Where a point of a visible page's text sits in this view (for tests and accessibility).
    func viewPoint(_ point: CGPoint, onPage page: Int) -> CGPoint? {
        let column = page - spreadStart
        guard column >= 0, column < columns, page < text.pages.count else { return nil }
        return CGPoint(x: columnX(column) + point.x, y: margins.top + point.y - text.pages[page].top)
    }

    func hit(_ viewPoint: CGPoint) -> ChapterTextLayout.Hit? {
        guard let (_, point) = locate(viewPoint), point.x >= 0, point.x <= columnWidth else { return nil }
        return text.hit(at: point)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        dragged = false
        if let pid = controlHit(point) {
            dragAnchor = nil
            if delegate?.readerTranslationProgress(self, paragraphId: pid) == nil,
                delegate?.readerCanTranslate(self) ?? true
            {
                // ⌘- or ⌃-click translates afresh, skipping the cache (as in the web app).
                let fresh = !event.modifierFlags.intersection([.command, .control]).isEmpty
                delegate?.reader(self, translate: pid, useCache: !fresh)
            }
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
        let point = convert(event.locationInWindow, from: nil)
        guard let hit = hit(point), let word = hit.wordIndex else {
            // A click on empty space deselects, as in the web app.
            if selected != nil {
                selected = nil
                delegate?.readerDeselected(self)
            }
            return
        }
        let key = WordKey(paragraphId: hit.paragraphId, wordIndex: word)
        revealed.insert(key)
        selected = (hit.paragraphId, word)
        delegate?.reader(self, selected: hit.paragraphId, wordIndex: word)
    }

    /// Storage offset under a view point on a visible page, clamped to the page.
    private func offset(at viewPoint: CGPoint) -> Int? {
        guard let (column, point) = locate(viewPoint) ?? nearestColumnPoint(viewPoint) else { return nil }
        _ = column
        let x = min(max(point.x, 0), columnWidth)
        return text.characterOffset(at: CGPoint(x: x, y: point.y))
    }

    private func nearestColumnPoint(_ viewPoint: CGPoint) -> (Int, CGPoint)? {
        for column in (0..<columns).reversed() {
            let index = spreadStart + column
            guard index < text.pages.count, viewPoint.x >= columnX(column) - columnGap / 2 || column == 0 else { continue }
            let page = text.pages[index]
            let y = min(max(viewPoint.y - margins.top + page.top, page.top), page.bottom - 1)
            return (column, CGPoint(x: viewPoint.x - columnX(column), y: y))
        }
        return nil
    }

    var selectedText: String? {
        guard let range = textSelection, range.length > 0, NSMaxRange(range) <= text.storage.length else { return nil }
        return (text.storage.string as NSString).substring(with: range).replacingOccurrences(of: "\u{2028}", with: "\n")
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

    override func resetCursorRects() {
        super.resetCursorRects()
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

    override func accessibilityLabel() -> String? {
        isProvisional ? "Page" : "Pages \(spreadStart + 1) to \(min(spreadStart + columns, text.pages.count)) of \(text.pages.count)"
    }

    /// The text on the visible pages, so VoiceOver can read the spread.
    override func accessibilityValue() -> Any? {
        guard !text.pages.isEmpty else { return nil }
        let first = text.pages[spreadStart]
        let last = text.pages[min(spreadStart + columns, text.pages.count) - 1]
        guard let start = text.layoutManager.textLayoutFragment(for: CGPoint(x: 1, y: first.top + 1)),
            let end = text.layoutManager.textLayoutFragment(for: CGPoint(x: 1, y: max(last.bottom - 1, first.top + 1)))
        else { return nil }
        let a = text.offset(of: start.rangeInElement.location)
        let b = text.offset(of: end.rangeInElement.endLocation)
        guard b > a else { return nil }
        return (text.storage.string as NSString).substring(with: NSRange(location: a, length: b - a))
            .replacingOccurrences(of: "\u{2028}", with: "\n")
    }

    override func accessibilityPerformIncrement() -> Bool {
        nextSpread()
        return true
    }

    override func accessibilityPerformDecrement() -> Bool {
        previousSpread()
        return true
    }

    // MARK: Paging

    func go(toPage page: Int, animated: Bool = true) {
        let target = min(max(page, 0), max(text.pages.count - 1, 0))
        guard target != spreadStart else { return }
        if animated, let layer {
            let transition = CATransition()
            transition.type = .push
            transition.subtype = target > spreadStart ? .fromRight : .fromLeft
            transition.duration = 0.18
            transition.timingFunction = CAMediaTimingFunction(name: .easeOut)
            layer.add(transition, forKey: "flip")
        }
        spreadStart = target
        textSelection = nil
        sticky = currentAnchor()
        needsDisplay = true
        delegate?.reader(self, movedTo: spreadStart, of: text.pages.count)
        NSAccessibility.post(element: self, notification: .valueChanged)
    }

    func nextSpread() { go(toPage: spreadStart + columns) }
    func previousSpread() { go(toPage: spreadStart - columns) }

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
        guard event.hasPreciseScrollingDeltas else {
            if event.scrollingDeltaY < 0 || event.scrollingDeltaX < 0 { nextSpread() } else { previousSpread() }
            return
        }
        if event.phase == .began { swipeAccumulator = 0 }
        let delta = abs(event.scrollingDeltaX) > abs(event.scrollingDeltaY) ? event.scrollingDeltaX : event.scrollingDeltaY
        swipeAccumulator += delta
        if swipeAccumulator < -60 {
            swipeAccumulator = .infinity
            nextSpread()
        } else if swipeAccumulator > 60, swipeAccumulator.isFinite {
            swipeAccumulator = -.infinity
            previousSpread()
        }
        if event.phase == .ended || event.phase == .cancelled { swipeAccumulator = 0 }
    }

    /// Shows the page holding a paragraph (e.g. one selected elsewhere).
    func reveal(paragraphId: Int) {
        guard let page = text.page(forParagraph: paragraphId, pageOffset: 0) else { return }
        if page < spreadStart || page >= spreadStart + columns { go(toPage: page, animated: false) }
    }
}

extension ChapterTextLayout {
    func firstLineTop(of paragraphId: Int) -> CGFloat? {
        firstLineOfParagraph[paragraphId].map { lines[$0].top }
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
