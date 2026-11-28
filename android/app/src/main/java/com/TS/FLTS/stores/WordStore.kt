package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.WordRef
import com.TS.FLTS.core.userMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.flts_ffi.CardResult
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.WordInfo

/// The word selected in the reader and everything shown about it.
class WordStore(private val core: CoreService, private val events: EventHub, private val scope: CoroutineScope) {
    sealed interface CardState {
        data object Idle : CardState
        data object Creating : CardState
        data object Created : CardState
        data object Exists : CardState
        data class Failed(val message: String) : CardState
    }

    var selection by mutableStateOf<WordRef?>(null)
        private set
    var surface by mutableStateOf<String?>(null)
        private set
    var info by mutableStateOf<WordInfo?>(null)
        private set
    var loading by mutableStateOf(false)
        private set
    var cardState by mutableStateOf<CardState>(CardState.Idle)
        private set
    var error by mutableStateOf<String?>(null)

    private var tokens = listOf<EventHub.Token>()
    private var generation = 0

    init {
        tokens = listOf(
            events.on { event ->
                if (event is CoreEvent.BookUpdated && event.bookId == selection?.bookId) scope.launch { reload() }
            },
        )
    }

    fun close() {
        events.cancel(tokens)
        tokens = emptyList()
    }

    fun select(ref: WordRef?, surface: String?) {
        if (ref == selection) return
        selection = ref
        this.surface = surface
        cardState = CardState.Idle
        // Going to another word keeps the last one until the new one loads,
        // so open details do not collapse and reopen in between.
        if (ref == null) {
            generation += 1
            info = null
            loading = false
            return
        }
        scope.launch { reload() }
    }

    suspend fun reload() {
        generation += 1
        val gen = generation
        val ref = selection ?: run {
            loading = false
            return
        }
        loading = true
        try {
            val fetched = core.getWordInfo(ref.bookId, ref.paragraphId, ref.sentence, ref.word)
            if (gen != generation) return
            info = fetched
            if (fetched != null && fetched.hasCard && cardState == CardState.Idle) cardState = CardState.Exists
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (gen != generation) return
            info = null
            error = e.userMessage
        } finally {
            if (gen == generation) loading = false
        }
    }

    suspend fun createCard() {
        val ref = selection ?: return
        if (cardState == CardState.Creating) return
        cardState = CardState.Creating
        try {
            val result = core.createCardForWord(ref.bookId, ref.paragraphId, ref.sentence, ref.word)
            if (selection != ref) return
            if (result == CardResult.ALREADY_EXISTS) {
                cardState = CardState.Exists
            } else {
                // A brief check, then the button goes, as in the web app.
                cardState = CardState.Created
                scope.launch {
                    delay(1200)
                    if (selection == ref && cardState == CardState.Created) cardState = CardState.Exists
                }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (selection == ref) cardState = CardState.Failed(e.userMessage)
        }
    }
}
