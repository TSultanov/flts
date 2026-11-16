package com.TS.FLTS.reader

/// One line of the chapter, in layout coordinates (one tall column).
data class LaidLine(
    val top: Float,
    val bottom: Float,
    val baseline: Float,
    val paragraphIndex: Int,
    /// UTF-16 offset of the line's first character within its paragraph.
    val start: Int,
)

/// A line identified by content, so it can be found again in another layout.
data class LineAnchor(val paragraphId: Int, val start: Int)

data class ReaderPage(
    val top: Float,
    val bottom: Float,
    /// Index into `order` of the paragraph owning the page's first line.
    val firstParagraph: Int,
)

/// A reading position: the synced (paragraph, pages into it) pair, plus the
/// exact top line when this device knows it.
data class ReadingAnchor(val paragraphId: Int, val pageOffset: Int, val line: LineAnchor? = null)

/// A chapter's lines cut into pages at line boundaries. Pages run forward
/// from the forced line and backward from it, so only the chapter's first
/// page may be short.
class PageMap(val order: List<Int>, val lines: List<LaidLine>, val pageHeight: Float, breakAt: LineAnchor? = null) {
    val pages: List<ReaderPage>
    val firstPageOfParagraph: Map<Int, Int>
    val firstLineOfParagraph: Map<Int, Int>

    init {
        var forced = 0
        if (breakAt != null) {
            val pIndex = order.indexOf(breakAt.paragraphId)
            if (pIndex >= 0) {
                forced = lines.indexOfLast { it.paragraphIndex == pIndex && it.start <= breakAt.start }
                if (forced < 0) forced = lines.indexOfFirst { it.paragraphIndex >= pIndex }
                if (forced < 0) forced = 0
            }
        }
        val starts = ArrayList<Int>()
        var i = forced
        while (i < lines.size) {
            starts.add(i)
            var j = i + 1
            while (j < lines.size && lines[j].bottom - lines[i].top <= pageHeight) j += 1
            i = j
        }
        val before = ArrayList<Int>()
        var end = forced
        while (end > 0) {
            var j = end - 1
            while (j > 0 && lines[end - 1].bottom - lines[j - 1].top <= pageHeight) j -= 1
            before.add(j)
            end = j
        }
        val all = before.asReversed() + starts
        val built = ArrayList<ReaderPage>(all.size)
        val firstPage = HashMap<Int, Int>()
        for ((n, start) in all.withIndex()) {
            val stop = if (n + 1 < all.size) all[n + 1] else lines.size
            // Up to the next line, but never past the page: paragraph spacing sits between lines.
            val bottom =
                if (stop < lines.size) minOf(lines[stop].top, lines[start].top + pageHeight) else lines[stop - 1].bottom
            built.add(ReaderPage(lines[start].top, bottom, lines[start].paragraphIndex))
            for (k in start until stop) firstPage.putIfAbsent(lines[k].paragraphIndex, n)
        }
        pages = if (built.isEmpty()) listOf(ReaderPage(0f, pageHeight, 0)) else built
        firstPageOfParagraph = firstPage.filterKeys { it < order.size }.mapKeys { order[it.key] }
        val firstLine = HashMap<Int, Int>()
        for ((n, line) in lines.withIndex()) {
            if (line.paragraphIndex < order.size) firstLine.putIfAbsent(order[line.paragraphIndex], n)
        }
        firstLineOfParagraph = firstLine
    }

    /// The first line of a page, by content.
    fun lineAnchor(page: Int): LineAnchor? {
        if (page !in pages.indices) return null
        val top = pages[page].top
        val line = lines.firstOrNull { it.top >= top - 0.5f } ?: return null
        if (line.paragraphIndex >= order.size) return null
        return LineAnchor(order[line.paragraphIndex], line.start)
    }

    fun page(anchor: LineAnchor): Int? {
        val pIndex = order.indexOf(anchor.paragraphId)
        if (pIndex < 0) return null
        val line = lines.indexOfLast { it.paragraphIndex == pIndex && it.start <= anchor.start }
        if (line < 0) return null
        return pageContaining(lines[line].top + 0.5f)
    }

    fun pageContaining(y: Float): Int {
        var lo = 0
        var hi = pages.size - 1
        while (lo < hi) {
            val mid = (lo + hi + 1) / 2
            if (pages[mid].top <= y) lo = mid else hi = mid - 1
        }
        return maxOf(lo, 0)
    }

    /// The paragraph the page starts in, and how many pages into it the page is.
    fun anchor(page: Int): ReadingAnchor? {
        if (page !in pages.indices || pages[page].firstParagraph >= order.size) return null
        val pid = order[pages[page].firstParagraph]
        val first = firstPageOfParagraph[pid] ?: page
        return ReadingAnchor(pid, maxOf(page - first, 0))
    }

    fun page(paragraphId: Int, pageOffset: Int): Int? {
        val index = order.indexOf(paragraphId)
        if (index < 0) return null
        val firstLine = firstLineOfParagraph[paragraphId]
        if (firstLine == null) {
            // A paragraph with no text has no line; use the next that has one.
            for (later in order.subList(index, order.size)) {
                firstLineOfParagraph[later]?.let { return pageContaining(lines[it].top + 0.5f) }
            }
            return null
        }
        val nextLine = order.subList(index + 1, order.size).firstNotNullOfOrNull { firstLineOfParagraph[it] } ?: lines.size
        val first = pageContaining(lines[firstLine].top + 0.5f)
        val last = pageContaining(lines[maxOf(nextLine - 1, firstLine)].top + 0.5f)
        return minOf(first + pageOffset, last)
    }

    fun firstLineTop(paragraphId: Int): Float? = firstLineOfParagraph[paragraphId]?.let { lines[it].top }
}
