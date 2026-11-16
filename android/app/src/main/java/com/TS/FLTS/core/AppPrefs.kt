package com.TS.FLTS.core

import android.content.Context
import android.content.SharedPreferences

/// UI state kept by this app (reader size, appearance, exact positions). A
/// run on a test config dir keeps its own file, never the real app's.
object AppPrefs {
    lateinit var store: SharedPreferences
        private set

    val isolated: Boolean get() = !System.getenv("FLTS_CONFIG_DIR").isNullOrEmpty()

    /// UI tests turn animations off (with a test config dir).
    val animationsOff: Boolean get() = isolated && System.getenv("FLTS_NO_ANIMATIONS") != null

    fun init(context: Context) {
        store = context.applicationContext.getSharedPreferences(if (isolated) "flts.isolated" else "flts", Context.MODE_PRIVATE)
    }
}
