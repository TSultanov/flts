package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.runCatchingCancellable
import com.TS.FLTS.core.userMessage
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.yield
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.ParagraphText

/// One chapter's paragraphs as display text, kept current under translation
/// and sync. A fetch in flight when its paragraph changes is superseded by a
/// newer one (per-paragraph epochs), so a late reply never overwrites fresher text.
class ChapterStore(
    val bookId: UUID,
    val chapterId: Int,
    private val core: CoreService,
    private val events: EventHub,
    private val scope: CoroutineScope,
) {
    var paragraphIds by mutableStateOf<List<Int>>(emptyList())
        private set
    var paragraphs by mutableStateOf<Map<Int, ParagraphText>>(emptyMap())
        private set
    var loaded by mutableStateOf(false)
        private set

    /// The book's language as the core names it, for hyphenation.
    var language by mutableStateOf<String?>(null)
        private set
    var error by mutableStateOf<String?>(null)

    /// Bumped on every content change, with the ids that changed (null = all).
    var revision by mutableIntStateOf(0)
        private set
    var changedIds: Set<Int>? = null
        private set

    private var tokens = listOf<EventHub.Token>()
    private val epochs = HashMap<Int, Int>()
    private val pending = sortedSetOf<Int>()
    private var flushScheduled = false

    init {
        tokens = listOf(
            events.on { event ->
                when {
                    event is CoreEvent.ParagraphUpdated && event.event.bookId == bookId ->
                        refetch(listOf(event.event.paragraphId))
                    event is CoreEvent.CardsUpdated -> refetch(paragraphIds)
                    event is CoreEvent.BookUpdated && event.bookId == bookId -> scope.launch { reloadIds() }
                }
            },
        )
    }

    fun close() {
        events.cancel(tokens)
        tokens = emptyList()
    }

    suspend fun load() {
        try {
            val ids = core.getBookChapterParagraphIds(bookId, chapterId)
            val fetched = fetch(ids)
            paragraphIds = ids
            paragraphs = fetched.associateBy { it.id }
            loaded = true
            changedIds = null
            revision += 1
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.userMessage
            loaded = true
        }
    }

    private suspend fun reloadIds() {
        val ids = runCatchingCancellable { core.getBookChapterParagraphIds(bookId, chapterId) }.getOrNull() ?: return
        // Same ids: text changes arrive as `paragraph_updated`, and the queue
        // emits `book_updated` on every save, so refetching here would storm.
        if (ids != paragraphIds) load()
    }

    private suspend fun fetch(ids: List<Int>): List<ParagraphText> {
        val out = ArrayList<ParagraphText>(ids.size)
        for (slice in ids.chunked(BATCH_SIZE)) {
            val batch = core.getParagraphTexts(bookId, slice)
            if (language == null && batch.language.isNotEmpty()) language = batch.language
            out.addAll(batch.paragraphs)
        }
        return out
    }

    /// Coalesces bursts (a chapter translation emits one event per paragraph).
    fun refetch(ids: List<Int>) {
        val mine = paragraphIds.toSet()
        val wanted = ids.filter(mine::contains)
        if (wanted.isEmpty()) return
        for (id in wanted) epochs[id] = (epochs[id] ?: 0) + 1
        pending.addAll(wanted)
        if (flushScheduled) return
        flushScheduled = true
        scope.launch {
            yield()
            flushScheduled = false
            val batch = pending.toList()
            pending.clear()
            val started = batch.associateWith { epochs[it] ?: 0 }
            val fetched = runCatchingCancellable { fetch(batch) }.getOrNull() ?: return@launch
            val changed = HashSet<Int>()
            val next = paragraphs.toMutableMap()
            for (p in fetched) {
                if ((epochs[p.id] ?: 0) != started[p.id]) continue
                if (next[p.id] != p) {
                    next[p.id] = p
                    changed.add(p.id)
                }
            }
            if (changed.isEmpty()) return@launch
            paragraphs = next
            changedIds = changed
            revision += 1
        }
    }

    suspend fun translate(paragraphId: Int, model: String, useCache: Boolean = true) {
        try {
            core.translateParagraph(bookId, paragraphId, model, useCache)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.userMessage
        }
    }

    val untranslatedCount: Int get() = paragraphIds.count { paragraphs[it]?.translated != true }

    companion object {
        const val BATCH_SIZE = 64
    }
}
