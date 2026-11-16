package com.TS.FLTS.reader

import android.graphics.Canvas
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.text.LineBreaker
import android.text.Layout
import android.text.SpannableString
import android.text.Spanned
import android.text.StaticLayout
import android.text.TextPaint
import android.text.style.LineHeightSpan
import android.text.style.StyleSpan
import java.util.Locale
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import uniffi.flts_ffi.ParagraphText

/// Sizes are in pixels; the view converts from sp and dp.
data class ReaderTypography(
    val fontSize: Float = 18f,
    /// Pixels per dp, for the few fixed distances (hit slop).
    val density: Float = 1f,
    /// The text's language as the core names it; hyphenation follows it.
    val language: String? = null,
    val lineHeightMultiple: Float = 1.75f,
    val paragraphSpacingEm: Float = 0.6f,
    val hyphenate: Boolean = true,
    val justify: Boolean = true,
) {
    val lineHeight: Int get() = (fontSize * lineHeightMultiple).roundToInt()
    val paragraphSpacing: Int get() = (fontSize * paragraphSpacingEm).roundToInt()
    val translationFontSize: Float get() = floor(fontSize * 0.55f)

    val locale: Locale? get() = language?.takeIf { it.isNotEmpty() }?.let(::localeFor)

    companion object {
        private val iso3 by lazy {
            Locale.getISOLanguages().associateBy { runCatching { Locale(it).isO3Language }.getOrDefault(it) }
        }

        /// The core names languages by ISO 639-3; the platform's hyphenators
        /// go by the two-letter code where there is one.
        fun localeFor(language: String): Locale {
            val tag = language.replace('_', '-')
            val primary = tag.substringBefore('-')
            val short = if (primary.length == 3) iso3[primary.lowercase()] else null
            return Locale.forLanguageTag(if (short != null) short + tag.removePrefix(primary) else tag)
        }
    }
}

data class ReaderColors(
    val background: Int = 0xFFFFFFFF.toInt(),
    val text: Int = 0xFF1B1B1F.toInt(),
    val secondary: Int = 0xFF6B6B74.toInt(),
    val tertiary: Int = 0xFFB0B0B8.toInt(),
    val accent: Int = 0xFF3A5BD9.toInt(),
)

/// A chapter's text as one tall column: a `StaticLayout` per paragraph,
/// stacked. Pages are bands of it cut at line boundaries, so nothing is laid
/// out per page, and a changed paragraph costs only its own layout. Used by
/// one thread at a time: a full layout is built off the main thread, then
/// handed over.
class ChapterTextLayout(val typography: ReaderTypography = ReaderTypography(), val colors: ReaderColors = ReaderColors()) {
    var order: List<Int> = emptyList()
        private set
    var paragraphs: Map<Int, ParagraphText> = emptyMap()
        private set
    var columnWidth = 400
        private set
    var map = PageMap(emptyList(), emptyList(), 600f)
        private set

    private val blocks = HashMap<Int, StaticLayout>()
    private var tops = FloatArray(0)
    private val paint = textPaint(colors.text)
    private val untranslatedPaint = textPaint(colors.secondary)

    val pages: List<ReaderPage> get() = map.pages
    val lines: List<LaidLine> get() = map.lines
    val pageHeight: Float get() = map.pageHeight

    private fun textPaint(color: Int) = TextPaint(TextPaint.ANTI_ALIAS_FLAG).apply {
        textSize = typography.fontSize
        typeface = Typeface.SERIF
        this.color = color
        typography.locale?.let { textLocale = it }
    }

    // Content

    fun setContent(order: List<Int>, paragraphs: Map<Int, ParagraphText>) {
        this.order = order
        this.paragraphs = paragraphs
        blocks.clear()
    }

    /// Replaces changed paragraphs; only their layout is redone by the next `layout`.
    fun replace(changed: List<ParagraphText>) {
        if (changed.isEmpty()) return
        val next = paragraphs.toMutableMap()
        for (p in changed) {
            if (p.id !in next) continue
            next[p.id] = p
            blocks.remove(p.id)
        }
        paragraphs = next
    }

    private fun build(p: ParagraphText): StaticLayout {
        // A line separator inside a paragraph breaks the line; same length, so offsets hold.
        val text = SpannableString(p.text.replace(' ', '\n'))
        val length = text.length
        text.setSpan(LineHeightSpan.Standard(typography.lineHeight), 0, length, Spanned.SPAN_INCLUSIVE_INCLUSIVE)
        for (run in p.runs) {
            if (run.marks and 3 == 0 || run.start + run.len > length) continue
            val style = when (run.marks and 3) {
                1 -> Typeface.ITALIC
                2 -> Typeface.BOLD
                else -> Typeface.BOLD_ITALIC
            }
            text.setSpan(StyleSpan(style), run.start, run.start + run.len, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        }
        return StaticLayout.Builder.obtain(text, 0, length, if (p.translated) paint else untranslatedPaint, columnWidth)
            .setAlignment(Layout.Alignment.ALIGN_NORMAL)
            .setIncludePad(false)
            .setBreakStrategy(LineBreaker.BREAK_STRATEGY_HIGH_QUALITY)
            .setHyphenationFrequency(
                if (typography.hyphenate) Layout.HYPHENATION_FREQUENCY_NORMAL else Layout.HYPHENATION_FREQUENCY_NONE)
            .setJustificationMode(
                if (typography.justify) LineBreaker.JUSTIFICATION_MODE_INTER_WORD else LineBreaker.JUSTIFICATION_MODE_NONE)
            .build()
    }

    // Layout

    /// Lays the chapter out at `width` and cuts it into pages; a page starts
    /// exactly at `breakAt` when given, so a swap keeps the view still.
    fun layout(width: Float, pageHeight: Float, breakAt: LineAnchor? = null) {
        val w = max(floor(width).toInt(), 120)
        if (w != columnWidth) {
            columnWidth = w
            blocks.clear()
        }
        val collected = ArrayList<LaidLine>(order.size * 8)
        tops = FloatArray(order.size) { Float.NaN }
        var y = 0f
        for ((index, id) in order.withIndex()) {
            val p = paragraphs[id] ?: continue
            if (p.text.isEmpty()) continue
            val block = blocks.getOrPut(id) { build(p) }
            tops[index] = y
            for (line in 0 until block.lineCount) {
                collected.add(
                    LaidLine(
                        y + block.getLineTop(line), y + block.getLineBottom(line), y + block.getLineBaseline(line),
                        index, block.getLineStart(line)))
            }
            y += block.height + typography.paragraphSpacing
        }
        map = PageMap(order, collected, max(pageHeight, typography.lineHeight * 2f), breakAt)
    }

    fun paginate(breakAt: LineAnchor?) {
        map = PageMap(order, map.lines, map.pageHeight, breakAt)
    }

    // Geometry

    private fun block(index: Int): StaticLayout? =
        if (index in tops.indices && !tops[index].isNaN()) blocks[order[index]] else null

    /// The paragraph whose block holds `y`, as an index into `order`.
    private fun blockIndex(y: Float): Int? {
        for (index in tops.indices) {
            val top = tops[index]
            if (top.isNaN()) continue
            if (y < top) return null
            val block = blocks[order[index]] ?: continue
            if (y < top + block.height) return index
        }
        return null
    }

    /// Frames of a paragraph's character range in layout coordinates, one per line piece.
    fun frames(paragraphId: Int, start: Int, end: Int): List<RectF> {
        val index = order.indexOf(paragraphId)
        val block = block(index) ?: return emptyList()
        val top = tops[index]
        val length = block.text.length
        val from = start.coerceIn(0, length)
        val to = end.coerceIn(from, length)
        if (to == from) return emptyList()
        val out = ArrayList<RectF>(2)
        for (line in block.getLineForOffset(from)..block.getLineForOffset(to - 1)) {
            val lineStart = block.getLineStart(line)
            val lineEnd = block.getLineEnd(line)
            val a = block.getPrimaryHorizontal(max(from, lineStart))
            // The end of the line belongs to the next one for `getPrimaryHorizontal`.
            val b = if (to < lineEnd) block.getPrimaryHorizontal(to) else block.getLineRight(line)
            if (b == a) continue
            out.add(RectF(min(a, b), top + block.getLineTop(line), max(a, b), top + block.getLineBottom(line)))
        }
        return out
    }

    /// The baseline of the line holding a paragraph offset.
    fun baseline(paragraphId: Int, offset: Int): Float? {
        val index = order.indexOf(paragraphId)
        val block = block(index) ?: return null
        val line = block.getLineForOffset(offset.coerceIn(0, max(block.text.length - 1, 0)))
        return tops[index] + block.getLineBaseline(line)
    }

    data class Hit(val paragraphId: Int, val wordIndex: Int?)

    fun hit(x: Float, y: Float): Hit? {
        val index = blockIndex(y) ?: return null
        val block = blocks[order[index]] ?: return null
        val pid = order[index]
        val p = paragraphs[pid] ?: return null
        val line = block.getLineForVertical((y - tops[index]).toInt())
        // The nearest caret position: the word is on one side of it or the other.
        val caret = block.getOffsetForHorizontal(line, x)
        val slopX = 2 * typography.density
        val slopY = 4 * typography.density
        for (candidate in intArrayOf(caret, caret - 1)) {
            if (candidate < 0 || candidate >= p.text.length) continue
            val w = p.words.indexOfFirst { candidate >= it.start && candidate < it.start + it.len }
            if (w < 0) continue
            val word = p.words[w]
            val inside = frames(pid, word.start, word.start + word.len).any {
                x >= it.left - slopX && x <= it.right + slopX && y >= it.top - slopY && y <= it.bottom + slopY
            }
            if (inside) return Hit(pid, w)
        }
        return Hit(pid, null)
    }

    fun wordFrames(paragraphId: Int, wordIndex: Int): List<RectF> {
        val word = paragraphs[paragraphId]?.words?.getOrNull(wordIndex) ?: return emptyList()
        return frames(paragraphId, word.start, word.start + word.len)
    }

    /// Draws the lines of a page with the column's left edge at `x` and layout
    /// coordinate `page.top` at `y`. The caller clips to the page.
    fun draw(canvas: Canvas, page: ReaderPage, x: Float, y: Float) {
        for (index in tops.indices) {
            val top = tops[index]
            if (top.isNaN()) continue
            if (top >= page.bottom) break
            val block = blocks[order[index]] ?: continue
            if (top + block.height <= page.top) continue
            canvas.save()
            canvas.translate(x, y + top - page.top)
            block.draw(canvas)
            canvas.restore()
        }
    }

    // Reading position

    fun lineAnchor(page: Int): LineAnchor? = map.lineAnchor(page)
    fun page(anchor: LineAnchor): Int? = map.page(anchor)
    fun anchor(page: Int): ReadingAnchor? = map.anchor(page)
    fun page(paragraphId: Int, pageOffset: Int): Int? = map.page(paragraphId, pageOffset)
}
