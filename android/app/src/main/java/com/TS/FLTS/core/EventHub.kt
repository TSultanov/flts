package com.TS.FLTS.core

import uniffi.flts_ffi.CoreEvent

/// Fans core events out to subscribers. Main thread only.
class EventHub {
    class Token internal constructor(internal val id: Int)

    private val handlers = LinkedHashMap<Int, (CoreEvent) -> Unit>()
    private var next = 0

    fun on(handler: (CoreEvent) -> Unit): Token {
        next += 1
        handlers[next] = handler
        return Token(next)
    }

    fun cancel(token: Token) {
        handlers.remove(token.id)
    }

    fun cancel(tokens: List<Token>) = tokens.forEach(::cancel)

    fun deliver(event: CoreEvent) {
        // A handler may subscribe or cancel while events are delivered.
        for (handler in handlers.values.toList()) handler(event)
    }
}
