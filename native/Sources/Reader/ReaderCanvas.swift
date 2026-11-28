#if os(macOS)
import AppKit
#else
import UIKit
#endif
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
final class ReaderCanvas: PlatformView {
    /// What is drawn: the full chapter, or while that is laid out off the
    /// main thread, a provisional layout starting at the reading position.
    private(set) var text = ChapterTextLayout()
    private(set) var isProvisional = false
    var order: [Int] = []
    var paragraphs: [Int: ParagraphText] = [:]
    var typography = ReaderTypography()
    var generation = 0
    /// Set when the full layout lands, for tests and timing.
    var onFullLayout: (() -> Void)?
    /// Lets tests simulate a window drag.
    var inLiveResizeOverride = false
    weak var delegate: ReaderCanvasDelegate?
    /// Whatever the platform's input code needs to keep (extensions cannot store).
    var platformState: AnyObject?

    /// The page in the first column. Spreads keep the pairing the chapter
    /// opened with, so no page is ever shown in two of them; when that pairing
    /// is odd, the first spread starts one page early, with a blank left page.
    private(set) var spreadStart = 0
    var firstVisiblePage: Int { max(spreadStart, 0) }
    private(set) var columns = 1
    var columnWidth: CGFloat = 400
    var laidOutSize: CGSize = .zero
    var pendingAnchor: ReadingAnchor?
    /// Where the reader is, as last set by the reader (opening, flipping).
    /// Relayouts restore to it rather than to whatever line they snapped to,
    /// so resizes and sidebar animations cannot walk the position.
    var sticky: ReadingAnchor?
    /// Full layouts started off the main thread, for tests.
    private(set) var fullLayoutsStarted = 0
    var swipeAccumulator: CGFloat = 0
    var swipeLastDelta: CGFloat = 0
    /// How far the spread is dragged sideways, in points.
    private(set) var slide: CGFloat = 0 {
        didSet { if slide != oldValue { redraw() } }
    }
    private nonisolated(unsafe) var slideLink: CADisplayLink?
    private var slideAnimator: SlideDisplayLink?

    /// A dragged text selection, as a storage range of the current layout.
    var textSelection: NSRange? {
        didSet { redraw() }
    }
    var dragAnchor: Int?
    var dragged = false

    var selected: (paragraphId: Int, wordIndex: Int)? {
        didSet { redraw() }
    }
    /// Words whose translation is shown above them.
    var revealed: Set<WordKey> = [] {
        didSet { redraw() }
    }

    struct WordKey: Hashable {
        let paragraphId: Int
        let wordIndex: Int
    }

    var margins = PlatformEdgeInsets(top: 40, left: 56, bottom: 44, right: 56)
    var columnGap: CGFloat = 64
    /// The paragraph button in the left margin, and its distance from the text.
    var controlSize: CGFloat = 22
    var controlGap: CGFloat = 10
    /// A phone-sized view: tighter margins and a smaller paragraph button.
    var tight = false
    private(set) var marginScale: CGFloat = 1

    override init(frame: CGRect) {
        super.init(frame: frame)
        platformSetUp()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    deinit {
        slideLink?.invalidate()
    }

    func detachFromWindow() {
        endSlide()
        drainScheduled = false
        generation += 1
        onFullLayout = nil
        onChangesApplied = nil
    }

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
        relayoutSoon()
        redraw()
    }

    /// Changed paragraphs not yet applied; applied a few per run-loop turn so
    /// a burst (sync, a finished chapter) never blocks a frame for long.
    var pendingChanges: [Int: ParagraphText] = [:]
    var drainScheduled = false
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

    func scheduleDrain() {
        guard !drainScheduled else { return }
        drainScheduled = true
        DispatchQueue.main.async { [weak self] in
            self?.drainChanges()
        }
    }

    func drainChanges() {
        drainScheduled = false
        guard !isProvisional else { return }
        let ids = pendingChanges.keys.sorted().prefix(Self.changesPerTurn)
        let batch = ids.compactMap { pendingChanges.removeValue(forKey: $0) }
        if !batch.isEmpty {
            textSelection = nil
            let wasOddFirst = spreadStart == -1 && columns > 1
            let keep = text.lineAnchor(forPage: firstVisiblePage)
            text.replace(batch)
            text.layout(width: columnWidth, pageHeight: text.pageHeight, breakAt: keep)
            let page = keep.flatMap { text.page(of: $0) } ?? min(firstVisiblePage, text.pages.count - 1)
            spreadStart = (wasOddFirst && page == 0) ? -1 : page
            redraw()
            delegate?.reader(self, movedTo: firstVisiblePage, of: text.pages.count)
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

    func setMarginScale(_ scale: CGFloat) {
        guard scale != marginScale else { return }
        marginScale = scale
        updateMargins()
        relayout(keeping: sticky ?? currentAnchor())
    }

    func updateMargins() {
        let side = ((tight ? 13 : 56) * marginScale).rounded()
        margins = PlatformEdgeInsets(top: tight ? 20 : 40, left: side, bottom: tight ? 34 : 44, right: side)
        // The paragraph button shrinks to fit the left margin.
        controlSize = min(tight ? 18 : 22, max(side - 2, 6))
        controlGap = min(tight ? 5 : 10, max(side - controlSize - 3, 1))
    }

    func currentAnchor() -> ReadingAnchor? {
        if text.pages.isEmpty || order.isEmpty { return pendingAnchor }
        guard let a = text.anchor(forPage: firstVisiblePage) else { return nil }
        return ReadingAnchor(
            paragraphId: a.paragraphId, pageOffset: a.pageOffset, line: text.lineAnchor(forPage: firstVisiblePage))
    }

    var pageCount: Int { text.pages.count }
    var contentOrder: [Int] { order }

    /// The paragraph as the reader has it (including changes not yet laid out).
    func contentParagraph(_ id: Int) -> ParagraphText? { paragraphs[id] }

    // MARK: Layout

    /// Called by the platform's layout pass.
    func sizeChanged() {
        if bounds.size != laidOutSize {
            relayout(keeping: laidOutSize == .zero ? pendingAnchor : (sticky ?? currentAnchor()))
        }
    }

    func relayout(keeping anchor: ReadingAnchor?) {
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
        redraw()
        delegate?.reader(self, movedTo: firstVisiblePage, of: text.pages.count)

        generation += 1
        let gen = generation
        // During a live resize only the cheap layout runs; the full one follows the drag.
        if isResizingLive || inLiveResizeOverride, isProvisional { return }
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

    func startFullLayout(generation gen: Int, pageHeight: CGFloat) {
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

    /// Swaps in the full layout, re-cut so the visible line still starts the page.
    func adopt(_ full: ChapterTextLayout, generation gen: Int) {
        guard gen == generation else { return }
        let keep = text.lineAnchor(forPage: firstVisiblePage)
        full.paginate(breakAt: keep)
        let selectedBefore = selected
        textSelection = nil
        text = full
        isProvisional = false
        spreadStart = keep.flatMap { full.page(of: $0) } ?? 0
        selected = selectedBefore
        redraw()
        delegate?.reader(self, movedTo: firstVisiblePage, of: text.pages.count)
        // Changes that arrived after this layout started.
        if !pendingChanges.isEmpty { scheduleDrain() }
        onFullLayout?()
    }

    /// Left edge of a column in view coordinates (the spread is centered).
    func columnX(_ column: Int) -> CGFloat {
        let spreadWidth = columnWidth * CGFloat(columns) + columnGap * CGFloat(columns - 1)
        let left = max(margins.left, (bounds.width - spreadWidth) / 2)
        return left + CGFloat(column) * (columnWidth + columnGap)
    }

    // MARK: Drawing

    /// The platform's `draw` calls this with its current context.
    func drawContents(_ dirtyRect: CGRect, in ctx: CGContext) {
        PlatformColor.readerBackground.setFill()
        ctx.fill(dirtyRect)
        guard !text.pages.isEmpty else { return }
        ctx.saveGState()
        ctx.translateBy(x: slide, y: 0)
        let rects = drawSpread(from: spreadStart, in: ctx)
        // While it is dragged aside, the spread it uncovers shows beside it.
        if slide != 0, let other = neighbour(slide < 0 ? 1 : -1) {
            ctx.translateBy(x: slide < 0 ? bounds.width : -bounds.width, y: 0)
            drawSpread(from: other, in: ctx)
        }
        ctx.restoreGState()
        drawFooter()
        if rects != controlSignature {
            controlSignature = rects
            DispatchQueue.main.async { [weak self] in self?.controlsMoved() }
        }
    }

    @discardableResult
    private func drawSpread(from start: Int, in ctx: CGContext) -> [CGRect] {
        var rects: [CGRect] = []
        for column in 0..<columns {
            let index = start + column
            guard index >= 0 else { continue }
            guard index < text.pages.count else { break }
            rects += drawPage(text.pages[index], at: CGPoint(x: columnX(column), y: margins.top), in: ctx)
        }
        return rects
    }

    @discardableResult
    func drawPage(_ page: ReaderPage, at origin: CGPoint, in ctx: CGContext) -> [CGRect] {
        let dy = origin.y - page.top
        let band = CGRect(x: origin.x - columnGap / 2, y: origin.y, width: columnWidth + columnGap, height: page.bottom - page.top)

        ctx.saveGState()
        // Descenders hang below a line's box (its extra leading sits above the
        // glyphs), so the text band is shifted down by their depth.
        let descent = (text.typography.fontSize * 0.4).rounded()
        ctx.clip(to: band.offsetBy(dx: 0, dy: descent))

        if let range = textSelection, range.length > 0 {
            PlatformColor.readerSelection.setFill()
            for frame in text.frames(of: range) {
                ctx.fill(highlightRect(frame).offsetBy(dx: origin.x, dy: dy))
            }
        }

        if let sel = selected, let range = text.wordRange(paragraphId: sel.paragraphId, wordIndex: sel.wordIndex) {
            PlatformColor.readerAccent.withAlphaComponent(0.22).setFill()
            for frame in text.frames(of: range) {
                let r = highlightRect(frame).offsetBy(dx: origin.x, dy: dy).insetBy(dx: -2, dy: 0)
                ctx.addPath(CGPath(roundedRect: r, cornerWidth: 3, cornerHeight: 3, transform: nil))
                ctx.fillPath()
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
        let controls = drawParagraphControls(in: page, origin: origin, dy: dy, ctx: ctx)
        ctx.restoreGState()
        return controls
    }

    /// The glyph box inside a line-height-tall segment frame; extra leading sits above.
    func glyphRect(_ frame: CGRect) -> CGRect {
        let fontSize = text.typography.fontSize
        let glyphHeight = (fontSize * 1.3).rounded()
        return CGRect(x: frame.minX, y: frame.maxY - glyphHeight, width: frame.width, height: glyphHeight)
    }

    /// The glyph box without the headroom above tall letters, where a word's
    /// translation is drawn: a highlight must not reach under it.
    func highlightRect(_ frame: CGRect) -> CGRect {
        let glyph = glyphRect(frame)
        let headroom = (text.typography.fontSize * 0.2).rounded()
        return CGRect(x: glyph.minX, y: glyph.minY + headroom, width: glyph.width, height: glyph.height - headroom)
    }

    func drawTranslations(in page: ReaderPage, origin: CGPoint, dy: CGFloat) {
        guard !revealed.isEmpty else { return }
        let font = text.typography.translationFont
        let style = NSMutableParagraphStyle()
        style.alignment = .center
        style.lineBreakMode = .byClipping
        let visibleParagraphs = Set(
            text.lines[page.lineRange].compactMap { $0.paragraphIndex < text.order.count ? text.order[$0.paragraphIndex] : nil })
        for key in revealed where visibleParagraphs.contains(key.paragraphId) {
            guard let p = text.paragraphs[key.paragraphId], key.wordIndex < p.words.count,
                let translation = p.words[key.wordIndex].translation, !translation.isEmpty,
                let range = text.wordRange(paragraphId: key.paragraphId, wordIndex: key.wordIndex),
                let first = text.frames(of: range).first,
                first.minY >= page.top - 1, first.maxY <= page.bottom + 1
            else { continue }
            let glyph = glyphRect(first)
            var attrs: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: PlatformColor.readerAccent, .paragraphStyle: style,
            ]
            var size = (translation as NSString).size(withAttributes: attrs)
            // Shrink to the word's width, as the web app does.
            if size.width > glyph.width, glyph.width > 0 {
                let scale = glyph.width / size.width
                let shrunk = PlatformFont.systemFont(ofSize: font.pointSize * scale)
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

    func drawParagraphControls(in page: ReaderPage, origin: CGPoint, dy: CGFloat, ctx: CGContext) -> [CGRect] {
        let controls = controlRects(in: page, origin: origin, dy: dy)
        for (pid, rect) in controls {
            if let progress = delegate?.readerTranslationProgress(self, paragraphId: pid) {
                drawProgress(progress, in: rect, ctx: ctx)
            } else {
                let enabled = delegate?.readerCanTranslate(self) ?? true
                let tint = enabled ? PlatformColor.readerSecondary : PlatformColor.readerTertiary.withAlphaComponent(0.35)
                let pointSize = (controlSize * (controlSize < 16 ? 0.85 : 0.6)).rounded()
                #if os(macOS)
                let symbol = PlatformImage.readerSymbol("character.bubble", pointSize: pointSize, tint: tint)
                #else
                let symbol = PlatformImage.readerSymbol(
                    "character.bubble", pointSize: pointSize, tint: tint, traitCollection: traitCollection)
                #endif
                guard let symbol else { continue }
                symbol.draw(in: rect.insetBy(dx: (rect.width - symbol.size.width) / 2, dy: (rect.height - symbol.size.height) / 2))
            }
        }
        return controls.map(\.1)
    }

    func drawProgress(_ progress: Double, in rect: CGRect, ctx: CGContext) {
        let ring = rect.insetBy(dx: 4, dy: 4)
        ctx.setLineWidth(2.5)
        PlatformColor.readerTertiary.setStroke()
        ctx.strokeEllipse(in: ring)
        PlatformColor.readerAccent.setStroke()
        let start = -CGFloat.pi / 2
        ctx.addArc(
            center: CGPoint(x: ring.midX, y: ring.midY), radius: ring.width / 2, startAngle: start,
            endAngle: start + 2 * .pi * min(max(progress, 0.03), 1), clockwise: false)
        ctx.strokePath()
    }

    /// Gutter buttons for untranslated or translating paragraphs that start on this page.
    func controlRects(in page: ReaderPage, origin: CGPoint, dy: CGFloat) -> [(Int, CGRect)] {
        guard !page.lineRange.isEmpty, page.lineRange.upperBound <= text.lines.count else { return [] }
        var out: [(Int, CGRect)] = []
        let size = controlSize
        for line in text.lines[page.lineRange] {
            guard line.paragraphIndex < text.order.count else { continue }
            let pid = text.order[line.paragraphIndex]
            guard text.firstLineTop(of: pid) == line.top else { continue }
            let translating = delegate?.readerTranslationProgress(self, paragraphId: pid) != nil
            guard translating || !(text.paragraphs[pid]?.translated ?? true) else { continue }
            // Centred on the first line's lowercase letters.
            let y = line.bottom - text.typography.fontSize * 0.55 - size / 2 + dy
            out.append((pid, CGRect(x: origin.x - size - controlGap, y: y, width: size, height: size)))
        }
        return out
    }

    func drawFooter() {
        guard !text.pages.isEmpty else { return }
        let last = min(spreadStart + columns, text.pages.count)
        var label =
            columns > 1 && last - firstVisiblePage > 1
            ? "\(firstVisiblePage + 1)–\(last) of \(text.pages.count)" : "\(firstVisiblePage + 1) of \(text.pages.count)"
        if isProvisional { label = "…" }
        let attrs: [NSAttributedString.Key: Any] = [
            .font: PlatformFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: PlatformColor.readerTertiary,
        ]
        let size = (label as NSString).size(withAttributes: attrs)
        let point = CGPoint(x: (bounds.width - size.width) / 2, y: bounds.height - margins.bottom / 2 - size.height / 2)
        (label as NSString).draw(at: point, withAttributes: attrs)
    }

    // MARK: Hit testing

    func locate(_ viewPoint: CGPoint) -> (column: Int, point: CGPoint)? {
        for column in 0..<columns {
            let index = spreadStart + column
            guard index >= 0 else { continue }
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

    func controlHit(_ viewPoint: CGPoint) -> Int? {
        for column in 0..<columns {
            let index = spreadStart + column
            guard index >= 0 else { continue }
            guard index < text.pages.count else { break }
            let page = text.pages[index]
            let origin = CGPoint(x: columnX(column), y: margins.top)
            for (pid, rect) in controlRects(in: page, origin: origin, dy: origin.y - page.top)
            where rect.insetBy(dx: -controlSlop, dy: -controlSlop).contains(viewPoint) {
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

    /// A press on a paragraph's translate button; false if the point is not on one.
    /// `fresh` translates again without the cache.
    func activateControl(at point: CGPoint, fresh: Bool) -> Bool {
        guard let pid = controlHit(point) else { return false }
        if delegate?.readerTranslationProgress(self, paragraphId: pid) == nil, delegate?.readerCanTranslate(self) ?? true {
            delegate?.reader(self, translate: pid, useCache: !fresh)
        }
        return true
    }

    /// A tap on the text: reveals the word's translation and selects it; on
    /// empty space it deselects, as in the web app.
    func selectWord(at point: CGPoint) {
        guard let hit = hit(point), let word = hit.wordIndex else {
            if selected != nil {
                selected = nil
                delegate?.readerDeselected(self)
            }
            return
        }
        revealed.insert(WordKey(paragraphId: hit.paragraphId, wordIndex: word))
        selected = (hit.paragraphId, word)
        delegate?.reader(self, selected: hit.paragraphId, wordIndex: word)
    }

    /// Storage offset under a view point on a visible page, clamped to the page.
    func offset(at viewPoint: CGPoint) -> Int? {
        guard let (column, point) = locate(viewPoint) ?? nearestColumnPoint(viewPoint) else { return nil }
        _ = column
        let x = min(max(point.x, 0), columnWidth)
        return text.characterOffset(at: CGPoint(x: x, y: point.y))
    }

    func nearestColumnPoint(_ viewPoint: CGPoint) -> (Int, CGPoint)? {
        for column in (0..<columns).reversed() {
            let index = spreadStart + column
            guard index >= 0, index < text.pages.count, viewPoint.x >= columnX(column) - columnGap / 2 || column == 0 else {
                continue
            }
            let page = text.pages[index]
            let y = min(max(viewPoint.y - margins.top + page.top, page.top), page.bottom - 1)
            return (column, CGPoint(x: viewPoint.x - columnX(column), y: y))
        }
        return nil
    }

    func nearControl(_ viewPoint: CGPoint, multiplier: CGFloat = 2) -> Bool {
        let pad = controlSlop * multiplier
        return visibleControlRects().contains { $0.1.insetBy(dx: -pad, dy: -pad).contains(viewPoint) }
    }

    var selectedText: String? {
        guard let range = textSelection, range.length > 0, NSMaxRange(range) <= text.storage.length else { return nil }
        return (text.storage.string as NSString).substring(with: range).replacingOccurrences(of: "\u{2028}", with: "\n")
    }

    func visibleControlRects() -> [(Int, CGRect)] {
        var out: [(Int, CGRect)] = []
        for column in 0..<columns {
            let index = spreadStart + column
            guard index >= 0 else { continue }
            guard index < text.pages.count else { break }
            let page = text.pages[index]
            let origin = CGPoint(x: columnX(column), y: margins.top)
            out += controlRects(in: page, origin: origin, dy: origin.y - page.top)
        }
        return out
    }

    var controlSignature: [CGRect] = []

    // MARK: Accessibility

    var pageLabel: String {
        isProvisional ? "Page" : "Pages \(firstVisiblePage + 1) to \(min(spreadStart + columns, text.pages.count)) of \(text.pages.count)"
    }

    /// The text on the visible pages, so VoiceOver can read the spread.
    var visibleText: String? {
        guard !text.pages.isEmpty else { return nil }
        let first = text.pages[firstVisiblePage]
        let last = text.pages[min(spreadStart + columns, text.pages.count) - 1]
        guard let start = text.layoutManager.textLayoutFragment(for: CGPoint(x: 1, y: first.top + 1)),
            let end = text.layoutManager.textLayoutFragment(for: CGPoint(x: 1, y: max(last.bottom - 1, first.top + 1)))
        else { return nil }
        let ns = text.storage.string as NSString
        let len = ns.length
        let a = min(max(text.offset(of: start.rangeInElement.location), 0), len)
        let b = min(max(text.offset(of: end.rangeInElement.endLocation), a), len)
        guard b > a else { return nil }
        return ns.substring(with: NSRange(location: a, length: b - a))
            .replacingOccurrences(of: "\u{2028}", with: "\n")
    }

    // MARK: Paging

    /// The spread of the current pairing that holds a page; nil if it is the one shown.
    private func spreadTarget(for page: Int) -> Int? {
        guard !text.pages.isEmpty else { return nil }
        let clamped = min(max(page, 0), text.pages.count - 1)
        let phase = ((spreadStart % columns) + columns) % columns
        let target = clamped - (((clamped - phase) % columns) + columns) % columns
        return target == spreadStart ? nil : target
    }

    /// The next (+1) or previous (-1) spread, if there is one.
    private func neighbour(_ direction: Int) -> Int? {
        spreadTarget(for: spreadStart + direction * columns)
    }

    private func show(spread target: Int) {
        spreadStart = target
        textSelection = nil
        sticky = currentAnchor()
        redraw()
        delegate?.reader(self, movedTo: firstVisiblePage, of: text.pages.count)
        pageChanged()
    }

    func go(toPage page: Int, animated: Bool = true) {
        guard let target = spreadTarget(for: page) else { return }
        endSlide()
        if animated, !AppDefaults.animationsOff, !ReaderAccessibility.reduceMotionEnabled, let layer = flipLayer {
            let transition = CATransition()
            transition.type = .push
            transition.subtype = target > spreadStart ? .fromRight : .fromLeft
            transition.duration = 0.18
            transition.timingFunction = CAMediaTimingFunction(name: .easeOut)
            layer.add(transition, forKey: "flip")
        }
        show(spread: target)
    }

    // MARK: Sliding

    /// A trackpad swipe or a finger drags the spread sideways by `offset`
    /// (negative toward the next spread). With no spread that way it resists.
    func slide(to offset: CGFloat) {
        endSlideLink()
        slide = neighbour(offset < 0 ? 1 : -1) == nil ? offset * 0.2 : offset
    }

    /// The drag ended: far or fast enough turns the spread, otherwise it springs back.
    func slideEnded(velocity: CGFloat) {
        guard slide != 0 else { return }
        let forward = slide < 0
        let far = abs(slide) > bounds.width * 0.22 || (abs(velocity) > 350 && (velocity < 0) == forward)
        if far, let target = neighbour(forward ? 1 : -1) {
            animateSlide(to: forward ? -bounds.width : bounds.width) { [weak self] in
                self?.slide = 0
                self?.show(spread: target)
            }
        } else {
            animateSlide(to: 0) {}
        }
    }

    /// Drops a slide in progress, e.g. when a key turns the page mid-drag.
    private func endSlide() {
        endSlideLink()
        slide = 0
    }

    private func endSlideLink() {
        slideLink?.invalidate()
        slideLink = nil
        slideAnimator = nil
    }

    fileprivate func updateSlideAnimation(_ value: CGFloat) {
        slide = value
    }

    fileprivate func finishSlideAnimation(link: CADisplayLink, completion: @MainActor () -> Void) {
        link.invalidate()
        if slideLink === link {
            slideLink = nil
            slideAnimator = nil
        }
        completion()
    }

    private func animateSlide(to end: CGFloat, completion: @escaping @MainActor () -> Void) {
        endSlideLink()
        let start = slide
        // Nothing to watch off screen (tests), or animations are off.
        guard window != nil, !AppDefaults.animationsOff, !ReaderAccessibility.reduceMotionEnabled,
            bounds.width > 0, start != end
        else {
            slide = end
            completion()
            return
        }
        let duration = 0.12 + 0.16 * Double(min(abs(end - start) / bounds.width, 1))
        let animator = SlideDisplayLink(
            canvas: self, duration: duration, start: start, end: end, completion: completion)
        #if os(macOS)
        let link = displayLink(target: animator, selector: #selector(SlideDisplayLink.step(_:)))
        #else
        let link = CADisplayLink(target: animator, selector: #selector(SlideDisplayLink.step(_:)))
        #endif
        slideAnimator = animator
        slideLink = link
        link.add(to: .main, forMode: .common)
    }

    func nextSpread() { go(toPage: spreadStart + columns) }
    func previousSpread() { go(toPage: spreadStart - columns) }

    var lastWheelFlip: TimeInterval = 0

    /// Shows the page holding a paragraph (e.g. one selected elsewhere).
    func reveal(paragraphId: Int) {
        guard let page = text.page(forParagraph: paragraphId, pageOffset: 0) else { return }
        if page < spreadStart || page >= spreadStart + columns { go(toPage: page, animated: false) }
    }
}

@MainActor
private final class SlideDisplayLink: NSObject {
    weak var canvas: ReaderCanvas?
    let began: TimeInterval
    let duration: Double
    let start: CGFloat
    let end: CGFloat
    let completion: @MainActor () -> Void

    init(
        canvas: ReaderCanvas, duration: Double, start: CGFloat, end: CGFloat,
        completion: @escaping @MainActor () -> Void
    ) {
        self.canvas = canvas
        self.began = CACurrentMediaTime()
        self.duration = duration
        self.start = start
        self.end = end
        self.completion = completion
    }

    @objc func step(_ link: CADisplayLink) {
        guard let canvas else {
            link.invalidate()
            return
        }
        let t = min((CACurrentMediaTime() - began) / duration, 1)
        let eased = 1 - pow(1 - t, 3)
        canvas.updateSlideAnimation(start + (end - start) * eased)
        if t >= 1 {
            canvas.finishSlideAnimation(link: link, completion: completion)
        }
    }
}

extension ChapterTextLayout {
    func firstLineTop(of paragraphId: Int) -> CGFloat? {
        firstLineOfParagraph[paragraphId].map { lines[$0].top }
    }
}
