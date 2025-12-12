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

fn model_provider(model: TranslationModel) -> Option<TranslationProvider> {
    match model {
        TranslationModel::Gemini25Flash
        | TranslationModel::Gemini25Pro
        | TranslationModel::Gemini25FlashLight => Some(TranslationProvider::Google),

        TranslationModel::OpenAIGpt52
        | TranslationModel::OpenAIGpt52Pro
        | TranslationModel::OpenAIGpt5Mini
        | TranslationModel::OpenAIGpt5Nano => Some(TranslationProvider::Openai),

        TranslationModel::Unknown => None,
    }
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
        TranslationModel::Unknown => "Not set",
    }
}

impl From<TranslationModel> for Model {
    fn from(value: TranslationModel) -> Self {
        Self {
            id: value as i32,
            name: model_pretty_name(value),
            provider: model_provider(value),
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
        .filter(|l| l.id == "rus" || l.id == "eng" || l.id == "kat")
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
