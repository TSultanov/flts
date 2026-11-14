import Foundation

// Wire types of the core's JSON commands and events. Field names match the
// Rust serde names; see app-core/src/dispatch.rs.

// MARK: Library

struct BookMeta: Codable, Identifiable, Hashable, Sendable {
    let id: UUID
    let title: String
    let chaptersCount: Int
    let paragraphsCount: Int
    let translationRatio: Double
    let path: [String]
}

struct ChapterMeta: Codable, Identifiable, Hashable, Sendable {
    let id: Int
    let title: String
    let translationRatio: Double
}

struct BookReadingState: Codable, Hashable, Sendable {
    var chapterId: Int
    var paragraphId: Int
    var pageOffset: Int
}

struct BookSummaryStatus: Codable, Sendable {
    let totalChapters: Int
    let generated: [Bool]
    let activelyGenerating: Int?
}

struct ImportProgress: Codable, Sendable {
    let done: Int
    let total: Int
}

/// A parsed EPUB, kept as JSON so it goes back to `import_epub` unchanged.
struct EpubPreview: Sendable {
    struct Chapter: Decodable, Sendable {
        struct Paragraph: Decodable, Sendable { let text: String }
        let title: String
        let paragraphs: [Paragraph]
    }

    struct Summary: Decodable, Sendable {
        let title: String
        let chapters: [Chapter]
        let language: String?
    }

    let raw: JSONValue
    let summary: Summary
}

// MARK: Reader

struct ParagraphTexts: Codable, Sendable {
    /// ISO 639-3 code of the book's language.
    let language: String
    let paragraphs: [ParagraphText]
}

struct ParagraphText: Codable, Identifiable, Sendable, Equatable {
    struct Run: Codable, Sendable, Equatable {
        let start: Int
        let len: Int
        /// Bit 0 emphasis, bit 1 strong.
        let marks: Int
    }

    struct Word: Codable, Sendable, Equatable {
        let start: Int
        let len: Int
        let s: Int
        let w: Int
        let f: Int
        let t: String?
    }

    let id: Int
    let text: String
    let runs: [Run]
    let words: [Word]
    let translated: Bool
}

struct Grammar: Codable, Sendable, Hashable {
    let originalInitialForm: String
    let targetInitialForm: String
    let partOfSpeech: String
    let plurality: String?
    let person: String?
    let tense: String?
    let `case`: String?
    let other: String?
}

struct WordInfo: Codable, Sendable, Hashable {
    let original: String
    let note: String
    let isPunctuation: Bool
    let contextualTranslations: [String]
    let grammar: Grammar
    let fullSentenceTranslation: String
    let translationModel: String
    let sourceLanguage: String
    let canCreateCard: Bool
    let hasCard: Bool
}

struct SystemDefinition: Codable, Sendable, Hashable {
    let definition: String
    let transcription: String?
}

/// A word in a chapter: the paragraph plus the core's sentence/word indices.
struct WordRef: Hashable, Sendable {
    let bookId: UUID
    let paragraphId: Int
    let sentence: Int
    let word: Int
    let flatIndex: Int
}

// MARK: Translation activity

struct TranslationActivity: Codable, Sendable, Equatable {
    var requestId: Int
    var progressChars: Int
    var expectedChars: Int
}

struct ActiveTranslationRow: Codable, Sendable {
    let bookId: UUID
    let paragraphId: Int
    let requestId: Int
    let progressChars: Int
    let expectedChars: Int
}

struct TranslationStartedEvent: Codable, Sendable {
    let bookId: UUID
    let paragraphId: Int
    let requestId: Int
    let expectedChars: Int
}

struct TranslationProgressEvent: Codable, Sendable {
    let bookId: UUID
    let paragraphId: Int
    let requestId: Int
    let progressChars: Int
    let expectedChars: Int
}

struct TranslationFinishedEvent: Codable, Sendable {
    let bookId: UUID
    let paragraphId: Int
    let requestId: Int
    let error: String?
}

struct ParagraphUpdatedEvent: Codable, Sendable {
    let bookId: UUID
    let paragraphId: Int
}

struct SummaryProgressEvent: Codable, Sendable {
    let bookId: UUID
    let current: Int
    let total: Int
    let status: String
    let error: String?
}

// MARK: Config

enum TranslationProvider: String, Codable, Sendable, CaseIterable, Identifiable {
    case google, openai, deepseek, zai, openrouter
    var id: String { rawValue }
}

struct ModelInfo: Codable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
    let provider: TranslationProvider?
}

struct ProviderMeta: Codable, Sendable, Hashable, Identifiable {
    let id: TranslationProvider
    let name: String
    let defaultModel: String
    let apiKeyField: String
    let modelSelection: String?
}

struct LanguageInfo: Codable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
    let localName: String?

    var label: String {
        if let localName, localName != name { "\(name) — \(localName)" } else { name }
    }
}

struct AppConfig: Codable, Sendable, Equatable {
    var targetLanguageId: String
    var translationProvider: TranslationProvider
    var geminiApiKey: String?
    var openaiApiKey: String?
    var deepseekApiKey: String?
    var zaiApiKey: String?
    var openrouterApiKey: String?
    var model: String
    var libraryPath: String?
    var spotifyClientId: String?
    var spotifyPreloadCount: Int
    var spotifyShowNextTrack: Bool
    var ankiEndpoint: String?
    var ankiApiKey: String?
    var ankiSyncEnabled: Bool
    var syncEnabled: Bool
    var syncDeviceName: String?
    var translationConcurrency: Int

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

// MARK: Sync

enum SyncState: String, Codable, Sendable {
    case disabled, starting, online, syncing, error
}

struct StoreIssues: Codable, Sendable, Equatable {
    let newerVersion: Int
    let unreadable: Int
    let strangers: Int
    let stalled: Bool?
    let sharedIdentity: Bool?

    var any: Bool {
        newerVersion > 0 || unreadable > 0 || strangers > 0 || stalled == true || sharedIdentity == true
    }
}

struct SyncStatus: Codable, Sendable, Equatable {
    let state: SyncState
    let deviceId: String?
    let deviceCount: Int
    let connectedCount: Int
    let completion: Double?
    let lastError: String?
    let storeIssues: StoreIssues?

    static let disabled = SyncStatus(
        state: .disabled, deviceId: nil, deviceCount: 0, connectedCount: 0, completion: nil,
        lastError: nil, storeIssues: nil)
}

struct ThisDevice: Codable, Sendable {
    let deviceId: String
    let name: String?
}

struct DeviceEntry: Codable, Sendable, Identifiable, Hashable {
    let deviceId: String
    let name: String
    let connected: Bool
    var id: String { deviceId }
}

struct PendingDevice: Codable, Sendable, Identifiable, Hashable {
    let deviceId: String
    let name: String
    var id: String { deviceId }
}

// MARK: Anki

enum AnkiSyncState: String, Codable, Sendable {
    case idle, syncing, ok, err, unreachable
}

struct AnkiSyncReport: Codable, Sendable, Equatable {
    let totalCards: Int
    let attempted: Int
    let succeeded: Int
    let failed: Int
    let persistentFailures: [String]
}

struct AnkiSyncStatus: Codable, Sendable, Equatable {
    let state: AnkiSyncState
    let lastFinishedAtMs: Double?
    let lastError: String?
    let lastReport: AnkiSyncReport?

    static let idle = AnkiSyncStatus(state: .idle, lastFinishedAtMs: nil, lastError: nil, lastReport: nil)
}

// MARK: Lyrics / Spotify

enum PlayerState: String, Codable, Sendable {
    case playing, paused, stopped, notrunning
}

struct NowPlaying: Codable, Sendable, Equatable {
    let state: PlayerState
    let trackId: String?
    let name: String?
    let artist: String?
    let album: String?
    let positionMs: Int?
    let durationMs: Int?
}

struct LyricsLine: Codable, Sendable, Hashable {
    let time_ms: Int?
    let text: String
}

struct Lyrics: Codable, Sendable, Equatable {
    let track_id: String
    let lines: [LyricsLine]
    let synced: Bool
}

struct Gloss: Codable, Sendable, Hashable {
    let fragment: String
    let gloss: String
    let note: String
}

struct LyricsLineTranslation: Codable, Sendable, Hashable {
    let translation: String
    let glosses: [Gloss]
}

struct LyricsTranslation: Codable, Sendable, Equatable {
    let track_id: String
    let target_lang: String
    let model: String
    let lines: [LyricsLineTranslation]
}

struct TrackLyricsState: Codable, Sendable {
    let lyrics: Lyrics?
    let translation: LyricsTranslation?
}

struct LyricsResolvedEvent: Codable, Sendable {
    let trackId: String
    let lyrics: Lyrics?
}

struct LyricsDoneEvent: Codable, Sendable {
    let trackId: String
    let translation: LyricsTranslation
}

struct LyricsProgressEvent: Codable, Sendable {
    let trackId: String
    let bytes: Int
}

struct LyricsErrorEvent: Codable, Sendable {
    let trackId: String
    let error: String
}

struct TrackMeta: Codable, Sendable, Hashable, Identifiable {
    let id: String
    let name: String
    let artist: String
    let album: String?
    let durationMs: Int
}

struct QueueSnapshot: Codable, Sendable, Equatable {
    let contextType: String?
    let currentlyPlayingId: String?
    let upcoming: [TrackMeta]
}

struct SpotifyWebStatus: Codable, Sendable, Equatable {
    let connected: Bool
    let premiumRequired: Bool
    let lastError: String?
}

struct SpotifyCdpStatus: Codable, Sendable, Equatable {
    let available: Bool
    let port: Int
    let hint: String?
}

struct SpotifyLoginAgentStatus: Codable, Sendable, Equatable {
    let installed: Bool
    let path: String
}
