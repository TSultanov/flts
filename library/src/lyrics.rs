pub mod cache;
pub mod lrclib;
pub mod translator;

use isolang::Language;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsLine {
    /// Start time of this line in milliseconds, when known (LRClib synced lyrics).
    /// `None` for unsynced lyrics or stanza-break lines.
    pub time_ms: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gloss {
    pub fragment: String,
    pub gloss: String,
    /// Short clause about register, idiom, or cultural context. Empty string when not applicable.
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsLineTranslation {
    pub translation: String,
    pub glosses: Vec<Gloss>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lyrics {
    pub track_id: String,
    pub lines: Vec<LyricsLine>,
    /// True if `time_ms` is populated on lines (LRClib `syncedLyrics`),
    /// false for `plainLyrics`.
    pub synced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsTranslation {
    pub track_id: String,
    #[serde(with = "lang_639_3")]
    pub target_lang: Language,
    /// `TranslationModel as usize` — matches the wire format used in `Config`.
    pub model: usize,
    pub lines: Vec<LyricsLineTranslation>,
}

mod lang_639_3 {
    use isolang::Language;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(lang: &Language, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(lang.to_639_3())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Language, D::Error> {
        let code = String::deserialize(d)?;
        Language::from_639_3(&code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown ISO-639-3 code: {code}")))
    }
}
