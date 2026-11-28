package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.runCatchingCancellable
import java.util.UUID
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.TranslationFinished
import uniffi.flts_ffi.TranslationProgress
import uniffi.flts_ffi.TranslationStarted

/// A paragraph's translation in flight.
data class TranslationActivity(val requestId: Int, val progressChars: Int, val expectedChars: Int)

/// Active paragraph translations. Progress and finished events are unordered,
/// so only `started` (or a snapshot) creates an entry, and each event touches
/// only its own request.
class TranslationActivityStore(private val events: EventHub, private val scope: CoroutineScope) : CoreBound {
    data class Key(val bookId: UUID, val paragraphId: Int)

    var entries by mutableStateOf<Map<Key, TranslationActivity>>(emptyMap())
        private set
    var lastFailure by mutableStateOf<String?>(null)
        private set
    private var reconcileToken = 0
    private var core: CoreService? = null

    override fun attach(core: CoreService) {
        this.core = core
        events.on { event ->
            when (event) {
                is CoreEvent.TranslationStarted -> started(event.event)
                is CoreEvent.TranslationProgress -> progress(event.event)
                is CoreEvent.TranslationFinished -> finished(event.event)
                is CoreEvent.BookDeleted -> {
                    reconcileToken += 1
                    entries = entries.filterKeys { it.bookId != event.bookId }
                }
                else -> {}
            }
        }
        scope.launch { reconcile() }
    }

    fun started(e: TranslationStarted) {
        reconcileToken += 1
        entries = entries + (Key(e.bookId, e.paragraphId) to TranslationActivity(e.requestId, 0, e.expectedChars))
    }

    fun progress(e: TranslationProgress) {
        val key = Key(e.bookId, e.paragraphId)
        if (entries[key]?.requestId != e.requestId) return
        entries = entries + (key to TranslationActivity(e.requestId, e.progressChars, e.expectedChars))
    }

    fun finished(e: TranslationFinished) {
        e.error?.let { lastFailure = it }
        reconcileToken += 1
        val key = Key(e.bookId, e.paragraphId)
        if (entries[key]?.requestId == e.requestId) entries = entries - key
    }

    /// Adopts translations whose `started` this process never saw.
    suspend fun reconcile() {
        val core = core ?: return
        reconcileToken += 1
        val token = reconcileToken
        val rows = runCatchingCancellable { core.listTranslationActivity() }.getOrNull() ?: return
        if (token != reconcileToken) return
        entries = rows.associate {
            Key(it.bookId, it.paragraphId) to TranslationActivity(it.requestId, it.progressChars, it.expectedChars)
        }
    }

    fun activity(bookId: UUID, paragraphId: Int): TranslationActivity? = entries[Key(bookId, paragraphId)]

    fun clearFailure() {
        lastFailure = null
    }
}
