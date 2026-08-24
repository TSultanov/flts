pub mod cache;
pub mod lrclib;
pub mod select;
pub mod spotify;
pub mod translation;

use isolang::Language;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsLine {
    /// Start time in ms; `None` for unsynced lyrics and stanza breaks.
    pub time_ms: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Gloss {
    pub fragment: String,
    pub gloss: String,
    /// Register, idiom, or cultural context; empty when not applicable.
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
    /// Whether lines carry `time_ms`.
    pub synced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LyricsTranslation {
    pub track_id: String,
    #[serde(with = "lang_639_3")]
    pub target_lang: Language,
    pub model: String,
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
