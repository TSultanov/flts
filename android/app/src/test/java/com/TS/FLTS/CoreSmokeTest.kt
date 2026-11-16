package com.TS.FLTS

import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.FltsCore
import uniffi.flts_ffi.MainTask
import uniffi.flts_ffi.Shell

/// The generated bindings against the real core, built for this machine, on
/// a seeded library: `FLTS_SMOKE_CONFIG_DIR=<a copy of a seeded config dir>`
/// (Gradle passes it on as the core's `FLTS_CONFIG_DIR`). Skipped otherwise.
class CoreSmokeTest {
    @Test
    fun readsALibraryAndDeliversEvents() {
        assumeTrue("FLTS_SMOKE_CONFIG_DIR not set", !System.getenv("FLTS_CONFIG_DIR").isNullOrEmpty())
        val events = LinkedBlockingQueue<CoreEvent>()
        val shell = object : Shell {
            override fun onEvent(event: CoreEvent) {
                events.add(event)
            }
            override fun runOnMain(task: MainTask) = task.run()
        }
        val core = FltsCore.start(shell)
        runBlocking {
            assertTrue(core.getLanguages().isNotEmpty())
            val books = core.listBooks()
            assertTrue("the seeded library has no books", books.isNotEmpty())
            val book = books.first()
            val chapters = core.listBookChapters(book.id)
            assertEquals(book.chaptersCount, chapters.size)
            val ids = core.getBookChapterParagraphIds(book.id, chapters.first().id)
            val texts = core.getParagraphTexts(book.id, ids.take(8))
            assertEquals(ids.take(8), texts.paragraphs.map { it.id })
            assertTrue(texts.language.isNotEmpty())
            // Word ranges index the text as Kotlin strings do (UTF-16).
            for (p in texts.paragraphs) {
                for (w in p.words) assertTrue(w.start + w.len <= p.text.length)
            }
            assertTrue(texts.paragraphs.any { it.words.isNotEmpty() })

            val config = core.getConfig()
            events.clear()
            core.updateConfig(config.copy(translationConcurrency = config.translationConcurrency % 4 + 1))
            var updated = false
            while (!updated) {
                val event = events.poll(10, TimeUnit.SECONDS) ?: break
                updated = event is CoreEvent.ConfigUpdated
            }
            assertTrue("no config_updated event reached the shell", updated)
            core.shutdown()
        }
    }
}
