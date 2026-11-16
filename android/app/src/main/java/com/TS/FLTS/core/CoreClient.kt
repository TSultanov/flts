package com.TS.FLTS.core

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.CoreException
import uniffi.flts_ffi.FltsCore
import uniffi.flts_ffi.FltsCoreInterface
import uniffi.flts_ffi.MainTask
import uniffi.flts_ffi.Shell

/// What stores call; the generated `FltsCore` in the app, a fake in tests.
typealias CoreService = FltsCoreInterface

const val LOG_TAG = "flts"

/// Starts the Rust core with a shell that hops events onto the main thread.
object CoreClient {
    fun start(context: Context, events: EventHub): FltsCore {
        val app = context.applicationContext
        Native.init(app)
        // Where the Tauri app kept them: config and data share the app's data
        // dir, so an install over it opens the same library.
        val data = app.dataDir.absolutePath
        return FltsCore.startWithDirs(ShellBridge(events), data, app.cacheDir.absolutePath, data)
    }
}

private class ShellBridge(private val events: EventHub) : Shell {
    // One queue keeps events in emission order.
    private val main = Handler(Looper.getMainLooper())

    override fun onEvent(event: CoreEvent) {
        main.post { events.deliver(event) }
    }

    override fun runOnMain(task: MainTask) {
        main.post { task.run() }
    }
}

/// The core's own message; the generated exception describes itself as a dump.
val Throwable.userMessage: String
    get() = when (this) {
        is CoreException.Failed -> detail
        else -> message ?: toString()
    }

fun logError(message: String) {
    runCatching { Log.e(LOG_TAG, message) }
}
