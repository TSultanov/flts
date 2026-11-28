package com.TS.FLTS

import android.os.Looper
import androidx.test.core.app.ApplicationProvider
import com.TS.FLTS.reader.ChapterTextLayout
import com.TS.FLTS.reader.ReaderListener
import com.TS.FLTS.reader.ReaderView
import com.TS.FLTS.reader.ReadingAnchor
import kotlin.random.Random
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import uniffi.flts_ffi.ParagraphText
import uniffi.flts_ffi.WordRange

private fun paragraph(id: Int, text: String, translated: Boolean = true): ParagraphText {
    val words = ArrayList<WordRange>()
    var offset = 0
    for ((i, w) in text.split(" ").withIndex()) {
        words.add(WordRange(offset, w.length, 0, i, i, "t$i"))
        offset += w.length + 1
    }
    return ParagraphText(id, text, emptyList(), words, translated)
}

// Real text measurement, so lines wrap as they do on a device.
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34])
class ChapterLayoutTests {
    @Test
    fun paginatesAndRoundTripsAnchors() {
        val layout = ChapterTextLayout()
        val long = List(40) { "lorem ipsum dolor sit amet" }.joinToString(" ")
        layout.setContent((0 until 20).toList(), (0 until 20).associateWith { paragraph(it, long) })
        layout.layout(400f, 500f)
        assertTrue(layout.pages.size > 10)
        for ((i, page) in layout.pages.withIndex()) {
            assertTrue("page $i too tall", page.bottom - page.top <= 500.5f)
            if (i > 0) assertTrue(page.top >= layout.pages[i - 1].bottom - 0.5f)
        }
        for (page in listOf(0, 3, layout.pages.size - 1)) {
            val anchor = layout.anchor(page)!!
            assertEquals(page, layout.page(anchor.paragraphId, anchor.pageOffset))
        }
        // Every line is exactly the typography's line height.
        for (line in layout.lines) assertEquals(layout.typography.lineHeight.toFloat(), line.bottom - line.top, 0.01f)
    }

    @Test
    fun hitTestFindsWords() {
        val layout = ChapterTextLayout()
        layout.setContent(listOf(0), mapOf(0 to paragraph(0, "alpha beta gamma")))
        layout.layout(600f, 500f)
        for (index in 0 until 3) {
            val frame = layout.wordFrames(0, index).first()
            assertEquals(index, layout.hit(frame.centerX(), frame.bottom - 6)?.wordIndex)
        }
        // Between the lines' block and nothing: no paragraph at all.
        assertEquals(null, layout.hit(10f, 4000f))
    }

    @Test
    fun replacingAParagraphKeepsOthers() {
        val layout = ChapterTextLayout()
        val long = List(30) { "uno dos tres" }.joinToString(" ")
        layout.setContent(listOf(0, 1, 2), mapOf(0 to paragraph(0, "one two"), 1 to paragraph(1, long), 2 to paragraph(2, "three")))
        layout.layout(300f, 500f)
        val before = layout.lines.filter { it.paragraphIndex == 1 }.map { it.start to it.bottom - it.top }
        val lastTop = layout.lines.last().top
        layout.replace(listOf(paragraph(0, List(20) { "uno dos tres cuatro" }.joinToString(" "))))
        layout.layout(300f, 500f)
        assertEquals("uno", layout.paragraphs[0]!!.text.take(3))
        assertEquals(before, layout.lines.filter { it.paragraphIndex == 1 }.map { it.start to it.bottom - it.top })
        assertTrue("later paragraphs move down", layout.lines.last().top > lastTop)
        assertEquals(2, layout.hit(5f, layout.lines.last().top + 5)?.paragraphId)
    }

    /// On a justified line the words' frames follow the stretched spaces:
    /// the last word ends at the column's edge, and no two words overlap.
    @Test
    fun framesFollowJustification() {
        val layout = ChapterTextLayout()
        val text = List(60) { listOf("uno", "dos", "tres", "cuatro", "cinco")[it % 5] }.joinToString(" ")
        val p = paragraph(0, text)
        layout.setContent(listOf(0), mapOf(0 to p))
        layout.layout(400f, 2000f)
        val lines = layout.lines
        assertTrue(lines.size > 3)
        for ((n, line) in lines.dropLast(1).withIndex()) {
            val next = lines[n + 1].start
            val onLine = p.words.indices.filter { p.words[it].start >= line.start && p.words[it].start < next }
            val frames = onLine.map { layout.wordFrames(0, it).single() }
            assertEquals("line $n starts at the column's edge", 0f, frames.first().left, 0.5f)
            assertEquals("line $n ends at the column's edge", 400f, frames.last().right, 1.5f)
            for (i in 1 until frames.size) assertTrue(frames[i].left > frames[i - 1].right)
            for ((i, w) in onLine.withIndex()) {
                assertEquals(w, layout.hit(frames[i].centerX(), frames[i].centerY())?.wordIndex)
            }
        }
    }

    /// A line separator inside a paragraph breaks the line and keeps offsets.
    @Test
    fun lineSeparatorsBreakLines() {
        val layout = ChapterTextLayout()
        layout.setContent(listOf(0), mapOf(0 to paragraph(0, "uno dos")))
        layout.layout(600f, 500f)
        assertEquals(listOf(0, 4), layout.lines.map { it.start })
    }

    @Test
    fun rtlWordFramesFollowVisualOrderAndHitTest() {
        val layout = ChapterTextLayout()
        val rtl = paragraph(0, "مرحبا بكم جميعا")
        layout.setContent(listOf(0), mapOf(0 to rtl))
        layout.layout(400f, 500f)
        val first = layout.wordFrames(0, 0).single()
        val second = layout.wordFrames(0, 1).single()
        assertTrue("first RTL word should sit to the right of the second", first.left >= second.right - 0.5f)
        assertEquals(0, layout.hit(first.centerX(), first.centerY())?.wordIndex)
        assertEquals(1, layout.hit(second.centerX(), second.centerY())?.wordIndex)
    }
}

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w1100dp-h760dp-mdpi")
class ReaderViewTests {
    private val vocab = "uno dos tres cuatro cinco seis siete ocho nueve diez".split(" ")

    private fun chapter(): Map<Int, ParagraphText> = (0 until 120).associateWith { id ->
        val n = 15 + (id * 17) % 140
        ParagraphText(id, (0 until n).joinToString(" ") { vocab[(it + id) % vocab.size] }, emptyList(), emptyList(), true)
    }

    private fun idle() = shadowOf(Looper.getMainLooper()).idle()

    private fun open(
        anchor: ReadingAnchor?, paragraphs: Map<Int, ParagraphText>, width: Int = 1100, height: Int = 760,
        inline: Boolean = true,
    ): ReaderView {
        val view = ReaderView(ApplicationProvider.getApplicationContext())
        if (inline) {
            view.layoutExecutor = java.util.concurrent.Executor { it.run() }
            view.fullLayoutDelayMs = 0
        }
        view.setContent(paragraphs.keys.sorted(), paragraphs, anchor)
        view.layout(0, 0, width, height)
        idle()
        return view
    }

    @Test
    fun twoPagesWhenWideOneWhenNarrow() {
        assertEquals(2, open(null, chapter()).columns)
        assertEquals(1, open(null, chapter(), width = 400, height = 760).columns)
    }

    /// Reopened on an odd page, paging back to the start never shows a page twice:
    /// the first spread has a blank left page instead.
    @Test
    fun noPageIsShownInTwoSpreads() {
        val paragraphs = chapter()
        var view = open(null, paragraphs)
        assertEquals(2, view.columns)
        // A position from another device (a paragraph, no line) can land on any page.
        for (pid in 5 until 110) {
            val reopened = open(ReadingAnchor(pid, 0), paragraphs)
            if (reopened.spreadStart % 2 == 1) {
                view = reopened
                break
            }
        }
        val seen = HashSet<Int>()
        for (step in 0 until view.pageCount) {
            val shown = (view.spreadStart until view.spreadStart + view.columns).filter { it >= 0 }
            assertTrue("pages $shown shown again", shown.none { it in seen })
            seen.addAll(shown)
            val before = view.spreadStart
            view.previousSpread()
            if (view.spreadStart == before) break
        }
        assertTrue("paging back did not reach the first page", 0 in seen)
        // The pairing must really have been odd, or this checked nothing.
        assertEquals("no saved position reopened on an odd page", -1, view.spreadStart)
    }

    /// Saving and reopening must land on the same top line, from any page.
    @Test
    fun savedPositionReopensInPlace() {
        val paragraphs = chapter()
        val first = open(null, paragraphs)
        assertFalse(first.isProvisional)
        for (page in 0 until first.pageCount step 7) {
            first.go(page, animated = false)
            val saved = first.currentAnchor() ?: continue
            val top = first.text.lineAnchor(first.spreadStart) ?: continue
            val reopened = open(saved, paragraphs)
            assertEquals("page $page", top, reopened.text.lineAnchor(reopened.spreadStart))
            // Another device's position (no line) lands in the right paragraph.
            val synced = open(saved.copy(line = null), paragraphs)
            assertEquals("page $page", saved.paragraphId, synced.currentAnchor()?.paragraphId)
        }
    }

    /// Rotations and resizes relayout many times; the top line must not walk.
    @Test
    fun positionSurvivesResizes() {
        val view = open(null, chapter(), width = 1280, height = 800, inline = false)
        fun settle() {
            repeat(200) {
                if (!view.isProvisional) return
                Thread.sleep(20)
                shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(20))
            }
        }
        settle()
        assertFalse(view.isProvisional)
        view.go(23, animated = false)
        val top = view.text.lineAnchor(view.spreadStart)
        val startedBefore = view.fullLayoutsStarted
        for (w in (1280 downTo 900 step 20) + (900..1280 step 20)) view.layout(0, 0, w, 800)
        settle()
        assertEquals("after a resize round trip", top, view.text.lineAnchor(view.spreadStart))
        // A phone turned on its side and back.
        repeat(5) {
            view.layout(0, 0, 800, 1280)
            view.layout(0, 0, 1280, 800)
        }
        settle()
        assertEquals("after rotations", top, view.text.lineAnchor(view.spreadStart))
        assertTrue("full layouts should wait for relayouts to settle", view.fullLayoutsStarted - startedBefore <= 3)
    }

    /// A steady stream of updates (a chapter translating) during the
    /// provisional layout must not keep the full layout from landing, and
    /// every update must end up in the text.
    @Test
    fun fullLayoutLandsUnderAStreamOfUpdates() {
        val paragraphs = (0 until 200).associateWith { id ->
            ParagraphText(id, List(30 + id % 50) { "palabra" }.joinToString(" "), emptyList(), emptyList(), false)
        }
        val view = open(ReadingAnchor(150, 0), paragraphs, width = 1280, height = 800, inline = false)
        assertTrue(view.isProvisional)
        var landedDuringStream = false
        for (round in 0 until 40) {
            val id = round * 5
            view.replace(listOf(paragraphs[id]!!.copy(text = paragraphs[id]!!.text + " fin", translated = true)))
            Thread.sleep(20)
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(20))
            if (!view.isProvisional) landedDuringStream = true
        }
        assertTrue("full layout never landed while updates streamed", landedDuringStream)
        repeat(100) {
            Thread.sleep(10)
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(20))
        }
        assertFalse(view.isProvisional)
        for (round in 0 until 40) {
            val p = view.text.paragraphs[round * 5]
            assertTrue("paragraph ${round * 5} not applied", p?.translated == true && p.text.endsWith(" fin"))
        }
        // The reader stayed where it was opened.
        assertEquals(150, view.currentAnchor()?.paragraphId)
    }

    private class Recorder : ReaderListener {
        var selected: Pair<Int, Int>? = null
        var translated: Pair<Int, Boolean>? = null
        var deselections = 0
        override fun selected(paragraphId: Int, wordIndex: Int) {
            selected = paragraphId to wordIndex
        }
        override fun translate(paragraphId: Int, useCache: Boolean) {
            translated = paragraphId to useCache
        }
        override fun movedTo(page: Int, count: Int) {}
        override fun translationProgress(paragraphId: Int): Double? = null
        override fun toggle(key: Char): Boolean = false
        override fun canTranslate(): Boolean = true
        override fun deselected() {
            deselections += 1
        }
    }

    /// Long words so lines wrap (and hyphenate where the platform can); every
    /// piece of a split word must hit it.
    private fun gateChapter(): Map<Int, ParagraphText> {
        val words = "desafortunadamente extraordinariamente gato incomprensiblemente la responsabilidades y sol internacionalización".split(" ")
        return (0 until 80).associateWith { id ->
            paragraph(id, (0 until 25 + id % 40).joinToString(" ") { words[(id * 3 + it) % words.size] } + ".")
        }
    }

    private fun check(view: ReaderView, rng: Random, misses: MutableList<String>): Pair<Int, Int> {
        var taps = 0
        var split = 0
        for (page in maxOf(view.spreadStart, 0) until minOf(view.spreadStart + view.columns, view.pageCount)) {
            val band = view.text.pages[page]
            val candidates = view.text.paragraphs.flatMap { (pid, p) ->
                p.words.indices.mapNotNull { i ->
                    val frames = view.text.wordFrames(pid, i).filter { it.top >= band.top - 0.5f && it.bottom <= band.bottom + 0.5f }
                    if (frames.isEmpty()) null else Triple(pid, i, frames)
                }
            }
            if (candidates.isEmpty()) continue
            repeat(12) {
                val (pid, i, frames) = candidates[rng.nextInt(candidates.size)]
                for (f in frames) {
                    val (x, y) = view.viewPoint(f.centerX(), f.bottom - view.text.typography.fontSize * 0.6f, page)!!
                    taps += 1
                    if (frames.size > 1) split += 1
                    val hit = view.hit(x, y)
                    if (hit?.paragraphId != pid || hit.wordIndex != i) misses.add("p$pid w$i got $hit")
                }
            }
        }
        return taps to split
    }

    @Test
    fun randomTapsHitTheirWord() {
        val paragraphs = gateChapter()
        val rng = Random(7)
        val misses = ArrayList<String>()
        var taps = 0
        var split = 0
        // Provisional layout right after opening deep, then the full one.
        val view = open(ReadingAnchor(50, 0), paragraphs, inline = false)
        assertTrue(view.isProvisional)
        check(view, rng, misses).let { taps += it.first; split += it.second }
        repeat(200) {
            if (view.isProvisional) {
                Thread.sleep(20)
                shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(20))
            }
        }
        assertFalse(view.isProvisional)
        for (page in 0 until view.pageCount step maxOf(view.pageCount / 8, 1)) {
            view.go(page, animated = false)
            check(view, rng, misses).let { taps += it.first; split += it.second }
        }
        println("HIT-GATE taps $taps, split-word taps $split, misses ${misses.size}")
        assertTrue(taps >= 100)
        assertTrue(misses.take(5).toString(), misses.isEmpty())
    }

    /// A tap reveals and selects a word; a tap on empty space deselects; the
    /// gutter button of an untranslated paragraph asks for its translation.
    @Test
    fun tapsSelectWordsAndPressParagraphButtons() {
        val paragraphs = (0 until 6).associateWith { paragraph(it, List(12) { "hola mundo" }.joinToString(" "), translated = it != 2) }
        val view = open(null, paragraphs)
        val recorder = Recorder()
        view.listener = recorder
        val frame = view.text.wordFrames(1, 3).first()
        val (x, y) = view.viewPoint(frame.centerX(), frame.centerY(), 0)!!
        view.selectWord(x, y)
        assertEquals(1 to 3, recorder.selected)
        assertEquals(ReaderView.WordKey(1, 3), view.selected)
        assertTrue(ReaderView.WordKey(1, 3) in view.revealed)
        view.selectWord(x, view.height - 2f)
        assertEquals(1, recorder.deselections)
        assertEquals(null, view.selected)

        val controls = view.visibleControlRects()
        assertEquals(listOf(2), controls.map { it.first })
        val rect = controls.single().second
        val down = android.os.SystemClock.uptimeMillis()
        for (action in listOf(android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP)) {
            val event = android.view.MotionEvent.obtain(down, down + 10, action, rect.centerX(), rect.centerY(), 0)
            view.dispatchTouchEvent(event)
            event.recycle()
        }
        assertEquals(2 to true, recorder.translated)
        assertNotNull(view.pageLabel)
    }

    @Test
    fun leftMarginTapTurnsBackWithoutConflictingWithTranslateButton() {
        fun tap(view: ReaderView, x: Float, y: Float) {
            val down = android.os.SystemClock.uptimeMillis()
            for (action in listOf(android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP)) {
                val event = android.view.MotionEvent.obtain(down, down + 10, action, x, y, 0)
                view.dispatchTouchEvent(event)
                event.recycle()
            }
        }

        // Phone-sized viewport: tapping the left margin turns back when clear of words/controls.
        val translatedParagraphs = (0 until 30).associateWith {
            paragraph(it, List(25) { "hola mundo" }.joinToString(" "), translated = true)
        }
        val phone = open(null, translatedParagraphs, width = 400, height = 760)
        phone.go(1, animated = false)
        assertEquals(1, phone.spreadStart)
        tap(phone, 6f, phone.height - 8f)
        assertEquals(0, phone.spreadStart)

        // With an untranslated paragraph button on page 1, tapping it or just beside it must not turn back.
        val mixed = (0 until 30).associateWith {
            paragraph(it, List(25) { "hola mundo" }.joinToString(" "), translated = false)
        }
        val withControls = open(null, mixed, width = 400, height = 760)
        val recorder = Recorder()
        withControls.listener = recorder
        withControls.go(1, animated = false)
        val controlRect = withControls.visibleControlRects().first().second
        val nearMissY = controlRect.bottom + 15f
        tap(withControls, 6f, nearMissY)
        assertEquals(1, withControls.spreadStart)
        assertEquals(null, recorder.translated)

        tap(withControls, controlRect.centerX(), controlRect.centerY())
        assertEquals(1, withControls.spreadStart)
        assertEquals(withControls.visibleControlRects().first().first to true, recorder.translated)
    }

    @Test
    fun detachingCancelsPendingFullLayout() {
        val activity = org.robolectric.Robolectric.buildActivity(android.app.Activity::class.java).setup().get()
        val container = android.widget.FrameLayout(activity)
        activity.setContentView(container)
        val view = ReaderView(activity)
        var fullLayoutCallbacks = 0
        view.onFullLayout = { fullLayoutCallbacks += 1 }
        view.setContent(chapter().keys.sorted(), chapter(), ReadingAnchor(80, 0))
        container.addView(view, 1100, 760)
        view.layout(0, 0, 1100, 760)
        assertTrue(view.isProvisional)
        assertEquals(0, view.fullLayoutsStarted)

        container.removeView(view)
        Thread.sleep(200)
        shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(300))

        assertEquals(0, view.fullLayoutsStarted)
        assertEquals(0, fullLayoutCallbacks)
    }

    @Test
    fun drainChangesPreservesOddSpreadPairing() {
        val paragraphs = chapter()
        var targetView: ReaderView? = null
        for (pid in 5 until 110) {
            val candidate = open(ReadingAnchor(pid, 0), paragraphs)
            if (candidate.spreadStart % 2 == 1) {
                targetView = candidate
                break
            }
        }
        val view = targetView!!
        view.go(0, animated = false)
        assertEquals(-1, view.spreadStart)
        var reportedPage: Int? = null
        view.listener = object : ReaderListener {
            override fun selected(paragraphId: Int, wordIndex: Int) {}
            override fun translate(paragraphId: Int, useCache: Boolean) {}
            override fun movedTo(page: Int, count: Int) {
                reportedPage = page
            }
            override fun translationProgress(paragraphId: Int): Double? = null
            override fun toggle(key: Char): Boolean = false
            override fun canTranslate(): Boolean = true
            override fun deselected() {}
        }
        view.replace(listOf(paragraphs[0]!!.copy(text = paragraphs[0]!!.text + " fin")))
        idle()
        assertEquals(-1, view.spreadStart)
        assertEquals(0, reportedPage)
    }
}
