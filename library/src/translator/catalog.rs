use super::TranslationProvider;

pub const FALLBACK_GOOGLE: &str = "models/gemini-3.7-flash";
pub const FALLBACK_OPENAI: &str = "gpt-5-mini";
pub const FALLBACK_DEEPSEEK: &str = "deepseek-v4-flash";
pub const FALLBACK_ZAI: &str = "glm-5.2";

const LEGACY_API_IDS: &[&str] = &[
    "", // 0 / unknown
    "models/gemini-2.5-flash",
    "models/gemini-2.5-pro",
    "models/gemini-2.5-flash-lite",
    "gpt-5-mini",
    "gpt-5.2",
    "gpt-5.2-pro",
    "gpt-5-nano",
    "models/gemini-3-pro-preview",
    "models/gemini-3-flash-preview",
    "gpt-5.4",
    "gpt-5.4-mini",
    "models/gemini-3.1-pro-preview",
    "models/gemini-3.1-flash-lite-preview",
    "models/gemini-3.5-flash",
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "glm-5.2",
    "models/gemini-3.6-flash",
    "models/gemini-3.7-flash",
];

const GEMINI_NAME_EXCLUDES: &[&str] = &["embedding", "imagen", "veo", "-image", "aqa", "tts"];

const OPENAI_ID_EXCLUDES: &[&str] = &[
    "text-embedding-",
    "embedding-",
    "whisper-",
    "tts-",
    "dall-e-",
    "gpt-image",
    "chatgpt-image",
    "omni-moderation",
    "moderation-",
    "sora-",
    "computer-use",
    "babbage",
    "davinci",
    "curie",
    "ada",
    "gpt-4o-transcribe",
    "gpt-4o-mini-transcribe",
    "gpt-4o-mini-tts",
    "gpt-realtime",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    pub name: String,
    pub provider: TranslationProvider,
}

pub fn fallback_for(provider: TranslationProvider) -> ListedModel {
    let id = match provider {
        TranslationProvider::Google => FALLBACK_GOOGLE,
        TranslationProvider::Openai => FALLBACK_OPENAI,
        TranslationProvider::Deepseek => FALLBACK_DEEPSEEK,
        TranslationProvider::Zai => FALLBACK_ZAI,
    };
    ListedModel {
        id: id.to_string(),
        name: id.to_string(),
        provider,
    }
}

pub fn all_fallbacks() -> Vec<ListedModel> {
    vec![
        fallback_for(TranslationProvider::Google),
        fallback_for(TranslationProvider::Openai),
        fallback_for(TranslationProvider::Deepseek),
        fallback_for(TranslationProvider::Zai),
    ]
}

pub fn api_id_from_legacy(id: u64) -> String {
    LEGACY_API_IDS
        .get(id as usize)
        .copied()
        .unwrap_or("")
        .to_string()
}

pub fn legacy_id_from_api(id: &str) -> u64 {
    if id.is_empty() {
        return 0;
    }
    LEGACY_API_IDS
        .iter()
        .enumerate()
        .find(|(_, api)| **api == id)
        .map(|(i, _)| i as u64)
        .unwrap_or(0)
}

pub fn effective_model_id(provider: TranslationProvider, config_model: &str) -> String {
    if config_model.trim().is_empty() {
        fallback_for(provider).id
    } else {
        config_model.to_string()
    }
}

pub fn filter_gemini_models(body: &serde_json::Value) -> Vec<ListedModel> {
    let Some(models) = body.get("models").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for model in models {
        let Some(id) = model.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let methods = model
            .get("supportedGenerationMethods")
            .and_then(|v| v.as_array());
        let Some(methods) = methods else {
            continue;
        };
        let supports_generate = methods.iter().any(|m| m.as_str() == Some("generateContent"));
        if !supports_generate {
            continue;
        }
        if GEMINI_NAME_EXCLUDES.iter().any(|tok| id.contains(tok)) {
            continue;
        }
        let name = model
            .get("displayName")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(id)
            .to_string();
        out.push(ListedModel {
            id: id.to_string(),
            name,
            provider: TranslationProvider::Google,
        });
    }

    sort_by_display_name(&mut out);
    out
}

pub fn filter_openai_compat_models(
    body: &serde_json::Value,
    provider: TranslationProvider,
) -> Vec<ListedModel> {
    let Some(data) = body.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for model in data {
        let Some(id) = model.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if OPENAI_ID_EXCLUDES
            .iter()
            .any(|tok| id == *tok || id.starts_with(tok))
        {
            continue;
        }
        out.push(ListedModel {
            id: id.to_string(),
            name: id.to_string(),
            provider,
        });
    }

    sort_by_display_name(&mut out);
    out
}

fn sort_by_display_name(models: &mut [ListedModel]) {
    models.sort_by(|a, b| a.name.to_ascii_lowercase().cmp(&b.name.to_ascii_lowercase()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_table_round_trips_known_ids() {
        assert_eq!(api_id_from_legacy(1), "models/gemini-2.5-flash");
        assert_eq!(api_id_from_legacy(19), "models/gemini-3.7-flash");
        assert_eq!(api_id_from_legacy(0), "");
        assert_eq!(api_id_from_legacy(99), "");
        assert_eq!(legacy_id_from_api("models/gemini-2.5-flash"), 1);
        assert_eq!(legacy_id_from_api("gpt-9-ultra"), 0);
    }

    #[test]
    fn effective_id_uses_fallback_when_empty() {
        assert_eq!(
            effective_model_id(TranslationProvider::Google, ""),
            FALLBACK_GOOGLE
        );
        assert_eq!(
            effective_model_id(TranslationProvider::Openai, "gpt-5.2"),
            "gpt-5.2"
        );
    }

    #[test]
    fn gemini_filter_keeps_generate_content_drops_the_rest() {
        let body = serde_json::json!({
            "models": [
                {
                    "name": "models/gemini-3.7-flash",
                    "displayName": "Gemini 3.7 Flash",
                    "supportedGenerationMethods": ["generateContent", "countTokens"]
                },
                {
                    "name": "models/text-embedding-004",
                    "displayName": "Embeddings",
                    "supportedGenerationMethods": ["embedContent"]
                },
                {
                    "name": "models/gemini-2.5-flash-image",
                    "displayName": "Flash Image",
                    "supportedGenerationMethods": ["generateContent"]
                },
                {
                    "name": "models/gemini-9-ultra",
                    "supportedGenerationMethods": ["generateContent"]
                }
            ]
        });
        let got = filter_gemini_models(&body);
        let ids: Vec<_> = got.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["models/gemini-3.7-flash", "models/gemini-9-ultra"]);
        assert_eq!(got[0].name, "Gemini 3.7 Flash");
        assert_eq!(got[1].name, "models/gemini-9-ultra");
    }

    #[test]
    fn openai_filter_drops_non_chat_keeps_unknown_chat() {
        let body = serde_json::json!({
            "data": [
                {"id": "gpt-9-ultra"},
                {"id": "text-embedding-3-large"},
                {"id": "whisper-1"},
                {"id": "dall-e-3"},
                {"id": "gpt-5-mini"}
            ]
        });
        let got = filter_openai_compat_models(&body, TranslationProvider::Openai);
        let ids: Vec<_> = got.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["gpt-5-mini", "gpt-9-ultra"]); // sorted by display name
    }
}
