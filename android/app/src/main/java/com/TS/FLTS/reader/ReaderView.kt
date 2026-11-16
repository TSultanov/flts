package com.TS.FLTS.reader

import android.animation.ValueAnimator
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.os.Handler
import android.os.Looper
import android.view.ContextMenu
import android.view.GestureDetector
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.animation.DecelerateInterpolator
import androidx.core.content.ContextCompat
import com.TS.FLTS.R
import com.TS.FLTS.core.AppPrefs
import java.util.concurrent.Executor
import java.util.concurrent.Executors
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import uniffi.flts_ffi.ParagraphText

interface ReaderListener {
    fun selected(paragraphId: Int, wordIndex: Int)
    fun translate(paragraphId: Int, useCache: Boolean)
    fun movedTo(page: Int, count: Int)

    /// 0…1 while the paragraph is being translated, else null.
    fun translationProgress(paragraphId: Int): Double?

    /// A plain key the reader does not use itself; true if handled.
    fun toggle(key: Char): Boolean

    /// False until the chapter's translation context (summaries) exists.
    fun canTranslate(): Boolean
    fun deselected()
}

/// Draws a spread of pages from one `ChapterTextLayout` and turns taps, keys
/// and swipes into reader actions. Flipping only changes which bands are
/// drawn; text is never re-laid out for it.
class ReaderView(context: Context) : View(context) {
    data class WordKey(val paragraphId: Int, val wordIndex: Int)

    /// What is drawn: the full chapter, or while that is laid out off the
    /// main thread, a provisional layout starting at the reading position.
    var text = ChapterTextLayout()
        private set
    var isProvisional = false
        private set
    var order: List<Int> = emptyList()
        private set
    private var paragraphs: Map<Int, ParagraphText> = emptyMap()
    var typography = ReaderTypography()
        private set
    var colors = ReaderColors()
        private set
    private var generation = 0
    var listener: ReaderListener? = null

    /// Set when the full layout lands, for tests and timing.
    var onFullLayout: (() -> Unit)? = null

    /// Set when queued changes are all applied, for tests and timing.
    var onChangesApplied: (() -> Unit)? = null

    /// Where full layouts run; tests run them inline.
    var layoutExecutor: Executor = backgroundLayouts
    var fullLayoutDelayMs = 120L

    /// The page in the first column. Spreads keep the pairing the chapter
    /// opened with, so no page is ever shown in two of them; when that pairing
    /// is odd, the first spread starts one page early, with a blank left page.
    var spreadStart = 0
        private set
    val firstVisiblePage: Int get() = max(spreadStart, 0)
    var columns = 1
        private set
    var columnWidth = 400f
        private set
    private var laidOutWidth = 0
    private var laidOutHeight = 0
    private var pendingAnchor: ReadingAnchor? = null

    /// Where the reader is, as last set by the reader (opening, flipping).
    /// Relayouts restore to it rather than to whatever line they snapped to,
    /// so resizes cannot walk the position.
    private var sticky: ReadingAnchor? = null

    /// How far the spread is dragged sideways, in pixels.
    private var slide = 0f
        set(value) {
            if (value != field) {
                field = value
                invalidate()
            }
        }

    /// The spread drawn beside the current one while it slides.
    private var incoming: Int? = null
    private var slideAnimator: ValueAnimator? = null

    var selected: WordKey? = null
        set(value) {
            field = value
            invalidate()
        }

    /// Words whose translation is shown above them.
    var revealed: Set<WordKey> = emptySet()
        set(value) {
            field = value
            invalidate()
        }

    private val density = resources.displayMetrics.density
    private fun dp(value: Float) = value * density

    private var marginTop = dp(40f)
    private var marginLeft = dp(56f)
    private var marginBottom = dp(44f)
    private var marginRight = dp(56f)
    private val columnGap = dp(64f)
    private val controlSize = dp(22f)

    /// A finger needs about 44 dp; the button is drawn 22 dp wide.
    private val controlSlop = dp(11f)

    private val main = Handler(Looper.getMainLooper())
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val label = Paint(Paint.ANTI_ALIAS_FLAG)
    private val translateIcon: Drawable? = ContextCompat.getDrawable(context, R.drawable.ic_translate)?.mutate()
    private var menuHit: ChapterTextLayout.Hit? = null

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        // The reader draws its own focus state (none).
        defaultFocusHighlightEnabled = false
        setOnCreateContextMenuListener { menu, _, _ -> buildMenu(menu) }
    }

    // Content

    fun setContent(order: List<Int>, paragraphs: Map<Int, ParagraphText>, anchor: ReadingAnchor?) {
        this.order = order
        this.paragraphs = paragraphs
        pendingChanges.clear()
        pendingAnchor = anchor
        sticky = anchor
        laidOutWidth = 0
        laidOutHeight = 0
        sizeChanged()
        invalidate()
    }

    /// Changed paragraphs not yet applied; applied a few per frame so a burst
    /// (sync, a finished chapter) never blocks a frame for long.
    private val pendingChanges = sortedMapOf<Int, ParagraphText>()
    private var drainScheduled = false

    fun replace(changed: List<ParagraphText>) {
        val next = paragraphs.toMutableMap()
        for (p in changed) {
            next[p.id] = p
            pendingChanges[p.id] = p
        }
        paragraphs = next
        // During the provisional layout they wait for the full one (restarting
        // it on every change could keep it from ever landing).
        if (!isProvisional) scheduleDrain()
    }

    private fun scheduleDrain() {
        if (drainScheduled) return
        drainScheduled = true
        main.post(::drainChanges)
    }

    private fun drainChanges() {
        drainScheduled = false
        if (isProvisional) return
        val batch = pendingChanges.keys.take(CHANGES_PER_TURN).mapNotNull { pendingChanges.remove(it) }
        if (batch.isNotEmpty()) {
            val keep = text.lineAnchor(firstVisiblePage)
            text.replace(batch)
            text.layout(columnWidth, text.pageHeight, keep)
            spreadStart = keep?.let { text.page(it) } ?: min(spreadStart, text.pages.size - 1)
            invalidate()
            listener?.movedTo(spreadStart, text.pages.size)
        }
        if (pendingChanges.isEmpty()) onChangesApplied?.invoke() else scheduleDrain()
    }

    fun setStyle(typography: ReaderTypography, colors: ReaderColors) {
        if (typography == this.typography && colors == this.colors) return
        this.typography = typography
        this.colors = colors
        relayout(sticky ?: currentAnchor())
    }

    fun currentAnchor(): ReadingAnchor? {
        if (laidOutWidth == 0 || order.isEmpty()) return pendingAnchor
        val a = text.anchor(firstVisiblePage) ?: return null
        return a.copy(line = text.lineAnchor(firstVisiblePage))
    }

    val pageCount: Int get() = text.pages.size

    /// The paragraph as the reader has it (including changes not yet laid out).
    fun contentParagraph(id: Int): ParagraphText? = paragraphs[id]

    // Layout

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        sizeChanged()
    }

    private fun sizeChanged() {
        // A phone has no room for wide margins; the left one still holds the
        // paragraph buttons. A phone in landscape is wide but short: it keeps
        // the tight margins.
        val tight = width < dp(600f) || height < dp(500f)
        marginTop = dp(if (tight) 20f else 40f)
        marginLeft = dp(if (tight) 40f else 56f)
        marginBottom = dp(if (tight) 34f else 44f)
        marginRight = dp(if (tight) 18f else 56f)
        if (width != laidOutWidth || height != laidOutHeight) {
            relayout(if (laidOutWidth == 0) pendingAnchor else (sticky ?: currentAnchor()))
        }
    }

    fun relayout(anchor: ReadingAnchor?) {
        if (width <= 0 || height <= 0 || order.isEmpty()) return
        laidOutWidth = width
        laidOutHeight = height
        val fontSize = typography.fontSize
        val available = width - marginLeft - marginRight
        val minColumn = fontSize * 24
        columns = if (available >= minColumn * 2 + columnGap) 2 else 1
        val maxColumn = fontSize * 40
        columnWidth = min((available - columnGap * (columns - 1)) / columns, maxColumn)
        val pageHeight = height - marginTop - marginBottom
        pendingAnchor = null

        // Provisional: the anchor paragraph onward, just enough to fill the view.
        val anchorId = anchor?.paragraphId ?: order.first()
        val from = max(order.indexOf(anchorId), 0)
        val charsPerPage = ((columnWidth / (fontSize * 0.5f)) * (pageHeight / typography.lineHeight)).toInt()
        val lead = anchor?.line?.let { it.start / max(charsPerPage, 1) + 1 } ?: (anchor?.pageOffset ?: 0)
        val wanted = charsPerPage * (lead + columns + 1)
        val subset = ArrayList<Int>()
        var chars = 0
        for (id in order.subList(from, order.size)) {
            subset.add(id)
            chars += paragraphs[id]?.text?.length ?: 0
            if (chars > wanted) break
        }
        val provisional = ChapterTextLayout(typography, colors)
        provisional.setContent(subset, paragraphs)
        provisional.layout(columnWidth, pageHeight, anchor?.line)
        text = provisional
        isProvisional = subset.size < order.size || from > 0
        spreadStart = anchor?.line?.let { provisional.page(it) }
            ?: anchor?.let { provisional.page(it.paragraphId, it.pageOffset) }
            ?: 0
        // What the provisional layout was built from is laid out already.
        if (!isProvisional) pendingChanges.clear()
        invalidate()
        listener?.movedTo(spreadStart, text.pages.size)

        generation += 1
        val gen = generation
        if (!isProvisional) {
            onFullLayout?.invoke()
            return
        }
        // Wait for relayouts to settle (rotation, window resizes) so one full
        // layout runs, not one per frame.
        if (fullLayoutDelayMs <= 0) {
            startFullLayout(gen, pageHeight)
        } else {
            main.postDelayed({ if (gen == generation) startFullLayout(gen, pageHeight) }, fullLayoutDelayMs)
        }
    }

    /// Full layouts started, for tests.
    var fullLayoutsStarted = 0
        private set

    private fun startFullLayout(gen: Int, pageHeight: Float) {
        fullLayoutsStarted += 1
        val order = order
        val paragraphs = paragraphs
        val typography = typography
        val colors = colors
        val width = columnWidth
        // Changes queued so far are in `paragraphs`, so the full layout has them.
        pendingChanges.clear()
        layoutExecutor.execute {
            val full = ChapterTextLayout(typography, colors)
            full.setContent(order, paragraphs)
            full.layout(width, pageHeight)
            if (Looper.myLooper() == Looper.getMainLooper()) adopt(full, gen) else main.post { adopt(full, gen) }
        }
    }

    /// Swaps in the full layout, re-cut so the visible line still starts the page.
    private fun adopt(full: ChapterTextLayout, gen: Int) {
        if (gen != generation) return
        val keep = text.lineAnchor(firstVisiblePage)
        full.paginate(keep)
        text = full
        isProvisional = false
        spreadStart = keep?.let { full.page(it) } ?: 0
        invalidate()
        listener?.movedTo(spreadStart, text.pages.size)
        // Changes that arrived after this layout started.
        if (pendingChanges.isNotEmpty()) scheduleDrain()
        onFullLayout?.invoke()
    }

    /// Left edge of a column in view coordinates (the spread is centered).
    fun columnX(column: Int): Float {
        val spreadWidth = columnWidth * columns + columnGap * (columns - 1)
        val left = max(marginLeft, (width - spreadWidth) / 2)
        return left + column * (columnWidth + columnGap)
    }

    // Drawing

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(colors.background)
        if (text.lines.isEmpty()) return
        canvas.save()
        canvas.translate(slide, 0f)
        drawSpread(canvas, spreadStart)
        // While it is dragged aside, the spread it uncovers shows beside it.
        val other = incoming
        if (slide != 0f && other != null) {
            canvas.translate(if (slide < 0) width.toFloat() else -width.toFloat(), 0f)
            drawSpread(canvas, other)
        }
        canvas.restore()
        drawFooter(canvas)
    }

    private fun drawSpread(canvas: Canvas, start: Int) {
        for (column in 0 until columns) {
            val index = start + column
            if (index < 0) continue
            if (index >= text.pages.size) break
            drawPage(canvas, text.pages[index], columnX(column), marginTop)
        }
    }

    private fun drawPage(canvas: Canvas, page: ReaderPage, x: Float, y: Float) {
        val dy = y - page.top
        canvas.save()
        canvas.clipRect(x - columnGap / 2, y, x + columnWidth + columnGap / 2, y + page.bottom - page.top)

        selected?.let { sel ->
            fill.color = (colors.accent and 0x00FFFFFF) or (0x38 shl 24)
            for (frame in text.wordFrames(sel.paragraphId, sel.wordIndex)) {
                val r = highlightRect(sel.paragraphId, sel.wordIndex, frame)
                r.offset(x, dy)
                r.inset(-dp(2f), 0f)
                canvas.drawRoundRect(r, dp(3f), dp(3f), fill)
            }
        }
        text.draw(canvas, page, x, y)
        drawTranslations(canvas, page, x, dy)
        drawParagraphControls(canvas, page, x, dy)
        canvas.restore()
    }

    /// The glyph box inside a line-height-tall word frame.
    private fun glyphRect(paragraphId: Int, wordIndex: Int, frame: RectF): RectF {
        val start = text.paragraphs[paragraphId]?.words?.getOrNull(wordIndex)?.start ?: 0
        // The frame's own line: a word split by a hyphen has a frame on each.
        val baseline = text.lines.firstOrNull { it.top == frame.top }?.baseline
            ?: text.baseline(paragraphId, start) ?: frame.bottom
        val fontSize = text.typography.fontSize
        return RectF(frame.left, baseline - fontSize, frame.right, baseline + fontSize * 0.3f)
    }

    /// The glyph box without the headroom above tall letters, where a word's
    /// translation is drawn: a highlight must not reach under it.
    private fun highlightRect(paragraphId: Int, wordIndex: Int, frame: RectF): RectF {
        val glyph = glyphRect(paragraphId, wordIndex, frame)
        glyph.top += (text.typography.fontSize * 0.2f).roundToInt()
        return glyph
    }

    private fun drawTranslations(canvas: Canvas, page: ReaderPage, x: Float, dy: Float) {
        if (revealed.isEmpty()) return
        val base = text.typography.translationFontSize
        label.typeface = Typeface.DEFAULT
        label.color = colors.accent
        label.textAlign = Paint.Align.CENTER
        for (key in revealed) {
            val translation = text.paragraphs[key.paragraphId]?.words?.getOrNull(key.wordIndex)?.translation
            if (translation.isNullOrEmpty()) continue
            val first = text.wordFrames(key.paragraphId, key.wordIndex).firstOrNull() ?: continue
            if (first.top < page.top - 1 || first.bottom > page.bottom + 1) continue
            val glyph = glyphRect(key.paragraphId, key.wordIndex, first)
            label.textSize = base
            val measured = label.measureText(translation)
            // Shrink to the word's width, as the web app does.
            if (measured > glyph.width() && glyph.width() > 0) label.textSize = base * glyph.width() / measured
            canvas.drawText(translation, glyph.centerX() + x, glyph.top + dp(2f) + dy - label.descent(), label)
        }
    }

    private fun drawParagraphControls(canvas: Canvas, page: ReaderPage, x: Float, dy: Float) {
        for ((pid, rect) in controlRects(page, x, dy)) {
            val progress = listener?.translationProgress(pid)
            if (progress != null) {
                drawProgress(canvas, progress, rect)
                continue
            }
            val icon = translateIcon ?: continue
            val enabled = listener?.canTranslate() ?: true
            icon.setTint(if (enabled) colors.secondary else (colors.tertiary and 0x00FFFFFF) or (0x59 shl 24))
            val inset = dp(2f).roundToInt()
            icon.setBounds(
                rect.left.roundToInt() + inset, rect.top.roundToInt() + inset, rect.right.roundToInt() - inset,
                rect.bottom.roundToInt() - inset)
            icon.draw(canvas)
        }
    }

    private fun drawProgress(canvas: Canvas, progress: Double, rect: RectF) {
        val ring = RectF(rect).apply { inset(dp(4f), dp(4f)) }
        stroke.strokeWidth = dp(2.5f)
        stroke.color = colors.tertiary
        canvas.drawOval(ring, stroke)
        stroke.color = colors.accent
        canvas.drawArc(ring, -90f, (360 * progress.coerceIn(0.03, 1.0)).toFloat(), false, stroke)
    }

    /// Gutter buttons for untranslated or translating paragraphs that start on this page.
    private fun controlRects(page: ReaderPage, x: Float, dy: Float): List<Pair<Int, RectF>> {
        val out = ArrayList<Pair<Int, RectF>>()
        for (line in text.lines) {
            if (line.top < page.top) continue
            if (line.top >= page.bottom) break
            if (line.paragraphIndex >= text.order.size) continue
            val pid = text.order[line.paragraphIndex]
            if (text.map.firstLineTop(pid) != line.top) continue
            val translating = listener?.translationProgress(pid) != null
            if (!translating && text.paragraphs[pid]?.translated != false) continue
            // Centred on the first line's lowercase letters.
            val y = line.baseline - text.typography.fontSize * 0.25f - controlSize / 2 + dy
            val left = x - controlSize - dp(10f)
            out.add(pid to RectF(left, y, left + controlSize, y + controlSize))
        }
        return out
    }

    private fun drawFooter(canvas: Canvas) {
        val count = text.pages.size
        val last = min(spreadStart + columns, count)
        val text = when {
            isProvisional -> "…"
            columns > 1 && last - firstVisiblePage > 1 -> "${firstVisiblePage + 1}–$last of $count"
            else -> "${firstVisiblePage + 1} of $count"
        }
        label.typeface = Typeface.DEFAULT
        label.textSize = dp(11f)
        label.color = colors.tertiary
        label.textAlign = Paint.Align.CENTER
        val y = height - marginBottom / 2 - (label.ascent() + label.descent()) / 2
        canvas.drawText(text, width / 2f, y, label)
    }

    // Hit testing

    /// A view point as a point of the layout, if it is on a visible page.
    private fun locate(x: Float, y: Float): Pair<Float, Float>? {
        for (column in 0 until columns) {
            val index = spreadStart + column
            if (index < 0) continue
            if (index >= text.pages.size) break
            val page = text.pages[index]
            val x0 = columnX(column)
            val localY = y - marginTop + page.top
            if (x >= x0 - columnGap / 2 && x <= x0 + columnWidth + columnGap / 2 && localY >= page.top && localY < page.bottom) {
                return (x - x0) to localY
            }
        }
        return null
    }

    private fun controlHit(x: Float, y: Float): Int? {
        for ((pid, rect) in visibleControlRects()) {
            if (x >= rect.left - controlSlop && x <= rect.right + controlSlop && y >= rect.top - controlSlop && y <= rect.bottom + controlSlop) {
                return pid
            }
        }
        return null
    }

    /// Where a point of a visible page's text sits in this view (for tests).
    fun viewPoint(x: Float, y: Float, page: Int): Pair<Float, Float>? {
        val column = page - spreadStart
        if (column !in 0 until columns || page >= text.pages.size) return null
        return (columnX(column) + x) to (marginTop + y - text.pages[page].top)
    }

    fun hit(x: Float, y: Float): ChapterTextLayout.Hit? {
        val (px, py) = locate(x, y) ?: return null
        if (px < 0 || px > columnWidth) return null
        return text.hit(px, py)
    }

    /// A press on a paragraph's translate button; false if the point is not on one.
    private fun activateControl(x: Float, y: Float): Boolean {
        val pid = controlHit(x, y) ?: return false
        if (listener?.translationProgress(pid) == null && listener?.canTranslate() != false) {
            listener?.translate(pid, true)
        }
        return true
    }

    /// A tap on the text: reveals the word's translation and selects it; on
    /// empty space it deselects, as in the web app.
    fun selectWord(x: Float, y: Float) {
        val hit = hit(x, y)
        val word = hit?.wordIndex
        if (hit == null || word == null) {
            if (selected != null) {
                selected = null
                listener?.deselected()
            }
            return
        }
        val key = WordKey(hit.paragraphId, word)
        revealed = revealed + key
        selected = key
        listener?.selected(hit.paragraphId, word)
    }

    fun visibleControlRects(): List<Pair<Int, RectF>> {
        val out = ArrayList<Pair<Int, RectF>>()
        for (column in 0 until columns) {
            val index = spreadStart + column
            if (index < 0) continue
            if (index >= text.pages.size) break
            val page = text.pages[index]
            out += controlRects(page, columnX(column), marginTop - page.top)
        }
        return out
    }

    val pageLabel: String
        get() = if (isProvisional) "Page" else
            "Pages ${firstVisiblePage + 1} to ${min(spreadStart + columns, text.pages.size)} of ${text.pages.size}"

    // Paging

    /// The spread of the current pairing that holds a page; null if it is the one shown.
    private fun spreadTarget(page: Int): Int? {
        if (text.lines.isEmpty()) return null
        val clamped = page.coerceIn(0, text.pages.size - 1)
        val phase = spreadStart.mod(columns)
        val target = clamped - (clamped - phase).mod(columns)
        return if (target == spreadStart) null else target
    }

    /// The next (+1) or previous (-1) spread, if there is one.
    private fun neighbour(direction: Int): Int? = spreadTarget(spreadStart + direction * columns)

    private fun show(target: Int) {
        spreadStart = target
        sticky = currentAnchor()
        invalidate()
        listener?.movedTo(firstVisiblePage, text.pages.size)
        contentDescription = pageLabel
    }

    fun go(page: Int, animated: Boolean = true) {
        val target = spreadTarget(page) ?: return
        endSlide()
        if (animated && animates) {
            // The new spread pushes the old one out.
            incoming = target
            animateSlide(if (target > spreadStart) -width.toFloat() else width.toFloat(), 180L) {
                slide = 0f
                incoming = null
                show(target)
            }
        } else {
            show(target)
        }
    }

    private val animates: Boolean get() = isAttachedToWindow && !AppPrefs.animationsOff && width > 0

    // Sliding

    /// A finger drags the spread sideways by `offset` (negative toward the
    /// next spread). With no spread that way it resists.
    private fun slideTo(offset: Float) {
        slideAnimator?.cancel()
        slideAnimator = null
        incoming = neighbour(if (offset < 0) 1 else -1)
        slide = if (incoming == null) offset * 0.2f else offset
    }

    /// The drag ended: far or fast enough turns the spread, otherwise it springs back.
    private fun slideEnded(velocity: Float) {
        if (slide == 0f) return
        val forward = slide < 0
        val far = abs(slide) > width * 0.22f || (abs(velocity) > dp(350f) && (velocity < 0) == forward)
        val target = neighbour(if (forward) 1 else -1)
        if (far && target != null) {
            incoming = target
            val end = if (forward) -width.toFloat() else width.toFloat()
            animateSlide(end, slideDuration(end)) {
                slide = 0f
                incoming = null
                show(target)
            }
        } else {
            animateSlide(0f, slideDuration(0f)) { incoming = null }
        }
    }

    private fun slideDuration(end: Float): Long =
        (120 + 160 * min(abs(end - slide) / max(width, 1), 1f)).toLong()

    /// Drops a slide in progress, e.g. when a key turns the page mid-drag.
    private fun endSlide() {
        slideAnimator?.cancel()
        slideAnimator = null
        incoming = null
        slide = 0f
    }

    private fun animateSlide(end: Float, duration: Long, completion: () -> Unit) {
        slideAnimator?.cancel()
        slideAnimator = null
        val start = slide
        // Nothing to watch off screen (tests), or animations are off.
        if (!animates || start == end) {
            slide = end
            completion()
            return
        }
        var finished = false
        slideAnimator = ValueAnimator.ofFloat(start, end).apply {
            this.duration = duration
            // Cubic ease-out.
            interpolator = DecelerateInterpolator(1.5f)
            addUpdateListener {
                slide = it.animatedValue as Float
                if (it.animatedFraction >= 1f && !finished) {
                    finished = true
                    slideAnimator = null
                    completion()
                }
            }
            start()
        }
    }

    fun nextSpread() = go(spreadStart + columns)
    fun previousSpread() = go(spreadStart - columns)
    fun takeKeys() {
        requestFocus()
    }

    // Touch

    private var panning = false
    private val gestures = GestureDetector(
        context,
        object : GestureDetector.SimpleOnGestureListener() {
            override fun onDown(e: MotionEvent): Boolean = true

            override fun onSingleTapUp(e: MotionEvent): Boolean {
                tapped(e.x, e.y)
                return true
            }

            override fun onScroll(e1: MotionEvent?, e2: MotionEvent, dx: Float, dy: Float): Boolean {
                val start = e1 ?: return false
                val movedX = e2.x - start.x
                val movedY = e2.y - start.y
                // Sideways drags only, unless the spread is already on the move.
                if (abs(movedX) > abs(movedY) || slide != 0f) {
                    panning = true
                    parent?.requestDisallowInterceptTouchEvent(true)
                    slideTo(movedX)
                }
                return true
            }

            override fun onFling(e1: MotionEvent?, e2: MotionEvent, vx: Float, vy: Float): Boolean {
                if (!panning) return false
                panning = false
                slideEnded(vx)
                return true
            }

            override fun onLongPress(e: MotionEvent) {
                val point = nearestWord(e.x, e.y) ?: (e.x to e.y)
                menuHit = hit(point.first, point.second) ?: return
                showContextMenu(e.x, e.y)
            }
        },
    )

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val handled = gestures.onTouchEvent(event)
        if (event.actionMasked == MotionEvent.ACTION_UP || event.actionMasked == MotionEvent.ACTION_CANCEL) {
            if (panning) {
                panning = false
                slideEnded(0f)
            }
        }
        return handled || super.onTouchEvent(event)
    }

    private fun tapped(x: Float, y: Float) {
        takeKeys()
        if (activateControl(x, y)) return
        // A finger is not a pointer: a tap just beside a word still means the word.
        nearestWord(x, y)?.let { (wx, wy) ->
            selectWord(wx, wy)
            return
        }
        // In the margins (not over text), a tap turns the page, as in other readers.
        if (x > width - marginRight) return nextSpread()
        if (x < marginLeft - 2 * controlSlop - controlSize) return previousSpread()
        selectWord(x, y)
    }

    /// The point itself if it is on a word, else the closest of a few points
    /// around it that is; short words are narrower than a fingertip.
    private fun nearestWord(x: Float, y: Float): Pair<Float, Float>? {
        for (reach in floatArrayOf(0f, 7f, 14f)) {
            val r = dp(reach)
            val offsets = if (reach == 0f) listOf(0f to 0f) else listOf(
                -r to 0f, r to 0f, 0f to -r, 0f to r, -r to -r, r to -r, -r to r, r to r)
            for ((ox, oy) in offsets) {
                if (hit(x + ox, y + oy)?.wordIndex != null) return (x + ox) to (y + oy)
            }
        }
        return null
    }

    private fun buildMenu(menu: ContextMenu) {
        val hit = menuHit ?: return
        val p = text.paragraphs[hit.paragraphId] ?: return
        val word = hit.wordIndex?.let { p.words.getOrNull(it) }
        if (word != null) {
            val surface = p.text.substring(word.start, word.start + word.len)
            menu.add("Copy Word").setOnMenuItemClickListener { copy(surface) }
            // The sentence the word is in (there is no drag selection by touch).
            val sentence = p.words.filter { it.sentence == word.sentence }
            val first = sentence.firstOrNull()
            val last = sentence.lastOrNull()
            if (first != null && last != null) {
                val text = p.text.substring(first.start, last.start + last.len).replace(' ', ' ')
                menu.add("Copy Sentence").setOnMenuItemClickListener { copy(text) }
            }
        }
        menu.add("Copy Paragraph").setOnMenuItemClickListener { copy(p.text.replace(' ', '\n')) }
        menu.add(if (p.translated) "Translate Again" else "Translate").setOnMenuItemClickListener {
            listener?.translate(hit.paragraphId, !p.translated)
            true
        }
    }

    private fun copy(text: String): Boolean {
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        clipboard?.setPrimaryClip(ClipData.newPlainText("FLTS", text))
        return true
    }

    // A mouse wheel or a trackpad's two-finger scroll turns pages too.
    private var lastWheelFlip = 0L

    override fun onGenericMotionEvent(event: MotionEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_CLASS_POINTER) && event.actionMasked == MotionEvent.ACTION_SCROLL) {
            val amount = event.getAxisValue(MotionEvent.AXIS_VSCROLL) - event.getAxisValue(MotionEvent.AXIS_HSCROLL)
            if (amount != 0f && event.eventTime - lastWheelFlip > 250) {
                lastWheelFlip = event.eventTime
                if (amount < 0) nextSpread() else previousSpread()
            }
            return true
        }
        return super.onGenericMotionEvent(event)
    }

    // Keyboard

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (event.isCtrlPressed || event.isAltPressed || event.isMetaPressed) return super.onKeyDown(keyCode, event)
        when (keyCode) {
            KeyEvent.KEYCODE_DPAD_RIGHT, KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_PAGE_DOWN -> nextSpread()
            KeyEvent.KEYCODE_SPACE -> if (event.isShiftPressed) previousSpread() else nextSpread()
            KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_PAGE_UP -> previousSpread()
            KeyEvent.KEYCODE_MOVE_HOME -> go(0)
            KeyEvent.KEYCODE_MOVE_END -> go(text.pages.size - 1)
            KeyEvent.KEYCODE_W -> listener?.toggle('w')
            KeyEvent.KEYCODE_C -> listener?.toggle('c')
            KeyEvent.KEYCODE_ESCAPE -> if (listener?.toggle('\u001b') != true) return super.onKeyDown(keyCode, event)
            else -> return super.onKeyDown(keyCode, event)
        }
        return true
    }

    companion object {
        const val CHANGES_PER_TURN = 5
        private val backgroundLayouts: Executor = Executors.newSingleThreadExecutor { Thread(it, "flts-reader-layout") }
    }
}
