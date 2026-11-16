package com.TS.FLTS.reader

import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.viewinterop.AndroidView
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.core.WordRef
import com.TS.FLTS.stores.BookStore
import com.TS.FLTS.stores.ChapterStore
import com.TS.FLTS.stores.TranslationActivityStore
import com.TS.FLTS.stores.WordStore
import com.TS.FLTS.ui.ReaderPreferences
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.flts_ffi.BookReadingState

/// Commands the toolbar and the book page send to the reader.
class ReaderControls {
    internal var view: ReaderView? = null
    var page by mutableIntStateOf(0)
    var pageCount by mutableIntStateOf(0)

    /// Single-key toggles as in the web reader (W, C), set by the book page.
    var toggleWordPanel: (() -> Unit)? = null
    var toggleChapters: (() -> Unit)? = null

    /// True if the word's details were open.
    var collapseWordPanel: (() -> Boolean)? = null

    /// Idempotent, so two paths closing the list cannot reopen it.
    var hideChapters: (() -> Unit)? = null
    var chaptersShown by mutableStateOf(false)

    fun next() = view?.nextSpread()
    fun previous() = view?.previousSpread()
    fun first() = view?.go(0)
    fun last() = view?.let { it.go(it.pageCount - 1) }
    fun focus() = view?.takeKeys()
}

/// The exact top line of the last position this device saved, next to the
/// synced (paragraph, pageOffset), which cannot express a line.
data class LocalPosition(val state: BookReadingState, val line: LineAnchor) {
    fun save(book: UUID) {
        val fields = listOf(state.chapterId, state.paragraphId, state.pageOffset, line.paragraphId, line.start)
        AppPrefs.store.edit().putString(KEY + book, fields.joinToString(",")).apply()
    }

    companion object {
        private const val KEY = "position."

        fun load(book: UUID): LocalPosition? {
            val fields = AppPrefs.store.getString(KEY + book, null)?.split(",")?.mapNotNull { it.toIntOrNull() }
            if (fields == null || fields.size != 5) return null
            return LocalPosition(BookReadingState(fields[0], fields[1], fields[2]), LineAnchor(fields[3], fields[4]))
        }
    }
}

/// Feeds a `ReaderView` from the stores and turns its events into store calls.
private class ReaderCoordinator(
    val book: BookStore,
    val chapter: ChapterStore,
    val word: WordStore,
    val activity: TranslationActivityStore,
    val controls: ReaderControls,
    val scope: CoroutineScope,
) : ReaderListener {
    var model = ""
    var view: ReaderView? = null
    private var applied = false
    private var revision = -1
    private var saveJob: Job? = null

    fun update(view: ReaderView, typography: ReaderTypography, colors: ReaderColors) {
        this.view = view
        controls.view = view
        view.setStyle(typography, colors)
        if (!applied) {
            if (!chapter.loaded) return
            applied = true
            revision = chapter.revision
            val saved = book.readingState?.takeIf { it.chapterId == chapter.chapterId }
            val state = saved ?: book.positions[chapter.chapterId]
            var anchor = state?.let { ReadingAnchor(it.paragraphId, it.pageOffset) }
            // The exact line, if this device saved this very position.
            if (state != null) {
                LocalPosition.load(book.bookId)?.takeIf { it.state == state }?.let { anchor = anchor?.copy(line = it.line) }
            }
            view.revealed = emptySet()
            view.selected = null
            view.setContent(chapter.paragraphIds, chapter.paragraphs, anchor)
            view.post { view.takeKeys() }
        } else if (revision != chapter.revision) {
            revision = chapter.revision
            if (view.order == chapter.paragraphIds) {
                // Diff rather than trust `changedIds`: several store revisions
                // can land between two updates of this view.
                val changed = chapter.paragraphIds.mapNotNull { id ->
                    chapter.paragraphs[id]?.takeIf { it != view.contentParagraph(id) }
                }
                if (changed.isNotEmpty()) view.replace(changed)
            } else {
                view.setContent(chapter.paragraphIds, chapter.paragraphs, view.currentAnchor())
            }
        }
        // Progress rings follow activity.
        view.invalidate()
    }

    override fun selected(paragraphId: Int, wordIndex: Int) {
        val p = view?.contentParagraph(paragraphId) ?: return
        val w = p.words.getOrNull(wordIndex) ?: return
        val end = (w.start + w.len).coerceAtMost(p.text.length)
        word.select(
            WordRef(book.bookId, paragraphId, w.sentence, w.word, w.flatIndex),
            p.text.substring(w.start.coerceAtMost(end), end))
    }

    override fun translate(paragraphId: Int, useCache: Boolean) {
        // Every way to translate waits for the chapter's summaries; a
        // retranslation (useCache false on a translated paragraph) does not.
        val translated = view?.contentParagraph(paragraphId)?.translated ?: false
        if (!translated && !book.canTranslate(chapter.chapterId)) return
        scope.launch { chapter.translate(paragraphId, model, useCache) }
    }

    override fun movedTo(page: Int, count: Int) {
        controls.page = page
        controls.pageCount = count
        val view = view ?: return
        // A provisional layout knows only its own pages; wait for the full one.
        if (view.isProvisional) return
        val anchor = view.currentAnchor() ?: return
        val chapterId = chapter.chapterId
        saveJob?.cancel()
        saveJob = scope.launch {
            delay(400)
            val state = BookReadingState(chapterId, anchor.paragraphId, anchor.pageOffset)
            anchor.line?.let { LocalPosition(state, it).save(book.bookId) }
            book.save(chapterId, anchor.paragraphId, anchor.pageOffset)
        }
    }

    /// Leaving the reader must not lose the last page turn.
    fun flush() {
        val view = view ?: return
        if (!applied || view.isProvisional || saveJob?.isActive != true) return
        saveJob?.cancel()
        val anchor = view.currentAnchor() ?: return
        val state = BookReadingState(chapter.chapterId, anchor.paragraphId, anchor.pageOffset)
        anchor.line?.let { LocalPosition(state, it).save(book.bookId) }
        book.save(chapter.chapterId, anchor.paragraphId, anchor.pageOffset)
    }

    override fun translationProgress(paragraphId: Int): Double? {
        val a = activity.activity(book.bookId, paragraphId) ?: return null
        return if (a.expectedChars > 0) a.progressChars.toDouble() / a.expectedChars else 0.0
    }

    override fun toggle(key: Char): Boolean {
        when (key) {
            'w' -> controls.toggleWordPanel?.invoke()
            'c' -> controls.toggleChapters?.invoke()
            // Escape backs out one step: the chapter list, the word's details, the word.
            '\u001b' -> when {
                controls.chaptersShown -> controls.hideChapters?.invoke()
                controls.collapseWordPanel?.invoke() == true -> {}
                word.selection != null -> {
                    view?.selected = null
                    word.select(null, null)
                }
                else -> return false
            }
            else -> return false
        }
        return true
    }

    override fun canTranslate(): Boolean = book.canTranslate(chapter.chapterId)

    override fun deselected() = word.select(null, null)
}

@Composable
fun readerColors(): ReaderColors {
    val scheme = MaterialTheme.colorScheme
    return ReaderColors(
        background = scheme.surface.toArgb(),
        text = scheme.onSurface.toArgb(),
        secondary = scheme.onSurfaceVariant.toArgb(),
        tertiary = scheme.outline.toArgb(),
        accent = scheme.primary.toArgb(),
    )
}

/// One chapter in the reader. Keyed by the caller on (book, chapter).
@Composable
fun ReaderHost(
    book: BookStore,
    chapter: ChapterStore,
    word: WordStore,
    activity: TranslationActivityStore,
    controls: ReaderControls,
    model: String,
    scope: CoroutineScope,
    modifier: Modifier = Modifier,
) {
    val coordinator = remember(book, chapter) { ReaderCoordinator(book, chapter, word, activity, controls, scope) }
    coordinator.model = model
    val density = LocalDensity.current
    val typography = ReaderTypography(
        fontSize = ReaderPreferences.fontSize * density.density * density.fontScale,
        density = density.density,
        language = chapter.language,
    )
    val colors = readerColors()
    AndroidView(
        factory = { context -> ReaderView(context).also { it.listener = coordinator } },
        modifier = modifier,
        update = { view ->
            // Read here so the view updates when they change.
            chapter.revision
            chapter.loaded
            activity.entries
            book.summary
            coordinator.update(view, typography, colors)
        },
    )
    DisposableEffect(coordinator) {
        onDispose {
            coordinator.flush()
            if (controls.view === coordinator.view) controls.view = null
        }
    }
}
