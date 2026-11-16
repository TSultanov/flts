package com.TS.FLTS.stores

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreClient
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.logError
import com.TS.FLTS.core.userMessage
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.launch
import uniffi.flts_ffi.CoreEvent

/// A store that starts loading once the core exists.
interface CoreBound {
    fun attach(core: CoreService)
}

/// Owns the core and the app-wide stores. One per process: the core outlives
/// any activity.
class AppModel(val scope: CoroutineScope = MainScope()) {
    sealed interface Phase {
        data object Starting : Phase
        data object Running : Phase
        data class Failed(val message: String) : Phase
    }

    var phase by mutableStateOf<Phase>(Phase.Starting)
        private set
    val events = EventHub()
    var core: CoreService? = null
        private set

    val config = ConfigStore(events, scope)
    val library = LibraryStore(events, scope)
    val activity = TranslationActivityStore(events, scope)
    val sync = SyncStore(events, scope)
    val anki = AnkiStore(events, scope)

    /// The open book. Launch opens the library, as the web app does.
    var selectedBookId by mutableStateOf<UUID?>(null)
    var showImport by mutableStateOf(false)
    var showSettings by mutableStateOf(false)

    /// The settings page to open on, when a toolbar item asks for one.
    var settingsPage by mutableStateOf<String?>(null)
    var lastError by mutableStateOf<String?>(null)

    /// Set when the open book was deleted (often on another device).
    var deletedBookNotice by mutableStateOf<String?>(null)

    /// For tests: stores over a fake core.
    constructor(core: CoreService, scope: CoroutineScope) : this(scope) {
        attach(core)
    }

    fun start(context: Context) {
        if (phase != Phase.Starting || core != null) return
        try {
            attach(CoreClient.start(context, events))
        } catch (e: Throwable) {
            logError("core did not start: ${e.userMessage}")
            phase = Phase.Failed(e.userMessage)
        }
    }

    private fun attach(core: CoreService) {
        this.core = core
        for (store in listOf(config, library, activity, sync, anki)) store.attach(core)
        events.on { event ->
            if (event is CoreEvent.BookDeleted && event.bookId == selectedBookId) {
                val title = library.book(event.bookId)?.title
                deletedBookNotice = title?.let { "“$it” was deleted, possibly on another device." }
                    ?: "The book you were reading was deleted, possibly on another device."
                selectedBookId = null
            }
        }
        phase = Phase.Running
    }

    /// Back in the foreground: sync catches up, stale progress is dropped.
    fun foregrounded() {
        if (core == null) return
        sync.wake()
        scope.launch { activity.reconcile() }
    }

    fun report(error: Throwable) {
        lastError = error.userMessage
        logError(error.userMessage)
    }
}
