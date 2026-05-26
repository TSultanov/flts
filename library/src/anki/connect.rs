//! AnkiConnect transport: trait, HTTP implementation, in-memory mock, and factory.
//!
//! The shape mirrors the [`Translator`](crate::translator::Translator) family:
//! one async trait, typed methods, concrete impls behind a `Box<dyn _>` factory
//! gated by an environment variable. Higher layers (Stage 6 sync orchestrator,
//! Stage 5 [`bootstrap`](super::model::bootstrap)) program against the trait
//! and switch between real HTTP and the mock without code changes.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const ANKI_CONNECT_VERSION: u32 = 6;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Total attempts (initial + retries) for a single AnkiConnect HTTP send.
/// Only retries `.send()` failures — i.e. connection refused / reset before
/// any response was received. Body-read failures and AnkiConnect-level errors
/// return immediately, since the server has already side-effected (retrying a
/// non-idempotent action like addNote would duplicate notes).
const HTTP_RETRY_ATTEMPTS: u32 = 3;
/// Sleep durations between retry attempts, in ms. Length must be
/// `HTTP_RETRY_ATTEMPTS - 1`.
const HTTP_RETRY_DELAYS_MS: [u64; 2] = [100, 300];

#[async_trait]
pub trait AnkiConnect: Send + Sync {
    async fn version(&self) -> Result<u32>;
    async fn model_names_and_ids(&self) -> Result<HashMap<String, i64>>;
    async fn create_model(&self, spec: ModelSpec) -> Result<i64>;
    async fn deck_names_and_ids(&self) -> Result<HashMap<String, i64>>;
    async fn create_deck(&self, name: &str) -> Result<i64>;
    async fn find_notes(&self, query: &str) -> Result<Vec<i64>>;
    async fn add_note(&self, note: NewNote) -> Result<i64>;
    async fn update_note_fields(
        &self,
        note_id: i64,
        fields: BTreeMap<String, String>,
    ) -> Result<()>;
    async fn cards_info(&self, card_ids: &[i64]) -> Result<Vec<CardInfo>>;
    async fn notes_info(&self, note_ids: &[i64]) -> Result<Vec<NoteInfo>>;
    async fn multi(&self, actions: Vec<MultiSubAction>) -> Result<Vec<serde_json::Value>>;
}

// ---------- Wire types ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSpec {
    #[serde(rename = "modelName")]
    pub model_name: String,
    #[serde(rename = "inOrderFields")]
    pub in_order_fields: Vec<String>,
    pub css: String,
    #[serde(rename = "isCloze")]
    pub is_cloze: bool,
    #[serde(rename = "cardTemplates")]
    pub card_templates: Vec<CardTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardTemplate {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Front")]
    pub front: String,
    #[serde(rename = "Back")]
    pub back: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewNote {
    #[serde(rename = "deckName")]
    pub deck_name: String,
    #[serde(rename = "modelName")]
    pub model_name: String,
    pub fields: BTreeMap<String, String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CardInfo {
    #[serde(rename = "cardId")]
    pub card_id: i64,
    // AnkiConnect's cardsInfo returns the parent note id as `"note"` (not
    // `"noteId"`). Real Anki responses fail to deserialize otherwise.
    #[serde(rename = "note")]
    pub note_id: i64,
    pub queue: i64,
    pub interval: i64,
    pub factor: i64,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl CardInfo {
    pub fn is_suspended(&self) -> bool {
        self.queue == -1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteInfo {
    #[serde(rename = "noteId")]
    pub note_id: i64,
    #[serde(default)]
    pub cards: Vec<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSubAction {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

// ---------- HTTP envelope ----------

#[derive(Debug, Serialize)]
struct Envelope<'a> {
    action: &'a str,
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Response<T> {
    result: Option<T>,
    error: Option<String>,
}

pub(crate) fn build_envelope_json(
    action: &str,
    api_key: Option<&str>,
    params: Option<serde_json::Value>,
) -> serde_json::Value {
    serde_json::to_value(Envelope {
        action,
        version: ANKI_CONNECT_VERSION,
        key: api_key,
        params,
    })
    .expect("Envelope serializes")
}

pub(crate) fn decode_response<T: for<'de> Deserialize<'de>>(body: &str) -> Result<T> {
    let parsed: Response<T> =
        serde_json::from_str(body).map_err(|e| anyhow!("AnkiConnect: malformed response: {e}"))?;
    if let Some(message) = parsed.error {
        bail!("AnkiConnect: {message}");
    }
    parsed
        .result
        .ok_or_else(|| anyhow!("AnkiConnect: empty result with no error"))
}

/// Like `decode_response` but for AnkiConnect actions that return `null` as
/// their success result (e.g. `updateNoteFields`, `addTags`). Only an explicit
/// `error` is treated as failure; a null/missing result is success.
pub(crate) fn decode_void_response(body: &str) -> Result<()> {
    let parsed: Response<serde_json::Value> =
        serde_json::from_str(body).map_err(|e| anyhow!("AnkiConnect: malformed response: {e}"))?;
    if let Some(message) = parsed.error {
        bail!("AnkiConnect: {message}");
    }
    Ok(())
}

/// Decode one element of a `multi` response array. Real AnkiConnect packages
/// a sub-action error as `{"result": null, "error": "msg"}` inside the array,
/// rather than failing the whole `multi` call. This helper distinguishes
/// per-sub-action success from per-sub-action error, and tolerates the legacy
/// shape where a successful sub-result is the bare value (some older
/// AnkiConnect builds emitted that for `findNotes`).
pub(crate) fn decode_multi_sub<T: for<'de> Deserialize<'de>>(
    value: serde_json::Value,
) -> Result<T> {
    if let Some(obj) = value.as_object() {
        if let Some(serde_json::Value::String(msg)) = obj.get("error") {
            bail!("AnkiConnect: {msg}");
        }
        if obj.contains_key("result") {
            let parsed: Response<T> = serde_json::from_value(value)
                .map_err(|e| anyhow!("AnkiConnect: malformed multi sub-response: {e}"))?;
            if let Some(message) = parsed.error {
                bail!("AnkiConnect: {message}");
            }
            return parsed
                .result
                .ok_or_else(|| anyhow!("AnkiConnect: empty multi sub-result with no error"));
        }
    }
    serde_json::from_value(value)
        .map_err(|e| anyhow!("AnkiConnect: malformed multi sub-response: {e}"))
}

/// Like [`decode_multi_sub`] but for sub-actions that return `null` on success
/// (`updateNoteFields`, etc.). Only a non-null `"error"` is treated as failure.
pub(crate) fn decode_multi_sub_void(value: serde_json::Value) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    if let Some(obj) = value.as_object() {
        if let Some(serde_json::Value::String(msg)) = obj.get("error") {
            bail!("AnkiConnect: {msg}");
        }
    }
    Ok(())
}

// ---------- HTTP implementation ----------

pub struct HttpAnkiConnect {
    endpoint: String,
    api_key: Option<String>,
    client: reqwest::Client,
}

impl HttpAnkiConnect {
    pub fn new(endpoint: String, api_key: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("reqwest client builds");
        Self {
            endpoint,
            api_key,
            client,
        }
    }

    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        action: &str,
        params: Option<serde_json::Value>,
    ) -> Result<T> {
        let body = self.fetch_body(action, params).await?;
        decode_response::<T>(&body)
    }

    /// Like `call` but for AnkiConnect actions that return null on success.
    async fn call_void(&self, action: &str, params: Option<serde_json::Value>) -> Result<()> {
        let body = self.fetch_body(action, params).await?;
        decode_void_response(&body)
    }

    async fn fetch_body(&self, action: &str, params: Option<serde_json::Value>) -> Result<String> {
        let envelope = build_envelope_json(action, self.api_key.as_deref(), params);
        // Retry only `.send()` failures — the request never reached the
        // server, so even non-idempotent actions like addNote are safe to
        // retry. Once `.send()` resolves, we commit to the response.
        let mut last_err: Option<reqwest::Error> = None;
        let mut resp = None;
        for attempt in 0..HTTP_RETRY_ATTEMPTS {
            match self
                .client
                .post(&self.endpoint)
                .json(&envelope)
                .send()
                .await
            {
                Ok(r) => {
                    resp = Some(r);
                    break;
                }
                Err(e) => {
                    if attempt + 1 < HTTP_RETRY_ATTEMPTS {
                        let delay_ms = HTTP_RETRY_DELAYS_MS[attempt as usize];
                        log::debug!(
                            "AnkiConnect: transient send error on attempt {}/{}: {e}; retrying in {delay_ms}ms",
                            attempt + 1,
                            HTTP_RETRY_ATTEMPTS,
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    last_err = Some(e);
                }
            }
        }
        let resp = match resp {
            Some(r) => r,
            None => {
                let e = last_err.expect("at least one send error when resp is None");
                bail!("AnkiConnect: HTTP request failed: {e}");
            }
        };
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| anyhow!("AnkiConnect: reading response body failed: {e}"))?;
        if !status.is_success() {
            bail!("AnkiConnect: HTTP {status}: {body}");
        }
        Ok(body)
    }
}

#[async_trait]
impl AnkiConnect for HttpAnkiConnect {
    async fn version(&self) -> Result<u32> {
        self.call::<u32>("version", None).await
    }

    async fn model_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        self.call::<HashMap<String, i64>>("modelNamesAndIds", None)
            .await
    }

    async fn create_model(&self, spec: ModelSpec) -> Result<i64> {
        let params = serde_json::to_value(&spec)?;
        let result: serde_json::Value = self.call("createModel", Some(params)).await?;
        result
            .get("id")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow!("AnkiConnect: createModel returned no id"))
    }

    async fn deck_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        self.call::<HashMap<String, i64>>("deckNamesAndIds", None)
            .await
    }

    async fn create_deck(&self, name: &str) -> Result<i64> {
        let params = serde_json::json!({ "deck": name });
        self.call::<i64>("createDeck", Some(params)).await
    }

    async fn find_notes(&self, query: &str) -> Result<Vec<i64>> {
        let params = serde_json::json!({ "query": query });
        self.call::<Vec<i64>>("findNotes", Some(params)).await
    }

    async fn add_note(&self, note: NewNote) -> Result<i64> {
        let params = serde_json::json!({ "note": note });
        self.call::<i64>("addNote", Some(params)).await
    }

    async fn update_note_fields(
        &self,
        note_id: i64,
        fields: BTreeMap<String, String>,
    ) -> Result<()> {
        let params = serde_json::json!({
            "note": {
                "id": note_id,
                "fields": fields,
            }
        });
        self.call_void("updateNoteFields", Some(params)).await
    }

    async fn cards_info(&self, card_ids: &[i64]) -> Result<Vec<CardInfo>> {
        let params = serde_json::json!({ "cards": card_ids });
        self.call::<Vec<CardInfo>>("cardsInfo", Some(params)).await
    }

    async fn notes_info(&self, note_ids: &[i64]) -> Result<Vec<NoteInfo>> {
        let params = serde_json::json!({ "notes": note_ids });
        self.call::<Vec<NoteInfo>>("notesInfo", Some(params)).await
    }

    async fn multi(&self, actions: Vec<MultiSubAction>) -> Result<Vec<serde_json::Value>> {
        let params = serde_json::json!({ "actions": actions });
        self.call::<Vec<serde_json::Value>>("multi", Some(params))
            .await
    }
}

// ---------- Serialized wrapper (single-flight worker task) ----------

/// Wraps any `AnkiConnect` and serializes all method calls through a
/// dedicated worker task. AnkiConnect handles concurrent requests poorly,
/// so we guarantee at most one in-flight call by having the worker drain
/// a request channel one task at a time. Callers see the normal async
/// API; the serialization is structural (single consumer of a single
/// channel), not lock-based.
pub struct SerializedAnkiConnect {
    tx: tokio::sync::mpsc::UnboundedSender<AnkiTask>,
    worker: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

type AnkiTask = Box<
    dyn FnOnce(Arc<dyn AnkiConnect>) -> futures_util::future::BoxFuture<'static, ()>
        + Send,
>;

impl SerializedAnkiConnect {
    pub fn new(inner: Arc<dyn AnkiConnect>) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AnkiTask>();
        let worker = tokio::spawn(async move {
            while let Some(task) = rx.recv().await {
                task(inner.clone()).await;
            }
        });
        Self {
            tx,
            worker: std::sync::Mutex::new(Some(worker)),
        }
    }

    fn dispatch<F, Fut, T>(&self, f: F) -> Result<tokio::sync::oneshot::Receiver<Result<T>>>
    where
        F: FnOnce(Arc<dyn AnkiConnect>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let task: AnkiTask = Box::new(move |inner| {
            Box::pin(async move {
                let _ = reply_tx.send(f(inner).await);
            })
        });
        self.tx
            .send(task)
            .map_err(|_| anyhow!("SerializedAnkiConnect worker has shut down"))?;
        Ok(reply_rx)
    }
}

impl Drop for SerializedAnkiConnect {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.worker.lock()
            && let Some(handle) = guard.take()
        {
            handle.abort();
        }
    }
}

#[async_trait]
impl AnkiConnect for SerializedAnkiConnect {
    async fn version(&self) -> Result<u32> {
        self.dispatch(|inner| async move { inner.version().await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn model_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        self.dispatch(|inner| async move { inner.model_names_and_ids().await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn create_model(&self, spec: ModelSpec) -> Result<i64> {
        self.dispatch(move |inner| async move { inner.create_model(spec).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn deck_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        self.dispatch(|inner| async move { inner.deck_names_and_ids().await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn create_deck(&self, name: &str) -> Result<i64> {
        let name = name.to_owned();
        self.dispatch(move |inner| async move { inner.create_deck(&name).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn find_notes(&self, query: &str) -> Result<Vec<i64>> {
        let query = query.to_owned();
        self.dispatch(move |inner| async move { inner.find_notes(&query).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn add_note(&self, note: NewNote) -> Result<i64> {
        self.dispatch(move |inner| async move { inner.add_note(note).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn update_note_fields(
        &self,
        note_id: i64,
        fields: BTreeMap<String, String>,
    ) -> Result<()> {
        self.dispatch(move |inner| async move {
            inner.update_note_fields(note_id, fields).await
        })?
        .await
        .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn cards_info(&self, card_ids: &[i64]) -> Result<Vec<CardInfo>> {
        let card_ids = card_ids.to_vec();
        self.dispatch(move |inner| async move { inner.cards_info(&card_ids).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn notes_info(&self, note_ids: &[i64]) -> Result<Vec<NoteInfo>> {
        let note_ids = note_ids.to_vec();
        self.dispatch(move |inner| async move { inner.notes_info(&note_ids).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }

    async fn multi(&self, actions: Vec<MultiSubAction>) -> Result<Vec<serde_json::Value>> {
        self.dispatch(move |inner| async move { inner.multi(actions).await })?
            .await
            .map_err(|_| anyhow!("SerializedAnkiConnect reply dropped"))?
    }
}

// ---------- In-memory mock ----------

#[derive(Debug, Default)]
struct MockState {
    next_id: i64,
    version: u32,
    models: HashMap<String, i64>,
    decks: HashMap<String, i64>,
    notes: HashMap<i64, MockNote>,
    cards: HashMap<i64, MockCard>,
}

#[derive(Debug, Clone)]
struct MockNote {
    #[allow(dead_code)]
    model: String,
    #[allow(dead_code)]
    deck: String,
    fields: BTreeMap<String, String>,
    tags: Vec<String>,
}

#[derive(Debug, Clone)]
struct MockCard {
    note_id: i64,
    queue: i64,
    interval: i64,
    factor: i64,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct MockAnkiConnect {
    inner: Arc<Mutex<MockState>>,
    fail_quota: Arc<std::sync::atomic::AtomicUsize>,
    multi_call_count: Arc<std::sync::atomic::AtomicUsize>,
    find_notes_direct_count: Arc<std::sync::atomic::AtomicUsize>,
    notes_info_call_count: Arc<std::sync::atomic::AtomicUsize>,
    cards_info_call_count: Arc<std::sync::atomic::AtomicUsize>,
    fail_add_note_tags: Arc<Mutex<Vec<String>>>,
}

impl Default for MockAnkiConnect {
    fn default() -> Self {
        Self::new()
    }
}

impl MockAnkiConnect {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockState {
                next_id: 1,
                version: ANKI_CONNECT_VERSION,
                ..Default::default()
            })),
            fail_quota: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            multi_call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            find_notes_direct_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            notes_info_call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            cards_info_call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            fail_add_note_tags: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Test-only instrumentation: number of times `multi` has been called.
    pub fn multi_call_count(&self) -> usize {
        self.multi_call_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test-only instrumentation: number of times `find_notes` has been called
    /// directly (i.e., not through `multi`).
    pub fn find_notes_direct_count(&self) -> usize {
        self.find_notes_direct_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test-only instrumentation: number of times `notes_info` has been called.
    pub fn notes_info_call_count(&self) -> usize {
        self.notes_info_call_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test-only instrumentation: number of times `cards_info` has been called.
    pub fn cards_info_call_count(&self) -> usize {
        self.cards_info_call_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Test-only knob: any subsequent `add_note` call whose tag list contains
    /// `tag` will fail with a transient-style error. Used to exercise
    /// per-sub-action failure isolation inside `multi` batches.
    pub fn fail_add_note_with_tag(&self, tag: &str) {
        self.fail_add_note_tags
            .lock()
            .unwrap()
            .push(tag.to_owned());
    }

    pub fn set_version(&self, version: u32) {
        self.inner.lock().unwrap().version = version;
    }

    pub fn suspend_card(&self, card_id: i64) {
        if let Some(card) = self.inner.lock().unwrap().cards.get_mut(&card_id) {
            card.queue = -1;
        }
    }

    /// Test-only knob: simulate the user deleting a note in Anki. Removes the
    /// note and all of its associated cards from mock state; subsequent
    /// `find_notes(tag:...)` for the note's tag will return zero hits.
    pub fn remove_note(&self, note_id: i64) {
        let mut state = self.inner.lock().unwrap();
        state.notes.remove(&note_id);
        state.cards.retain(|_, c| c.note_id != note_id);
    }

    /// Test-only knob: simulate the user deleting a deck in Anki. Subsequent
    /// `add_note` / `update_note_fields` against the deck will fail with the
    /// same "deck was not found" string real AnkiConnect emits.
    pub fn remove_deck(&self, name: &str) {
        self.inner.lock().unwrap().decks.remove(name);
    }

    /// Test-only knob: cause the next `n` trait method invocations to return
    /// an error before touching mock state. Decrements one per call.
    pub fn fail_next_n_calls(&self, n: usize) {
        self.fail_quota
            .store(n, std::sync::atomic::Ordering::SeqCst);
    }

    /// If a failure is queued, decrement the quota and return Err.
    fn check_fail_quota(&self) -> Result<()> {
        use std::sync::atomic::Ordering;
        let mut current = self.fail_quota.load(Ordering::SeqCst);
        while current > 0 {
            match self.fail_quota.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Err(anyhow!("mock transient failure")),
                Err(actual) => current = actual,
            }
        }
        Ok(())
    }

    /// Internal `findNotes` logic: shared by the direct trait method and by
    /// `multi` dispatch. Does NOT touch the instrumentation counter — the
    /// caller decides whether the call counts as "direct" or as part of a batch.
    fn find_notes_impl(&self, query: &str) -> Result<Vec<i64>> {
        let tag = query
            .strip_prefix("tag:")
            .ok_or_else(|| anyhow!("MockAnkiConnect: only `tag:<value>` queries are supported"))?;
        let state = self.inner.lock().unwrap();
        let mut hits: Vec<i64> = state
            .notes
            .iter()
            .filter(|(_, n)| n.tags.iter().any(|t| t == tag))
            .map(|(id, _)| *id)
            .collect();
        hits.sort_unstable();
        Ok(hits)
    }

    /// Test-only accessor: returns the (fields, tags) pair for a note, if present.
    pub fn peek_note(&self, note_id: i64) -> Option<(BTreeMap<String, String>, Vec<String>)> {
        self.inner
            .lock()
            .unwrap()
            .notes
            .get(&note_id)
            .map(|n| (n.fields.clone(), n.tags.clone()))
    }

    /// Test-only accessor: returns the first note id whose tags contain `tag`,
    /// if any. Matches the lookup `findNotes` performs internally; provided so
    /// scenario tests can chain `tag → note id → peek_note` without re-running
    /// `find_notes` through the trait surface.
    pub fn note_id_for_tag(&self, tag: &str) -> Option<i64> {
        let state = self.inner.lock().unwrap();
        let mut hits: Vec<i64> = state
            .notes
            .iter()
            .filter(|(_, n)| n.tags.iter().any(|t| t == tag))
            .map(|(id, _)| *id)
            .collect();
        hits.sort_unstable();
        hits.into_iter().next()
    }
}

#[async_trait]
impl AnkiConnect for MockAnkiConnect {
    async fn version(&self) -> Result<u32> {
        self.check_fail_quota()?;
        Ok(self.inner.lock().unwrap().version)
    }

    async fn model_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        self.check_fail_quota()?;
        Ok(self.inner.lock().unwrap().models.clone())
    }

    async fn create_model(&self, spec: ModelSpec) -> Result<i64> {
        self.check_fail_quota()?;
        let mut state = self.inner.lock().unwrap();
        if let Some(existing) = state.models.get(&spec.model_name) {
            return Ok(*existing);
        }
        let id = state.next_id;
        state.next_id += 1;
        state.models.insert(spec.model_name, id);
        Ok(id)
    }

    async fn deck_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        self.check_fail_quota()?;
        Ok(self.inner.lock().unwrap().decks.clone())
    }

    async fn create_deck(&self, name: &str) -> Result<i64> {
        self.check_fail_quota()?;
        let mut state = self.inner.lock().unwrap();
        if let Some(existing) = state.decks.get(name) {
            return Ok(*existing);
        }
        let id = state.next_id;
        state.next_id += 1;
        state.decks.insert(name.to_owned(), id);
        Ok(id)
    }

    async fn find_notes(&self, query: &str) -> Result<Vec<i64>> {
        self.check_fail_quota()?;
        self.find_notes_direct_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.find_notes_impl(query)
    }

    async fn add_note(&self, note: NewNote) -> Result<i64> {
        self.check_fail_quota()?;
        {
            let fail_tags = self.fail_add_note_tags.lock().unwrap();
            if let Some(hit) = fail_tags.iter().find(|t| note.tags.iter().any(|nt| nt == *t)) {
                bail!("MockAnkiConnect: forced add_note failure for tag {hit}");
            }
        }
        let mut state = self.inner.lock().unwrap();
        // Mirror real AnkiConnect: addNote against a missing deck fails with
        // "deck was not found: <name>". Lets sync recovery tests exercise the
        // out-of-band deck-deletion path.
        if !state.decks.contains_key(&note.deck_name) {
            bail!("AnkiConnect: deck was not found: {}", note.deck_name);
        }
        let note_id = state.next_id;
        state.next_id += 1;
        let card_a = state.next_id;
        state.next_id += 1;
        let card_b = state.next_id;
        state.next_id += 1;
        state.cards.insert(
            card_a,
            MockCard {
                note_id,
                queue: 0,
                interval: 0,
                factor: 0,
                data: None,
            },
        );
        state.cards.insert(
            card_b,
            MockCard {
                note_id,
                queue: 0,
                interval: 0,
                factor: 0,
                data: None,
            },
        );
        state.notes.insert(
            note_id,
            MockNote {
                model: note.model_name,
                deck: note.deck_name,
                fields: note.fields,
                tags: note.tags,
            },
        );
        let _ = (card_a, card_b);
        Ok(note_id)
    }

    async fn update_note_fields(
        &self,
        note_id: i64,
        fields: BTreeMap<String, String>,
    ) -> Result<()> {
        self.check_fail_quota()?;
        let mut state = self.inner.lock().unwrap();
        let deck = state
            .notes
            .get(&note_id)
            .ok_or_else(|| anyhow!("MockAnkiConnect: unknown note {note_id}"))?
            .deck
            .clone();
        // Same out-of-band-deletion fidelity as `add_note` above.
        if !state.decks.contains_key(&deck) {
            bail!("AnkiConnect: deck was not found: {}", deck);
        }
        let stored = state
            .notes
            .get_mut(&note_id)
            .expect("note existed under same lock");
        for (field, value) in fields {
            stored.fields.insert(field, value);
        }
        Ok(())
    }

    async fn cards_info(&self, card_ids: &[i64]) -> Result<Vec<CardInfo>> {
        self.check_fail_quota()?;
        self.cards_info_call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let state = self.inner.lock().unwrap();
        Ok(card_ids
            .iter()
            .filter_map(|id| {
                state.cards.get(id).map(|c| CardInfo {
                    card_id: *id,
                    note_id: c.note_id,
                    queue: c.queue,
                    interval: c.interval,
                    factor: c.factor,
                    data: c.data.clone(),
                })
            })
            .collect())
    }

    async fn notes_info(&self, note_ids: &[i64]) -> Result<Vec<NoteInfo>> {
        self.check_fail_quota()?;
        self.notes_info_call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let state = self.inner.lock().unwrap();
        Ok(note_ids
            .iter()
            .filter_map(|id| {
                state.notes.get(id).map(|note| {
                    let mut cards: Vec<i64> = state
                        .cards
                        .iter()
                        .filter_map(|(card_id, c)| (c.note_id == *id).then_some(*card_id))
                        .collect();
                    cards.sort_unstable();
                    NoteInfo {
                        note_id: *id,
                        cards,
                        tags: note.tags.clone(),
                    }
                })
            })
            .collect())
    }

    async fn multi(&self, actions: Vec<MultiSubAction>) -> Result<Vec<serde_json::Value>> {
        self.check_fail_quota()?;
        self.multi_call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Per-sub-action errors are packaged as `{"result": null, "error": "..."}`
        // in the response array, mirroring real AnkiConnect. The whole call
        // only fails if shape parsing fails (`bail!` for an unsupported action,
        // top-level fail_quota) — i.e. things that can't happen sub-action-by-
        // sub-action in real AnkiConnect either.
        let mut out = Vec::with_capacity(actions.len());
        for sub in actions {
            let params = sub.params.unwrap_or(serde_json::Value::Null);
            let sub_result: Result<serde_json::Value> = match sub.action.as_str() {
                "version" => self
                    .version()
                    .await
                    .and_then(|v| Ok(serde_json::to_value(v)?)),
                "modelNamesAndIds" => self
                    .model_names_and_ids()
                    .await
                    .and_then(|v| Ok(serde_json::to_value(v)?)),
                "deckNamesAndIds" => self
                    .deck_names_and_ids()
                    .await
                    .and_then(|v| Ok(serde_json::to_value(v)?)),
                "createDeck" => match params.get("deck").and_then(|v| v.as_str()) {
                    Some(name) => self
                        .create_deck(name)
                        .await
                        .and_then(|v| Ok(serde_json::to_value(v)?)),
                    None => Err(anyhow!("multi createDeck: missing deck")),
                },
                "createModel" => match serde_json::from_value::<ModelSpec>(params) {
                    Ok(spec) => self
                        .create_model(spec)
                        .await
                        .and_then(|v| Ok(serde_json::to_value(v)?)),
                    Err(e) => Err(anyhow!("multi createModel: {e}")),
                },
                "findNotes" => match params.get("query").and_then(|v| v.as_str()) {
                    Some(query) => self
                        .find_notes_impl(query)
                        .and_then(|v| Ok(serde_json::to_value(v)?)),
                    None => Err(anyhow!("multi findNotes: missing query")),
                },
                "addNote" => match params
                    .get("note")
                    .cloned()
                    .ok_or_else(|| anyhow!("multi addNote: missing note"))
                    .and_then(|v| serde_json::from_value::<NewNote>(v).map_err(|e| anyhow!(e)))
                {
                    Ok(note) => self
                        .add_note(note)
                        .await
                        .and_then(|v| Ok(serde_json::to_value(v)?)),
                    Err(e) => Err(e),
                },
                "updateNoteFields" => {
                    let parsed: Result<(i64, BTreeMap<String, String>)> = (|| {
                        let note = params
                            .get("note")
                            .ok_or_else(|| anyhow!("multi updateNoteFields: missing note"))?;
                        let note_id = note
                            .get("id")
                            .and_then(|v| v.as_i64())
                            .ok_or_else(|| anyhow!("multi updateNoteFields: missing id"))?;
                        let fields: BTreeMap<String, String> = serde_json::from_value(
                            note.get("fields")
                                .cloned()
                                .ok_or_else(|| anyhow!("multi updateNoteFields: missing fields"))?,
                        )?;
                        Ok((note_id, fields))
                    })();
                    match parsed {
                        Ok((note_id, fields)) => self
                            .update_note_fields(note_id, fields)
                            .await
                            .map(|()| serde_json::Value::Null),
                        Err(e) => Err(e),
                    }
                }
                "cardsInfo" => match params
                    .get("cards")
                    .cloned()
                    .ok_or_else(|| anyhow!("multi cardsInfo: missing cards"))
                    .and_then(|v| serde_json::from_value::<Vec<i64>>(v).map_err(|e| anyhow!(e)))
                {
                    Ok(cards) => self
                        .cards_info(&cards)
                        .await
                        .and_then(|v| Ok(serde_json::to_value(v)?)),
                    Err(e) => Err(e),
                },
                "notesInfo" => match params
                    .get("notes")
                    .cloned()
                    .ok_or_else(|| anyhow!("multi notesInfo: missing notes"))
                    .and_then(|v| serde_json::from_value::<Vec<i64>>(v).map_err(|e| anyhow!(e)))
                {
                    Ok(notes) => self
                        .notes_info(&notes)
                        .await
                        .and_then(|v| Ok(serde_json::to_value(v)?)),
                    Err(e) => Err(e),
                },
                other => bail!("MockAnkiConnect: unsupported multi sub-action `{other}`"),
            };
            let packaged = match sub_result {
                Ok(v) => v,
                Err(e) => serde_json::json!({ "result": null, "error": e.to_string() }),
            };
            out.push(packaged);
        }
        Ok(out)
    }
}

// ---------- Factory ----------

pub fn get_anki_connect(endpoint: String, api_key: Option<String>) -> Box<dyn AnkiConnect> {
    if std::env::var_os("FLTS_MOCK_ANKICONNECT").is_some_and(|v| !v.is_empty()) {
        Box::new(MockAnkiConnect::new())
    } else {
        let http: Arc<dyn AnkiConnect> = Arc::new(HttpAnkiConnect::new(endpoint, api_key));
        Box::new(SerializedAnkiConnect::new(http))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_model_spec() -> ModelSpec {
        ModelSpec {
            model_name: "FLTS Bilingual v1".to_owned(),
            in_order_fields: vec!["Source".into(), "Target".into(), "Example".into()],
            css: ".card{}".to_owned(),
            is_cloze: false,
            card_templates: vec![CardTemplate {
                name: "Source → Target".into(),
                front: "{{Source}}".into(),
                back: "{{Target}}".into(),
            }],
        }
    }

    fn sample_note(tag: &str) -> NewNote {
        let mut fields = BTreeMap::new();
        fields.insert("Source".into(), "poder".into());
        fields.insert("Target".into(), "мочь".into());
        fields.insert("Example".into(), "".into());
        NewNote {
            deck_name: "FLTS::spa-rus".into(),
            model_name: "FLTS Bilingual v1".into(),
            fields,
            tags: vec![tag.into()],
        }
    }

    /// Mock seeded with the deck `sample_note` targets. The mock's add_note now
    /// validates the deck (matching real AnkiConnect), so unit tests that push
    /// a `sample_note` must create the deck first.
    async fn mock_with_sample_deck() -> MockAnkiConnect {
        let mock = MockAnkiConnect::new();
        mock.create_deck("FLTS::spa-rus").await.unwrap();
        mock
    }

    #[tokio::test]
    async fn mock_version_returns_six() {
        let mock = MockAnkiConnect::new();
        assert_eq!(mock.version().await.unwrap(), 6);
    }

    #[tokio::test]
    async fn mock_set_version_overrides_default() {
        let mock = MockAnkiConnect::new();
        mock.set_version(5);
        assert_eq!(mock.version().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn mock_create_deck_is_idempotent() {
        let mock = MockAnkiConnect::new();
        let id1 = mock.create_deck("FLTS::spa-rus").await.unwrap();
        let id2 = mock.create_deck("FLTS::spa-rus").await.unwrap();
        assert_eq!(id1, id2);
        let decks = mock.deck_names_and_ids().await.unwrap();
        assert_eq!(decks.len(), 1);
        assert_eq!(decks.get("FLTS::spa-rus"), Some(&id1));
    }

    #[tokio::test]
    async fn mock_create_model_then_lookup() {
        let mock = MockAnkiConnect::new();
        let id = mock.create_model(sample_model_spec()).await.unwrap();
        let models = mock.model_names_and_ids().await.unwrap();
        assert_eq!(models.get("FLTS Bilingual v1"), Some(&id));
    }

    #[tokio::test]
    async fn mock_create_model_is_idempotent() {
        let mock = MockAnkiConnect::new();
        let a = mock.create_model(sample_model_spec()).await.unwrap();
        let b = mock.create_model(sample_model_spec()).await.unwrap();
        assert_eq!(a, b);
    }

    #[tokio::test]
    async fn mock_add_note_then_find_by_tag() {
        let mock = mock_with_sample_deck().await;
        let id = mock
            .add_note(sample_note("flts_spa_rus_poder_verb"))
            .await
            .unwrap();
        let hits = mock
            .find_notes("tag:flts_spa_rus_poder_verb")
            .await
            .unwrap();
        assert_eq!(hits, vec![id]);
    }

    #[tokio::test]
    async fn mock_find_notes_rejects_non_tag_query() {
        let mock = MockAnkiConnect::new();
        let err = mock.find_notes("deck:Default").await.unwrap_err();
        assert!(
            format!("{err}").contains("tag:"),
            "expected tag-only error, got {err}"
        );
    }

    #[tokio::test]
    async fn mock_update_note_fields_mutates_visible_state() {
        let mock = mock_with_sample_deck().await;
        let id = mock.add_note(sample_note("flts_test")).await.unwrap();
        let mut fields = BTreeMap::new();
        fields.insert("Target".into(), "уметь; мочь".into());
        mock.update_note_fields(id, fields).await.unwrap();
        let (stored, _) = mock.peek_note(id).expect("note exists");
        assert_eq!(stored.get("Target"), Some(&"уметь; мочь".to_owned()));
        assert_eq!(stored.get("Source"), Some(&"poder".to_owned()));
    }

    #[tokio::test]
    async fn mock_cards_info_returns_card_records_for_added_note() {
        let mock = mock_with_sample_deck().await;
        let _ = mock.add_note(sample_note("flts_test")).await.unwrap();
        // We don't know the card ids without peeking, but cards_info on an empty
        // slice should return empty; on a non-existent id, also empty.
        let info = mock.cards_info(&[]).await.unwrap();
        assert!(info.is_empty());
        let info = mock.cards_info(&[9999]).await.unwrap();
        assert!(info.is_empty());
    }

    #[tokio::test]
    async fn mock_cards_info_reflects_suspension() {
        let mock = mock_with_sample_deck().await;
        let note_id = mock.add_note(sample_note("flts_test")).await.unwrap();
        // The note's two cards were assigned ids note_id+1 and note_id+2.
        let card_a = note_id + 1;
        mock.suspend_card(card_a);
        let info = mock.cards_info(&[card_a]).await.unwrap();
        assert_eq!(info.len(), 1);
        assert!(info[0].is_suspended());
    }

    #[tokio::test]
    async fn mock_notes_info_returns_two_cards_for_added_note() {
        let mock = mock_with_sample_deck().await;
        let note_id = mock
            .add_note(sample_note("flts_spa_rus_poder_verb"))
            .await
            .unwrap();
        let infos = mock.notes_info(&[note_id]).await.unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].note_id, note_id);
        assert_eq!(infos[0].cards.len(), 2);
        assert!(infos[0].tags.iter().any(|t| t == "flts_spa_rus_poder_verb"));
    }

    #[tokio::test]
    async fn mock_notes_info_skips_unknown_ids() {
        let mock = MockAnkiConnect::new();
        let infos = mock.notes_info(&[9999]).await.unwrap();
        assert!(infos.is_empty());
    }

    #[tokio::test]
    async fn mock_multi_dispatches_subactions_in_order() {
        let mock = mock_with_sample_deck().await;
        let actions = vec![
            MultiSubAction {
                action: "addNote".into(),
                params: Some(serde_json::json!({ "note": sample_note("flts_a") })),
            },
            MultiSubAction {
                action: "addNote".into(),
                params: Some(serde_json::json!({ "note": sample_note("flts_b") })),
            },
        ];
        let results = mock.multi(actions).await.unwrap();
        assert_eq!(results.len(), 2);
        let id_a = results[0].as_i64().unwrap();
        let id_b = results[1].as_i64().unwrap();
        assert_ne!(id_a, id_b);
    }

    #[tokio::test]
    async fn mock_multi_packages_sub_action_error_without_failing_whole_call() {
        // Deck "FLTS::spa-rus" is NOT created, so the first addNote will fail
        // with "deck was not found"; the second targets a deck we DO create.
        // Real AnkiConnect packages this as a per-element error object inside
        // the multi result array; the whole call must still return Ok.
        let mock = MockAnkiConnect::new();
        mock.create_deck("OtherDeck").await.unwrap();
        let mut good_fields = BTreeMap::new();
        good_fields.insert("Source".into(), "ok".into());
        good_fields.insert("Target".into(), "ok".into());
        good_fields.insert("Example".into(), String::new());
        let good_note = NewNote {
            deck_name: "OtherDeck".into(),
            model_name: "FLTS Bilingual v1".into(),
            fields: good_fields,
            tags: vec!["good".into()],
        };
        let actions = vec![
            MultiSubAction {
                action: "addNote".into(),
                params: Some(serde_json::json!({ "note": sample_note("flts_missing_deck") })),
            },
            MultiSubAction {
                action: "addNote".into(),
                params: Some(serde_json::json!({ "note": good_note })),
            },
        ];
        let results = mock.multi(actions).await.expect("multi returns Ok");
        assert_eq!(results.len(), 2);
        // First element is the wrapped error.
        let err_obj = results[0].as_object().expect("first element is an object");
        assert!(err_obj.get("result").map(|v| v.is_null()).unwrap_or(false));
        let err_msg = err_obj
            .get("error")
            .and_then(|v| v.as_str())
            .expect("error string present");
        assert!(
            err_msg.contains("deck was not found"),
            "expected real AnkiConnect-style error, got {err_msg}"
        );
        // Second element is the bare success value (the new note id).
        assert!(
            results[1].as_i64().is_some(),
            "second element must be a bare i64 success, got {}",
            results[1]
        );
    }

    #[tokio::test]
    async fn mock_fail_add_note_with_tag_makes_matching_add_note_fail() {
        let mock = mock_with_sample_deck().await;
        mock.fail_add_note_with_tag("flts_spa_rus_poder_verb");
        let err = mock
            .add_note(sample_note("flts_spa_rus_poder_verb"))
            .await
            .expect_err("flagged tag must fail");
        assert!(format!("{err}").contains("flts_spa_rus_poder_verb"));
        // Non-matching tag still succeeds.
        mock.add_note(sample_note("other_tag")).await.unwrap();
    }

    #[tokio::test]
    async fn mock_instrumentation_counts_notes_info_and_cards_info_calls() {
        let mock = mock_with_sample_deck().await;
        let note_id = mock.add_note(sample_note("flts_x")).await.unwrap();
        assert_eq!(mock.notes_info_call_count(), 0);
        assert_eq!(mock.cards_info_call_count(), 0);
        mock.notes_info(&[note_id]).await.unwrap();
        assert_eq!(mock.notes_info_call_count(), 1);
        mock.cards_info(&[]).await.unwrap();
        assert_eq!(mock.cards_info_call_count(), 1);
    }

    #[test]
    fn decode_multi_sub_decodes_bare_success_value() {
        // Legacy / current mock shape: success result is just the value.
        let v = serde_json::json!(42);
        let n: i64 = decode_multi_sub(v).unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn decode_multi_sub_decodes_response_envelope() {
        // Some AnkiConnect builds wrap success values in the Response envelope.
        let v = serde_json::json!({ "result": 42, "error": null });
        let n: i64 = decode_multi_sub(v).unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn decode_multi_sub_propagates_error_object() {
        let v = serde_json::json!({ "result": null, "error": "deck was not found" });
        let err = decode_multi_sub::<i64>(v).unwrap_err();
        assert!(format!("{err}").contains("deck was not found"));
    }

    #[test]
    fn decode_multi_sub_void_accepts_null_and_propagates_error() {
        decode_multi_sub_void(serde_json::Value::Null).unwrap();
        decode_multi_sub_void(serde_json::json!({ "result": null, "error": null })).unwrap();
        let err = decode_multi_sub_void(serde_json::json!({
            "result": null,
            "error": "note was not found",
        }))
        .unwrap_err();
        assert!(format!("{err}").contains("note was not found"));
    }

    #[tokio::test]
    async fn http_anki_connect_retries_send_errors_with_backoff() {
        // Bind to an ephemeral port, capture it, then drop the listener so
        // subsequent connects to that port are refused. This exercises the
        // real reqwest `.send()` error path against the production
        // HttpAnkiConnect (no mocking).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let client = HttpAnkiConnect::new(format!("http://127.0.0.1:{port}/"), None);

        let start = std::time::Instant::now();
        let err = client.version().await.unwrap_err();
        let elapsed = start.elapsed();

        assert!(
            format!("{err}").contains("HTTP request failed"),
            "expected the canonical send-failure message, got: {err}"
        );

        let expected_min = std::time::Duration::from_millis(
            HTTP_RETRY_DELAYS_MS.iter().sum::<u64>(),
        );
        assert!(
            elapsed >= expected_min,
            "expected at least {expected_min:?} elapsed (one sleep per retry), got {elapsed:?}"
        );
        // Sanity bound: must not have spent the full HTTP_TIMEOUT looping.
        assert!(
            elapsed < HTTP_TIMEOUT,
            "retries took {elapsed:?}, suspiciously close to HTTP_TIMEOUT — runaway loop?"
        );
    }

    #[test]
    fn http_envelope_omits_key_when_unset() {
        let env = build_envelope_json("version", None, None);
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"action\":\"version\""));
        assert!(s.contains("\"version\":6"));
        assert!(!s.contains("\"key\""));
        assert!(!s.contains("\"params\""));
    }

    #[test]
    fn http_envelope_includes_key_when_set() {
        let env = build_envelope_json("version", Some("secret"), None);
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"key\":\"secret\""));
    }

    #[test]
    fn http_envelope_serializes_params() {
        let env = build_envelope_json(
            "createDeck",
            None,
            Some(serde_json::json!({ "deck": "FLTS::spa-rus" })),
        );
        let s = serde_json::to_string(&env).unwrap();
        assert!(s.contains("\"action\":\"createDeck\""));
        assert!(s.contains("\"deck\":\"FLTS::spa-rus\""));
    }

    #[test]
    fn http_response_error_propagates_message() {
        let body = r#"{"result":null,"error":"deck not found"}"#;
        let err = decode_response::<i64>(body).unwrap_err();
        assert!(format!("{err}").contains("deck not found"));
    }

    #[test]
    fn http_response_decodes_typed_result() {
        let body = r#"{"result":6,"error":null}"#;
        let v: u32 = decode_response(body).unwrap();
        assert_eq!(v, 6);
    }

    #[test]
    fn http_response_rejects_empty_result_without_error() {
        let body = r#"{"result":null,"error":null}"#;
        let err = decode_response::<i64>(body).unwrap_err();
        assert!(format!("{err}").contains("empty result"));
    }

    #[test]
    fn http_void_response_accepts_null_result() {
        // AnkiConnect's updateNoteFields returns `{"result":null,"error":null}`
        // on success; decode_void_response must treat that as Ok.
        let body = r#"{"result":null,"error":null}"#;
        decode_void_response(body).unwrap();
    }

    #[test]
    fn http_void_response_propagates_error_message() {
        let body = r#"{"result":null,"error":"note was not found: 123"}"#;
        let err = decode_void_response(body).unwrap_err();
        assert!(format!("{err}").contains("note was not found"));
    }

    #[test]
    fn card_info_is_suspended_reads_queue_negative_one() {
        let info = CardInfo {
            card_id: 1,
            note_id: 2,
            queue: -1,
            interval: 5,
            factor: 2500,
            data: None,
        };
        assert!(info.is_suspended());
        let active = CardInfo { queue: 0, ..info };
        assert!(!active.is_suspended());
    }

    // ---------- SerializedAnkiConnect ----------

    /// Test-only `AnkiConnect` that sleeps on every call and panics if it
    /// observes more than one concurrent invocation. Used to assert
    /// `SerializedAnkiConnect`'s single-flight guarantee.
    struct SerializationProbe {
        in_flight: std::sync::atomic::AtomicUsize,
        delay: Duration,
    }

    impl SerializationProbe {
        fn new(delay: Duration) -> Self {
            Self {
                in_flight: std::sync::atomic::AtomicUsize::new(0),
                delay,
            }
        }

        async fn guarded<T>(&self, value: T) -> T {
            use std::sync::atomic::Ordering;
            let before = self.in_flight.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                before, 0,
                "SerializedAnkiConnect must serialize: observed {} in-flight",
                before + 1
            );
            tokio::time::sleep(self.delay).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            value
        }
    }

    #[async_trait]
    impl AnkiConnect for SerializationProbe {
        async fn version(&self) -> Result<u32> {
            Ok(self.guarded(6).await)
        }
        async fn model_names_and_ids(&self) -> Result<HashMap<String, i64>> {
            Ok(self.guarded(HashMap::new()).await)
        }
        async fn create_model(&self, _spec: ModelSpec) -> Result<i64> {
            Ok(self.guarded(1).await)
        }
        async fn deck_names_and_ids(&self) -> Result<HashMap<String, i64>> {
            Ok(self.guarded(HashMap::new()).await)
        }
        async fn create_deck(&self, _name: &str) -> Result<i64> {
            Ok(self.guarded(1).await)
        }
        async fn find_notes(&self, _query: &str) -> Result<Vec<i64>> {
            Ok(self.guarded(vec![]).await)
        }
        async fn add_note(&self, _note: NewNote) -> Result<i64> {
            Ok(self.guarded(1).await)
        }
        async fn update_note_fields(
            &self,
            _note_id: i64,
            _fields: BTreeMap<String, String>,
        ) -> Result<()> {
            self.guarded(()).await;
            Ok(())
        }
        async fn cards_info(&self, _card_ids: &[i64]) -> Result<Vec<CardInfo>> {
            Ok(self.guarded(vec![]).await)
        }
        async fn notes_info(&self, _note_ids: &[i64]) -> Result<Vec<NoteInfo>> {
            Ok(self.guarded(vec![]).await)
        }
        async fn multi(
            &self,
            _actions: Vec<MultiSubAction>,
        ) -> Result<Vec<serde_json::Value>> {
            Ok(self.guarded(vec![]).await)
        }
    }

    #[tokio::test]
    async fn serialized_anki_connect_serializes_concurrent_version_calls() {
        let probe: Arc<dyn AnkiConnect> = Arc::new(SerializationProbe::new(
            Duration::from_millis(50),
        ));
        let serialized = Arc::new(SerializedAnkiConnect::new(probe));

        let n = 5;
        let start = std::time::Instant::now();
        let mut handles = Vec::new();
        for _ in 0..n {
            let s = serialized.clone();
            handles.push(tokio::spawn(async move { s.version().await }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap().unwrap(), 6);
        }
        let elapsed = start.elapsed();
        // 5 × 50 ms = 250 ms; allow a generous lower bound to absorb
        // scheduler jitter. If the wrapper failed to serialize, total
        // wall-clock would collapse to ~50 ms.
        assert!(
            elapsed >= Duration::from_millis(200),
            "expected serialized run ≥ 200 ms, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn serialized_anki_connect_propagates_results_through_worker() {
        let probe: Arc<dyn AnkiConnect> = Arc::new(SerializationProbe::new(
            Duration::from_millis(1),
        ));
        let serialized = SerializedAnkiConnect::new(probe);
        assert_eq!(serialized.version().await.unwrap(), 6);
        assert_eq!(serialized.create_deck("FLTS::spa-rus").await.unwrap(), 1);
        assert!(serialized.find_notes("tag:foo").await.unwrap().is_empty());
    }
}
