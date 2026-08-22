use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Result, anyhow};
use isolang::Language;

use crate::anki::connect::{
    AnkiConnect, CardInfo, MultiSubAction, NewNote, NoteInfo, decode_multi_sub,
    decode_multi_sub_void,
};
use crate::anki::model::{FLTS_MODEL_NAME, bootstrap, deck_name};
use crate::card::{AnkiData, AnkiState, Card};
use crate::library::Library;

/// Spec cap on sub-actions per `multi` call.
const MULTI_BATCH_SIZE: usize = 50;

/// In-session sync orchestration state; reset on restart by design.
#[allow(dead_code)]
#[derive(Debug)]
pub struct AnkiSyncState {
    bootstrapped: bool,
    backoff: HashMap<String, BackoffEntry>,
    persistent_set: HashSet<String>,
    persistent_threshold: u32,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct BackoffEntry {
    failure_count: u32,
    next_attempt: tokio::time::Instant,
}

/// Summary of one `sync_pass`.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct SyncReport {
    pub total_cards: usize,
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub persistent_failures: Vec<String>,
}

const DEFAULT_PERSISTENT_THRESHOLD: u32 = 5;

/// Linear backoff capped at ten minutes; `n=0` means "not in backoff".
#[allow(dead_code)]
pub(crate) fn next_delay(n: u32) -> std::time::Duration {
    std::time::Duration::from_secs(60 * n.min(10) as u64)
}

/// An eligible card plus its per-card lock guard, held until the pass ends.
struct Eligible {
    card_id: String,
    src_str: String,
    tgt_str: String,
    src: Language,
    tgt: Language,
    card: Card,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl AnkiSyncState {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            bootstrapped: false,
            backoff: HashMap::new(),
            persistent_set: HashSet::new(),
            persistent_threshold: DEFAULT_PERSISTENT_THRESHOLD,
        }
    }

    #[allow(dead_code)]
    pub fn with_threshold(mut self, threshold: u32) -> Self {
        self.persistent_threshold = threshold;
        self
    }

    fn in_cooldown(&self, card_id: &str, now: tokio::time::Instant) -> bool {
        self.backoff
            .get(card_id)
            .is_some_and(|e| e.next_attempt > now)
    }

    fn record_success(&mut self, card_id: &str) {
        self.backoff.remove(card_id);
        self.persistent_set.remove(card_id);
    }

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

/// Run one sync pass over every card on disk, bootstrapping model+decks on the
/// first call. Phase 1 gathers eligible cards under their locks and batches
/// their `findNotes`; phase 2 classifies and applies the batched writes.
#[allow(dead_code)]
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
            .filter_map(|(s, t)| Some((Language::from_639_3(s)?, Language::from_639_3(t)?)))
            .collect();
        bootstrap(client, &lang_pairs).await?;
        state.bootstrapped = true;
    }

    let mut report = SyncReport::default();

    // Phase 1a: walk disk, acquire locks, load, filter.
    let mut eligible: Vec<Eligible> = Vec::new();

    for (src_str, tgt_str) in &pairs {
        let (Some(src), Some(tgt)) = (Language::from_639_3(src_str), Language::from_639_3(tgt_str))
        else {
            continue;
        };

        let card_files = card_store.list_cards_in_pair(src_str, tgt_str).await?;
        for lemma_slug in card_files {
            report.total_cards += 1;

            let card_id = crate::card::card_id(src_str, tgt_str, &lemma_slug);
            let lock_arc = card_store.lock_for(&card_id).await;
            let guard = lock_arc.lock_owned().await;

            let Some(card) = card_store.load(src_str, tgt_str, &lemma_slug).await? else {
                continue;
            };

            // Opt-out: counted as total, not attempted.
            if matches!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Suspended) | Some(AnkiState::Deleted)
            ) {
                continue;
            }
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

    // Phase 1b: batched findNotes. None = lookup failed; phase 2 counts it.
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
                if results.len() != chunk.len() {
                    // A mismatched result count would desync `lookups` from
                    // `eligible` and panic later; fail the whole chunk.
                    log::warn!(
                        "multi findNotes returned {} results for {} actions; treating batch as failed",
                        results.len(),
                        chunk.len()
                    );
                    for _ in 0..chunk.len() {
                        lookups.push(None);
                    }
                    continue;
                }
                for value in results {
                    match decode_multi_sub::<Vec<i64>>(value) {
                        Ok(hits) => lookups.push(Some(hits)),
                        Err(err) => {
                            log::warn!("multi findNotes sub-action failed: {err}");
                            if is_missing_resource_error(&err) {
                                log::info!(
                                    "Anki deck/model missing; clearing bootstrap flag so next sync re-creates it"
                                );
                                state.bootstrapped = false;
                            }
                            lookups.push(None);
                        }
                    }
                }
            }
            Err(err) => {
                log::warn!("multi findNotes batch failed: {err}");
                if is_missing_resource_error(&err) {
                    log::info!(
                        "Anki deck/model missing; clearing bootstrap flag so next sync re-creates it"
                    );
                    state.bootstrapped = false;
                }
                for _ in 0..chunk.len() {
                    lookups.push(None);
                }
            }
        }
    }

    // Phase 2: classify, then batch writes (2a), state pull (2b+2c), apply (2d).
    let actions: Vec<CardAction> = eligible
        .iter()
        .zip(lookups.iter())
        .map(|(e, hits)| match hits {
            None => CardAction::LookupFailed,
            Some(hits) if hits.is_empty() && e.card.anki_data.is_none() => CardAction::Add,
            Some(hits) if hits.is_empty() => CardAction::LocalDeleteOnly,
            Some(hits) => CardAction::UpdateNote(hits[0]),
        })
        .collect();

    // Data-loss guard: a wrong/empty collection reports 0 hits for every card,
    // which would irreversibly mark them all Deleted. Trust a LocalDeleteOnly
    // only with positive proof the collection holds FLTS notes.
    let any_note_found = actions
        .iter()
        .any(|a| matches!(a, CardAction::UpdateNote(_)));

    let mut write_outcomes = batch_writes(client, &eligible, &actions, state).await?;
    let (notes_by_id, cards_by_id) =
        batch_pull_state(client, &actions, &mut write_outcomes, state).await;

    for (idx, mut e) in eligible.into_iter().enumerate() {
        let pre_card = e.card.clone();
        let outcome: Result<()> = match &actions[idx] {
            CardAction::LookupFailed => Err(anyhow!("lookup batch failed for {}", e.card_id)),
            CardAction::LocalDeleteOnly => {
                if any_note_found {
                    e.card.anki_data = Some(AnkiData {
                        state: AnkiState::Deleted,
                        interval_days: None,
                        ease_factor: None,
                        fsrs_difficulty: None,
                        fsrs_stability: None,
                    });
                }
                Ok(())
            }
            CardAction::Add | CardAction::UpdateNote(_) => {
                // Move the outcome out; anyhow::Error isn't Clone.
                let outcome = std::mem::replace(&mut write_outcomes[idx], WriteOutcome::Skipped);
                match outcome {
                    WriteOutcome::Err(err) => Err(err),
                    WriteOutcome::Skipped => {
                        unreachable!("Add/UpdateNote actions must have a write outcome")
                    }
                    WriteOutcome::AddOk { note_id } | WriteOutcome::UpdateOk { note_id } => {
                        match notes_by_id.get(&note_id) {
                            None => Err(anyhow!("notes_info returned no entry for note {note_id}")),
                            Some(note) => {
                                let cards: Vec<CardInfo> = note
                                    .cards
                                    .iter()
                                    .filter_map(|cid| cards_by_id.get(cid).cloned())
                                    .collect();
                                if cards.is_empty() {
                                    Err(anyhow!("no cards returned for note {note_id}"))
                                } else if cards.iter().any(|c| c.is_suspended()) {
                                    e.card.anki_data = Some(AnkiData {
                                        state: AnkiState::Suspended,
                                        interval_days: None,
                                        ease_factor: None,
                                        fsrs_difficulty: None,
                                        fsrs_stability: None,
                                    });
                                    Ok(())
                                } else {
                                    e.card.anki_data = Some(active_data_from(&cards));
                                    Ok(())
                                }
                            }
                        }
                    }
                }
            }
        };

        match outcome {
            Ok(()) => {
                // Silent save: only `anki_data` changed, from our own
                // round-trip; waking the watcher would self-trigger a pass.
                if e.card != pre_card {
                    card_store
                        .save_without_wake(&e.card, &e.src_str, &e.tgt_str)
                        .await?;
                }
                state.record_success(&e.card_id);
                report.succeeded += 1;
            }
            Err(err) => {
                log::warn!("sync failed for {}: {err}", e.card_id);
                state.record_failure(&e.card_id, now);
                report.failed += 1;
                if is_missing_resource_error(&err) {
                    log::info!(
                        "Anki deck/model missing; clearing bootstrap flag so next sync re-creates it"
                    );
                    state.bootstrapped = false;
                }
            }
        }
    }

    // HashSet order varies; sort for a run-to-run deterministic report.
    let mut persistent_failures: Vec<String> = state.persistent_set.iter().cloned().collect();
    persistent_failures.sort();
    report.persistent_failures = persistent_failures;
    Ok(report)
}

/// What phase 2 should do with one eligible card.
#[derive(Debug)]
enum CardAction {
    /// No prior anki_data and no note found: create it.
    Add,
    /// Note exists (the id): push fields, pull state.
    UpdateNote(i64),
    /// Had anki_data but the note is gone — deleted in Anki out-of-band.
    /// Applied only under `sync_pass`'s `any_note_found` guard.
    LocalDeleteOnly,
    /// Phase 1b's chunk failed; recorded as a failure without further calls.
    LookupFailed,
}

/// Result of one card's Phase 2a write attempt.
#[derive(Debug)]
enum WriteOutcome {
    /// Never entered the write batch, or its outcome was already consumed.
    Skipped,
    AddOk {
        note_id: i64,
    },
    UpdateOk {
        note_id: i64,
    },
    Err(anyhow::Error),
}

/// Phase 2a: batched addNote / updateNoteFields. Returns one `WriteOutcome`
/// per element of `eligible`, index-aligned. Clears `state.bootstrapped` when
/// an error says the deck/model is missing.
async fn batch_writes(
    client: &dyn AnkiConnect,
    eligible: &[Eligible],
    actions: &[CardAction],
    state: &mut AnkiSyncState,
) -> Result<Vec<WriteOutcome>> {
    struct PendingWrite {
        idx: usize,
        kind: WriteKind,
    }
    enum WriteKind {
        Add,
        UpdateNoteFields { note_id: i64 },
    }

    let mut pending: Vec<PendingWrite> = Vec::new();
    for (idx, action) in actions.iter().enumerate() {
        match action {
            CardAction::Add => pending.push(PendingWrite {
                idx,
                kind: WriteKind::Add,
            }),
            CardAction::UpdateNote(note_id) => pending.push(PendingWrite {
                idx,
                kind: WriteKind::UpdateNoteFields { note_id: *note_id },
            }),
            CardAction::LocalDeleteOnly | CardAction::LookupFailed => {}
        }
    }

    let mut outcomes: Vec<WriteOutcome> =
        (0..eligible.len()).map(|_| WriteOutcome::Skipped).collect();

    for chunk in pending.chunks(MULTI_BATCH_SIZE) {
        let mut sub_actions: Vec<MultiSubAction> = Vec::with_capacity(chunk.len());
        for p in chunk {
            let e = &eligible[p.idx];
            match &p.kind {
                WriteKind::Add => {
                    let note = NewNote {
                        deck_name: deck_name(e.src, e.tgt)?,
                        model_name: FLTS_MODEL_NAME.to_owned(),
                        fields: render_fields(&e.card),
                        tags: vec![e.card_id.clone()],
                    };
                    sub_actions.push(MultiSubAction {
                        action: "addNote".to_owned(),
                        params: Some(serde_json::json!({ "note": note })),
                    });
                }
                WriteKind::UpdateNoteFields { note_id } => {
                    sub_actions.push(MultiSubAction {
                        action: "updateNoteFields".to_owned(),
                        params: Some(serde_json::json!({
                            "note": {
                                "id": note_id,
                                "fields": render_fields(&e.card),
                            }
                        })),
                    });
                }
            }
        }

        match client.multi(sub_actions).await {
            Ok(results) => {
                for (p, value) in chunk.iter().zip(results) {
                    let outcome = match &p.kind {
                        WriteKind::Add => match decode_multi_sub::<i64>(value) {
                            Ok(note_id) => WriteOutcome::AddOk { note_id },
                            Err(err) => {
                                if is_missing_resource_error(&err) {
                                    state.bootstrapped = false;
                                }
                                WriteOutcome::Err(err)
                            }
                        },
                        WriteKind::UpdateNoteFields { note_id } => {
                            match decode_multi_sub_void(value) {
                                Ok(()) => WriteOutcome::UpdateOk { note_id: *note_id },
                                Err(err) => {
                                    if is_missing_resource_error(&err) {
                                        state.bootstrapped = false;
                                    }
                                    WriteOutcome::Err(err)
                                }
                            }
                        }
                    };
                    outcomes[p.idx] = outcome;
                }
            }
            Err(err) => {
                log::warn!("multi write batch failed: {err}");
                if is_missing_resource_error(&err) {
                    log::info!(
                        "Anki deck/model missing; clearing bootstrap flag so next sync re-creates it"
                    );
                    state.bootstrapped = false;
                }
                let msg = err.to_string();
                for p in chunk {
                    outcomes[p.idx] = WriteOutcome::Err(anyhow!("multi write batch failed: {msg}"));
                }
            }
        }
    }

    Ok(outcomes)
}

/// Phase 2b + 2c: one `notes_info` then one `cards_info`, keyed by id. A
/// failure downgrades the matching `write_outcomes[i]` to Err so phase 2d
/// counts the card failed; the next tick reconciles it idempotently.
async fn batch_pull_state(
    client: &dyn AnkiConnect,
    actions: &[CardAction],
    write_outcomes: &mut [WriteOutcome],
    state: &mut AnkiSyncState,
) -> (HashMap<i64, NoteInfo>, HashMap<i64, CardInfo>) {
    let pull_note_ids: Vec<i64> = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            WriteOutcome::AddOk { note_id } | WriteOutcome::UpdateOk { note_id } => Some(*note_id),
            _ => None,
        })
        .collect();
    let _ = actions;

    if pull_note_ids.is_empty() {
        return (HashMap::new(), HashMap::new());
    }

    let notes_by_id: HashMap<i64, NoteInfo> = match client.notes_info(&pull_note_ids).await {
        Ok(infos) => infos.into_iter().map(|n| (n.note_id, n)).collect(),
        Err(err) => {
            log::warn!("notes_info batch failed: {err}");
            if is_missing_resource_error(&err) {
                state.bootstrapped = false;
            }
            let msg = err.to_string();
            for outcome in write_outcomes.iter_mut() {
                if matches!(
                    outcome,
                    WriteOutcome::AddOk { .. } | WriteOutcome::UpdateOk { .. }
                ) {
                    *outcome = WriteOutcome::Err(anyhow!("notes_info failed: {msg}"));
                }
            }
            return (HashMap::new(), HashMap::new());
        }
    };

    let all_card_ids: Vec<i64> = notes_by_id
        .values()
        .flat_map(|n| n.cards.iter().copied())
        .collect();
    if all_card_ids.is_empty() {
        return (notes_by_id, HashMap::new());
    }

    let cards_by_id: HashMap<i64, CardInfo> = match client.cards_info(&all_card_ids).await {
        Ok(cards) => cards.into_iter().map(|c| (c.card_id, c)).collect(),
        Err(err) => {
            log::warn!("cards_info batch failed: {err}");
            if is_missing_resource_error(&err) {
                state.bootstrapped = false;
            }
            let msg = err.to_string();
            for outcome in write_outcomes.iter_mut() {
                if matches!(
                    outcome,
                    WriteOutcome::AddOk { .. } | WriteOutcome::UpdateOk { .. }
                ) {
                    *outcome = WriteOutcome::Err(anyhow!("cards_info failed: {msg}"));
                }
            }
            return (notes_by_id, HashMap::new());
        }
    };

    (notes_by_id, cards_by_id)
}

/// True when the deck or model was deleted in Anki out-of-band. Walks the
/// whole chain so wrapped errors match; callers re-bootstrap on a hit.
fn is_missing_resource_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let s = cause.to_string().to_lowercase();
        s.contains("deck was not found")
            || s.contains("deck not found")
            || s.contains("model was not found")
            || s.contains("model not found")
    })
}

/// Render a card into the Anki note fields; see
/// `.specs/ANKI_REFINED.md § Field contents pushed to Anki`.
#[allow(dead_code)]
pub(crate) fn render_fields(card: &Card) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    out.insert("Source".into(), card.lemma.clone());
    out.insert("Target".into(), card.translations_flat().join("; "));

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

/// Push one card and pull back its state into `card.anki_data`. The caller
/// owns load/lock/save.
#[allow(dead_code)]
pub async fn sync_card(
    client: &dyn AnkiConnect,
    card: &mut Card,
    src: Language,
    tgt: Language,
) -> Result<()> {
    // Opted out in Anki: never re-push, never overwrite the explicit state.
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
                // Note deleted in Anki: mark deleted, never re-add.
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
    use crate::anki::sync::{AnkiSyncState, next_delay, render_fields, sync_card, sync_pass};
    use crate::card::{AnkiData, AnkiState, Card, Example};
    use crate::library::Library;
    use crate::test_utils::{TempDir, full_word, one_sentence_paragraph};

    fn make_card(lemma: &str, translations: Vec<&str>, examples: Vec<Example>) -> Card {
        let mut by_pos: BTreeMap<String, Vec<String>> = BTreeMap::new();
        by_pos.insert(
            "verb".into(),
            translations.into_iter().map(String::from).collect(),
        );
        Card {
            version: 2,
            id: format!("flts_spa_rus_{lemma}"),
            lemma: lemma.into(),
            translations: by_pos,
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

        let hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
        assert!(hits.is_empty(), "suspended card must not be pushed");
        assert_eq!(
            card.anki_data.as_ref(),
            Some(&before),
            "anki_data preserved"
        );
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
        assert_eq!(
            card.anki_data.as_ref(),
            Some(&before),
            "anki_data preserved"
        );
    }

    #[tokio::test]
    async fn sync_card_flags_suspension_when_any_card_suspended_in_anki() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();
        let note_id = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap()[0];
        let cards = mock.notes_info(&[note_id]).await.unwrap()[0].cards.clone();
        mock.suspend_card(cards[0]);

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let anki = card.anki_data.as_ref().expect("anki_data populated");
        assert_eq!(anki.state, AnkiState::Suspended);
        assert_eq!(
            anki.interval_days, None,
            "retention fields dropped on suspended"
        );
        assert_eq!(anki.ease_factor, None);
    }

    #[tokio::test]
    async fn sync_card_flags_deletion_when_note_vanished_from_anki() {
        let mock = bootstrap_mock().await;
        let mut card = make_card("poder", vec!["мочь"], vec![]);
        card.anki_data = Some(AnkiData {
            state: AnkiState::Active,
            interval_days: Some(30.0),
            ease_factor: Some(2.5),
            fsrs_difficulty: None,
            fsrs_stability: None,
        });

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

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

        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();
        let original_hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
        assert_eq!(original_hits.len(), 1);
        let note_id = original_hits[0];

        card.translations
            .entry("verb".into())
            .or_default()
            .push("уметь".into());
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();

        let hits_after = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
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

        let hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
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

        let models = mock.model_names_and_ids().await.unwrap();
        assert!(models.contains_key(crate::anki::model::FLTS_MODEL_NAME));
        let decks = mock.deck_names_and_ids().await.unwrap();
        assert!(decks.contains_key("FLTS::Español-Русский"));

        for lemma in ["poder", "comer"] {
            let id = format!("flts_spa_rus_{lemma}");
            let hits = mock.find_notes(&format!("tag:{id}")).await.unwrap();
            assert_eq!(hits.len(), 1, "expected one note for {id}");
        }

        for lemma in ["poder", "comer"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
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
    async fn sync_pass_clears_bootstrapped_when_deck_deleted_out_of_band() {
        use std::time::Duration;

        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_deck_deleted",
            &[make_card("poder", vec!["мочь"], vec![])],
        )
        .await;

        let mut state = AnkiSyncState::new();

        let r1 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r1.succeeded, 1);
        assert!(state.bootstrapped);
        assert!(
            mock.deck_names_and_ids()
                .await
                .unwrap()
                .contains_key("FLTS::Español-Русский")
        );

        mock.remove_deck("FLTS::Español-Русский");

        // An Active card takes the update path, which hits the missing deck.
        tokio::time::advance(Duration::from_secs(1)).await;
        let r2 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r2.failed, 1);
        assert_eq!(r2.succeeded, 0);
        assert!(
            !state.bootstrapped,
            "missing-deck error must invalidate the bootstrap gate"
        );

        tokio::time::advance(Duration::from_secs(61)).await;
        let r3 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r3.succeeded, 1);
        assert!(state.bootstrapped);
        assert!(
            mock.deck_names_and_ids()
                .await
                .unwrap()
                .contains_key("FLTS::Español-Русский"),
            "bootstrap must have re-created the deleted deck"
        );
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

        let mut state = AnkiSyncState::new();
        let now0 = tokio::time::Instant::now();
        let report0 = sync_pass(&mock, &library, &mut state, now0).await.unwrap();
        assert_eq!(report0.succeeded, 1);
        assert_eq!(report0.failed, 0);

        // An already-Active card hits find_notes first, so that call fails.
        mock.fail_next_n_calls(1);

        let now1 = tokio::time::Instant::now();
        let report1 = sync_pass(&mock, &library, &mut state, now1).await.unwrap();
        assert_eq!(report1.attempted, 1);
        assert_eq!(report1.failed, 1);
        assert_eq!(report1.succeeded, 0);

        let report2 = sync_pass(&mock, &library, &mut state, now1).await.unwrap();
        assert_eq!(report2.attempted, 0, "card must be skipped during cooldown");
        assert_eq!(report2.total_cards, 1);

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

        let mut state = AnkiSyncState::new().with_threshold(3);
        let card_id = format!("flts_spa_rus_poder");

        // Pre-bootstrap so the injected failures land on sync_card.
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

        let r1 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r1.failed, 1);
        assert!(
            r1.persistent_failures.is_empty(),
            "after 1 failure: not persistent yet"
        );

        tokio::time::advance(Duration::from_secs(61)).await;
        let r2 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r2.failed, 1);
        assert!(
            r2.persistent_failures.is_empty(),
            "after 2 failures: not persistent yet"
        );

        tokio::time::advance(Duration::from_secs(121)).await;
        let r3 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r3.failed, 1);
        assert_eq!(
            r3.persistent_failures,
            vec![card_id.clone()],
            "after threshold hit: surfaced"
        );

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

        // Pre-bootstrap so failures land on per-card sync; bootstrap itself is
        // retried unconditionally.
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

        for lemma in ["poder", "comer", "ver", "ir", "ser"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
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

        // Fresh cards skip the lookup batch; the second pass exercises it.
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
            2,
            "expected 1 findNotes multi + 1 updateNoteFields multi for 3 cards"
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
            4,
            "75 cards: 2 findNotes chunks + 2 updateNoteFields chunks = 4 multi calls"
        );
    }

    #[tokio::test]
    async fn sync_pass_phase_2a_batches_writes_via_multi() {
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_phase_2a_batches",
            &[
                make_card("poder", vec!["мочь"], vec![]),
                make_card("comer", vec!["есть"], vec![]),
                make_card("ver", vec!["видеть"], vec![]),
            ],
        )
        .await;

        let mut state = AnkiSyncState::new();
        let multi_before = mock.multi_call_count();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let multi_after = mock.multi_call_count();
        assert_eq!(
            multi_after - multi_before,
            2,
            "first pass over 3 fresh cards: 1 findNotes batch + 1 addNote batch"
        );
        for lemma in ["poder", "comer", "ver"] {
            let tag = format!("flts_spa_rus_{lemma}");
            assert!(
                mock.note_id_for_tag(&tag).is_some(),
                "note for {tag} must exist after batched add"
            );
        }
    }

    #[tokio::test]
    async fn sync_pass_uses_single_notes_info_and_cards_info_per_pass() {
        let mock = MockAnkiConnect::new();
        let cards: Vec<Card> = (0..5)
            .map(|i| make_card(&format!("verb{i}"), vec!["x"], vec![]))
            .collect();
        let (_tmp, library) =
            seed_library_with_cards("flts_sync_state_pull_singletons", &cards).await;

        let mut state = AnkiSyncState::new();
        let notes_before = mock.notes_info_call_count();
        let cards_before = mock.cards_info_call_count();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(
            mock.notes_info_call_count() - notes_before,
            1,
            "state pull must collapse to a single notes_info call across all 5 cards"
        );
        assert_eq!(
            mock.cards_info_call_count() - cards_before,
            1,
            "state pull must collapse to a single cards_info call across all 5 cards"
        );

        let notes_before = mock.notes_info_call_count();
        let cards_before = mock.cards_info_call_count();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(
            mock.notes_info_call_count() - notes_before,
            1,
            "second pass must also use one notes_info call"
        );
        assert_eq!(
            mock.cards_info_call_count() - cards_before,
            1,
            "second pass must also use one cards_info call"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn sync_pass_isolates_per_sub_action_failure_in_phase_2a() {
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_per_sub_action_failure",
            &[
                make_card("good_a", vec!["a"], vec![]),
                make_card("bad", vec!["b"], vec![]),
                make_card("good_b", vec!["c"], vec![]),
            ],
        )
        .await;

        // Pre-bootstrap so the injected failure lands on phase 2a.
        let mut state = AnkiSyncState::new();
        crate::anki::model::bootstrap(&mock, &[(spa(), rus())])
            .await
            .unwrap();
        state.bootstrapped = true;

        mock.fail_add_note_with_tag("flts_spa_rus_bad");

        let now = tokio::time::Instant::now();
        let report = sync_pass(&mock, &library, &mut state, now).await.unwrap();
        assert_eq!(report.succeeded, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.attempted, 3);

        // Only the bad card enters cooldown.
        for lemma in ["good_a", "good_b"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
                .await
                .unwrap()
                .expect("card present");
            assert_eq!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Active),
                "{lemma} must end Active when its sub-action succeeded"
            );
        }
        let bad = library
            .card_store()
            .load("spa", "rus", "bad")
            .await
            .unwrap()
            .expect("bad card present");
        assert!(
            bad.anki_data.is_none(),
            "bad card must not be marked Active when its addNote sub-action failed"
        );
    }

    #[tokio::test]
    async fn sync_pass_local_delete_branch_skips_phase_2a_writes() {
        // Lone LocalDeleteOnly card: nothing corroborates the collection, so
        // the guard must leave its state intact and skip the phase 2a batch.
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_local_delete_only",
            &[make_card("poder", vec!["мочь"], vec![])],
        )
        .await;

        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let note_id = mock
            .note_id_for_tag("flts_spa_rus_poder")
            .expect("note exists after first pass");

        mock.remove_note(note_id);

        let multi_before = mock.multi_call_count();
        let notes_before = mock.notes_info_call_count();
        let cards_before = mock.cards_info_call_count();
        let report = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let multi_after = mock.multi_call_count();
        assert_eq!(report.succeeded, 1);
        assert_eq!(
            multi_after - multi_before,
            1,
            "LocalDeleteOnly must skip Phase 2a; only the Phase 1b findNotes multi fires"
        );
        assert_eq!(mock.notes_info_call_count() - notes_before, 0);
        assert_eq!(mock.cards_info_call_count() - cards_before, 0);

        let card = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Active),
            "with no corroborating hit in the pass, the guard must leave state \
             intact rather than flip a lone card to Deleted"
        );
    }

    #[tokio::test]
    async fn sync_pass_guard_leaves_all_states_intact_when_no_note_matches() {
        // Wrong/empty collection: every synced card reports 0 hits. The guard
        // must not mass-flip them to Deleted.
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_guard_all_zero",
            &[
                make_card("poder", vec!["мочь"], vec![]),
                make_card("comer", vec!["есть"], vec![]),
            ],
        )
        .await;

        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        for lemma in ["poder", "comer"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
                .await
                .unwrap()
                .expect("card present");
            assert_eq!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Active),
                "{lemma} must be Active after first sync"
            );
        }

        for lemma in ["poder", "comer"] {
            let note_id = mock
                .note_id_for_tag(&format!("flts_spa_rus_{lemma}"))
                .expect("note exists");
            mock.remove_note(note_id);
        }

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        for lemma in ["poder", "comer"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
                .await
                .unwrap()
                .expect("card present");
            assert_eq!(
                card.anki_data.as_ref().map(|a| a.state),
                Some(AnkiState::Active),
                "{lemma} must NOT be flipped to Deleted when no card in the pass matched a note"
            );
        }
    }

    #[tokio::test]
    async fn sync_pass_guard_honors_single_delete_when_another_card_matches() {
        // comer's hit corroborates the collection, so the guard honors poder's
        // genuine out-of-band deletion.
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_guard_mixed",
            &[
                make_card("poder", vec!["мочь"], vec![]),
                make_card("comer", vec!["есть"], vec![]),
            ],
        )
        .await;

        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_note = mock
            .note_id_for_tag("flts_spa_rus_poder")
            .expect("poder note exists");
        mock.remove_note(poder_note);

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("poder present");
        assert_eq!(
            poder.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Deleted),
            "a genuine single deletion is still honored when another card corroborates"
        );

        let comer = library
            .card_store()
            .load("spa", "rus", "comer")
            .await
            .unwrap()
            .expect("comer present");
        assert_eq!(
            comer.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Active),
            "the surviving card stays Active"
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

        let poder_tag = "flts_spa_rus_poder";
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
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("poder card on disk");
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Active),
            "card state flips to Active after first sync"
        );

        assert!(
            mock.note_id_for_tag("flts_spa_rus_casa").is_some(),
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
                "Puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_tag = "flts_spa_rus_poder";
        let note_id = mock
            .note_id_for_tag(poder_tag)
            .expect("note exists after first sync");

        let card_ids = mock.notes_info(&[note_id]).await.unwrap()[0].cards.clone();
        assert!(!card_ids.is_empty(), "note has at least one direction card");
        mock.suspend_card(card_ids[0]);

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let card = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Suspended)
        );

        let (fields_before, _) = mock.peek_note(note_id).unwrap();

        // The local merge path is idempotent; state must not regress to Active.
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();
        let card_after_reencounter = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card_after_reencounter.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Suspended),
            "re-encountering the paragraph must not reset state to Active"
        );

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        assert_eq!(
            mock.note_id_for_tag(poder_tag),
            Some(note_id),
            "no second note created for the same tag"
        );
        let (fields_after, _) = mock.peek_note(note_id).unwrap();
        assert_eq!(
            fields_before, fields_after,
            "suspended note fields untouched"
        );

        let card_final = library
            .card_store()
            .load("spa", "rus", "poder")
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
                "Puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        // A co-resident synced card corroborates the collection, so the guard
        // honors poder's deletion.
        library
            .card_store()
            .save(&make_card("comer", vec!["есть"], vec![]), "spa", "rus")
            .await
            .unwrap();

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_tag = "flts_spa_rus_poder";
        let note_id = mock
            .note_id_for_tag(poder_tag)
            .expect("note exists after first sync");

        mock.remove_note(note_id);
        assert!(
            mock.note_id_for_tag(poder_tag).is_none(),
            "post-removal there's no note for the tag"
        );

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let card = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Deleted)
        );

        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();
        let card_after = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card_after.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Deleted),
            "re-encountering the paragraph must not reset state to Active"
        );

        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert!(
            mock.note_id_for_tag(poder_tag).is_none(),
            "deleted card must not be re-added to Anki on subsequent syncs"
        );
        let card_final = library
            .card_store()
            .load("spa", "rus", "poder")
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

        for lemma in ["poder", "casa"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("{lemma} card present on disk"));
            assert!(
                card.anki_data.is_none(),
                "no anki_data set before any successful sync"
            );
        }

        // An unreachable Anki must leave the local card store intact.
        let mock = MockAnkiConnect::new();
        mock.fail_next_n_calls(usize::MAX);
        let mut state = AnkiSyncState::new();
        let _ = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now()).await;

        for lemma in ["poder", "casa"] {
            let card = library
                .card_store()
                .load("spa", "rus", lemma)
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

        let paragraph = one_sentence_paragraph(
            "Я могу.",
            vec![full_word(
                "puedo",
                "poder",
                "мочь",
                "verb",
                &["могу"],
                false,
            )],
        );
        library
            .apply_paragraph_to_cards(book_id, 0, &paragraph, rus())
            .await
            .unwrap();

        // Syncthing-style conflict sibling with a divergent translation.
        let deck = tmp.path.join("lib").join("cards").join("spa-rus");
        let conflict_path = deck.join("poder.sync-conflict-20260520-153912-XYZ.json");
        let mut conflict_translations: BTreeMap<String, Vec<String>> = BTreeMap::new();
        conflict_translations.insert("verb".into(), vec!["иметь возможность".into()]);
        let conflict_card = Card {
            version: 2,
            id: "flts_spa_rus_poder".into(),
            lemma: "poder".into(),
            translations: conflict_translations,
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

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        let report = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(report.succeeded, 1);

        assert!(
            !conflict_path.exists(),
            "conflict sibling consumed by merge during sync_pass"
        );
        let merged = library
            .card_store()
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .expect("merged card on disk");
        assert_eq!(
            merged.translations_flat(),
            vec!["мочь", "иметь возможность"]
        );
        assert_eq!(
            merged.examples.len(),
            2,
            "both examples present after merge"
        );

        let note_id = mock
            .note_id_for_tag("flts_spa_rus_poder")
            .expect("merged note pushed to Anki");
        let (fields, _) = mock.peek_note(note_id).unwrap();
        assert_eq!(
            fields.get("Target"),
            Some(&"мочь; иметь возможность".to_owned())
        );
        // Examples sort by source: uppercase 'T' precedes lowercase 'p'.
        assert_eq!(
            fields.get("Example"),
            Some(&"Tu puedes. \u{2014} Ты можешь.<br>puedo \u{2014} Я могу.".to_owned()),
            "examples render alphabetically by source and join with <br>"
        );
    }
}
