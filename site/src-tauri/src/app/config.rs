use std::{fs::File, path::Path};

use library::translator::{TranslationModel, TranslationProvider};
use log::{info, warn};
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
            default_model_id: TranslationModel::Gemini25Flash as i32,
            api_key_field: "geminiApiKey",
        },
        ProviderMeta {
            id: TranslationProvider::Openai,
            name: TranslationProvider::Openai.display_name(),
            default_model_id: TranslationModel::OpenAIGpt5Mini as i32,
            api_key_field: "openaiApiKey",
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
    pub model: usize,
    #[serde(rename = "libraryPath")]
    pub library_path: Option<String>,
    /// Spotify Developer Dashboard client_id. Required for the Web API to work;
    /// users register their own dev app (PKCE flow, no client secret needed)
    /// and paste the id here. Empty/missing = Web API integration disabled.
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
}

fn default_preload_count() -> u32 {
    1
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
            model: TranslationModel::Gemini25Flash as usize,
            library_path: None,
            spotify_client_id: None,
            spotify_preload_count: default_preload_count(),
            spotify_show_next_track: default_show_next_track(),
            anki_endpoint: Some("http://127.0.0.1:8765".to_owned()),
            anki_api_key: None,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        Ok(match serde_json::from_reader::<_, Self>(file) {
            Ok(json) => json,
            Err(err) => {
                warn!("Failed to parse config: {}. Loading default values.", err);
                Self::default()
            }
        })
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        info!("Open {path:?}");
        let file = OpenOptions::new()
            .truncate(true)
            .write(true)
            .create(true)
            .open(path)?;
        info!("File opened");
        serde_json::to_writer(file, self)?;
        info!("File written");
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
        // Simulate a config persisted before the Anki fields existed.
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
}
