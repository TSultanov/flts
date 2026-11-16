package com.TS.FLTS.core

import java.util.UUID
import uniffi.flts_ffi.AnkiSyncState
import uniffi.flts_ffi.AnkiSyncStatus
import uniffi.flts_ffi.AppConfig
import uniffi.flts_ffi.LanguageInfo
import uniffi.flts_ffi.StoreIssues
import uniffi.flts_ffi.SyncState
import uniffi.flts_ffi.SyncStatus
import uniffi.flts_ffi.TranslationProvider

// The core's records are generated (uniffi/flts_ffi); these add what views need.

val LanguageInfo.label: String
    get() = localName?.takeIf { it != name }?.let { "$name — $it" } ?: name

val allProviders = listOf(
    TranslationProvider.GOOGLE, TranslationProvider.OPENAI, TranslationProvider.DEEPSEEK,
    TranslationProvider.ZAI, TranslationProvider.OPENROUTER,
)

fun AppConfig.apiKey(provider: TranslationProvider): String? = when (provider) {
    TranslationProvider.GOOGLE -> geminiApiKey
    TranslationProvider.OPENAI -> openaiApiKey
    TranslationProvider.DEEPSEEK -> deepseekApiKey
    TranslationProvider.ZAI -> zaiApiKey
    TranslationProvider.OPENROUTER -> openrouterApiKey
}

fun AppConfig.withApiKey(key: String?, provider: TranslationProvider): AppConfig {
    val stored = key?.trim()?.takeIf { it.isNotEmpty() }
    return when (provider) {
        TranslationProvider.GOOGLE -> copy(geminiApiKey = stored)
        TranslationProvider.OPENAI -> copy(openaiApiKey = stored)
        TranslationProvider.DEEPSEEK -> copy(deepseekApiKey = stored)
        TranslationProvider.ZAI -> copy(zaiApiKey = stored)
        TranslationProvider.OPENROUTER -> copy(openrouterApiKey = stored)
    }
}

val AppConfig.hasApiKeyForProvider: Boolean
    get() = !apiKey(translationProvider).isNullOrEmpty()

val StoreIssues.any: Boolean
    get() = newerVersion > 0 || unreadable > 0 || strangers > 0 || stalled || sharedIdentity

val syncDisabled = SyncStatus(
    state = SyncState.DISABLED, deviceId = null, deviceCount = 0, connectedCount = 0, completion = null,
    lastError = null, storeIssues = null,
)

val ankiIdle = AnkiSyncStatus(state = AnkiSyncState.IDLE, lastFinishedAtMs = null, lastError = null, lastReport = null)

/// A word in a chapter: the paragraph plus the core's sentence/word indices.
data class WordRef(val bookId: UUID, val paragraphId: Int, val sentence: Int, val word: Int, val flatIndex: Int)
