package com.TS.FLTS

import androidx.activity.ComponentActivity
import androidx.compose.ui.test.hasTestTag
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performTextInput
import androidx.test.core.app.ApplicationProvider
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.stores.AppModel
import com.TS.FLTS.ui.FltsTheme
import com.TS.FLTS.ui.RootScreen
import kotlinx.coroutines.MainScope
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import uniffi.flts_ffi.AppConfig
import uniffi.flts_ffi.LanguageInfo
import uniffi.flts_ffi.ProviderMeta
import uniffi.flts_ffi.TranslationProvider

/// A first run: onboarding until a key is entered, then the (empty) library.
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class OnboardingTest {
    @get:Rule
    val compose = createAndroidComposeRule<ComponentActivity>()

    private fun has(tag: String) = compose.onAllNodes(hasTestTag(tag)).fetchSemanticsNodes().isNotEmpty()

    @Test
    fun aKeyLeadsToTheLibrary() {
        AppPrefs.init(ApplicationProvider.getApplicationContext())
        var saved = AppConfig(
            targetLanguageId = "eng", translationProvider = TranslationProvider.GOOGLE, geminiApiKey = null,
            openaiApiKey = null, deepseekApiKey = null, zaiApiKey = null, openrouterApiKey = null, model = "m",
            libraryPath = null, spotifyClientId = null, spotifyPreloadCount = 0, spotifyShowNextTrack = false,
            ankiEndpoint = null, ankiApiKey = null, ankiSyncEnabled = false, syncEnabled = false,
            syncDeviceName = null, translationConcurrency = 1,
        )
        val core = FakeCore().apply {
            getLanguagesHandler = { listOf(LanguageInfo("eng", "English", null)) }
            getTranslationProvidersHandler = { listOf(ProviderMeta(TranslationProvider.GOOGLE, "Google", "m", "geminiApiKey", null)) }
            getConfigHandler = { saved }
            updateConfigHandler = { saved = it }
            getModelsHandler = { emptyList() }
            listBooksHandler = { emptyList() }
            listTranslationActivityHandler = { emptyList() }
            getSyncStatusHandler = { com.TS.FLTS.core.syncDisabled }
            getAnkiSyncStatusHandler = { com.TS.FLTS.core.ankiIdle }
        }
        lateinit var model: AppModel
        compose.runOnUiThread { model = AppModel(core, MainScope()) }
        compose.setContent { FltsTheme { RootScreen(model) } }

        compose.waitUntil(10_000) { has("onboarding") }
        compose.onNodeWithTag("onboarding.continue").performClick()
        compose.waitUntil(5_000) { has("onboarding.missing") }
        compose.onNodeWithTag("settings.apiKey").performTextInput("secret")
        compose.onNodeWithTag("onboarding.continue").performClick()
        compose.waitUntil(10_000) { compose.onAllNodes(hasText("Your Library Is Empty")).fetchSemanticsNodes().isNotEmpty() }
        assertEquals("secret", saved.geminiApiKey)
    }
}
