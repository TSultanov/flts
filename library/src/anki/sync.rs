// Stage 6: per-card push/pull. Stage 7 wraps this in a periodic loop.

use std::collections::BTreeMap;

use crate::card::Card;

/// Render a card into the three Anki note fields (`Source`, `Target`, `Example`).
/// See `.specs/ANKI_REFINED.md § Field contents pushed to Anki`.
#[allow(dead_code)] // first consumer lands in cycle 4 (sync_card)
pub(crate) fn render_fields(card: &Card) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("Source".into(), card.lemma.clone());
    out.insert(
        "Target".into(),
        card.translations.first().cloned().unwrap_or_default(),
    );
    out.insert("Example".into(), String::new());
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::anki::sync::render_fields;
    use crate::card::{Card, Example};

    fn make_card(lemma: &str, translations: Vec<&str>, examples: Vec<Example>) -> Card {
        Card {
            version: 1,
            id: format!("flts_spa_rus_{lemma}_verb"),
            lemma: lemma.into(),
            part_of_speech: "verb".into(),
            translations: translations.into_iter().map(String::from).collect(),
            examples,
            anki_data: None,
        }
    }

    #[test]
    fn render_fields_populates_source_target_example() {
        let card = make_card("poder", vec!["мочь"], vec![]);
        let fields: BTreeMap<String, String> = render_fields(&card);
        assert_eq!(fields.get("Source"), Some(&"poder".to_owned()));
        assert_eq!(fields.get("Target"), Some(&"мочь".to_owned()));
        assert_eq!(fields.get("Example"), Some(&String::new()));
    }
}
