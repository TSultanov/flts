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
    private var reloadGen = 0
    private var modelsGen = 0
    private var updating = false
    private var updateWaiters: [CheckedContinuation<Void, Never>] = []

    init(events: EventHub) {
        self.events = events
    }

    func attach(_ core: any CoreService) {
        self.core = core
        events.on { [weak self] event in
            if case .configUpdated = event { Task { await self?.reload() } }
        }
        languages = (try? core.getLanguages()) ?? []
        providers = (try? core.getTranslationProviders()) ?? []
        Task {
            libraryRoot = try? await core.getLibraryRoot()
            await reload()
        }
    }

    func reload() async {
        guard let core else { return }
        reloadGen += 1
        let gen = reloadGen
        do {
            let next = try await core.getConfig()
            guard gen == reloadGen else { return }
            config = next
            loaded = true
            await loadModels()
        } catch {
            guard gen == reloadGen else { return }
            self.error = error.userMessage
            loaded = true
        }
    }

    func loadModels() async {
        guard let core else { return }
        modelsGen += 1
        let gen = modelsGen
        modelsLoading = true
        defer { if gen == modelsGen { modelsLoading = false } }
        let fetched = (try? await core.getModels()) ?? []
        guard gen == modelsGen else { return }
        models = fetched
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
        if updating {
            await withCheckedContinuation { updateWaiters.append($0) }
        }
        updating = true
        defer {
            if updateWaiters.isEmpty {
                updating = false
            } else {
                updateWaiters.removeFirst().resume()
            }
        }
        guard let core, var next = config else { return }
        change(&next)
        guard next != config else { return }
        let previous = config
        config = next
        do {
            try await core.updateConfig(config: next)
        } catch {
            config = previous
            self.error = error.userMessage
        }
    }

    func purgeGeminiCaches() async throws -> Int {
        guard let core else { return 0 }
        return try await core.purgeGeminiCaches()
    }

    /// An EPUB's declared language as a known ISO 639-3 id, else English.
    func suggestedSourceLanguage(for code: String?) -> String {
        guard let parsed = core?.parseLanguageId(code: code) else { return "eng" }
        return languages.isEmpty || languages.contains { $0.id == parsed } ? parsed : "eng"
    }
}
