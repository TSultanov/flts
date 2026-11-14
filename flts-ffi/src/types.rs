//! Records and enums the Swift app sees. Field names follow the core's serde
//! names, so a core value converts by `transcode` without per-type code.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Surfaces in Swift as `Int` (all counts and ids fit in i64).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Number(pub i64);

uniffi::custom_type!(Number, i64, {
    lower: |n| n.0,
    try_lift: |v| Ok(Number(v)),
});

uniffi::custom_type!(Uuid, String, {
    remote,
    lower: |id| id.to_string(),
    try_lift: |s| Ok(Uuid::parse_str(&s)?),
});

impl From<usize> for Number {
    fn from(v: usize) -> Self {
        Number(v as i64)
    }
}

impl Number {
    pub fn index(self) -> usize {
        self.0.max(0) as usize
    }
}

// MARK: Library

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct BookMeta {
    pub id: Uuid,
    pub title: String,
    pub chapters_count: Number,
    pub paragraphs_count: Number,
    pub translation_ratio: f64,
    pub path: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ChapterMeta {
    pub id: Number,
    pub title: String,
    pub translation_ratio: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct BookReadingState {
    pub chapter_id: Number,
    pub paragraph_id: Number,
    pub page_offset: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct BookSummaryStatus {
    pub total_chapters: Number,
    pub generated: Vec<bool>,
    #[serde(default)]
    pub actively_generating: Option<Number>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct ImportProgress {
    pub done: Number,
    pub total: Number,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct EpubChapterSummary {
    pub title: String,
    pub paragraph_count: Number,
}

// MARK: Reader

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ParagraphTexts {
    /// ISO 639-3 code of the book's language.
    pub language: String,
    pub paragraphs: Vec<ParagraphText>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ParagraphText {
    pub id: Number,
    pub text: String,
    pub runs: Vec<MarkRun>,
    pub words: Vec<WordRange>,
    pub translated: bool,
}

/// UTF-16 range with uniform emphasis; `marks` bit 0 is emphasis, bit 1 strong.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct MarkRun {
    pub start: Number,
    pub len: Number,
    pub marks: Number,
}

/// A word's UTF-16 range in its paragraph's text.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct WordRange {
    pub start: Number,
    pub len: Number,
    pub sentence: Number,
    pub word: Number,
    pub flat_index: Number,
    pub translation: Option<String>,
}

impl From<app_core::app::library_view::text::ParagraphTexts> for ParagraphTexts {
    fn from(t: app_core::app::library_view::text::ParagraphTexts) -> Self {
        ParagraphTexts {
            language: t.language,
            paragraphs: t
                .paragraphs
                .into_iter()
                .map(|p| ParagraphText {
                    id: p.id.into(),
                    text: p.text,
                    runs: p
                        .runs
                        .into_iter()
                        .map(|r| MarkRun {
                            start: r.start.into(),
                            len: r.len.into(),
                            marks: Number(r.marks as i64),
                        })
                        .collect(),
                    words: p
                        .words
                        .into_iter()
                        .map(|w| WordRange {
                            start: w.start.into(),
                            len: w.len.into(),
                            sentence: w.sentence.into(),
                            word: w.word.into(),
                            flat_index: w.flat_index.into(),
                            translation: w.translation,
                        })
                        .collect(),
                    translated: p.translated,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct Grammar {
    pub original_initial_form: String,
    pub target_initial_form: String,
    pub part_of_speech: String,
    pub plurality: Option<String>,
    pub person: Option<String>,
    pub tense: Option<String>,
    #[serde(rename = "case")]
    pub grammatical_case: Option<String>,
    pub other: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct WordInfo {
    pub original: String,
    pub note: String,
    pub is_punctuation: bool,
    pub contextual_translations: Vec<String>,
    pub grammar: Grammar,
    pub full_sentence_translation: String,
    pub translation_model: String,
    pub source_language: String,
    pub can_create_card: bool,
    pub has_card: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "camelCase")]
pub enum CardResult {
    Created,
    AlreadyExists,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct SystemDefinition {
    pub definition: String,
    pub transcription: Option<String>,
}

// MARK: Translation activity and its events

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTranslation {
    pub book_id: Uuid,
    pub paragraph_id: Number,
    pub request_id: Number,
    pub progress_chars: Number,
    pub expected_chars: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct TranslationStarted {
    pub book_id: Uuid,
    pub paragraph_id: Number,
    pub request_id: Number,
    pub expected_chars: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct TranslationProgress {
    pub book_id: Uuid,
    pub paragraph_id: Number,
    pub request_id: Number,
    pub progress_chars: Number,
    pub expected_chars: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct TranslationFinished {
    pub book_id: Uuid,
    pub paragraph_id: Number,
    pub request_id: Number,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ParagraphUpdated {
    pub book_id: Uuid,
    pub paragraph_id: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct SummaryProgress {
    pub book_id: Uuid,
    pub current: Number,
    pub total: Number,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

// MARK: Config

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum TranslationProvider {
    Google,
    Openai,
    Deepseek,
    Zai,
    Openrouter,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub provider: Option<TranslationProvider>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMeta {
    pub id: TranslationProvider,
    pub name: String,
    pub default_model: String,
    pub api_key_field: String,
    #[serde(default)]
    pub model_selection: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct LanguageInfo {
    pub id: String,
    pub name: String,
    pub local_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub target_language_id: String,
    pub translation_provider: TranslationProvider,
    pub gemini_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub deepseek_api_key: Option<String>,
    pub zai_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub model: String,
    pub library_path: Option<String>,
    pub spotify_client_id: Option<String>,
    pub spotify_preload_count: Number,
    pub spotify_show_next_track: bool,
    pub anki_endpoint: Option<String>,
    pub anki_api_key: Option<String>,
    pub anki_sync_enabled: bool,
    pub sync_enabled: bool,
    pub sync_device_name: Option<String>,
    pub translation_concurrency: Number,
}

// MARK: Sync

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    Disabled,
    Starting,
    Online,
    Syncing,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct StoreIssues {
    pub newer_version: Number,
    pub unreadable: Number,
    pub strangers: Number,
    #[serde(default)]
    pub stalled: bool,
    #[serde(default)]
    pub shared_identity: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub state: SyncState,
    #[serde(default)]
    pub device_id: Option<String>,
    pub device_count: Number,
    pub connected_count: Number,
    #[serde(default)]
    pub completion: Option<f64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub store_issues: Option<StoreIssues>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct ThisDevice {
    pub device_id: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEntry {
    pub device_id: String,
    pub name: String,
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct PendingDevice {
    pub device_id: String,
    pub name: String,
}

// MARK: Anki

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum AnkiSyncState {
    Idle,
    Syncing,
    Ok,
    Err,
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AnkiSyncReport {
    pub total_cards: Number,
    pub attempted: Number,
    pub succeeded: Number,
    pub failed: Number,
    pub persistent_failures: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AnkiSyncStatus {
    pub state: AnkiSyncState,
    #[serde(default)]
    pub last_finished_at_ms: Option<f64>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_report: Option<AnkiSyncReport>,
}

// MARK: Lyrics / Spotify

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    Playing,
    Paused,
    Stopped,
    NotRunning,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub state: PlayerState,
    pub track_id: Option<String>,
    pub name: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub position_ms: Option<Number>,
    pub duration_ms: Option<Number>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct LyricsLine {
    pub time_ms: Option<Number>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Lyrics {
    pub track_id: String,
    pub lines: Vec<LyricsLine>,
    pub synced: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct Gloss {
    pub fragment: String,
    pub gloss: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct LyricsLineTranslation {
    pub translation: String,
    pub glosses: Vec<Gloss>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct LyricsTranslation {
    pub track_id: String,
    pub target_lang: String,
    pub model: String,
    pub lines: Vec<LyricsLineTranslation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct TrackLyricsState {
    pub lyrics: Option<Lyrics>,
    pub translation: Option<LyricsTranslation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct LyricsResolved {
    pub track_id: String,
    pub lyrics: Option<Lyrics>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct LyricsTranslationProgress {
    pub track_id: String,
    pub bytes: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct LyricsTranslationDone {
    pub track_id: String,
    pub translation: LyricsTranslation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct LyricsTranslationError {
    pub track_id: String,
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct TrackMeta {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_ms: Number,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct QueueSnapshot {
    pub context_type: Option<String>,
    pub currently_playing_id: Option<String>,
    pub upcoming: Vec<TrackMeta>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct SpotifyWebStatus {
    pub connected: bool,
    pub premium_required: bool,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct SpotifyCdpStatus {
    pub available: bool,
    pub port: Number,
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct SpotifyLoginAgentStatus {
    pub installed: bool,
    pub path: String,
}

// MARK: Events

/// Everything the core pushes to the app.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum CoreEvent {
    ConfigUpdated,
    LibraryUpdated,
    LibraryImportProgress { progress: ImportProgress },
    BookUpdated { book_id: Uuid },
    BookDeleted { book_id: Uuid },
    ParagraphUpdated { event: ParagraphUpdated },
    CardsUpdated,
    TranslationStarted { event: TranslationStarted },
    TranslationProgress { event: TranslationProgress },
    TranslationFinished { event: TranslationFinished },
    SummaryGenerationProgress { event: SummaryProgress },
    AnkiSyncStatusChanged,
    SyncStatusChanged,
    SpotifyState { now_playing: NowPlaying },
    SpotifyQueue { snapshot: QueueSnapshot },
    LyricsResolved { event: LyricsResolved },
    LyricsTranslationProgress { event: LyricsTranslationProgress },
    LyricsTranslationDone { event: LyricsTranslationDone },
    LyricsTranslationError { event: LyricsTranslationError },
}

impl CoreEvent {
    /// The core's (name, JSON payload) form, as every shell receives it.
    pub fn from_wire(name: &str, payload: serde_json::Value) -> Result<CoreEvent, String> {
        fn de<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, String> {
            serde_json::from_value(v).map_err(|e| e.to_string())
        }
        Ok(match name {
            "config_updated" => CoreEvent::ConfigUpdated,
            "library_updated" => CoreEvent::LibraryUpdated,
            "library_import_progress" => CoreEvent::LibraryImportProgress {
                progress: de(payload)?,
            },
            "book_updated" => CoreEvent::BookUpdated {
                book_id: de(payload)?,
            },
            "book_deleted" => CoreEvent::BookDeleted {
                book_id: de(payload)?,
            },
            "paragraph_updated" => CoreEvent::ParagraphUpdated {
                event: de(payload)?,
            },
            "cards_updated" => CoreEvent::CardsUpdated,
            "paragraph_translation_started" => CoreEvent::TranslationStarted {
                event: de(payload)?,
            },
            "paragraph_translation_progress" => CoreEvent::TranslationProgress {
                event: de(payload)?,
            },
            "paragraph_translation_finished" => CoreEvent::TranslationFinished {
                event: de(payload)?,
            },
            "summary_generation_progress" => CoreEvent::SummaryGenerationProgress {
                event: de(payload)?,
            },
            "anki_sync_status_changed" => CoreEvent::AnkiSyncStatusChanged,
            "sync_status_changed" => CoreEvent::SyncStatusChanged,
            "spotify_state" => CoreEvent::SpotifyState {
                now_playing: de(payload)?,
            },
            "spotify_queue" => CoreEvent::SpotifyQueue {
                snapshot: de(payload)?,
            },
            "lyrics_resolved" => CoreEvent::LyricsResolved {
                event: de(payload)?,
            },
            "lyrics_translation_progress" => CoreEvent::LyricsTranslationProgress {
                event: de(payload)?,
            },
            "lyrics_translation_done" => CoreEvent::LyricsTranslationDone {
                event: de(payload)?,
            },
            "lyrics_translation_error" => CoreEvent::LyricsTranslationError {
                event: de(payload)?,
            },
            other => return Err(format!("unknown event {other}")),
        })
    }
}
