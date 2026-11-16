package com.TS.FLTS

import com.TS.FLTS.reader.LaidLine
import com.TS.FLTS.reader.LineAnchor
import com.TS.FLTS.reader.PageMap
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PaginationTests {
    /// `counts[i]` lines of height 20 for paragraph `i`, with a 10 px gap between paragraphs.
    private fun lines(counts: List<Int>): List<LaidLine> {
        val out = ArrayList<LaidLine>()
        var y = 0f
        for ((p, count) in counts.withIndex()) {
            for (i in 0 until count) {
                out.add(LaidLine(y, y + 20, y + 15, p, i * 40))
                y += 20
            }
            y += 10
        }
        return out
    }

    private val counts = List(30) { 1 + (it * 7) % 9 }
    private val order = List(30) { 100 + it }

    @Test
    fun pagesFitAndCoverEveryLineOnce() {
        val map = PageMap(order, lines(counts), 200f)
        assertTrue(map.pages.size > 5)
        for ((i, page) in map.pages.withIndex()) {
            assertTrue("page $i too tall", page.bottom - page.top <= 200.5f)
            if (i > 0) assertTrue(page.top >= map.pages[i - 1].bottom - 0.5f)
        }
        for (line in map.lines) {
            val page = map.pages[map.pageContaining(line.top + 0.5f)]
            assertTrue(line.top >= page.top && line.bottom <= page.bottom + 0.5f)
        }
    }

    @Test
    fun anchorsRoundTrip() {
        val map = PageMap(order, lines(counts), 200f)
        for (page in listOf(0, 3, map.pages.size - 1)) {
            val anchor = map.anchor(page)!!
            assertEquals(page, map.page(anchor.paragraphId, anchor.pageOffset))
            assertEquals(page, map.page(map.lineAnchor(page)!!))
        }
    }

    /// A forced break starts a page exactly there; only the first page may be short.
    @Test
    fun forcedBreakStartsAPage() {
        val all = lines(counts)
        for (target in listOf(LineAnchor(110, 80), LineAnchor(117, 0), LineAnchor(129, 40))) {
            val map = PageMap(order, all, 200f, target)
            val page = map.page(target)!!
            assertEquals(target, map.lineAnchor(page))
            for (i in 1 until map.pages.size - 1) {
                val next = map.lines.first { it.top >= map.pages[i + 1].top - 0.5f }
                assertTrue("page $i could hold another line", next.bottom - map.pages[i].top > 200f)
            }
        }
    }

    @Test
    fun emptyParagraphsUseTheNextLine() {
        // Paragraph index 1 has no lines.
        val all = lines(listOf(3, 0, 4)).map { if (it.paragraphIndex == 1) it.copy(paragraphIndex = 2) else it }
            .filter { it.paragraphIndex != 1 }
        val map = PageMap(listOf(7, 8, 9), all, 200f)
        assertEquals(map.page(9, 0), map.page(8, 0))
        assertNull(map.page(42, 0))
    }

    @Test
    fun noLinesStillHasOnePage() {
        val map = PageMap(emptyList(), emptyList(), 300f)
        assertEquals(1, map.pages.size)
        assertNull(map.lineAnchor(0))
        assertNull(map.anchor(0))
    }
}
