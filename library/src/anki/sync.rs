// Stage 6: per-card push/pull. Stage 7 wraps this in a periodic loop.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, anyhow};
use isolang::Language;

use crate::anki::connect::{AnkiConnect, CardInfo, MultiSubAction, NewNote};
use crate::anki::model::{FLTS_MODEL_NAME, bootstrap, deck_name};
use crate::card::{AnkiData, AnkiState, Card};
use crate::library::Library;

/// Per-spec batch-size cap for `multi` calls (§ Action set).
const MULTI_BATCH_SIZE: usize = 50;

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

/// Linear backoff schedule capped at ten minutes per spec § Failure modes.
/// `n=0` yields zero delay (treated as "not in backoff").
#[allow(dead_code)] // first non-test consumer is cycle 5
pub(crate) fn next_delay(n: u32) -> std::time::Duration {
    std::time::Duration::from_secs(60 * n.min(10) as u64)
}

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

    /// Override the default persistent-failure threshold (5 by spec default).
    #[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
    pub fn with_threshold(mut self, threshold: u32) -> Self {
        self.persistent_threshold = threshold;
        self
    }

    /// Returns true if the card is in cooldown and should be skipped this tick.
    fn in_cooldown(&self, card_id: &str, now: tokio::time::Instant) -> bool {
        self.backoff
            .get(card_id)
            .is_some_and(|e| e.next_attempt > now)
    }

    /// Clears any backoff state for this card after a successful sync.
    fn record_success(&mut self, card_id: &str) {
        self.backoff.remove(card_id);
        self.persistent_set.remove(card_id);
    }

    /// Increments the card's failure counter and schedules the next attempt.
    /// Surfaces the card in `persistent_set` once it crosses the threshold.
    fn record_failure(&mut self, card_id: &str, now: tokio::time::Instant) {
        let entry = self
            .backoff
            .entry(card_id.to_owned())
            .or_insert(BackoffEntry {
                failure_count: 0,
                next_attempt: now,
            });
        entry.failure_count += 1;
        entry.next_attempt = now + next_delay(entry.failure_count);
        if entry.failure_count >= self.persistent_threshold {
            self.persistent_set.insert(card_id.to_owned());
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
/// `state.bootstrapped`). Two-phase:
///   Phase 1: gather eligible cards (post opt-out + cooldown filters),
///            holding per-card locks. Batch their `findNotes` lookups via
///            `multi` (chunked at MULTI_BATCH_SIZE).
///   Phase 2: per eligible card, run the inline state machine using its
///            prefetched lookup result.
#[allow(dead_code)] // first non-test consumer is the Stage 9 AnkiSyncTask
pub async fn sync_pass(
    client: &dyn AnkiConnect,
    library: &Library,
    state: &mut AnkiSyncState,
    now: tokio::time::Instant,
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

    // Phase 1a: walk disk, acquire locks, load, filter.
    struct Eligible {
        card_id: String,
        src_str: String,
        tgt_str: String,
        src: Language,
        tgt: Language,
        card: Card,
        _guard: tokio::sync::OwnedMutexGuard<()>,
    }
    let mut eligible: Vec<Eligible> = Vec::new();

    for (src_str, tgt_str) in &pairs {
        let (Some(src), Some(tgt)) =
            (Language::from_639_3(src_str), Language::from_639_3(tgt_str))
        else {
            continue;
        };

        let card_files = card_store.list_cards_in_pair(src_str, tgt_str).await?;
        for (lemma_slug, pos_slug) in card_files {
            report.total_cards += 1;

            let card_id =
                crate::card::card_id(src_str, tgt_str, &lemma_slug, &pos_slug);
            let lock_arc = card_store.lock_for(&card_id).await;
            let guard = lock_arc.lock_owned().await;

            let Some(card) = card_store
                .load(src_str, tgt_str, &lemma_slug, &pos_slug)
                .await?
            else {
                continue;
            };

            // Opt-out short-circuit: counted as total but not attempted.
            if matches!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Suspended) | Some(AnkiState::Deleted)
            ) {
                continue;
            }
            // Backoff cooldown: counted as total but not attempted.
            if state.in_cooldown(&card_id, now) {
                continue;
            }

            report.attempted += 1;
            eligible.push(Eligible {
                card_id,
                src_str: src_str.clone(),
                tgt_str: tgt_str.clone(),
                src,
                tgt,
                card,
                _guard: guard,
            });
        }
    }

    // Phase 1b: batched findNotes lookup via multi, chunked. Per-card lookup
    // result is None for chunks whose multi call errored — those cards are
    // attributed a failure in phase 2.
    let mut lookups: Vec<Option<Vec<i64>>> = Vec::with_capacity(eligible.len());
    for chunk in eligible.chunks(MULTI_BATCH_SIZE) {
        let actions: Vec<MultiSubAction> = chunk
            .iter()
            .map(|e| MultiSubAction {
                action: "findNotes".to_owned(),
                params: Some(serde_json::json!({
                    "query": format!("tag:{}", e.card_id),
                })),
            })
            .collect();
        match client.multi(actions).await {
            Ok(results) => {
                for value in results {
                    let hits: Vec<i64> = serde_json::from_value(value)?;
                    lookups.push(Some(hits));
                }
            }
            Err(err) => {
                log::warn!("multi findNotes batch failed: {err}");
                for _ in 0..chunk.len() {
                    lookups.push(None);
                }
            }
        }
    }

    // Phase 2: per-card state machine using prefetched lookup results.
    for (mut e, hits) in eligible.into_iter().zip(lookups.into_iter()) {
        let outcome = match hits {
            None => Err(anyhow!("lookup batch failed for {}", e.card_id)),
            Some(hits) => apply_lookup(client, &mut e.card, hits, e.src, e.tgt).await,
        };
        match outcome {
            Ok(()) => {
                card_store
                    .save(&e.card, &e.src_str, &e.tgt_str)
                    .await?;
                state.record_success(&e.card_id);
                report.succeeded += 1;
            }
            Err(err) => {
                log::warn!("sync failed for {}: {err}", e.card_id);
                state.record_failure(&e.card_id, now);
                report.failed += 1;
            }
        }
    }

    report.persistent_failures = state.persistent_set.iter().cloned().collect();
    Ok(report)
}

/// Phase-2 state machine: given the per-card findNotes result, dispatch to
/// addNote / updateNoteFields and pull state. Mirrors `sync_card` post-lookup
/// behavior so `sync_card` remains usable for single-card callers.
async fn apply_lookup(
    client: &dyn AnkiConnect,
    card: &mut Card,
    hits: Vec<i64>,
    src: Language,
    tgt: Language,
) -> Result<()> {
    if hits.is_empty() {
        match card.anki_data.as_ref() {
            None => {
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
        let note_id = hits[0];
        client
            .update_note_fields(note_id, render_fields(card))
            .await?;
        card.anki_data = Some(pull_state(client, note_id).await?);
    }
    Ok(())
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
    use crate::anki::sync::{
        AnkiSyncState, next_delay, render_fields, sync_card, sync_pass,
    };
    use crate::card::{AnkiData, AnkiState, Card, Example};
    use crate::library::Library;
    use crate::test_utils::{TempDir, full_word, one_sentence_paragraph};

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

    #[test]
    fn next_delay_is_linear_with_ten_minute_cap() {
        use std::time::Duration;
        assert_eq!(next_delay(0), Duration::from_secs(0));
        assert_eq!(next_delay(1), Duration::from_secs(60));
        assert_eq!(next_delay(5), Duration::from_secs(300));
        assert_eq!(next_delay(10), Duration::from_secs(600));
        assert_eq!(next_delay(11), Duration::from_secs(600));
        assert_eq!(next_delay(1_000_000), Duration::from_secs(600));
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

    #[tokio::test(start_paused = true)]
    async fn sync_pass_skips_card_in_cooldown_and_retries_after_delay() {
        use std::time::Duration;

        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_backoff",
            &[make_card("poder", vec!["мочь"], vec![])],
        )
        .await;

        // Bootstrap happens inside sync_pass; have the FIRST sync call fail.
        // We need the failure to land on the per-card sync, not on bootstrap.
        // Approach: pre-bootstrap explicitly so the next batch of failures
        // applies to the sync_card phase.
        let mut state = AnkiSyncState::new();
        let now0 = tokio::time::Instant::now();
        // First pass: succeeds, populates anki_data (Active).
        let report0 = sync_pass(&mock, &library, &mut state, now0).await.unwrap();
        assert_eq!(report0.succeeded, 1);
        assert_eq!(report0.failed, 0);

        // Inject one failure for the next sync_card call. sync_card on an
        // already-Active card hits find_notes first — that call will fail.
        mock.fail_next_n_calls(1);

        let now1 = tokio::time::Instant::now();
        let report1 = sync_pass(&mock, &library, &mut state, now1).await.unwrap();
        assert_eq!(report1.attempted, 1);
        assert_eq!(report1.failed, 1);
        assert_eq!(report1.succeeded, 0);

        // Without advancing time, the card must be in cooldown.
        let report2 = sync_pass(&mock, &library, &mut state, now1).await.unwrap();
        assert_eq!(report2.attempted, 0, "card must be skipped during cooldown");
        assert_eq!(report2.total_cards, 1);

        // Advance past the 60s delay; the card must be retried and succeed.
        tokio::time::advance(Duration::from_secs(61)).await;
        let now2 = tokio::time::Instant::now();
        let report3 = sync_pass(&mock, &library, &mut state, now2).await.unwrap();
        assert_eq!(report3.attempted, 1, "card must retry after cooldown");
        assert_eq!(report3.succeeded, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn sync_pass_surfaces_card_in_persistent_failures_after_threshold() {
        use std::time::Duration;

        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_persistent",
            &[make_card("poder", vec!["мочь"], vec![])],
        )
        .await;

        // Threshold of 3 means card surfaces after the 3rd consecutive failure.
        let mut state = AnkiSyncState::new().with_threshold(3);
        let card_id = format!("flts_spa_rus_poder_verb");

        // Pre-bootstrap so failure-injection lands on sync_card, not bootstrap.
        crate::anki::model::bootstrap(
            &mock,
            &[(
                Language::from_639_3("spa").unwrap(),
                Language::from_639_3("rus").unwrap(),
            )],
        )
        .await
        .unwrap();
        state.bootstrapped = true;

        mock.fail_next_n_calls(100);

        // Tick 1: first failure. Not yet persistent.
        let r1 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r1.failed, 1);
        assert!(r1.persistent_failures.is_empty(), "after 1 failure: not persistent yet");

        tokio::time::advance(Duration::from_secs(61)).await;
        // Tick 2: second failure. Still not persistent.
        let r2 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r2.failed, 1);
        assert!(r2.persistent_failures.is_empty(), "after 2 failures: not persistent yet");

        tokio::time::advance(Duration::from_secs(121)).await;
        // Tick 3: third failure — threshold hit.
        let r3 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r3.failed, 1);
        assert_eq!(
            r3.persistent_failures,
            vec![card_id.clone()],
            "after threshold hit: surfaced"
        );

        // Clear the failure injector; advance past 3-minute cooldown, retry succeeds.
        mock.fail_next_n_calls(0);
        tokio::time::advance(Duration::from_secs(181)).await;
        let r4 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r4.succeeded, 1);
        assert!(
            r4.persistent_failures.is_empty(),
            "successful retry clears persistent set"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sync_pass_converges_under_transient_failures_over_simulated_session() {
        use std::time::Duration;

        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_convergence",
            &[
                make_card("poder", vec!["мочь"], vec![]),
                make_card("comer", vec!["есть"], vec![]),
                make_card("ver", vec!["видеть"], vec![]),
                make_card("ir", vec!["идти"], vec![]),
                make_card("ser", vec!["быть"], vec![]),
            ],
        )
        .await;

        let mut state = AnkiSyncState::new();

        // Pre-bootstrap so failures land on per-card sync, not on bootstrap
        // (which is unconditionally retried on failure inside sync_pass).
        crate::anki::model::bootstrap(
            &mock,
            &[(
                Language::from_639_3("spa").unwrap(),
                Language::from_639_3("rus").unwrap(),
            )],
        )
        .await
        .unwrap();
        state.bootstrapped = true;

        // 13 transient failures, sparse enough to never trip threshold=5 per card.
        mock.fail_next_n_calls(13);

        let mut consecutive_clean = 0;
        for tick in 0..30 {
            tokio::time::advance(Duration::from_secs(60 * (tick + 1))).await;
            let report = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
                .await
                .unwrap();
            if report.failed == 0 {
                consecutive_clean += 1;
            } else {
                consecutive_clean = 0;
            }
            if consecutive_clean >= 2 {
                break;
            }
        }
        assert!(
            consecutive_clean >= 2,
            "expected two consecutive clean ticks within 30 ticks"
        );

        // All 5 cards must end Active; persistent set must be empty.
        for lemma in ["poder", "comer", "ver", "ir", "ser"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma, "verb")
                .await
                .unwrap()
                .expect("card present");
            assert_eq!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Active),
                "card `{lemma}` did not converge to Active"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn sync_pass_batches_find_notes_via_multi() {
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_multi_batch",
            &[
                make_card("poder", vec!["мочь"], vec![]),
                make_card("comer", vec!["есть"], vec![]),
                make_card("ver", vec!["видеть"], vec![]),
            ],
        )
        .await;

        // First sync_pass: creates notes (add_note path doesn't go through
        // the find_notes lookup batch for fresh cards). Second pass exercises
        // the multi-batched lookup against existing notes.
        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let multi_before = mock.multi_call_count();
        let direct_before = mock.find_notes_direct_count();

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let multi_after = mock.multi_call_count();
        let direct_after = mock.find_notes_direct_count();
        assert_eq!(
            multi_after - multi_before,
            1,
            "expected exactly one multi call for 3 cards' findNotes lookup"
        );
        assert_eq!(
            direct_after - direct_before,
            0,
            "no per-card find_notes calls should fire during the batched lookup"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sync_pass_chunks_find_notes_at_fifty() {
        let mock = MockAnkiConnect::new();
        let cards: Vec<Card> = (0..75)
            .map(|i| make_card(&format!("verb{i:03}"), vec!["x"], vec![]))
            .collect();
        let (_tmp, library) = seed_library_with_cards("flts_sync_chunk_50", &cards).await;

        // Seed-and-sync once to create the notes, then re-sync to exercise
        // the multi-batched lookup over all 75 existing notes.
        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let multi_before = mock.multi_call_count();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let multi_after = mock.multi_call_count();
        assert_eq!(
            multi_after - multi_before,
            2,
            "75 cards must split into 50 + 25 → 2 multi calls"
        );
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

    async fn library_with_one_paragraph_book(
        library_path: std::path::PathBuf,
        paragraph_text: &str,
    ) -> (Library, Uuid) {
        let library = Library::open(library_path).await.unwrap();
        let book = library.create_book("Test Book", &spa()).await.unwrap();
        let book_id = {
            let mut b = book.lock().await;
            b.book.push_chapter(Some("Intro"));
            b.book.push_paragraph(0, paragraph_text, None);
            b.save().await.unwrap();
            b.book.id
        };
        (library, book_id)
    }

    #[tokio::test]
    async fn e2e_paragraph_translation_creates_card_and_syncs_to_anki() {
        let tmp = TempDir::new("flts_e2e_translate_sync");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar en casa.").await;

        let paragraph = one_sentence_paragraph(
            "Я могу войти в дом.",
            vec![
                full_word("Puedo", "poder", "мочь", "verb", &["могу"], false),
                full_word("entrar", "entrar", "входить", "verb", &["войти"], false),
                full_word("en", "en", "в", "prep", &["в"], false),
                full_word("casa", "casa", "дом", "noun", &["дом"], false),
                full_word(".", ".", ".", "punct", &[], true),
            ],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        let report = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(report.attempted, 4, "four eligible lemmas (punct skipped)");
        assert_eq!(report.succeeded, 4);
        assert_eq!(report.failed, 0);

        // Spot-check the `poder` card end-to-end: tag lookup → note fields → on-disk state.
        let poder_tag = "flts_spa_rus_poder_verb";
        let poder_note = mock
            .note_id_for_tag(poder_tag)
            .expect("poder note exists in mock");
        let (fields, tags) = mock.peek_note(poder_note).expect("note state present");
        assert_eq!(fields.get("Source"), Some(&"poder".to_owned()));
        assert_eq!(fields.get("Target"), Some(&"мочь".to_owned()));
        assert_eq!(
            fields.get("Example"),
            Some(&"Puedo entrar en casa. \u{2014} Я могу войти в дом.".to_owned()),
            "example carries the paragraph source + full translation joined by em-dash"
        );
        assert!(
            tags.iter().any(|t| t == poder_tag),
            "FLTS card-id tag persists on the note"
        );

        let card = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("poder card on disk");
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Active),
            "card state flips to Active after first sync"
        );

        // Sanity: the noun gets its own note in the same deck.
        assert!(
            mock.note_id_for_tag("flts_spa_rus_casa_noun").is_some(),
            "casa note exists in mock"
        );
    }

    #[tokio::test]
    async fn e2e_suspend_in_anki_persists_through_re_translation() {
        let tmp = TempDir::new("flts_e2e_suspend");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar.").await;

        let paragraph = one_sentence_paragraph(
            "Я могу войти.",
            vec![full_word(
                "Puedo", "poder", "мочь", "verb", &["могу"], false,
            )],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        // Sync #1: create the note.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_tag = "flts_spa_rus_poder_verb";
        let note_id = mock.note_id_for_tag(poder_tag).expect("note exists after first sync");

        // User suspends one of the note's direction cards in Anki.
        let card_ids = mock.notes_info(&[note_id]).await.unwrap()[0].cards.clone();
        assert!(!card_ids.is_empty(), "note has at least one direction card");
        mock.suspend_card(card_ids[0]);

        // Sync #2: detection branch flips state to Suspended.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let card = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(card.anki_data.as_ref().map(|a| a.state), Some(AnkiState::Suspended));

        // Snapshot the note's fields before the re-encounter so we can detect mutation.
        let (fields_before, _) = mock.peek_note(note_id).unwrap();

        // Re-encounter: apply the same paragraph again. The local merge path is
        // idempotent (provenance dedup); state must not regress to Active.
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();
        let card_after_reencounter = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card_after_reencounter.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Suspended),
            "re-encountering the paragraph must not reset state to Active"
        );

        // Sync #3: opt-out branch short-circuits — no addNote, no updateNoteFields.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        // Same note id, same fields. No second note was created.
        assert_eq!(
            mock.note_id_for_tag(poder_tag),
            Some(note_id),
            "no second note created for the same tag"
        );
        let (fields_after, _) = mock.peek_note(note_id).unwrap();
        assert_eq!(fields_before, fields_after, "suspended note fields untouched");

        let card_final = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card_final.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Suspended),
            "state stays Suspended across the third sync"
        );
    }

    #[tokio::test]
    async fn mock_remove_note_clears_find_notes_hits() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let tag = format!("tag:{}", card.id);
        let hits_before = mock.find_notes(&tag).await.unwrap();
        assert_eq!(hits_before.len(), 1);

        mock.remove_note(hits_before[0]);

        let hits_after = mock.find_notes(&tag).await.unwrap();
        assert!(
            hits_after.is_empty(),
            "remove_note must clear findNotes hits for the note's tag"
        );
    }

    #[tokio::test]
    async fn e2e_delete_in_anki_persists_through_re_translation() {
        let tmp = TempDir::new("flts_e2e_delete");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar.").await;

        let paragraph = one_sentence_paragraph(
            "Я могу войти.",
            vec![full_word(
                "Puedo", "poder", "мочь", "verb", &["могу"], false,
            )],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        // Sync #1: create the note.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_tag = "flts_spa_rus_poder_verb";
        let note_id = mock.note_id_for_tag(poder_tag).expect("note exists after first sync");

        // User deletes the note in Anki.
        mock.remove_note(note_id);
        assert!(
            mock.note_id_for_tag(poder_tag).is_none(),
            "post-removal there's no note for the tag"
        );

        // Sync #2: detection branch flips state to Deleted (findNotes returns 0
        // hits and the card previously had anki_data != None).
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let card = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(card.anki_data.as_ref().map(|a| a.state), Some(AnkiState::Deleted));

        // Re-encounter: applying the same paragraph must not regress state.
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();
        let card_after = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card_after.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Deleted),
            "re-encountering the paragraph must not reset state to Active"
        );

        // Sync #3: opt-out branch short-circuits — no addNote (no resurrection).
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert!(
            mock.note_id_for_tag(poder_tag).is_none(),
            "deleted card must not be re-added to Anki on subsequent syncs"
        );
        let card_final = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card_final.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Deleted),
            "state stays Deleted across the third sync"
        );
    }

    #[tokio::test]
    async fn e2e_translation_creates_cards_when_anki_unreachable() {
        let tmp = TempDir::new("flts_e2e_unreachable");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Puedo entrar en casa.").await;

        let paragraph = one_sentence_paragraph(
            "Я могу войти в дом.",
            vec![
                full_word("Puedo", "poder", "мочь", "verb", &["могу"], false),
                full_word("casa", "casa", "дом", "noun", &["дом"], false),
            ],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        // Card files exist on disk regardless of AnkiConnect state — the
        // translation pipeline writes through LibraryCardStore directly.
        for (lemma, pos) in [("poder", "verb"), ("casa", "noun")] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma, pos)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{lemma} card present on disk"));
            assert!(
                card.anki_data.is_none(),
                "no anki_data set before any successful sync"
            );
        }

        // A sync attempt against an unreachable Anki must leave the local
        // card store intact. Bootstrap may bubble (the version() probe is the
        // first call); per-card failures are recorded in backoff. Either way,
        // the disk state is the contract.
        let mock = MockAnkiConnect::new();
        mock.fail_next_n_calls(usize::MAX);
        let mut state = AnkiSyncState::new();
        let _ = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now()).await;

        for (lemma, pos) in [("poder", "verb"), ("casa", "noun")] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma, pos)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{lemma} card still on disk after failed sync"));
            assert!(
                card.anki_data.is_none(),
                "no anki_data after a fully-failing sync"
            );
        }
    }

    #[tokio::test]
    async fn e2e_sync_conflict_sibling_merges_then_syncs_union_to_anki() {
        let tmp = TempDir::new("flts_e2e_conflict_sync");
        let (library, book_id) =
            library_with_one_paragraph_book(tmp.path.join("lib"), "Yo puedo.").await;

        // Canonical card: one translation, one example.
        let paragraph = one_sentence_paragraph(
            "Я могу.",
            vec![full_word(
                "puedo", "poder", "мочь", "verb", &["могу"], false,
            )],
        );
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        // Drop a Syncthing-style conflict sibling carrying a divergent translation
        // and a different example. Mirrors Stage 3's load_merges_single_sync_conflict_sibling
        // layout in library_card.rs.
        let deck = tmp.path.join("lib").join("cards").join("spa-rus");
        let conflict_path = deck.join("poder_verb.sync-conflict-20260520-153912-XYZ.json");
        let conflict_card = Card {
            version: 1,
            id: "flts_spa_rus_poder_verb".into(),
            lemma: "poder".into(),
            part_of_speech: "verb".into(),
            translations: vec!["иметь возможность".into()],
            examples: vec![Example {
                source: "Tu puedes.".into(),
                translation: "Ты можешь.".into(),
                book_id,
                chapter: 0,
                paragraph: 0,
            }],
            anki_data: None,
        };
        let bytes = serde_json::to_vec_pretty(&conflict_card).unwrap();
        tokio::fs::write(&conflict_path, bytes).await.unwrap();

        // Sync runs sync_pass; LibraryCardStore::load auto-merges the sibling.
        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        let report = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(report.succeeded, 1);

        // The conflict sibling is gone; the canonical file carries the union.
        assert!(
            !conflict_path.exists(),
            "conflict sibling consumed by merge during sync_pass"
        );
        let merged = library
            .card_store()
            .load("spa", "rus", "poder", "verb")
            .await
            .unwrap()
            .expect("merged card on disk");
        assert_eq!(merged.translations, vec!["мочь", "иметь возможность"]);
        assert_eq!(merged.examples.len(), 2, "both examples present after merge");

        // Anki receives the merged content: Target joins both translations with
        // "; ", and Example carries both source/translation pairs sorted by
        // source (alphabetic), joined by "<br>".
        let note_id = mock
            .note_id_for_tag("flts_spa_rus_poder_verb")
            .expect("merged note pushed to Anki");
        let (fields, _) = mock.peek_note(note_id).unwrap();
        assert_eq!(
            fields.get("Target"),
            Some(&"мочь; иметь возможность".to_owned())
        );
        // The canonical example's source is derived from the paragraph's
        // words (`extract_card_updates` builds it via `render_example_source`),
        // so it's just "puedo" here. The conflict sibling carries a hand-written
        // source "Tu puedes." Uppercase 'T' (0x54) sorts before lowercase 'p'
        // (0x70), so the conflict example renders first.
        assert_eq!(
            fields.get("Example"),
            Some(&"Tu puedes. \u{2014} Ты можешь.<br>puedo \u{2014} Я могу.".to_owned()),
            "examples render alphabetically by source and join with <br>"
        );
    }
}
