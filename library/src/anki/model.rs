//! First-run bootstrap for AnkiConnect: version check, note-model creation,
//! per-language-pair deck creation.
//!
//! Stage 5 surface: the bootstrap helper is callable from tests today and
//! from the Stage 7 sync orchestrator once it lands. Idempotent — re-running
//! against an already-bootstrapped Anki is a no-op.

use anyhow::{Result, anyhow, bail};
use isolang::Language;

use crate::anki::connect::{AnkiConnect, CardTemplate, ModelSpec};

pub const FLTS_MODEL_NAME: &str = "FLTS Bilingual v1";
const MIN_ANKI_CONNECT_VERSION: u32 = 6;

const FLTS_MODEL_CSS: &str = ".card { font-family: 'Segoe UI', Arial, sans-serif; font-size: 24px; \
text-align: center; color: #1a1a1a; background-color: #fafafa; padding: 20px; } \
.front { font-size: 28px; font-weight: bold; margin-bottom: 10px; } \
.back { font-size: 26px; color: #2c5f2d; margin-top: 10px; } \
.example { font-size: 18px; color: #555; font-style: italic; margin-top: 15px; \
padding: 10px; border-top: 1px dashed #ccc; }";

/// Build the canonical `FLTS Bilingual v1` model spec with generic field names
/// (`Source`, `Target`, `Example`) so a single model serves every language
/// pair. Card templates and CSS are adapted from `.specs/ANKI.md`.
pub fn flts_model_spec() -> ModelSpec {
    let front_source = "<div class=\"front\">{{Source}}</div>".to_owned();
    let back_source_to_target = "<div class=\"front\">{{Source}}</div>\
<hr id=\"answer\">\
<div class=\"back\">{{Target}}</div>\
{{#Example}}<div class=\"example\">💬 {{Example}}</div>{{/Example}}"
        .to_owned();
    let front_target = "<div class=\"front\">{{Target}}</div>".to_owned();
    let back_target_to_source = "<div class=\"front\">{{Target}}</div>\
<hr id=\"answer\">\
<div class=\"back\">{{Source}}</div>\
{{#Example}}<div class=\"example\">💬 {{Example}}</div>{{/Example}}"
        .to_owned();

    ModelSpec {
        model_name: FLTS_MODEL_NAME.to_owned(),
        in_order_fields: vec!["Source".into(), "Target".into(), "Example".into()],
        css: FLTS_MODEL_CSS.to_owned(),
        is_cloze: false,
        card_templates: vec![
            CardTemplate {
                name: "Source → Target".into(),
                front: front_source,
                back: back_source_to_target,
            },
            CardTemplate {
                name: "Target → Source".into(),
                front: front_target,
                back: back_target_to_source,
            },
        ],
    }
}

/// The deck name FLTS uses for a given language pair:
/// `FLTS::<src_pretty>-<tgt_pretty>` where each side prefers the
/// language's autonym (e.g. "Русский", "Español") and falls back to
/// the English name (e.g. "English") when no autonym is available.
///
/// The card-store directory layout at `cards/<src>-<tgt>/` continues to
/// use ISO 639-3 codes and is unaffected.
pub fn deck_name(src: Language, tgt: Language) -> Result<String> {
    let s = pretty_name(src);
    let t = pretty_name(tgt);
    if s.is_empty() || t.is_empty() {
        bail!("language without a printable name: {src:?} or {tgt:?}");
    }
    Ok(format!("FLTS::{s}-{t}"))
}

fn pretty_name(lang: Language) -> String {
    let raw = lang.to_autonym().unwrap_or_else(|| lang.to_name());
    // isolang's autonyms use each language's native casing — Russian
    // and Spanish render lowercase ("русский", "español") because that
    // matches the local convention. Capitalize the first grapheme for
    // visual consistency with the English-style "FLTS::" prefix in
    // Anki's deck list.
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// First-run bootstrap. Verifies the AnkiConnect version, ensures the
/// `FLTS Bilingual v1` note model exists, and ensures a `FLTS::<src>-<tgt>`
/// deck exists for every language pair the caller hands in.
///
/// Idempotent: calling twice against the same Anki instance is a no-op on
/// the second call.
pub async fn bootstrap(
    client: &dyn AnkiConnect,
    lang_pairs: &[(Language, Language)],
) -> Result<()> {
    let version = client.version().await?;
    if version < MIN_ANKI_CONNECT_VERSION {
        return Err(anyhow!(
            "AnkiConnect ≥ {MIN_ANKI_CONNECT_VERSION} required, got {version}"
        ));
    }

    let models = client.model_names_and_ids().await?;
    if !models.contains_key(FLTS_MODEL_NAME) {
        log::info!("Creating Anki note model `{FLTS_MODEL_NAME}`");
        client.create_model(flts_model_spec()).await?;
    }

    // AnkiConnect's `createDeck` is idempotent (no-op when the deck exists),
    // so we call it unconditionally rather than gating on
    // `deckNamesAndIds`. Field reports from real Anki show the pre-check can
    // return a false positive — `deckNamesAndIds` lists the deck name but a
    // subsequent `addNote` against that same name fails with "deck was not
    // found". Calling `createDeck` unconditionally sidesteps that mismatch.
    for (src, tgt) in lang_pairs {
        let name = deck_name(*src, *tgt)?;
        log::info!("Ensuring Anki deck `{name}`");
        client.create_deck(&name).await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anki::connect::MockAnkiConnect;

    fn spa() -> Language {
        Language::from_639_3("spa").unwrap()
    }

    fn rus() -> Language {
        Language::from_639_3("rus").unwrap()
    }

    fn eng() -> Language {
        Language::from_639_3("eng").unwrap()
    }

    #[test]
    fn deck_name_uses_autonym_or_english_name() {
        assert_eq!(deck_name(spa(), rus()).unwrap(), "FLTS::Español-Русский");
        assert_eq!(deck_name(eng(), rus()).unwrap(), "FLTS::English-Русский");
    }

    #[test]
    fn flts_model_spec_uses_generic_field_names() {
        let spec = flts_model_spec();
        assert_eq!(spec.model_name, FLTS_MODEL_NAME);
        assert_eq!(spec.in_order_fields, vec!["Source", "Target", "Example"]);
        assert_eq!(spec.card_templates.len(), 2);
        assert!(spec.card_templates[0].front.contains("{{Source}}"));
        assert!(spec.card_templates[1].front.contains("{{Target}}"));
    }

    #[tokio::test]
    async fn bootstrap_creates_model_and_decks_on_fresh_install() {
        let mock = MockAnkiConnect::new();
        bootstrap(&mock, &[(spa(), rus()), (eng(), rus())])
            .await
            .unwrap();

        let models = mock.model_names_and_ids().await.unwrap();
        assert!(models.contains_key(FLTS_MODEL_NAME));

        let decks = mock.deck_names_and_ids().await.unwrap();
        assert!(decks.contains_key("FLTS::Español-Русский"));
        assert!(decks.contains_key("FLTS::English-Русский"));
        assert_eq!(decks.len(), 2);
    }

    #[tokio::test]
    async fn bootstrap_is_idempotent() {
        let mock = MockAnkiConnect::new();
        let pairs = [(spa(), rus()), (eng(), rus())];

        bootstrap(&mock, &pairs).await.unwrap();
        let models_before = mock.model_names_and_ids().await.unwrap();
        let decks_before = mock.deck_names_and_ids().await.unwrap();

        bootstrap(&mock, &pairs).await.unwrap();
        let models_after = mock.model_names_and_ids().await.unwrap();
        let decks_after = mock.deck_names_and_ids().await.unwrap();

        assert_eq!(models_before, models_after);
        assert_eq!(decks_before, decks_after);
    }

    #[tokio::test]
    async fn bootstrap_rejects_version_below_six() {
        let mock = MockAnkiConnect::new();
        mock.set_version(5);
        let err = bootstrap(&mock, &[(spa(), rus())]).await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("AnkiConnect ≥ 6"),
            "expected version-floor error, got {msg}"
        );
        assert!(msg.contains("got 5"), "expected actual version in error: {msg}");
    }

    #[tokio::test]
    async fn bootstrap_accepts_empty_lang_pairs() {
        let mock = MockAnkiConnect::new();
        bootstrap(&mock, &[]).await.unwrap();
        let models = mock.model_names_and_ids().await.unwrap();
        assert!(models.contains_key(FLTS_MODEL_NAME));
        let decks = mock.deck_names_and_ids().await.unwrap();
        assert!(decks.is_empty());
    }
}
