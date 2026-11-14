import Foundation
import Observation

@MainActor
@Observable
final class ConfigStore: CoreBound {
    private(set) var config: AppConfig?
    private(set) var loaded = false
    private(set) var languages: [LanguageInfo] = []
    private(set) var providers: [ProviderMeta] = []
    private(set) var models: [ModelInfo] = []
    private(set) var modelsLoading = false
    private(set) var libraryRoot: String?
    var error: String?

    private let events: EventHub
    private var core: (any CoreService)?

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on("config_updated") { [weak self] _ in
            Task { await self?.reload() }
        }
        Task {
            async let languages: [LanguageInfo] = core.invoke("get_languages")
            async let providers: [ProviderMeta] = core.invoke("get_translation_providers")
            async let root: String = core.invoke("get_library_root")
            self.languages = (try? await languages) ?? []
            self.providers = (try? await providers) ?? []
            self.libraryRoot = try? await root
            await reload()
        }
    }

    func reload() async {
        guard let core else { return }
        do {
            config = try await core.invoke("get_config")
            loaded = true
            await loadModels()
        } catch {
            self.error = error.localizedDescription
            loaded = true
        }
    }

    func loadModels() async {
        guard let core else { return }
        modelsLoading = true
        defer { modelsLoading = false }
        models = (try? await core.invoke("get_models")) ?? []
    }

    /// Onboarding is done once a target language and the provider's key are set.
    var isConfigured: Bool {
        guard let config else { return false }
        return !config.targetLanguageId.isEmpty && config.hasApiKeyForProvider
    }

    var targetLanguage: LanguageInfo? {
        guard let id = config?.targetLanguageId else { return nil }
        return languages.first { $0.id == id }
    }

    func provider(_ id: TranslationProvider) -> ProviderMeta? {
        providers.first { $0.id == id }
    }

    func models(for provider: TranslationProvider) -> [ModelInfo] {
        models.filter { $0.provider == nil || $0.provider == provider }
    }

    func update(_ change: (inout AppConfig) -> Void) async {
        guard let core, var next = config else { return }
        change(&next)
        guard next != config else { return }
        let previous = config
        config = next
        do {
            try await core.call("update_config", ["config": try JSONValue.from(next)])
        } catch {
            config = previous
            self.error = error.localizedDescription
        }
    }

    func purgeGeminiCaches() async throws -> Int {
        guard let core else { return 0 }
        return try await core.invoke("purge_gemini_caches")
    }

    /// An EPUB's declared language as a known ISO 639-3 id, else English.
    func suggestedSourceLanguage(for code: String?) async -> String {
        guard let core else { return "eng" }
        var args: [String: JSONValue] = [:]
        if let code { args["code"] = .string(code) }
        let parsed = (try? await core.invoke("parse_language_id", args, as: String?.self)) ?? nil
        guard let parsed else { return "eng" }
        return languages.isEmpty || languages.contains { $0.id == parsed } ? parsed : "eng"
    }
}
