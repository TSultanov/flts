package com.TS.FLTS

import com.TS.FLTS.core.EventHub
import com.TS.FLTS.reader.ReaderTypography
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.stores.BookStore
import com.TS.FLTS.stores.ChapterStore
import com.TS.FLTS.stores.ConfigStore
import com.TS.FLTS.stores.LibraryFolder
import com.TS.FLTS.stores.SyncStore
import com.TS.FLTS.stores.TranslationActivityStore
import com.TS.FLTS.stores.WordStore
import com.TS.FLTS.core.WordRef
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import uniffi.flts_ffi.AppConfig
import uniffi.flts_ffi.BookMeta
import uniffi.flts_ffi.BookSummaryStatus
import uniffi.flts_ffi.CardResult
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.CoreException
import uniffi.flts_ffi.Grammar
import uniffi.flts_ffi.MarkRun
import uniffi.flts_ffi.ParagraphText
import uniffi.flts_ffi.ParagraphTexts
import uniffi.flts_ffi.ParagraphUpdated
import uniffi.flts_ffi.TranslationFinished
import uniffi.flts_ffi.TranslationProgress
import uniffi.flts_ffi.TranslationProvider
import uniffi.flts_ffi.TranslationStarted
import uniffi.flts_ffi.WordInfo
import uniffi.flts_ffi.WordRange

private fun book(title: String, path: List<String>) =
    BookMeta(UUID.randomUUID(), title, 1, 1, 0.0, path)

class LibraryFolderTests {
    @Test
    fun buildsNestedFoldersSorted() {
        val tree = LibraryFolder.build(listOf(book("b", listOf("X", "Y")), book("a", emptyList()), book("c", listOf("X"))))
        assertEquals(listOf("a"), tree.books.map { it.title })
        assertEquals(listOf("X"), tree.folders.map { it.name })
        assertEquals(listOf("c"), tree.folders[0].books.map { it.title })
        assertEquals(listOf("b"), tree.folders[0].folders[0].books.map { it.title })
        assertEquals(listOf(emptyList(), listOf("X"), listOf("X", "Y")), tree.allPaths)
    }
}

// Robolectric: the stores use org.json and the Compose snapshot runtime.
@OptIn(ExperimentalCoroutinesApi::class)
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class StoreTests {
    private val id = UUID.randomUUID()

    @Test
    fun progressNeverCreatesAnEntry() = runTest {
        val store = TranslationActivityStore(EventHub(), this)
        store.progress(TranslationProgress(id, 1, 7, 5, 10))
        assertNull(store.activity(id, 1))
    }

    @Test
    fun staleFinishedKeepsNewerRequest() = runTest {
        val store = TranslationActivityStore(EventHub(), this)
        store.started(TranslationStarted(id, 1, 1, 10))
        store.started(TranslationStarted(id, 1, 2, 10))
        store.finished(TranslationFinished(id, 1, 1, null))
        assertEquals(2, store.activity(id, 1)?.requestId)
        store.finished(TranslationFinished(id, 1, 2, "boom"))
        assertNull(store.activity(id, 1))
        assertEquals("boom", store.lastFailure)
    }

    @Test
    fun parsesJsonAndBareIds() {
        val json = SyncStore.pairingPayload("ABC", "Mac")
        assertEquals("ABC" to "Mac", SyncStore.parsePairing(json))
        assertEquals("XYZ" to null, SyncStore.parsePairing("  XYZ "))
        assertNull(SyncStore.parsePairing("  "))
        // What the other shells produce: sorted keys, no spaces.
        assertEquals("""{"deviceId":"ABC","name":"Mac"}""", json)
    }

    private fun texts() = ParagraphTexts(
        "spa",
        listOf(
            ParagraphText(0, "Hola", emptyList(), listOf(WordRange(0, 4, 0, 0, 0, "Hello")), true),
            ParagraphText(1, "mundo", listOf(MarkRun(0, 5, 1)), emptyList(), false),
        ),
    )

    @Test
    fun loadsParagraphTexts() = runTest {
        val core = FakeCore()
        core.getBookChapterParagraphIdsHandler = { _, _ -> listOf(0, 1) }
        core.getParagraphTextsHandler = { _, _ -> texts() }
        val store = ChapterStore(id, 0, core, EventHub(), this)
        store.load()
        assertEquals(listOf(0, 1), store.paragraphIds)
        assertEquals("Hello", store.paragraphs[0]?.words?.first()?.translation)
        assertEquals(1, store.untranslatedCount)
        assertEquals("es", ReaderTypography.localeFor(store.language!!).language)
    }

    /// `book_updated` comes once per saved paragraph while a chapter
    /// translates; with unchanged ids it must not refetch the chapter.
    @Test
    fun bookUpdatedWithSameIdsDoesNotRefetch() = runTest {
        val core = FakeCore()
        val events = EventHub()
        core.getBookChapterParagraphIdsHandler = { _, _ -> listOf(0, 1) }
        core.getParagraphTextsHandler = { _, _ -> texts() }
        val store = ChapterStore(id, 0, core, events, this)
        store.load()
        val before = core.count("getParagraphTexts")
        repeat(20) { events.deliver(CoreEvent.BookUpdated(id)) }
        advanceUntilIdle()
        assertEquals(before, core.count("getParagraphTexts"))
        events.deliver(CoreEvent.ParagraphUpdated(ParagraphUpdated(id, 1)))
        events.deliver(CoreEvent.ParagraphUpdated(ParagraphUpdated(id, 0)))
        advanceUntilIdle()
        assertEquals("a burst is one fetch", before + 1, core.count("getParagraphTexts"))
        store.close()
    }

    /// A reply that was already in flight when its paragraph changed again is dropped.
    @Test
    fun aLateReplyNeverOverwritesFresherText() = runTest {
        val core = FakeCore()
        val events = EventHub()
        var version = 0
        val gate = kotlinx.coroutines.CompletableDeferred<Unit>()
        core.getBookChapterParagraphIdsHandler = { _, _ -> listOf(0) }
        core.getParagraphTextsHandler = { _, _ ->
            val mine = version++
            if (mine == 1) gate.await()
            ParagraphTexts("spa", listOf(ParagraphText(0, "v$mine", emptyList(), emptyList(), true)))
        }
        val store = ChapterStore(id, 0, core, events, this)
        store.load()
        events.deliver(CoreEvent.ParagraphUpdated(ParagraphUpdated(id, 0)))
        advanceUntilIdle()
        events.deliver(CoreEvent.ParagraphUpdated(ParagraphUpdated(id, 0)))
        advanceUntilIdle()
        assertEquals("v2", store.paragraphs[0]?.text)
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals("v2", store.paragraphs[0]?.text)
        store.close()
    }

    @Test
    fun chapterTranslatesOnceThePreviousSummaryExists() = runTest {
        val core = FakeCore()
        core.listBookChaptersHandler = { emptyList() }
        core.getBookReadingStateHandler = { null }
        core.getBookSummaryStatusHandler = { BookSummaryStatus(3, listOf(true, false, false), 1) }
        val book = BookStore(id, core, EventHub(), this)
        assertTrue("before the status loads nothing is held back", book.canTranslate(1))
        book.load()
        assertTrue(book.canTranslate(0))
        assertTrue(book.canTranslate(1))
        assertFalse(book.canTranslate(2))
        book.close()
    }

    private fun fakeAppCore(): FakeCore = FakeCore().apply {
        getLanguagesHandler = { emptyList() }
        getTranslationProvidersHandler = { emptyList() }
        listBooksHandler = { listOf(BookMeta(id, "Libro", 1, 1, 0.0, emptyList())) }
    }

    @Test
    fun deletingTheOpenBookLeavesANotice() = runTest {
        val model = AppModel(fakeAppCore(), this)
        model.library.reload()
        model.selectedBookId = id
        model.events.deliver(CoreEvent.BookDeleted(id))
        assertNull(model.selectedBookId)
        assertTrue(model.deletedBookNotice!!.contains("Libro"))
    }

    private fun config(key: String?) = AppConfig(
        targetLanguageId = "eng", translationProvider = TranslationProvider.GOOGLE, geminiApiKey = key,
        openaiApiKey = null, deepseekApiKey = null, zaiApiKey = null, openrouterApiKey = null, model = "m",
        libraryPath = null, spotifyClientId = null, spotifyPreloadCount = 0, spotifyShowNextTrack = false,
        ankiEndpoint = null, ankiApiKey = null, ankiSyncEnabled = false, syncEnabled = false, syncDeviceName = null,
        translationConcurrency = 1,
    )

    /// A rejected update puts the old config back and says why.
    @Test
    fun aFailedConfigUpdateIsRolledBack() = runTest {
        val core = fakeAppCore()
        core.getConfigHandler = { config(null) }
        core.getModelsHandler = { emptyList() }
        core.updateConfigHandler = { throw CoreException.Failed("disk full") }
        val store = ConfigStore(EventHub(), this)
        store.attach(core)
        advanceUntilIdle()
        assertFalse(store.isConfigured)
        store.update { it.copy(geminiApiKey = "k") }
        assertNull(store.config?.geminiApiKey)
        assertEquals("disk full", store.error)
        core.updateConfigHandler = {}
        store.update { it.copy(geminiApiKey = "k") }
        assertTrue(store.isConfigured)
    }

    /// Going to another word keeps the last one's details until the new one loads.
    @Test
    fun selectingAWordLoadsItAndMarksAnExistingCard() = runTest {
        val core = FakeCore()
        val grammar = Grammar("ser", "be", "verb", null, null, null, null, null)
        core.getWordInfoHandler = { _, _, _, w ->
            WordInfo("w$w", "", false, listOf("t"), grammar, "s", "m", "spa", canCreateCard = true, hasCard = w == 1)
        }
        core.createCardForWordHandler = { _, _, _, _ -> CardResult.ALREADY_EXISTS }
        val store = WordStore(core, EventHub(), this)
        store.select(WordRef(id, 0, 0, 0, 0), "uno")
        advanceUntilIdle()
        assertEquals("w0", store.info?.original)
        assertEquals(WordStore.CardState.Idle, store.cardState)
        store.select(WordRef(id, 0, 0, 1, 1), "dos")
        assertEquals("w0", store.info?.original)
        advanceUntilIdle()
        assertEquals("w1", store.info?.original)
        assertEquals(WordStore.CardState.Exists, store.cardState)
        store.select(null, null)
        assertNull(store.info)
        advanceUntilIdle()
        store.close()
    }

    @Test
    fun cancellingChapterLoadRethrowsWithoutSettingError() = runTest {
        val core = FakeCore()
        val gate = CompletableDeferred<Unit>()
        var cancelled = false
        core.getBookChapterParagraphIdsHandler = { _, _ ->
            gate.await()
            emptyList()
        }
        val store = ChapterStore(id, 0, core, EventHub(), this)
        val job = launch {
            try {
                store.load()
            } catch (e: CancellationException) {
                cancelled = true
                throw e
            }
        }
        advanceUntilIdle()
        job.cancel()
        advanceUntilIdle()
        assertTrue(cancelled)
        assertNull(store.error)
        assertFalse(store.loaded)
        store.close()
    }
}
