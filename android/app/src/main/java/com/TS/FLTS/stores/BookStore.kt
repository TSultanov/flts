package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.userMessage
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.flts_ffi.BookReadingState
import uniffi.flts_ffi.BookSummaryStatus
import uniffi.flts_ffi.ChapterMeta
import uniffi.flts_ffi.CoreEvent

/// One open book: its chapters, summary status and saved reading position.
class BookStore(
    val bookId: UUID,
    private val core: CoreService,
    private val events: EventHub,
    private val scope: CoroutineScope,
) {
    var chapters by mutableStateOf<List<ChapterMeta>>(emptyList())
        private set
    var summary by mutableStateOf<BookSummaryStatus?>(null)
        private set
    var readingState by mutableStateOf<BookReadingState?>(null)
        private set

    /// Where each chapter was left in this session; the saved state holds only the last one.
    var positions by mutableStateOf<Map<Int, BookReadingState>>(emptyMap())
        private set
    var loaded by mutableStateOf(false)
        private set
    var currentChapter by mutableStateOf<Int?>(null)
    var error by mutableStateOf<String?>(null)

    private var tokens = listOf<EventHub.Token>()
    private var chaptersReloadScheduled = false

    init {
        tokens = listOf(
            events.on { event ->
                when {
                    event is CoreEvent.BookUpdated && event.bookId == bookId && !chaptersReloadScheduled -> {
                        chaptersReloadScheduled = true
                        scope.launch {
                            delay(300)
                            chaptersReloadScheduled = false
                            loadChapters()
                        }
                    }
                    event is CoreEvent.SummaryGenerationProgress && event.event.bookId == bookId ->
                        scope.launch { loadSummary() }
                }
            },
        )
    }

    fun close() {
        events.cancel(tokens)
        tokens = emptyList()
    }

    suspend fun load() {
        loadChapters()
        readingState = runCatching { core.getBookReadingState(bookId) }.getOrNull()
        if (currentChapter == null) {
            currentChapter = readingState?.chapterId ?: chapters.firstOrNull()?.id
        }
        loaded = true
        loadSummary()
    }

    suspend fun loadChapters() {
        try {
            chapters = core.listBookChapters(bookId)
        } catch (e: Exception) {
            error = e.userMessage
        }
    }

    suspend fun loadSummary() {
        summary = runCatching { core.getBookSummaryStatus(bookId) }.getOrNull()
    }

    /// As in the web app: a chapter translates once the previous chapter's
    /// summary exists (it is the context for the translation).
    fun canTranslate(chapterId: Int): Boolean {
        val summary = summary
        if (chapterId <= 0 || summary == null) return true
        return summary.generated.getOrNull(chapterId - 1) == true
    }

    fun save(chapterId: Int, paragraphId: Int, pageOffset: Int) {
        val state = BookReadingState(chapterId, paragraphId, pageOffset)
        if (state == readingState) return
        readingState = state
        positions = positions + (chapterId to state)
        scope.launch { runCatching { core.saveBookReadingState(bookId, state) } }
    }

    suspend fun translateChapter(chapterId: Int, model: String) {
        try {
            core.translateChapter(bookId, chapterId, model, true)
        } catch (e: Exception) {
            error = e.userMessage
        }
    }
}
