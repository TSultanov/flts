import AppKit

extension NSAttributedString.Key {
    static let fltsParagraph = NSAttributedString.Key("flts.paragraph")
    /// Index into the paragraph's `words`.
    static let fltsWord = NSAttributedString.Key("flts.word")
}

struct ReaderTypography: Equatable {
    var fontSize: CGFloat = 18
    var lineHeightMultiple: CGFloat = 1.75
    var paragraphSpacingEm: CGFloat = 0.6
    var hyphenate = true
    var justify = true
    /// BCP 47 tag of the text's language; hyphenation follows it.
    var language: String?

    var lineHeight: CGFloat { (fontSize * lineHeightMultiple).rounded() }

    func baseFont() -> NSFont {
        let system = NSFont.systemFont(ofSize: fontSize)
        let serif = system.fontDescriptor.withDesign(.serif) ?? system.fontDescriptor
        return NSFont(descriptor: serif, size: fontSize) ?? system
    }

    func font(marks: Int) -> NSFont {
        let base = baseFont()
        var traits: NSFontDescriptor.SymbolicTraits = []
        if marks & 1 != 0 { traits.insert(.italic) }
        if marks & 2 != 0 { traits.insert(.bold) }
        guard !traits.isEmpty else { return base }
        let descriptor = base.fontDescriptor.withSymbolicTraits(base.fontDescriptor.symbolicTraits.union(traits))
        return NSFont(descriptor: descriptor, size: fontSize) ?? base
    }

    func paragraphStyle() -> NSParagraphStyle {
        let style = NSMutableParagraphStyle()
        style.minimumLineHeight = lineHeight
        style.maximumLineHeight = lineHeight
        style.paragraphSpacing = (fontSize * paragraphSpacingEm).rounded()
        style.alignment = justify ? .justified : .natural
        style.usesDefaultHyphenation = hyphenate
        style.lineBreakStrategy = .standard
        return style
    }

    var translationFont: NSFont {
        NSFont.systemFont(ofSize: (fontSize * 0.55).rounded(.down))
    }
}

/// One line on a page, in container coordinates.
struct LaidLine {
    let top: CGFloat
    let bottom: CGFloat
    let paragraphIndex: Int
    /// UTF-16 offset of the line's first character within its paragraph.
    let start: Int
}

/// A line identified by content, so it can be found again in another layout.
struct LineAnchor: Equatable {
    let paragraphId: Int
    let start: Int
}

struct ReaderPage {
    let top: CGFloat
    let bottom: CGFloat
    /// Index into `order` of the paragraph owning the page's first line.
    let firstParagraph: Int
}

/// A chapter's text in one TextKit 2 layout of column width; pages are bands
/// of it cut at line boundaries, so nothing is laid out per page. Used by one
/// thread at a time: a full layout is built off the main thread, then handed over.
final class ChapterTextLayout: @unchecked Sendable {
    let contentStorage = NSTextContentStorage()
    let layoutManager = NSTextLayoutManager()
    let container = NSTextContainer(size: CGSize(width: 400, height: 0))

    private(set) var order: [Int] = []
    private(set) var paragraphs: [Int: ParagraphText] = [:]
    /// UTF-16 range of each paragraph in the storage, separator excluded.
    private(set) var ranges: [Int: NSRange] = [:]
    private(set) var pages: [ReaderPage] = []
    private(set) var lines: [LaidLine] = []
    private(set) var firstPageOfParagraph: [Int: Int] = [:]
    private(set) var typography = ReaderTypography()
    private(set) var pageHeight: CGFloat = 600

    init() {
        container.lineFragmentPadding = 0
        layoutManager.textContainer = container
        contentStorage.addTextLayoutManager(layoutManager)
        contentStorage.primaryTextLayoutManager = layoutManager
    }

    var storage: NSTextStorage { contentStorage.textStorage! }

    // MARK: Content

    func setContent(order: [Int], paragraphs: [Int: ParagraphText], typography: ReaderTypography) {
        self.order = order
        self.paragraphs = paragraphs
        self.typography = typography
        let text = NSMutableAttributedString()
        for (i, id) in order.enumerated() {
            if i > 0 { text.append(NSAttributedString(string: "\n", attributes: baseAttributes())) }
            if let p = paragraphs[id] { text.append(attributed(p)) }
        }
        contentStorage.performEditingTransaction {
            storage.setAttributedString(text)
        }
        rebuildRanges()
    }

    /// Replaces changed paragraphs in place, so only their layout is redone.
    func replace(_ changed: [ParagraphText]) {
        guard !changed.isEmpty else { return }
        let sorted = changed.compactMap { p in ranges[p.id].map { (p, $0) } }
            .sorted { $0.1.location > $1.1.location }
        contentStorage.performEditingTransaction {
            for (p, range) in sorted {
                paragraphs[p.id] = p
                storage.replaceCharacters(in: range, with: attributed(p))
            }
        }
        rebuildRanges()
    }

    private func rebuildRanges() {
        var location = 0
        ranges = [:]
        for (i, id) in order.enumerated() {
            if i > 0 { location += 1 }
            let len = paragraphs[id].map { ($0.text as NSString).length } ?? 0
            ranges[id] = NSRange(location: location, length: len)
            location += len
        }
    }

    private var cachedBase: [NSAttributedString.Key: Any] = [:]
    private var cachedFonts: [Int: NSFont] = [:]
    private var cachedFor: ReaderTypography?

    private func baseAttributes() -> [NSAttributedString.Key: Any] {
        if cachedFor != typography {
            cachedFor = typography
            cachedFonts = [:]
            cachedBase = [
                .font: typography.baseFont(), .paragraphStyle: typography.paragraphStyle(),
                .foregroundColor: NSColor.textColor,
            ]
            if let language = typography.language {
                cachedBase[NSAttributedString.Key(kCTLanguageAttributeName as String)] = language
            }
        }
        return cachedBase
    }

    private func font(marks: Int) -> NSFont {
        if let f = cachedFonts[marks] { return f }
        let f = typography.font(marks: marks)
        cachedFonts[marks] = f
        return f
    }

    private func attributed(_ p: ParagraphText) -> NSAttributedString {
        var attrs = baseAttributes()
        attrs[.fltsParagraph] = p.id
        if !p.translated { attrs[.foregroundColor] = NSColor.secondaryLabelColor }
        let s = NSMutableAttributedString(string: p.text, attributes: attrs)
        let length = s.length
        for run in p.runs where run.start + run.len <= length {
            s.addAttribute(.font, value: font(marks: run.marks), range: NSRange(location: run.start, length: run.len))
        }
        for (i, w) in p.words.enumerated() where w.start + w.len <= length {
            s.addAttribute(.fltsWord, value: i, range: NSRange(location: w.start, length: w.len))
        }
        return s
    }

    // MARK: Layout

    /// Lays the whole chapter out at `width` and cuts it into pages; a page
    /// starts exactly at `breakAt` when given, so a swap keeps the view still.
    func layout(width: CGFloat, pageHeight: CGFloat, breakAt: LineAnchor? = nil) {
        let width = max(width.rounded(.down), 120)
        if container.size.width != width {
            container.size = CGSize(width: width, height: 0)
        }
        self.pageHeight = max(pageHeight, typography.lineHeight * 2)
        layoutManager.ensureLayout(for: layoutManager.documentRange)
        collectLines()
        paginate(breakAt: breakAt)
    }

    var columnWidth: CGFloat { container.size.width }

    private func collectLines() {
        var lines: [LaidLine] = []
        lines.reserveCapacity(order.count * 8)
        let indexOf = Dictionary(uniqueKeysWithValues: order.enumerated().map { ($1, $0) })
        layoutManager.enumerateTextLayoutFragments(from: layoutManager.documentRange.location, options: []) {
            fragment in
            let frame = fragment.layoutFragmentFrame
            let offset = self.offset(of: fragment.rangeInElement.location)
            let pid = self.paragraph(atOffset: offset)
            let pIndex = pid.flatMap { indexOf[$0] } ?? 0
            for line in fragment.textLineFragments {
                let b = line.typographicBounds
                lines.append(
                    LaidLine(
                        top: frame.minY + b.minY, bottom: frame.minY + b.maxY, paragraphIndex: pIndex,
                        start: line.characterRange.location))
            }
            return true
        }
        self.lines = lines
    }

    /// Pages run forward from the forced line and backward from it, so only
    /// the chapter's first page may be short.
    func paginate(breakAt: LineAnchor?) {
        var forced = 0
        if let breakAt, let pIndex = order.firstIndex(of: breakAt.paragraphId) {
            forced =
                lines.lastIndex { $0.paragraphIndex == pIndex && $0.start <= breakAt.start }
                ?? lines.firstIndex { $0.paragraphIndex >= pIndex } ?? 0
        }
        var starts: [Int] = []
        var i = forced
        while i < lines.count {
            starts.append(i)
            var j = i + 1
            while j < lines.count, lines[j].bottom - lines[i].top <= pageHeight { j += 1 }
            i = j
        }
        var before: [Int] = []
        var end = forced
        while end > 0 {
            var j = end - 1
            while j > 0, lines[end - 1].bottom - lines[j - 1].top <= pageHeight { j -= 1 }
            before.append(j)
            end = j
        }
        starts = before.reversed() + starts
        var pages: [ReaderPage] = []
        var firstPage: [Int: Int] = [:]
        for (n, start) in starts.enumerated() {
            let stop = n + 1 < starts.count ? starts[n + 1] : lines.count
            let bottom = stop < lines.count ? lines[stop].top : lines[stop - 1].bottom
            pages.append(ReaderPage(top: lines[start].top, bottom: bottom, firstParagraph: lines[start].paragraphIndex))
            for line in lines[start..<stop] where firstPage[line.paragraphIndex] == nil {
                firstPage[line.paragraphIndex] = n
            }
        }
        if pages.isEmpty { pages = [ReaderPage(top: 0, bottom: pageHeight, firstParagraph: 0)] }
        self.pages = pages
        self.firstPageOfParagraph = Dictionary(
            uniqueKeysWithValues: firstPage.compactMap { idx, page in idx < order.count ? (order[idx], page) : nil })
        self.firstLineOfParagraph = [:]
        for (n, line) in lines.enumerated() where line.paragraphIndex < order.count {
            let pid = order[line.paragraphIndex]
            if firstLineOfParagraph[pid] == nil { firstLineOfParagraph[pid] = n }
        }
    }

    private(set) var firstLineOfParagraph: [Int: Int] = [:]

    /// The first line of a page, by content.
    func lineAnchor(forPage page: Int) -> LineAnchor? {
        guard page >= 0, page < pages.count else { return nil }
        let top = pages[page].top
        guard let line = lines.first(where: { $0.top >= top - 0.5 }), line.paragraphIndex < order.count else {
            return nil
        }
        return LineAnchor(paragraphId: order[line.paragraphIndex], start: line.start)
    }

    func page(of anchor: LineAnchor) -> Int? {
        guard let pIndex = order.firstIndex(of: anchor.paragraphId),
            let line = lines.lastIndex(where: { $0.paragraphIndex == pIndex && $0.start <= anchor.start })
        else { return nil }
        return page(containingY: lines[line].top + 0.5)
    }

    func offset(of location: NSTextLocation) -> Int {
        contentStorage.offset(from: contentStorage.documentRange.location, to: location)
    }

    func location(at offset: Int) -> NSTextLocation? {
        contentStorage.location(contentStorage.documentRange.location, offsetBy: offset)
    }

    func paragraph(atOffset offset: Int) -> Int? {
        guard offset >= 0, offset <= storage.length else { return nil }
        // The separator after a paragraph belongs to it.
        for probe in [offset, offset - 1] where probe >= 0 && probe < storage.length {
            if let id = storage.attribute(.fltsParagraph, at: probe, effectiveRange: nil) as? Int { return id }
        }
        return order.first { ranges[$0].map { NSLocationInRange(offset, $0) } ?? false }
    }

    // MARK: Geometry

    /// Frames of a storage range in container coordinates, one per line piece.
    func frames(of range: NSRange) -> [CGRect] {
        guard let start = location(at: range.location), let end = location(at: NSMaxRange(range)),
            let textRange = NSTextRange(location: start, end: end)
        else { return [] }
        var rects: [CGRect] = []
        layoutManager.enumerateTextSegments(in: textRange, type: .standard, options: []) { _, frame, _, _ in
            if frame.width > 0 { rects.append(frame) }
            return true
        }
        return rects
    }

    struct Hit {
        let paragraphId: Int
        let wordIndex: Int?
    }

    func hit(at point: CGPoint) -> Hit? {
        guard let fragment = layoutManager.textLayoutFragment(for: point) else { return nil }
        let frame = fragment.layoutFragmentFrame
        let local = CGPoint(x: point.x - frame.minX, y: point.y - frame.minY)
        guard
            let line = fragment.textLineFragments.first(where: {
                local.y >= $0.typographicBounds.minY && local.y < $0.typographicBounds.maxY
            })
        else { return nil }
        let base = offset(of: fragment.rangeInElement.location)
        let inLine = CGPoint(x: local.x - line.typographicBounds.minX, y: local.y - line.typographicBounds.minY)
        let index = line.characterIndex(for: inLine)
        guard let pid = paragraph(atOffset: base + max(index, 0)) ?? paragraph(atOffset: base) else { return nil }
        for candidate in [index, index - 1] where candidate >= 0 {
            let docOffset = base + candidate
            guard docOffset < storage.length,
                let w = storage.attribute(.fltsWord, at: docOffset, effectiveRange: nil) as? Int,
                let p = paragraphs[pid], w < p.words.count, let range = ranges[pid]
            else { continue }
            let word = p.words[w]
            let wordRange = NSRange(location: range.location + word.start, length: word.len)
            if frames(of: wordRange).contains(where: { $0.insetBy(dx: -2, dy: -4).contains(point) }) {
                return Hit(paragraphId: pid, wordIndex: w)
            }
        }
        return Hit(paragraphId: pid, wordIndex: nil)
    }

    /// Nearest character boundary to a container point, as a storage offset.
    func characterOffset(at point: CGPoint) -> Int? {
        guard let fragment = layoutManager.textLayoutFragment(for: point) else { return nil }
        let frame = fragment.layoutFragmentFrame
        let local = CGPoint(x: point.x - frame.minX, y: point.y - frame.minY)
        let lines = fragment.textLineFragments
        guard
            let line = lines.first(where: { local.y < $0.typographicBounds.maxY }) ?? lines.last
        else { return nil }
        let base = offset(of: fragment.rangeInElement.location)
        let inLine = CGPoint(x: local.x - line.typographicBounds.minX, y: local.y - line.typographicBounds.minY)
        return min(base + max(line.characterIndex(for: inLine), 0), storage.length)
    }

    func wordRange(paragraphId: Int, wordIndex: Int) -> NSRange? {
        guard let p = paragraphs[paragraphId], wordIndex < p.words.count, let range = ranges[paragraphId] else {
            return nil
        }
        let w = p.words[wordIndex]
        return NSRange(location: range.location + w.start, length: w.len)
    }

    func page(containingY y: CGFloat) -> Int {
        var lo = 0
        var hi = pages.count - 1
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if pages[mid].top <= y { lo = mid } else { hi = mid - 1 }
        }
        return max(lo, 0)
    }

    /// Text fragments that intersect a page, for drawing.
    func fragments(in page: ReaderPage) -> [NSTextLayoutFragment] {
        var out: [NSTextLayoutFragment] = []
        let start =
            layoutManager.textLayoutFragment(for: CGPoint(x: 0, y: page.top))?.rangeInElement.location
            ?? layoutManager.documentRange.location
        layoutManager.enumerateTextLayoutFragments(from: start, options: []) { fragment in
            let frame = fragment.layoutFragmentFrame
            if frame.minY >= page.bottom { return false }
            if frame.maxY > page.top { out.append(fragment) }
            return true
        }
        return out
    }

    // MARK: Reading position

    /// The paragraph the page starts in, and how many pages into it the page is.
    func anchor(forPage page: Int) -> (paragraphId: Int, pageOffset: Int)? {
        guard page >= 0, page < pages.count, pages[page].firstParagraph < order.count else { return nil }
        let pid = order[pages[page].firstParagraph]
        let first = firstPageOfParagraph[pid] ?? page
        return (pid, max(page - first, 0))
    }

    func page(forParagraph pid: Int, pageOffset: Int) -> Int? {
        guard let index = order.firstIndex(of: pid) else { return nil }
        guard let firstLine = firstLineOfParagraph[pid] else {
            // A paragraph with no text has no line; use the next that has one.
            for later in order[index...] {
                if let line = firstLineOfParagraph[later] { return page(containingY: lines[line].top + 0.5) }
            }
            return nil
        }
        let nextLine = order[(index + 1)...].lazy.compactMap { self.firstLineOfParagraph[$0] }.first ?? lines.count
        let first = page(containingY: lines[firstLine].top + 0.5)
        let last = page(containingY: lines[max(nextLine - 1, firstLine)].top + 0.5)
        return min(first + pageOffset, last)
    }
}
