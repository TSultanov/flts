use std::{fs::File, path::Path, str::FromStr, sync::Arc};

use library::translator::{
    TranslationProvider,
    catalog::{
        FALLBACK_DEEPSEEK, FALLBACK_GOOGLE, FALLBACK_OPENAI, FALLBACK_OPENROUTER, FALLBACK_ZAI,
        ListedModel, api_id_from_legacy, effective_model_id, list_base_url_from_env,
    },
};
use log::warn;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;

use super::AppState;

#[derive(Serialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<TranslationProvider>,
}

impl From<ListedModel> for Model {
    fn from(value: ListedModel) -> Self {
        Self {
            id: value.id,
            name: value.name,
            provider: Some(value.provider),
        }
    }
}

#[derive(Serialize)]
pub struct ProviderMeta {
    pub id: TranslationProvider,
    pub name: &'static str,
    #[serde(rename = "defaultModel")]
    pub default_model: String,
    #[serde(rename = "apiKeyField")]
    pub api_key_field: &'static str,
    #[serde(rename = "modelSelection", skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<&'static str>,
}

#[tauri::command]
pub async fn get_models(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<Model>, String> {
    let config = state.config.borrow().clone();
    let keys = config.api_keys();
    let catalog = state.model_catalog.clone();

    let google_base = list_base_url_from_env(TranslationProvider::Google);
    let openai_base = list_base_url_from_env(TranslationProvider::Openai);
    let deepseek_base = list_base_url_from_env(TranslationProvider::Deepseek);
    let zai_base = list_base_url_from_env(TranslationProvider::Zai);
    let openrouter_base = list_base_url_from_env(TranslationProvider::Openrouter);

    let (google, openai, deepseek, zai, openrouter) = tokio::join!(
        catalog.models_for(
            TranslationProvider::Google,
            keys.for_provider(TranslationProvider::Google),
            &google_base
        ),
        catalog.models_for(
            TranslationProvider::Openai,
            keys.for_provider(TranslationProvider::Openai),
            &openai_base
        ),
        catalog.models_for(
            TranslationProvider::Deepseek,
            keys.for_provider(TranslationProvider::Deepseek),
            &deepseek_base
        ),
        catalog.models_for(
            TranslationProvider::Zai,
            keys.for_provider(TranslationProvider::Zai),
            &zai_base
        ),
        catalog.models_for(
            TranslationProvider::Openrouter,
            keys.for_provider(TranslationProvider::Openrouter),
            &openrouter_base
        ),
    );

    Ok(google
        .into_iter()
        .chain(openai)
        .chain(deepseek)
        .chain(zai)
        .chain(openrouter)
        .map(Model::from)
        .collect())
}

#[tauri::command]
pub fn get_translation_providers() -> Vec<ProviderMeta> {
    vec![
        ProviderMeta {
            id: TranslationProvider::Google,
            name: TranslationProvider::Google.display_name(),
            default_model: FALLBACK_GOOGLE.to_string(),
            api_key_field: "geminiApiKey",
            model_selection: None,
        },
        ProviderMeta {
            id: TranslationProvider::Openai,
            name: TranslationProvider::Openai.display_name(),
            default_model: FALLBACK_OPENAI.to_string(),
            api_key_field: "openaiApiKey",
            model_selection: None,
        },
        ProviderMeta {
            id: TranslationProvider::Deepseek,
            name: TranslationProvider::Deepseek.display_name(),
            default_model: FALLBACK_DEEPSEEK.to_string(),
            api_key_field: "deepseekApiKey",
            model_selection: None,
        },
        ProviderMeta {
            id: TranslationProvider::Zai,
            name: TranslationProvider::Zai.display_name(),
            default_model: FALLBACK_ZAI.to_string(),
            api_key_field: "zaiApiKey",
            model_selection: None,
        },
        ProviderMeta {
            id: TranslationProvider::Openrouter,
            name: TranslationProvider::Openrouter.display_name(),
            default_model: FALLBACK_OPENROUTER.to_string(),
            api_key_field: "openrouterApiKey",
            model_selection: Some("family"),
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
    pub openrouter: Option<String>,
}

impl ApiKeys {
    pub fn for_provider(&self, provider: TranslationProvider) -> Option<&str> {
        match provider {
            TranslationProvider::Google => self.gemini.as_deref(),
            TranslationProvider::Openai => self.openai.as_deref(),
            TranslationProvider::Deepseek => self.deepseek.as_deref(),
            TranslationProvider::Zai => self.zai.as_deref(),
            TranslationProvider::Openrouter => self.openrouter.as_deref(),
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
    #[serde(rename = "openrouterApiKey", default)]
    pub openrouter_api_key: Option<String>,
    #[serde(deserialize_with = "deserialize_model")]
    pub model: String,
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
    /// Automatic AnkiConnect sync. Defaults on: configs written before this
    /// field existed synced, and must keep syncing.
    #[serde(rename = "ankiSyncEnabled", default = "default_anki_sync_enabled")]
    pub anki_sync_enabled: bool,
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

fn default_anki_sync_enabled() -> bool {
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
            openrouter_api_key: None,
            model: FALLBACK_GOOGLE.to_string(),
            library_path: None,
            spotify_client_id: None,
            spotify_preload_count: default_preload_count(),
            spotify_show_next_track: default_show_next_track(),
            anki_endpoint: Some("http://127.0.0.1:8765".to_owned()),
            anki_api_key: None,
            anki_sync_enabled: default_anki_sync_enabled(),
            sync_enabled: false,
            sync_device_name: None,
            translation_concurrency: default_translation_concurrency(),
            tap_to_reveal_translations: false,
        }
    }
}

fn deserialize_model<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct ModelVisitor;
    impl<'de> serde::de::Visitor<'de> for ModelVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a model id string or legacy numeric id")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_string<E: serde::de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<String, E> {
            Ok(api_id_from_legacy(v))
        }

        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<String, E> {
            if v < 0 {
                Ok(String::new())
            } else {
                Ok(api_id_from_legacy(v as u64))
            }
        }
    }
    deserializer.deserialize_any(ModelVisitor)
}

impl Config {
    pub fn resolved_model_id(&self, model: &str) -> String {
        let chosen = if model.trim().is_empty() {
            self.model.as_str()
        } else {
            model
        };
        effective_model_id(self.translation_provider, chosen)
    }

    pub fn api_keys(&self) -> ApiKeys {
        ApiKeys {
            gemini: self.gemini_api_key.clone(),
            openai: self.openai_api_key.clone(),
            deepseek: self.deepseek_api_key.clone(),
            zai: self.zai_api_key.clone(),
            openrouter: self.openrouter_api_key.clone(),
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
        assert_eq!(c.model, FALLBACK_GOOGLE);
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
    fn config_default_anki_sync_enabled_is_true() {
        assert!(Config::default().anki_sync_enabled);
    }

    #[test]
    fn config_round_trips_anki_sync_enabled() {
        let original = Config {
            anki_sync_enabled: false,
            ..Config::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(json.contains("\"ankiSyncEnabled\":false"));
        let parsed: Config = serde_json::from_str(&json).unwrap();
        assert!(!parsed.anki_sync_enabled);
    }

    #[test]
    fn config_loads_legacy_file_without_anki_sync_enabled() {
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
            parsed.anki_sync_enabled,
            "existing installs synced to Anki and must keep doing so"
        );
    }

    #[test]
    fn config_model_number_migrates_without_corrupt_path() {
        let legacy = serde_json::json!({
            "targetLanguageId": "eng",
            "translationProvider": "google",
            "model": 1
        });
        let parsed: Config = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.model, "models/gemini-2.5-flash");
    }

    #[test]
    fn config_model_string_passthrough() {
        let v = serde_json::json!({
            "targetLanguageId": "eng",
            "translationProvider": "openai",
            "model": "gpt-9-ultra"
        });
        let parsed: Config = serde_json::from_value(v).unwrap();
        assert_eq!(parsed.model, "gpt-9-ultra");
        let dumped = serde_json::to_value(&parsed).unwrap();
        assert_eq!(dumped["model"], "gpt-9-ultra");
    }

    #[test]
    fn config_model_zero_and_unknown_become_empty() {
        for n in [0, 99] {
            let v = serde_json::json!({
                "targetLanguageId": "eng",
                "translationProvider": "google",
                "model": n
            });
            let parsed: Config = serde_json::from_value(v).unwrap();
            assert_eq!(parsed.model, "");
        }
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
