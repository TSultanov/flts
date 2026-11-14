import Foundation

// The core's records are generated (Generated/flts_ffi.swift); these add what
// views need on top.

extension BookMeta: Identifiable {}
extension ChapterMeta: Identifiable {}
extension ModelInfo: Identifiable {}
extension ProviderMeta: Identifiable {}
extension LanguageInfo: Identifiable {
    var label: String {
        if let localName, localName != name { "\(name) — \(localName)" } else { name }
    }
}
extension DeviceEntry: Identifiable {
    public var id: String { deviceId }
}
extension PendingDevice: Identifiable {
    public var id: String { deviceId }
}
extension TrackMeta: Identifiable {}
extension ParagraphText: Identifiable {}
extension TranslationProvider: Identifiable, CaseIterable {
    public var id: Self { self }
    public static let allCases: [TranslationProvider] = [.google, .openai, .deepseek, .zai, .openrouter]
}

extension AppConfig {
    func apiKey(for provider: TranslationProvider) -> String? {
        switch provider {
        case .google: geminiApiKey
        case .openai: openaiApiKey
        case .deepseek: deepseekApiKey
        case .zai: zaiApiKey
        case .openrouter: openrouterApiKey
        }
    }

    mutating func setApiKey(_ key: String?, for provider: TranslationProvider) {
        let value = key?.trimmingCharacters(in: .whitespacesAndNewlines)
        let stored = (value?.isEmpty ?? true) ? nil : value
        switch provider {
        case .google: geminiApiKey = stored
        case .openai: openaiApiKey = stored
        case .deepseek: deepseekApiKey = stored
        case .zai: zaiApiKey = stored
        case .openrouter: openrouterApiKey = stored
        }
    }

    var hasApiKeyForProvider: Bool {
        !(apiKey(for: translationProvider) ?? "").isEmpty
    }
}

extension StoreIssues {
    var any: Bool {
        newerVersion > 0 || unreadable > 0 || strangers > 0 || stalled || sharedIdentity
    }
}

extension SyncStatus {
    static let disabled = SyncStatus(
        state: .disabled, deviceId: nil, deviceCount: 0, connectedCount: 0, completion: nil, lastError: nil,
        storeIssues: nil)
}

extension AnkiSyncStatus {
    static let idle = AnkiSyncStatus(state: .idle, lastFinishedAtMs: nil, lastError: nil, lastReport: nil)
}

/// A word in a chapter: the paragraph plus the core's sentence/word indices.
struct WordRef: Hashable, Sendable {
    let bookId: UUID
    let paragraphId: Int
    let sentence: Int
    let word: Int
    let flatIndex: Int
}
