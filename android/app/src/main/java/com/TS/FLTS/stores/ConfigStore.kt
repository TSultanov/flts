package com.TS.FLTS.stores

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.TS.FLTS.core.CoreService
import com.TS.FLTS.core.EventHub
import com.TS.FLTS.core.hasApiKeyForProvider
import com.TS.FLTS.core.userMessage
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.flts_ffi.AppConfig
import uniffi.flts_ffi.CoreEvent
import uniffi.flts_ffi.LanguageInfo
import uniffi.flts_ffi.ModelInfo
import uniffi.flts_ffi.ProviderMeta
import uniffi.flts_ffi.TranslationProvider

class ConfigStore(private val events: EventHub, private val scope: CoroutineScope) : CoreBound {
    var config by mutableStateOf<AppConfig?>(null)
        private set
    var loaded by mutableStateOf(false)
        private set
    var languages by mutableStateOf<List<LanguageInfo>>(emptyList())
        private set
    var providers by mutableStateOf<List<ProviderMeta>>(emptyList())
        private set
    var models by mutableStateOf<List<ModelInfo>>(emptyList())
        private set
    var modelsLoading by mutableStateOf(false)
        private set
    var error by mutableStateOf<String?>(null)

    private var core: CoreService? = null

    override fun attach(core: CoreService) {
        this.core = core
        events.on { event ->
            if (event is CoreEvent.ConfigUpdated) scope.launch { reload() }
        }
        languages = runCatching { core.getLanguages() }.getOrDefault(emptyList())
        providers = runCatching { core.getTranslationProviders() }.getOrDefault(emptyList())
        scope.launch { reload() }
    }

    suspend fun reload() {
        val core = core ?: return
        try {
            config = core.getConfig()
            loaded = true
            loadModels()
        } catch (e: Exception) {
            error = e.userMessage
            loaded = true
        }
    }

    suspend fun loadModels() {
        val core = core ?: return
        modelsLoading = true
        models = runCatching { core.getModels() }.getOrDefault(emptyList())
        modelsLoading = false
    }

    /// Onboarding is done once a target language and the provider's key are set.
    val isConfigured: Boolean
        get() = config?.let { it.targetLanguageId.isNotEmpty() && it.hasApiKeyForProvider } ?: false

    val targetLanguage: LanguageInfo?
        get() = config?.targetLanguageId?.let { id -> languages.firstOrNull { it.id == id } }

    fun provider(id: TranslationProvider): ProviderMeta? = providers.firstOrNull { it.id == id }

    fun models(provider: TranslationProvider): List<ModelInfo> =
        models.filter { it.provider == null || it.provider == provider }

    suspend fun update(change: (AppConfig) -> AppConfig) {
        val core = core ?: return
        val previous = config ?: return
        val next = change(previous)
        if (next == previous) return
        config = next
        try {
            core.updateConfig(next)
        } catch (e: Exception) {
            config = previous
            error = e.userMessage
        }
    }

    suspend fun purgeGeminiCaches(): Int = core?.purgeGeminiCaches() ?: 0

    /// An EPUB's declared language as a known ISO 639-3 id, else English.
    fun suggestedSourceLanguage(code: String?): String {
        val parsed = core?.parseLanguageId(code) ?: return "eng"
        return if (languages.isEmpty() || languages.any { it.id == parsed }) parsed else "eng"
    }
}
