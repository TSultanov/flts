use std::{fs::{File, OpenOptions}, path::Path};

use library::translator::TranslationModel;
use log::warn;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;

#[derive(Serialize)]
pub struct Model {
    id: i32,
    name: &'static str,
}

fn model_pretty_name(model: TranslationModel) -> &'static str {
    match model {
        TranslationModel::GeminiFlash => "Gemini 2.5 Flash",
        TranslationModel::GeminiPro => "Gemini 2.5 Pro",
    }
}

impl From<TranslationModel> for Model {
    fn from(value: TranslationModel) -> Self {
        Self {
            id: value as i32,
            name: model_pretty_name(value),
        }
    }
}

#[tauri::command]
pub fn get_models() -> Vec<Model> {
    TranslationModel::iter().map(|m| m.into()).collect()
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
        .collect();
    languages.sort_by_key(|l| l.name);
    languages
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(rename = "targetLanguageId")]
    pub target_language_id: Option<String>,
    #[serde(rename = "geminiApiKey")]
    pub gemini_api_key: Option<String>,
    pub model: i32,
    #[serde(rename = "libraryPath")]
    pub library_path: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            target_language_id: None,
            gemini_api_key: None,
            model: TranslationModel::GeminiFlash as i32,
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
            },
        })
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let file = OpenOptions::new().truncate(true).write(true).create(true).open(path)?;
        serde_json::to_writer(file, self)?;
        Ok(())
    }
}
