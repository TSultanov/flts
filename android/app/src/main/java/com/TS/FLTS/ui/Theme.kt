package com.TS.FLTS.ui

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import com.TS.FLTS.core.AppPrefs

/// Light, dark, or the system's choice, under Reading in Settings.
enum class Appearance(val title: String) {
    SYSTEM("Match System"), LIGHT("Light"), DARK("Dark")
}

/// Reader preferences shared by the reader's menu, Settings and the reader.
object ReaderPreferences {
    const val DEFAULT_FONT_SIZE = 18f
    val MARGIN_SCALES = 0.5f..3f
    const val MARGIN_STEP = 0.25f

    private var loaded = false
    private var size by mutableFloatStateOf(DEFAULT_FONT_SIZE)
    private var margins by mutableFloatStateOf(1f)
    private var look by mutableStateOf(Appearance.SYSTEM)

    private fun load() {
        if (loaded) return
        loaded = true
        val store = AppPrefs.store
        size = store.getFloat("readerFontSize", DEFAULT_FONT_SIZE)
        margins = store.getFloat("readerMarginScale", 1f)
        look = runCatching { Appearance.valueOf(store.getString("appearance", null) ?: "SYSTEM") }
            .getOrDefault(Appearance.SYSTEM)
    }

    /// In sp.
    var fontSize: Float
        get() {
            load()
            return size
        }
        set(value) {
            load()
            size = value.coerceIn(12f, 36f)
            AppPrefs.store.edit().putFloat("readerFontSize", size).apply()
        }

    /// Side margins, as a multiple of the default for the screen.
    var marginScale: Float
        get() {
            load()
            return margins
        }
        set(value) {
            load()
            margins = value.coerceIn(MARGIN_SCALES)
            AppPrefs.store.edit().putFloat("readerMarginScale", margins).apply()
        }

    var appearance: Appearance
        get() {
            load()
            return look
        }
        set(value) {
            load()
            look = value
            AppPrefs.store.edit().putString("appearance", value.name).apply()
        }
}

@Composable
fun FltsTheme(content: @Composable () -> Unit) {
    val dark = when (ReaderPreferences.appearance) {
        Appearance.SYSTEM -> isSystemInDarkTheme()
        Appearance.LIGHT -> false
        Appearance.DARK -> true
    }
    val context = LocalContext.current
    val colors = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
            if (dark) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
        dark -> darkColorScheme()
        else -> lightColorScheme()
    }
    MaterialTheme(colorScheme = colors, content = content)
}

/// A dialog is its own window: without this its status bar icons stay light
/// on a light page.
@Composable
fun DialogSystemBars() {
    val view = androidx.compose.ui.platform.LocalView.current
    val light = MaterialTheme.colorScheme.surface.luminance() > 0.5f
    androidx.compose.runtime.SideEffect {
        val window = (view.parent as? androidx.compose.ui.window.DialogWindowProvider)?.window ?: return@SideEffect
        androidx.core.view.WindowCompat.getInsetsController(window, view).apply {
            isAppearanceLightStatusBars = light
            isAppearanceLightNavigationBars = light
        }
    }
}
