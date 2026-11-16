package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import java.util.UUID
import kotlinx.coroutines.CoroutineScope

/// The open book: chapters, the chapter being read and the selected word.
class BookSession(
    bookId: UUID,
    private val core: CoreService,
    private val events: EventHub,
    private val scope: CoroutineScope,
) {
    val book = BookStore(bookId, core, events, scope)
    val word = WordStore(core, events, scope)
    var chapter by mutableStateOf<ChapterStore?>(null)
        private set

    suspend fun start() {
        book.load()
        book.currentChapter?.let { open(it) }
    }

    suspend fun open(chapterId: Int) {
        if (chapter?.chapterId == chapterId) return
        chapter?.close()
        book.currentChapter = chapterId
        word.select(null, null)
        val store = ChapterStore(book.bookId, chapterId, core, events, scope)
        chapter = store
        store.load()
    }

    fun close() {
        chapter?.close()
        book.close()
        word.close()
    }
}
