// Stage 6: per-card push/pull. Stage 7 wraps this in a periodic loop.

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

/// One eligible card after Phase 1a's filtering. Carries the loaded card,
/// its identifiers, and the owned per-card lock guard (dropped at end of pass).
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

    // Phase 2: classify each eligible card into one action kind, then run
    // batched writes (Phase 2a), batched state pull (2b + 2c), and apply
    // results per card (2d). Replaces the old per-card serial state machine.
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

    let mut write_outcomes = batch_writes(client, &eligible, &actions, state).await?;
    let (notes_by_id, cards_by_id) =
        batch_pull_state(client, &actions, &mut write_outcomes, state).await;

    for (idx, mut e) in eligible.into_iter().enumerate() {
        let pre_card = e.card.clone();
        let outcome: Result<()> = match &actions[idx] {
            CardAction::LookupFailed => Err(anyhow!("lookup batch failed for {}", e.card_id)),
            CardAction::LocalDeleteOnly => {
                e.card.anki_data = Some(AnkiData {
                    state: AnkiState::Deleted,
                    interval_days: None,
                    ease_factor: None,
                    fsrs_difficulty: None,
                    fsrs_stability: None,
                });
                Ok(())
            }
            CardAction::Add | CardAction::UpdateNote(_) => {
                // Take ownership of the write outcome so we can move the Err
                // out (anyhow::Error doesn't Clone).
                let outcome =
                    std::mem::replace(&mut write_outcomes[idx], WriteOutcome::Skipped);
                match outcome {
                    WriteOutcome::Err(err) => Err(err),
                    WriteOutcome::Skipped => {
                        unreachable!("Add/UpdateNote actions must have a write outcome")
                    }
                    WriteOutcome::AddOk { note_id } | WriteOutcome::UpdateOk { note_id } => {
                        match notes_by_id.get(&note_id) {
                            None => {
                                Err(anyhow!("notes_info returned no entry for note {note_id}"))
                            }
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
                // Skip the disk write — and the resulting watcher event —
                // when nothing changed. Use the silent save: only `anki_data`
                // was touched in response to our own AnkiConnect round-trip;
                // waking ourselves would self-trigger a redundant pass.
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

    report.persistent_failures = state.persistent_set.iter().cloned().collect();
    Ok(report)
}

/// Classification of one eligible card based on its Phase 1b lookup result
/// and its prior `anki_data`. Drives the Phase 2 batched dispatch.
#[derive(Debug)]
enum CardAction {
    /// Fresh card (no prior anki_data, findNotes returned 0 hits): create
    /// the note via addNote and pull its state.
    Add,
    /// Existing note (findNotes returned ≥1 hit): push current fields via
    /// updateNoteFields and pull its state. The carried i64 is the note id.
    UpdateNote(i64),
    /// Card had prior anki_data but findNotes returned 0 hits — user
    /// deleted the note in Anki out-of-band. Mark Deleted locally; no HTTP.
    LocalDeleteOnly,
    /// Phase 1b's lookup batch failed for this card's chunk. Skip Phase 2a/b/c
    /// and record a failure in Phase 2d.
    LookupFailed,
}

/// Result of one card's Phase 2a write attempt.
#[derive(Debug)]
enum WriteOutcome {
    /// Card didn't enter the write batch (LocalDeleteOnly / LookupFailed) or
    /// its outcome has already been consumed.
    Skipped,
    /// addNote succeeded; the new note id is carried for the state-pull phase.
    AddOk { note_id: i64 },
    /// updateNoteFields succeeded; the existing note id is carried for the
    /// state-pull phase.
    UpdateOk { note_id: i64 },
    /// addNote or updateNoteFields errored (either a per-sub-action error
    /// inside the multi response, or the whole multi HTTP call failed).
    Err(anyhow::Error),
}

/// Phase 2a: batched addNote / updateNoteFields via `multi`, chunked at
/// `MULTI_BATCH_SIZE`. Returns one `WriteOutcome` per element of `eligible`.
/// Flips `state.bootstrapped = false` on any error whose chain indicates a
/// missing deck/model.
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
                    outcomes[p.idx] =
                        WriteOutcome::Err(anyhow!("multi write batch failed: {msg}"));
                }
            }
        }
    }

    Ok(outcomes)
}

/// Phase 2b + 2c: single `notes_info` plural call followed by a single
/// `cards_info` plural call. Returns lookup maps keyed by id. On either
/// plural-call failure, downgrades the corresponding `write_outcomes[i]` from
/// AddOk/UpdateOk to Err so Phase 2d records those cards as failed; next tick
/// reconciles via findNotes → idempotent updateNoteFields → retry pull.
async fn batch_pull_state(
    client: &dyn AnkiConnect,
    actions: &[CardAction],
    write_outcomes: &mut [WriteOutcome],
    state: &mut AnkiSyncState,
) -> (HashMap<i64, NoteInfo>, HashMap<i64, CardInfo>) {
    let pull_note_ids: Vec<i64> = write_outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            WriteOutcome::AddOk { note_id } | WriteOutcome::UpdateOk { note_id } => {
                Some(*note_id)
            }
            _ => None,
        })
        .collect();
    let _ = actions; // reserved for future per-action diagnostics

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

/// Detects AnkiConnect "…was not found" failures for the FLTS deck or note
/// model — i.e. the user deleted the deck/model in Anki out-of-band. Walks the
/// anyhow chain so wrapped errors are matched too. The next `sync_pass` will
/// re-run `bootstrap()` (idempotent for both `create_model` and `create_deck`)
/// and the failing cards retry via the existing backoff/cooldown.
fn is_missing_resource_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let s = cause.to_string().to_lowercase();
        s.contains("deck was not found")
            || s.contains("deck not found")
            || s.contains("model was not found")
            || s.contains("model not found")
    })
}

/// Render a card into the three Anki note fields (`Source`, `Target`, `Example`).
/// See `.specs/ANKI_REFINED.md § Field contents pushed to Anki`.
#[allow(dead_code)] // first non-test consumer is the Stage 7 sync orchestrator
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

        // No note created, no AnkiConnect mutation visible to find_notes.
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

        // First push to create the note + cards, then suspend one of them.
        sync_card(&mock, &mut card, spa(), rus()).await.unwrap();
        let note_id = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap()[0];
        let cards = mock.notes_info(&[note_id]).await.unwrap()[0].cards.clone();
        mock.suspend_card(cards[0]); // suspend just one direction

        // Force a re-sync; the existing-note branch should detect suspension.
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
        let original_hits = mock.find_notes(&format!("tag:{}", card.id)).await.unwrap();
        assert_eq!(original_hits.len(), 1);
        let note_id = original_hits[0];

        // Mutate translations locally, sync again — should update, not create.
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

        // Bootstrap occurred: model and deck must exist.
        let models = mock.model_names_and_ids().await.unwrap();
        assert!(models.contains_key(crate::anki::model::FLTS_MODEL_NAME));
        let decks = mock.deck_names_and_ids().await.unwrap();
        assert!(decks.contains_key("FLTS::Español-Русский"));

        // Each card got a note tagged with its id.
        for lemma in ["poder", "comer"] {
            let id = format!("flts_spa_rus_{lemma}");
            let hits = mock.find_notes(&format!("tag:{id}")).await.unwrap();
            assert_eq!(hits.len(), 1, "expected one note for {id}");
        }

        // Reloaded cards have Active anki_data.
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

        // Tick 1: happy path — bootstraps, deck created, card pushed.
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

        // User deletes the deck in Anki out-of-band.
        mock.remove_deck("FLTS::Español-Русский");

        // Tick 2: card is Active locally, so sync_pass goes through the
        // update_note_fields path — which now fails with the missing-deck
        // string. sync_pass must record the failure AND clear bootstrapped.
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

        // Tick 3: after the backoff window, sync_pass re-runs bootstrap (deck
        // reappears) and the card succeeds again.
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
        let card_id = format!("flts_spa_rus_poder");

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
        assert!(
            r1.persistent_failures.is_empty(),
            "after 1 failure: not persistent yet"
        );

        tokio::time::advance(Duration::from_secs(61)).await;
        // Tick 2: second failure. Still not persistent.
        let r2 = sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(r2.failed, 1);
        assert!(
            r2.persistent_failures.is_empty(),
            "after 2 failures: not persistent yet"
        );

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
        // Second pass over 3 existing notes: one multi for Phase 1b findNotes,
        // one multi for Phase 2a updateNoteFields. Both fit in a single 50-cap
        // chunk.
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
        // 2 for Phase 1b findNotes (50+25) + 2 for Phase 2a updateNoteFields
        // (50+25). Both phases use the same MULTI_BATCH_SIZE=50 cap.
        assert_eq!(
            multi_after - multi_before,
            4,
            "75 cards: 2 findNotes chunks + 2 updateNoteFields chunks = 4 multi calls"
        );
    }

    #[tokio::test]
    async fn sync_pass_phase_2a_batches_writes_via_multi() {
        // Three fresh cards: Phase 1b runs one findNotes multi (all hits empty);
        // Phase 2a runs one addNote multi (all in a single 50-cap chunk). No
        // per-card add_note path should fire on its own.
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
        // Three notes really did get created.
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
        // First pass: 5 fresh cards → addNote batch → state pull.
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

        // Second pass: 5 existing cards → updateNoteFields batch → state pull.
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

        // Pre-bootstrap so the failure injection lands on Phase 2a, not on
        // bootstrap's create_model/create_deck.
        let mut state = AnkiSyncState::new();
        crate::anki::model::bootstrap(&mock, &[(spa(), rus())])
            .await
            .unwrap();
        state.bootstrapped = true;

        // Flag the middle card; its addNote sub-action will fail inside the
        // multi response while the other two succeed.
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
        // Card has prior anki_data (was Active), but the matching note in Anki
        // has been removed out-of-band. findNotes returns 0 hits → LocalDeleteOnly.
        // That path must NOT enter the Phase 2a multi batch; only Phase 1b's
        // findNotes multi fires.
        let mock = MockAnkiConnect::new();
        let (_tmp, library) = seed_library_with_cards(
            "flts_sync_local_delete_only",
            &[make_card("poder", vec!["мочь"], vec![])],
        )
        .await;

        let mut state = AnkiSyncState::new();
        // First pass: creates the note normally.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();
        let note_id = mock
            .note_id_for_tag("flts_spa_rus_poder")
            .expect("note exists after first pass");

        // User deletes the note in Anki out-of-band.
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
        // No notes_info / cards_info either — no notes to pull state for.
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
            Some(AnkiState::Deleted),
            "LocalDeleteOnly must flip state to Deleted"
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

        // Sanity: the noun gets its own note in the same deck.
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
        // Sync #1: create the note.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_tag = "flts_spa_rus_poder";
        let note_id = mock
            .note_id_for_tag(poder_tag)
            .expect("note exists after first sync");

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
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Suspended)
        );

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
            .load("spa", "rus", "poder")
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

        let mock = MockAnkiConnect::new();
        let mut state = AnkiSyncState::new();
        // Sync #1: create the note.
        sync_pass(&mock, &library, &mut state, tokio::time::Instant::now())
            .await
            .unwrap();

        let poder_tag = "flts_spa_rus_poder";
        let note_id = mock
            .note_id_for_tag(poder_tag)
            .expect("note exists after first sync");

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
            .load("spa", "rus", "poder")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            card.anki_data.as_ref().map(|a| a.state),
            Some(AnkiState::Deleted)
        );

        // Re-encounter: applying the same paragraph must not regress state.
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

        // Card files exist on disk regardless of AnkiConnect state — the
        // translation pipeline writes through LibraryCardStore directly.
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

        // A sync attempt against an unreachable Anki must leave the local
        // card store intact. Bootstrap may bubble (the version() probe is the
        // first call); per-card failures are recorded in backoff. Either way,
        // the disk state is the contract.
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

        // Canonical card: one translation, one example.
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

        // Drop a Syncthing-style conflict sibling carrying a divergent translation
        // and a different example. Mirrors Stage 3's load_merges_single_sync_conflict_sibling
        // layout in library_card.rs.
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

        // Anki receives the merged content: Target joins both translations with
        // "; ", and Example carries both source/translation pairs sorted by
        // source (alphabetic), joined by "<br>".
        let note_id = mock
            .note_id_for_tag("flts_spa_rus_poder")
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
