// Stage 6: per-card push/pull. Stage 7 wraps this in a periodic loop.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, anyhow};
use isolang::Language;

use crate::anki::connect::{AnkiConnect, CardInfo, NewNote};
use crate::anki::model::{FLTS_MODEL_NAME, bootstrap, deck_name};
use crate::card::{AnkiData, AnkiState, Card};
use crate::library::Library;

/// In-session sync orchestration state: bootstrap flag, per-card backoff
/// counters, and the persistent-failure set. Lives in the app crate for the
/// app's lifetime; reset on restart by design.
#[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
#[derive(Debug)]
pub struct AnkiSyncState {
    bootstrapped: bool,
    backoff: HashMap<String, BackoffEntry>,
    persistent_set: HashSet<String>,
    persistent_threshold: u32,
}

#[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
#[derive(Debug, Clone)]
struct BackoffEntry {
    failure_count: u32,
    next_attempt: tokio::time::Instant,
}

/// Summary of a single `sync_pass` invocation. Caller decides what to surface.
#[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub total_cards: usize,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub persistent_failures: Vec<String>,
}

const DEFAULT_PERSISTENT_THRESHOLD: u32 = 5;

impl AnkiSyncState {
    #[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
    pub fn new() -> Self {
        Self {
            bootstrapped: false,
            backoff: HashMap::new(),
            persistent_set: HashSet::new(),
            persistent_threshold: DEFAULT_PERSISTENT_THRESHOLD,
        }
    }
}

impl Default for AnkiSyncState {
    fn default() -> Self {
        Self::new()
    }
}

/// Run one orchestrated sync pass over every card on disk. Bootstraps the
/// AnkiConnect-side model+decks on the first call (gated by
/// `state.bootstrapped`). For each card: per-id lock → reload → `sync_card`
/// → save. Sequential per-card (cycle 7 adds `multi`-batched lookup).
#[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
pub async fn sync_pass(
    client: &dyn AnkiConnect,
    library: &Library,
    state: &mut AnkiSyncState,
    _now: tokio::time::Instant,
) -> Result<SyncReport> {
    let card_store = library.card_store();
    let pairs = card_store.list_pairs().await?;

    if !state.bootstrapped {
        let lang_pairs: Vec<(Language, Language)> = pairs
            .iter()
            .filter_map(|(s, t)| {
                Some((Language::from_639_3(s)?, Language::from_639_3(t)?))
            })
            .collect();
        bootstrap(client, &lang_pairs).await?;
        state.bootstrapped = true;
    }

    let mut report = SyncReport::default();
    for (src_str, tgt_str) in &pairs {
        let (Some(src), Some(tgt)) =
            (Language::from_639_3(src_str), Language::from_639_3(tgt_str))
        else {
            continue;
        };

        let card_ids = card_store
            .list_cards_in_pair(src_str, tgt_str)
            .await?;
        for (lemma_slug, pos_slug) in card_ids {
            report.total_cards += 1;

            let card_id =
                crate::card::card_id(src_str, tgt_str, &lemma_slug, &pos_slug);
            let lock_arc = card_store.lock_for(&card_id).await;
            let _guard = lock_arc.lock().await;

            let Some(mut card) = card_store
                .load(src_str, tgt_str, &lemma_slug, &pos_slug)
                .await?
            else {
                // Race: file vanished between list and load. Skip silently.
                continue;
            };

            // Opt-out short-circuit: counted as total but not attempted.
            if matches!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Suspended) | Some(AnkiState::Deleted)
            ) {
                continue;
            }

            report.attempted += 1;
            match sync_card(client, &mut card, src, tgt).await {
                Ok(()) => {
                    card_store.save(&card, src_str, tgt_str).await?;
                    report.succeeded += 1;
                }
                Err(err) => {
                    log::warn!("sync_card failed for {card_id}: {err}");
                    report.failed += 1;
                }
            }
        }
    }

    report.persistent_failures = state.persistent_set.iter().cloned().collect();
    Ok(report)
}

/// Render a card into the three Anki note fields (`Source`, `Target`, `Example`).
/// See `.specs/ANKI_REFINED.md § Field contents pushed to Anki`.
#[allow(dead_code)] // first non-test consumer is the Stage 7 sync orchestrator
pub(crate) fn render_fields(card: &Card) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("Source".into(), card.lemma.clone());
    out.insert("Target".into(), card.translations.join("; "));

    let mut examples = card.examples.clone();
    examples.sort_by(|a, b| a.source.cmp(&b.source));
    let example_field = examples
        .iter()
        .map(|e| format!("{} \u{2014} {}", e.source, e.translation))
        .collect::<Vec<_>>()
        .join("<br>");
    out.insert("Example".into(), example_field);
    out
}

/// Push a single card to Anki and pull back its state, mutating
/// `card.anki_data` in place. Caller (Stage 7 loop) owns load/lock/save.
#[allow(dead_code)] // first non-test consumer is the Stage 7 sync orchestrator
pub async fn sync_card(
    client: &dyn AnkiConnect,
    card: &mut Card,
    src: Language,
    tgt: Language,
) -> Result<()> {
    // The user opted out in Anki: never re-push, never overwrite the
    // explicit state. Local accumulation (translations/examples) continues
    // upstream of this call; only network sync is suppressed.
    if matches!(
        card.anki_data.as_ref().map(|a| a.state),
        Some(AnkiState::Suspended) | Some(AnkiState::Deleted)
    ) {
        return Ok(());
    }

    let query = format!("tag:{}", card.id);
    let hits = client.find_notes(&query).await?;

    if hits.is_empty() {
        match card.anki_data.as_ref() {
            None => {
                // Fresh card: create the note in Anki, then pull state.
                let note = NewNote {
                    deck_name: deck_name(src, tgt)?,
                    model_name: FLTS_MODEL_NAME.to_owned(),
                    fields: render_fields(card),
                    tags: vec![card.id.clone()],
                };
                let note_id = client.add_note(note).await?;
                card.anki_data = Some(pull_state(client, note_id).await?);
            }
            Some(_) => {
                // Card was previously synced but the user deleted the note
                // in Anki. Mark as deleted; do NOT re-add. Subsequent encounters
                // of this lemma keep the local card but never re-push.
                card.anki_data = Some(AnkiData {
                    state: AnkiState::Deleted,
                    interval_days: None,
                    ease_factor: None,
                    fsrs_difficulty: None,
                    fsrs_stability: None,
                });
            }
        }
    } else {
        // Note already exists in Anki: push current fields, then pull state.
        let note_id = hits[0];
        client
            .update_note_fields(note_id, render_fields(card))
            .await?;
        card.anki_data = Some(pull_state(client, note_id).await?);
    }
    Ok(())
}

async fn pull_state(client: &dyn AnkiConnect, note_id: i64) -> Result<AnkiData> {
    let notes = client.notes_info(&[note_id]).await?;
    let note = notes
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("notes_info returned no entry for note {note_id}"))?;
    let cards = client.cards_info(&note.cards).await?;
    if cards.iter().any(|c| c.is_suspended()) {
        return Ok(AnkiData {
            state: AnkiState::Suspended,
            interval_days: None,
            ease_factor: None,
            fsrs_difficulty: None,
            fsrs_stability: None,
        });
    }
    Ok(active_data_from(&cards))
}

fn active_data_from(cards: &[CardInfo]) -> AnkiData {
    let recognition = cards
        .iter()
        .min_by_key(|c| c.card_id)
        .expect("addNote always creates at least one card");
    AnkiData {
        state: AnkiState::Active,
        interval_days: Some(recognition.interval as f64),
        ease_factor: Some(recognition.factor as f64 / 1000.0),
        fsrs_difficulty: None,
        fsrs_stability: None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use isolang::Language;
    use uuid::Uuid;

    use crate::anki::connect::{AnkiConnect, MockAnkiConnect};
    use crate::anki::sync::{AnkiSyncState, render_fields, sync_card, sync_pass};
    use crate::card::{AnkiData, AnkiState, Card, Example};
    use crate::library::Library;
    use crate::test_utils::TempDir;

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

    fn example(source: &str, translation: &str) -> Example {
        Example {
            source: source.into(),
            translation: translation.into(),
            book_id: Uuid::nil(),
            chapter: 0,
            paragraph: 0,
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

    #[test]
    fn render_fields_joins_translations_with_semicolon_space() {
        let card = make_card("poder", vec!["мочь", "уметь"], vec![]);
        let fields = render_fields(&card);
        assert_eq!(fields.get("Target"), Some(&"мочь; уметь".to_owned()));
    }

    #[test]
    fn render_fields_handles_single_translation_without_separator() {
        let card = make_card("poder", vec!["мочь"], vec![]);
        let fields = render_fields(&card);
        assert_eq!(fields.get("Target"), Some(&"мочь".to_owned()));
    }

    #[test]
    fn render_fields_formats_examples_with_em_dash_and_br_joiner() {
        let card = make_card(
            "poder",
            vec!["мочь"],
            vec![example("No puedo más.", "Я больше не могу.")],
        );
        let fields = render_fields(&card);
        assert_eq!(
            fields.get("Example"),
            Some(&"No puedo más. \u{2014} Я больше не могу.".to_owned())
        );
    }

    fn spa() -> Language {
        Language::from_639_3("spa").unwrap()
    }

    fn rus() -> Language {
        Language::from_639_3("rus").unwrap()
    }

    async fn bootstrap_mock() -> MockAnkiConnect {
        let mock = MockAnkiConnect::new();
        crate::anki::model::bootstrap(&mock, &[(spa(), rus())])
            .await
            .unwrap();
        mock
    }

    #[tokio::test]
    async fn sync_card_skips_when_state_suspended() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);
        let before = AnkiData {
            state: AnkiState::Suspended,
            interval_days: None,
            ease_factor: None,
            fsrs_difficulty: None,
            fsrs_stability: None,
        };
        card.anki_data = Some(before.clone());

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        // No note created, no AnkiConnect mutation visible to find_notes.
        let hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
        assert!(hits.is_empty(), "suspended card must not be pushed");
        assert_eq!(card.anki_data.as_ref(), Some(&before), "anki_data preserved");
    }

    #[tokio::test]
    async fn sync_card_skips_when_state_deleted() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);
        let before = AnkiData {
            state: AnkiState::Deleted,
            interval_days: None,
            ease_factor: None,
            fsrs_difficulty: None,
            fsrs_stability: None,
        };
        card.anki_data = Some(before.clone());

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
        assert!(hits.is_empty(), "deleted card must not be re-added");
        assert_eq!(card.anki_data.as_ref(), Some(&before), "anki_data preserved");
    }

    #[tokio::test]
    async fn sync_card_flags_suspension_when_any_card_suspended_in_anki() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);

        // First push to create the note + cards, then suspend one of them.
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();
        let note_id = mock
            .find_notes(&format!("tag:{}", card.id))
            .await
            .unwrap()[0];
        let cards = mock.notes_info(&[note_id]).await.unwrap()[0].cards.clone();
        mock.suspend_card(cards[0]); // suspend just one direction

        // Force a re-sync; the existing-note branch should detect suspension.
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let anki = card.anki_data.as_ref().expect("anki_data populated");
        assert_eq!(anki.state, AnkiState::Suspended);
        assert_eq!(anki.interval_days, None, "retention fields dropped on suspended");
        assert_eq!(anki.ease_factor, None);
    }

    #[tokio::test]
    async fn sync_card_flags_deletion_when_note_vanished_from_anki() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);
        // Card was previously synced — anki_data carries prior Active state —
        // but the user has since deleted the note in Anki (mock has zero
        // matching notes).
        card.anki_data = Some(AnkiData {
            state: AnkiState::Active,
            interval_days: Some(30.0),
            ease_factor: Some(2.5),
            fsrs_difficulty: None,
            fsrs_stability: None,
        });

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        // Mock note count must stay zero — we MUST NOT re-add.
        let all_hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
        assert!(all_hits.is_empty(), "deleted card must not be re-added");

        let anki = card.anki_data.as_ref().expect("anki_data still set");
        assert_eq!(anki.state, AnkiState::Deleted);
        assert_eq!(anki.interval_days, None, "retention fields cleared");
        assert_eq!(anki.ease_factor, None);
    }

    #[tokio::test]
    async fn sync_card_updates_existing_note_via_update_note_fields() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);

        // First push: creates the note.
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();
        let original_hits = mock
            .find_notes(&format!("tag:{}", card.id))
            .await
            .unwrap();
        assert_eq!(original_hits.len(), 1);
        let note_id = original_hits[0];

        // Mutate translations locally, sync again — should update, not create.
        card.translations.push("уметь".into());
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let hits_after = mock
            .find_notes(&format!("tag:{}", card.id))
            .await
            .unwrap();
        assert_eq!(hits_after, vec![note_id], "no new note created on update");

        let (fields, _) = mock.peek_note(note_id).expect("note exists");
        assert_eq!(fields.get("Target"), Some(&"мочь; уметь".to_owned()));
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Active)
        );
    }

    #[tokio::test]
    async fn sync_card_pushes_fresh_card_via_add_note() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let hits = mock
            .find_notes(&format!("tag:{}", card.id))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "exactly one note exists after first push");
        let (fields, tags) = mock.peek_note(hits[0]).expect("note exists");
        assert_eq!(fields.get("Source"), Some(&"poder".to_owned()));
        assert_eq!(fields.get("Target"), Some(&"мочь".to_owned()));
        assert!(tags.iter().any(|t| t == &card.id));

        let anki = card.anki_data.as_ref().expect("anki_data populated");
        assert_eq!(anki.state, AnkiState::Active);
        assert_eq!(anki.interval_days, Some(0.0));
        assert_eq!(anki.ease_factor, Some(0.0));
    }

    async fn seed_library_with_cards(tmp_prefix: &str, cards: &[Card]) -> (TempDir, Library) {
        let tmp = TempDir::new(tmp_prefix);
        let library = Library::open(tmp.path.clone()).await.unwrap();
        for card in cards {
            library.card_store().save(card, "spa", "rus").await.unwrap();
        }
        (tmp, library)
    }

    #[tokio::test]
    async fn sync_pass_walks_all_cards_and_pushes_each() {
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_pass_happy",
            &[
                make_card("poder", vec!["мочь"], vec![]),
                make_card("comer", vec!["есть"], vec![]),
            ],
        )
        .await;

        let mut state = AnkiSyncState::new();
        let now = tokio::time::Instant::now();
        let report = sync_pass(&mock, &library, &mut state, now).await.unwrap();

        assert_eq!(report.total_cards, 2);
        assert_eq!(report.attempted, 2);
        assert_eq!(report.succeeded, 2);
        assert_eq!(report.failed, 0);

        // Bootstrap occurred: model and deck must exist.
        let models = mock.model_names_and_ids().await.unwrap();
        assert!(models.contains_key(crate::anki::model::FLTS_MODEL_NAME));
        let decks = mock.deck_names_and_ids().await.unwrap();
        assert!(decks.contains_key("FLTS::spa-rus"));

        // Each card got a note tagged with its id.
        for lemma in ["poder", "comer"] {
            let id = format!("flts_spa_rus_{lemma}_verb");
            let hits = mock.find_notes(&format!("tag:{id}")).await.unwrap();
            assert_eq!(hits.len(), 1, "expected one note for {id}");
        }

        // Reloaded cards have Active anki_data.
        for lemma in ["poder", "comer"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma, "verb")
                .await
                .unwrap()
                .expect("card present");
            assert_eq!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Active)
            );
        }
    }

    #[test]
    fn render_fields_sorts_examples_alphabetically_by_source() {
        let card = make_card(
            "poder",
            vec!["мочь"],
            vec![
                example("Pueden venir mañana.", "Они могут прийти завтра."),
                example("No puedo más.", "Я больше не могу."),
            ],
        );
        let fields = render_fields(&card);
        assert_eq!(
            fields.get("Example"),
            Some(
                &"No puedo más. \u{2014} Я больше не могу.<br>\
                Pueden venir mañana. \u{2014} Они могут прийти завтра."
                    .to_owned()
            )
        );
    }
}
