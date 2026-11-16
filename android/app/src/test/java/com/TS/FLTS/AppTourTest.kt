package com.TS.FLTS

import android.os.Handler
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.longClick
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.test.performTouchInput
import androidx.test.core.app.ApplicationProvider
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.reader.ReaderView
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.ui.FltsTheme
import com.TS.FLTS.ui.RootScreen
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.FltsCore
import uniffi.flts_ffi.MainTask
import uniffi.flts_ffi.Shell

/// The whole app on the JVM: the real screens and reader over the real core
/// (built for this machine) on a seeded library. `FLTS_SMOKE_CONFIG_DIR` as
/// for `CoreSmokeTest`; skipped otherwise.
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class AppTourTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private fun has(matcher: SemanticsMatcher) = compose.onAllNodes(matcher).fetchSemanticsNodes().isNotEmpty()

    private fun await(what: String, seconds: Int = 20, found: () -> Boolean) {
        // Robolectric holds delayed main-thread work (debounced reloads, the
        // reader's full layout) until the clock is moved.
        runCatching {
            compose.waitUntil(seconds * 1000L) {
                org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(50))
                found()
            }
        }
            .onFailure { throw AssertionError("$what (waited ${seconds}s)") }
    }

    private fun reader(view: View): ReaderView? = when (view) {
        is ReaderView -> view
        is ViewGroup -> (0 until view.childCount).firstNotNullOfOrNull { reader(view.getChildAt(it)) }
        else -> null
    }

    private fun currentReader(): ReaderView? = compose.runOnUiThread<ReaderView?> { reader(compose.activity.window.decorView) }

    @Test
    fun libraryReaderWordSettingsImport() {
        assumeTrue("FLTS_SMOKE_CONFIG_DIR not set", !System.getenv("FLTS_CONFIG_DIR").isNullOrEmpty())
        AppPrefs.init(ApplicationProvider.getApplicationContext())
        var hub: EventHub? = null
        val main = Handler(Looper.getMainLooper())
        val core = FltsCore.start(object : Shell {
            override fun onEvent(event: CoreEvent) {
                main.post { hub?.deliver(event) }
            }
            override fun runOnMain(task: MainTask) {
                main.post { task.run() }
            }
        })
        val books = runBlocking { core.listBooks() }
        assumeTrue(books.isNotEmpty())
        // The book with the most translated text has words to tap.
        val book = books.maxBy { it.translationRatio }
        lateinit var model: AppModel
        compose.runOnUiThread {
            model = AppModel(core, MainScope())
            hub = model.events
        }
        compose.setContent { FltsTheme { RootScreen(model) } }

        // Library
        await("the seeded book is not listed", 40) { has(hasTestTag("library.book.${book.title}")) }
        compose.onNodeWithTag("library.book.${book.title}").performClick()

        // Reader
        await("the book did not open") { has(hasContentDescription("Library")) }
        await("no reader view") { currentReader() != null }
        val view = currentReader()!!
        // Robolectric holds delayed main-thread work until the clock is moved.
        fun settled(): Boolean {
            org.robolectric.Shadows.shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(50))
            return !view.isProvisional && view.pageCount > 0 && view.text.lines.isNotEmpty()
        }
        runCatching { compose.waitUntil(30_000) { settled() } }.onFailure {
            throw AssertionError(
                "the chapter never finished laying out: provisional=${view.isProvisional} pages=${view.pageCount} " +
                    "lines=${view.text.lines.size} order=${view.order.size} size=${view.width}x${view.height}")
        }
        compose.runOnUiThread {
            val before = view.spreadStart
            view.go(view.spreadStart + view.columns, animated = false)
            if (view.pageCount > view.columns) assertTrue("the page did not turn", view.spreadStart > before)
            view.go(0, animated = false)
        }

        // A translated word: the bar shows it, the details open.
        var tapped = false
        compose.runOnUiThread {
            val page = view.text.pages[view.firstVisiblePage]
            loop@ for ((pid, p) in view.text.paragraphs) {
                if (!p.translated) continue
                for (i in p.words.indices) {
                    if (p.words[i].translation.isNullOrEmpty()) continue
                    val frame = view.text.wordFrames(pid, i).firstOrNull() ?: continue
                    if (frame.top < page.top || frame.bottom > page.bottom) continue
                    val (x, y) = view.viewPoint(frame.centerX(), frame.centerY(), view.firstVisiblePage) ?: continue
                    view.selectWord(x, y)
                    tapped = view.selected != null
                    if (tapped) break@loop
                }
            }
        }
        assertTrue("no translated word on the first page could be selected", tapped)
        await("the word's details never loaded") { model.selectedBookId != null && has(hasContentDescription("Show word details")) && !has(hasText("Select a word to show its translation")) }
        compose.onNodeWithContentDescription("Show word details").performClick()
        runCatching { compose.waitUntil(8_000) { has(hasText("TRANSLATIONS")) } }.onFailure {
            val texts = compose.onAllNodes(SemanticsMatcher("any") { true }, useUnmergedTree = true).fetchSemanticsNodes()
                .flatMap { n -> n.config.map { it.key.name + "=" + it.value } }.filter { it.startsWith("Text=") || it.startsWith("ContentDescription=") }
            throw AssertionError("the word's details did not open; on screen: $texts")
        }
        compose.onNodeWithContentDescription("Hide word details").performClick()
        compose.onNodeWithContentDescription("Text Size and Appearance").performClick()
        compose.onNodeWithText("Larger Text").performClick()
        await("the reader did not re-lay out at the new size", 30) { settled() }
        compose.onNodeWithText("Default Size").performClick()
        compose.runOnUiThread { model.selectedBookId = null }
        await("did not return to the library") { has(hasTestTag("library.import")) }

        // Settings: every page opens.
        compose.onNodeWithContentDescription("Settings").performClick()
        await("settings did not open") { has(hasTestTag("settings.close")) }
        for (section in listOf("translation", "reading", "sync", "anki", "storage")) {
            compose.onNodeWithTag("settings.section.$section").performClick()
            await("the $section page did not open") { has(hasTestTag("settings.back")) }
            compose.onNodeWithTag("settings.back").performClick()
            await("did not return from $section") { has(hasTestTag("settings.close")) }
        }
        compose.onNodeWithTag("settings.close").performClick()
        await("settings did not close") { !has(hasTestTag("settings.close")) }

        // Import a pasted text through the core, then find it listed.
        compose.onNodeWithTag("library.import").performClick()
        await("the import sheet did not open") { has(hasTestTag("import.tab.text")) }
        compose.onNodeWithTag("import.tab.text").performClick()
        compose.onNodeWithTag("import.title").performTextInput("Prueba de importación")
        compose.onNodeWithTag("import.text").performTextInput("Era una tarde tranquila. El gato dormía junto a la ventana.")
        compose.onNodeWithTag("import.submit").performClick()
        await("the text was not imported", 30) { has(hasTestTag("library.book.Prueba de importación")) }
        compose.onNodeWithTag("library.book.Prueba de importación").assertIsDisplayed()
        assertEquals(books.size + 1, model.library.books.size)
        assertFalse(model.showImport)
        assertNotNull(model.library.book(model.library.books.first { it.title == "Prueba de importación" }.id))

        // Move it into a new folder from its long-press menu.
        val imported = "library.book.Prueba de importación"
        compose.onNodeWithTag(imported).performTouchInput { longClick() }
        await("no menu on a book") { has(hasText("Move to Folder…")) }
        compose.onNodeWithText("Move to Folder…").performClick()
        await("the move sheet did not open") { has(hasTestTag("move-new-folder")) }
        compose.onNodeWithTag("move-new-folder").performClick()
        await("no field for the new folder's name") { has(hasTestTag("new-folder-name")) }
        compose.onNodeWithTag("new-folder-name").performTextInput("Pruebas")
        compose.onNodeWithTag("move-confirm").performClick()
        await("the folder did not appear") { has(hasTestTag("library.folder.Pruebas")) }
        assertEquals(listOf("Pruebas"), model.library.books.first { it.title == "Prueba de importación" }.path)
        if (!has(hasTestTag(imported))) compose.onNodeWithTag("library.folder.Pruebas").performClick()
        await("the moved book is not under its folder") { has(hasTestTag(imported)) }

        // Delete it through edit mode.
        compose.onNodeWithTag("library.select").performClick()
        compose.onNodeWithTag(imported).performClick()
        await("no selection bar") { has(hasTestTag("library.delete")) }
        compose.onNodeWithTag("library.delete").performClick()
        await("no delete confirmation") { has(hasText("Delete")) }
        compose.onNodeWithText("Delete").performClick()
        await("the deleted book is still listed") { model.library.books.none { it.title == "Prueba de importación" } }
        if (has(hasText("Done"))) compose.onNodeWithText("Done").performClick()

        // An EPUB: parsed, its chapters listed, imported.
        compose.onNodeWithTag("library.import").performClick()
        await("the import sheet did not open") { has(hasTestTag("import.pickFile")) }
        compose.onNodeWithTag("import.pickFile").performClick()
        await("the EPUB was not parsed", 30) { has(hasText("Select chapters to import")) }
        val before = model.library.books.size
        compose.onNodeWithTag("import.submit").performClick()
        await("the EPUB was not imported", 60) { model.library.books.size == before + 1 && !model.showImport }
        runBlocking { core.shutdown() }
    }
}
