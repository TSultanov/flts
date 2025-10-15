use library::translator::TranslationModel;
use serde::Serialize;
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
