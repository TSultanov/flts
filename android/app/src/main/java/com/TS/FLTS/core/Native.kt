package com.TS.FLTS.core

import android.content.Context

/// Hands the JVM to the Rust core (TLS verification runs through Android's
/// trust manager). Loaded by name here so JNI finds `init`; the bindings
/// reach the same library through JNA.
object Native {
    init {
        System.loadLibrary("flts_ffi")
    }

    @JvmStatic
    external fun init(context: Context)
}
