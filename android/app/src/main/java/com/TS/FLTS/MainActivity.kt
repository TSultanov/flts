package com.TS.FLTS

import android.os.Bundle
import android.system.Os
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.TS.FLTS.core.AppPrefs
import com.TS.FLTS.ui.FltsTheme
import com.TS.FLTS.ui.RootScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val model = (application as FltsApp).model
        if (model.core == null) {
            applyTestEnvironment()
            AppPrefs.init(this)
            model.start(this)
        }
        setContent {
            FltsTheme {
                RootScreen(model)
            }
        }
    }

    /// Debug builds take the core's test switches (`FLTS_CONFIG_DIR` and
    /// friends) as launch extras: `am start … -e FLTS_CONFIG_DIR <dir>`.
    private fun applyTestEnvironment() {
        if (!BuildConfig.DEBUG) return
        val extras = intent?.extras ?: return
        for (key in extras.keySet()) {
            if (!key.startsWith("FLTS_")) continue
            extras.getString(key)?.let { Os.setenv(key, it, true) }
        }
    }
}
