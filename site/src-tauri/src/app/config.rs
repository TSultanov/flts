use std::{fs::File, path::Path, str::FromStr};

use library::translator::{TranslationModel, TranslationProvider};
use log::warn;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use strum::IntoEnumIterator;

#[derive(Serialize)]
pub struct Model {
    id: i32,
    name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<TranslationProvider>,
}

#[derive(Serialize)]
pub struct ProviderMeta {
    pub id: TranslationProvider,
    pub name: &'static str,
    #[serde(rename = "defaultModelId")]
    pub default_model_id: i32,
    #[serde(rename = "apiKeyField")]
    pub api_key_field: &'static str,
}

fn model_pretty_name(model: TranslationModel) -> &'static str {
    match model {
        TranslationModel::Gemini25FlashLight => "Gemini 2.5 Flash Light",
        TranslationModel::Gemini25Flash => "Gemini 2.5 Flash",
        TranslationModel::Gemini25Pro => "Gemini 2.5 Pro",
        TranslationModel::OpenAIGpt52 => "OpenAI GPT-5.2",
        TranslationModel::OpenAIGpt52Pro => "OpenAI GPT-5.2 Pro",
        TranslationModel::OpenAIGpt5Mini => "OpenAI GPT-5 mini",
        TranslationModel::OpenAIGpt5Nano => "OpenAI GPT-5 nano",
        TranslationModel::Gemini3Pro => "Gemini 3 Pro (Preview)",
        TranslationModel::Gemini3Flash => "Gemini 3 Flash (Preview)",
        TranslationModel::OpenAIGpt54 => "OpenAI GPT-5.4",
        TranslationModel::OpenAIGpt54Mini => "OpenAI GPT-5.4 mini",
        TranslationModel::Gemini31Pro => "Gemini 3.1 Pro (Preview)",
        TranslationModel::Gemini31FlashLite => "Gemini 3.1 Flash-Lite (Preview)",
        TranslationModel::Gemini35Flash => "Gemini 3.5 Flash",
        TranslationModel::Gemini36Flash => "Gemini 3.6 Flash",
        TranslationModel::Gemini37Flash => "Gemini 3.7 Flash",
        TranslationModel::DeepSeekV4Flash => "DeepSeek V4 Flash",
        TranslationModel::DeepSeekV4Pro => "DeepSeek V4 Pro",
        TranslationModel::ZaiGlm52 => "z.AI GLM-5.2",
        TranslationModel::Unknown => "Not set",
    }
}

impl From<TranslationModel> for Model {
    fn from(value: TranslationModel) -> Self {
        Self {
            id: value as i32,
            name: model_pretty_name(value),
            provider: value.provider(),
        }
    }
}

#[tauri::command]
pub fn get_models() -> Vec<Model> {
    TranslationModel::iter().map(|m| m.into()).collect()
}

#[tauri::command]
pub fn get_translation_providers() -> Vec<ProviderMeta> {
    vec![
        ProviderMeta {
            id: TranslationProvider::Google,
            name: TranslationProvider::Google.display_name(),
            default_model_id: TranslationModel::Gemini37Flash as i32,
            api_key_field: "geminiApiKey",
        },
        ProviderMeta {
            id: TranslationProvider::Openai,
            name: TranslationProvider::Openai.display_name(),
            default_model_id: TranslationModel::OpenAIGpt5Mini as i32,
            api_key_field: "openaiApiKey",
        },
        ProviderMeta {
            id: TranslationProvider::Deepseek,
            name: TranslationProvider::Deepseek.display_name(),
            default_model_id: TranslationModel::DeepSeekV4Flash as i32,
            api_key_field: "deepseekApiKey",
        },
        ProviderMeta {
            id: TranslationProvider::Zai,
            name: TranslationProvider::Zai.display_name(),
            default_model_id: TranslationModel::ZaiGlm52 as i32,
            api_key_field: "zaiApiKey",
        },
    ]
}

#[derive(Serialize)]
pub struct Language {
    pub id: &'static str,
    pub name: &'static str,
    #[serde(rename = "localName")]
    pub local_name: Option<&'static str>,
}

#[tauri::command]
pub fn get_languages() -> Vec<Language> {
    let mut languages: Vec<_> = isolang::languages()
        .map(|l| Language {
            id: l.to_639_3(),
            name: l.to_name(),
            local_name: l.to_autonym(),
        })
        .filter(|l| {
            l.id == "rus"
                || l.id == "eng"
                || l.id == "kat"
                || l.id == "deu"
                || l.id == "zho"
                || l.id == "spa"
        })
        .collect();
    languages.sort_by_key(|l| l.name);
    languages
}

pub fn parse_language_code(code: Option<&str>) -> Option<String> {
    let raw = code?.trim();
    if raw.is_empty() {
        return None;
    }
    let primary = raw
        .split(['-', '_'])
        .next()
        .unwrap_or(raw)
        .to_ascii_lowercase();
    isolang::Language::from_str(&primary)
        .ok()
        .map(|language| language.to_639_3().to_string())
}

#[tauri::command]
pub fn parse_language_id(code: Option<String>) -> Option<String> {
    parse_language_code(code.as_deref())
}

#[derive(Clone)]
pub struct ApiKeys {
    pub gemini: Option<String>,
    pub openai: Option<String>,
    pub deepseek: Option<String>,
    pub zai: Option<String>,
}

impl ApiKeys {
    pub fn for_provider(&self, provider: TranslationProvider) -> Option<&str> {
        match provider {
            TranslationProvider::Google => self.gemini.as_deref(),
            TranslationProvider::Openai => self.openai.as_deref(),
            TranslationProvider::Deepseek => self.deepseek.as_deref(),
            TranslationProvider::Zai => self.zai.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(rename = "targetLanguageId")]
    pub target_language_id: String,
    #[serde(rename = "translationProvider")]
    #[serde(default)]
    pub translation_provider: TranslationProvider,
    #[serde(rename = "geminiApiKey")]
    pub gemini_api_key: Option<String>,
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
    #[serde(rename = "deepseekApiKey", default)]
    pub deepseek_api_key: Option<String>,
    #[serde(rename = "zaiApiKey", default)]
    pub zai_api_key: Option<String>,
    pub model: TranslationModel,
    /// Migration-read-only: read once to relocate a user-picked library into
    /// `resolve_library_root`, then cleared. Never write it.
    #[serde(rename = "libraryPath", default)]
    pub library_path: Option<String>,
    /// Client id from the user's own Spotify dev app (PKCE, no secret).
    /// Empty/missing disables the Web API integration.
    #[serde(rename = "spotifyClientId", default)]
    pub spotify_client_id: Option<String>,
    /// How many upcoming tracks to preload lyrics+translation for. 0 disables.
    #[serde(rename = "spotifyPreloadCount", default = "default_preload_count")]
    pub spotify_preload_count: u32,
    /// Show "Up next" in the now-playing card. Doesn't affect preloading.
    #[serde(rename = "spotifyShowNextTrack", default = "default_show_next_track")]
    pub spotify_show_next_track: bool,
    /// AnkiConnect HTTP endpoint. Default `http://127.0.0.1:8765`.
    #[serde(rename = "ankiEndpoint", default)]
    pub anki_endpoint: Option<String>,
    /// Optional AnkiConnect API key. Unset for default Anki desktop installs.
    #[serde(rename = "ankiApiKey", default)]
    pub anki_api_key: Option<String>,
    /// Opt-in from the sync UI; enabling it starts the embedded engine.
    #[serde(rename = "syncEnabled", default)]
    pub sync_enabled: bool,
    /// Display name in the sync roster; `None` falls back to the hostname.
    #[serde(rename = "syncDeviceName", default)]
    pub sync_device_name: Option<String>,
    /// Max paragraph translations run concurrently. 1 = serial.
    #[serde(
        rename = "translationConcurrency",
        default = "default_translation_concurrency"
    )]
    pub translation_concurrency: u32,
    /// Hides familiarity underlines and overlays until the reader taps a word.
    #[serde(rename = "tapToRevealTranslations", default)]
    pub tap_to_reveal_translations: bool,
}

fn default_preload_count() -> u32 {
    1
}

fn default_translation_concurrency() -> u32 {
    8
}

fn default_show_next_track() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target_language_id: "eng".to_owned(),
            translation_provider: TranslationProvider::Google,
            gemini_api_key: None,
            openai_api_key: None,
            deepseek_api_key: None,
            zai_api_key: None,
            model: TranslationModel::Gemini37Flash,
            library_path: None,
            spotify_client_id: None,
            spotify_preload_count: default_preload_count(),
            spotify_show_next_track: default_show_next_track(),
            anki_endpoint: Some("http://127.0.0.1:8765".to_owned()),
            anki_api_key: None,
            sync_enabled: false,
            sync_device_name: None,
            translation_concurrency: default_translation_concurrency(),
            tap_to_reveal_translations: false,
        }
    }
}

impl Config {
    pub fn api_keys(&self) -> ApiKeys {
        ApiKeys {
            gemini: self.gemini_api_key.clone(),
            openai: self.openai_api_key.clone(),
            deepseek: self.deepseek_api_key.clone(),
            zai: self.zai_api_key.clone(),
        }
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        Ok(match serde_json::from_reader::<_, Self>(file) {
            Ok(json) => json,
            Err(err) => {
                warn!("Failed to parse config: {}. Loading default values.", err);
                // Preserve the unparseable file; the next save would overwrite it.
                let corrupt = path.with_extension("json.corrupt");
                if let Err(copy_err) = std::fs::rename(path, &corrupt) {
                    warn!("Could not preserve corrupt config at {corrupt:?}: {copy_err}");
                }
                Self::default()
            }
        })
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        // Atomic (temp + fsync + rename): mobile OSes routinely kill a
        // backgrounded app mid-write, and a partial config.json would make
        // load() reset every setting. The sequence number keeps unserialized
        // concurrent saves off a shared temp path.
        static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = dir.join(format!("config.json~{}-{}.tmp", std::process::id(), seq));

        let write_result = (|| -> anyhow::Result<()> {
            let file = OpenOptions::new()
                .truncate(true)
                .write(true)
                .create(true)
                .open(&tmp)?;
            serde_json::to_writer(&file, self)?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }

        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_has_localhost_anki_endpoint() {
        let c = Config::default();
        assert_eq!(c.anki_endpoint.as_deref(), Some("http://127.0.0.1:8765"));
        assert!(c.anki_api_key.is_none());
    }

    #[test]
    fn config_round_trips_through_serde_with_anki_fields() {
        let original = Config {
            anki_endpoint: Some("http://anki.example.com:9999".into()),
            anki_api_key: Some("secret-key".into()),
            ..Config::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"ankiEndpoint\""));
        assert!(json.contains("\"ankiApiKey\""));
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.anki_endpoint, original.anki_endpoint);
        assert_eq!(parsed.anki_api_key, original.anki_api_key);
    }

    #[test]
    fn config_loads_legacy_file_without_anki_fields() {
        let legacy = serde_json::json!({
            "targetLanguageId": "eng",
            "translationProvider": "google",
            "geminiApiKey": null,
            "openaiApiKey": null,
            "model": 0,
            "libraryPath": null,
        });
        let parsed: Config = serde_json::from_value(legacy).unwrap();
        assert!(
            parsed.anki_endpoint.is_none(),
            "legacy config (pre-Anki) must NOT spontaneously populate endpoint"
        );
        assert!(parsed.anki_api_key.is_none());
    }

    #[test]
    fn config_default_translation_concurrency_is_eight() {
        assert_eq!(Config::default().translation_concurrency, 8);
    }

    #[test]
    fn config_loads_legacy_file_without_translation_concurrency() {
        let legacy = serde_json::json!({
            "targetLanguageId": "eng",
            "translationProvider": "google",
            "geminiApiKey": null,
            "openaiApiKey": null,
            "model": 0,
            "libraryPath": null,
        });
        let parsed: Config = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.translation_concurrency, 8);
    }

    #[test]
    fn config_default_tap_to_reveal_is_false() {
        assert!(!Config::default().tap_to_reveal_translations);
    }

    #[test]
    fn config_round_trips_tap_to_reveal_translations() {
        let original = Config {
            tap_to_reveal_translations: true,
            ..Config::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"tapToRevealTranslations\":true"));
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert!(parsed.tap_to_reveal_translations);
    }

    #[test]
    fn config_loads_legacy_file_without_tap_to_reveal() {
        let legacy = serde_json::json!({
            "targetLanguageId": "eng",
            "translationProvider": "google",
            "geminiApiKey": null,
            "openaiApiKey": null,
            "model": 0,
            "libraryPath": null,
        });
        let parsed: Config = serde_json::from_value(legacy).unwrap();
        assert!(
            !parsed.tap_to_reveal_translations,
            "legacy config must keep today's auto-underline / auto-overlay behaviour"
        );
    }

    #[test]
    fn parse_language_code_maps_iso_codes_and_bcp47_to_639_3() {
        let cases: &[(&str, Option<&str>)] = &[
            ("en", Some("eng")),
            ("eng", Some("eng")),
            ("es", Some("spa")),
            ("spa", Some("spa")),
            ("de", Some("deu")),
            ("deu", Some("deu")),
            ("ru", Some("rus")),
            ("zh", Some("zho")),
            ("ka", Some("kat")),
            ("fr", Some("fra")),
            ("nl", Some("nld")),
            ("und", Some("und")),
            ("en-US", Some("eng")),
            ("zh-Hans-CN", Some("zho")),
            ("de_DE", Some("deu")),
            ("spa-MX", Some("spa")),
            ("EN", Some("eng")),
            ("  es  ", Some("spa")),
            ("", None),
            ("   ", None),
            ("???", None),
            // Primary subtag "not" is ISO 639-3 Nomatsiguenga; use a non-code primary.
            ("xx-not-a-language", None),
            ("ger", None),
            ("chi", None),
        ];
        for (input, want) in cases {
            assert_eq!(
                parse_language_code(Some(input)).as_deref(),
                *want,
                "input {input:?}"
            );
        }
        assert_eq!(parse_language_code(None), None);
    }
}
