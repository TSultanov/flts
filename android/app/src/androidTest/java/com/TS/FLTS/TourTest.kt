package com.TS.FLTS

import android.content.Intent
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.hasContentDescription
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.ComposeTestRule
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/// A tour of the app on a seeded library (see scripts/ui-tests.sh, which
/// passes its place as `FLTS_CONFIG_DIR`). The core starts once per process,
/// so this is one test.
@RunWith(AndroidJUnit4::class)
class TourTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    private fun ComposeTestRule.await(what: String, seconds: Int, found: () -> Boolean) {
        runCatching { waitUntil(seconds * 1000L) { found() } }
            .onFailure { throw AssertionError("$what (waited ${seconds}s)") }
    }

    private fun ComposeTestRule.has(matcher: androidx.compose.ui.test.SemanticsMatcher) =
        onAllNodes(matcher).fetchSemanticsNodes().isNotEmpty()

    @Test
    fun libraryReaderSettingsImport() {
        val args = InstrumentationRegistry.getArguments()
        val configDir = args.getString("FLTS_CONFIG_DIR")
        assumeTrue("no seeded library given", !configDir.isNullOrEmpty())
        val book = args.getString("FLTS_UITEST_BOOK") ?: "Lluvia en la ciudad"

        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val intent = Intent(context, MainActivity::class.java)
            .putExtra("FLTS_CONFIG_DIR", configDir)
            .putExtra("FLTS_NO_ANIMATIONS", "1")
        ActivityScenario.launch<MainActivity>(intent).use {
            // Library
            compose.await("the seeded book is not listed", 40) { compose.has(hasTestTag("library.book.$book")) }
            compose.onNodeWithTag("library.book.$book").performClick()

            // Reader: the page count appears once the full layout lands.
            compose.await("the book did not open", 20) { compose.has(hasContentDescription("Library")) }
            compose.await("no word bar", 20) { compose.has(hasText("Select a word to show its translation")) }
            compose.onNodeWithContentDescription("Text Size and Appearance").performClick()
            compose.onNodeWithText("Larger Text").performClick()
            compose.onNodeWithText("Default Size").performClick()
            androidx.test.espresso.Espresso.pressBack()
            compose.onNodeWithContentDescription("Library").performClick()
            compose.await("back did not return to the library", 10) { compose.has(hasTestTag("library.import")) }

            // Settings
            compose.onNodeWithContentDescription("Settings").performClick()
            compose.await("settings did not open", 10) { compose.has(hasTestTag("settings.close")) }
            compose.onNodeWithTag("settings.close").performClick()

            // Import a pasted text, then find it in the library.
            compose.onNodeWithTag("library.import").performClick()
            compose.await("the import sheet did not open", 10) { compose.has(hasTestTag("import.tab.text")) }
            compose.onNodeWithTag("import.tab.text").performClick()
            compose.onNodeWithTag("import.title").performTextInput("Prueba de importación")
            compose.onNodeWithTag("import.text").performTextInput("Era una tarde tranquila. El gato dormía junto a la ventana.")
            compose.onNodeWithTag("import.submit").performClick()
            compose.await("the text was not imported", 30) { compose.has(hasTestTag("library.book.Prueba de importación")) }
            compose.onNodeWithTag("library.book.Prueba de importación").assertIsDisplayed()
        }
    }
}
