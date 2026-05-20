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
    #[serde(rename = "noteId")]
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
    let parsed: Response<T> = serde_json::from_str(body)
        .map_err(|e| anyhow!("AnkiConnect: malformed response: {e}"))?;
    if let Some(message) = parsed.error {
        bail!("AnkiConnect: {message}");
    }
    parsed
        .result
        .ok_or_else(|| anyhow!("AnkiConnect: empty result with no error"))
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
        let envelope = build_envelope_json(action, self.api_key.as_deref(), params);
        let resp = self
            .client
            .post(&self.endpoint)
            .json(&envelope)
            .send()
            .await
            .map_err(|e| anyhow!("AnkiConnect: HTTP request failed: {e}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| anyhow!("AnkiConnect: reading response body failed: {e}"))?;
        if !status.is_success() {
            bail!("AnkiConnect: HTTP {status}: {body}");
        }
        decode_response::<T>(&body)
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
        let _: Option<serde_json::Value> = self.call("updateNoteFields", Some(params)).await?;
        Ok(())
    }

    async fn cards_info(&self, card_ids: &[i64]) -> Result<Vec<CardInfo>> {
        let params = serde_json::json!({ "cards": card_ids });
        self.call::<Vec<CardInfo>>("cardsInfo", Some(params)).await
    }

    async fn multi(&self, actions: Vec<MultiSubAction>) -> Result<Vec<serde_json::Value>> {
        let params = serde_json::json!({ "actions": actions });
        self.call::<Vec<serde_json::Value>>("multi", Some(params))
            .await
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
        }
    }

    pub fn set_version(&self, version: u32) {
        self.inner.lock().unwrap().version = version;
    }

    pub fn suspend_card(&self, card_id: i64) {
        if let Some(card) = self.inner.lock().unwrap().cards.get_mut(&card_id) {
            card.queue = -1;
        }
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
}

#[async_trait]
impl AnkiConnect for MockAnkiConnect {
    async fn version(&self) -> Result<u32> {
        Ok(self.inner.lock().unwrap().version)
    }

    async fn model_names_and_ids(&self) -> Result<HashMap<String, i64>> {
        Ok(self.inner.lock().unwrap().models.clone())
    }

    async fn create_model(&self, spec: ModelSpec) -> Result<i64> {
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
        Ok(self.inner.lock().unwrap().decks.clone())
    }

    async fn create_deck(&self, name: &str) -> Result<i64> {
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

    async fn add_note(&self, note: NewNote) -> Result<i64> {
        let mut state = self.inner.lock().unwrap();
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
        let mut state = self.inner.lock().unwrap();
        let stored = state
            .notes
            .get_mut(&note_id)
            .ok_or_else(|| anyhow!("MockAnkiConnect: unknown note {note_id}"))?;
        for (field, value) in fields {
            stored.fields.insert(field, value);
        }
        Ok(())
    }

    async fn cards_info(&self, card_ids: &[i64]) -> Result<Vec<CardInfo>> {
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

    async fn multi(&self, actions: Vec<MultiSubAction>) -> Result<Vec<serde_json::Value>> {
        let mut out = Vec::with_capacity(actions.len());
        for sub in actions {
            let params = sub.params.unwrap_or(serde_json::Value::Null);
            let result = match sub.action.as_str() {
                "version" => serde_json::to_value(self.version().await?)?,
                "modelNamesAndIds" => serde_json::to_value(self.model_names_and_ids().await?)?,
                "deckNamesAndIds" => serde_json::to_value(self.deck_names_and_ids().await?)?,
                "createDeck" => {
                    let name = params
                        .get("deck")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("multi createDeck: missing deck"))?;
                    serde_json::to_value(self.create_deck(name).await?)?
                }
                "createModel" => {
                    let spec: ModelSpec = serde_json::from_value(params)?;
                    serde_json::to_value(self.create_model(spec).await?)?
                }
                "findNotes" => {
                    let query = params
                        .get("query")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("multi findNotes: missing query"))?;
                    serde_json::to_value(self.find_notes(query).await?)?
                }
                "addNote" => {
                    let note: NewNote = serde_json::from_value(
                        params
                            .get("note")
                            .cloned()
                            .ok_or_else(|| anyhow!("multi addNote: missing note"))?,
                    )?;
                    serde_json::to_value(self.add_note(note).await?)?
                }
                "updateNoteFields" => {
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
                    self.update_note_fields(note_id, fields).await?;
                    serde_json::Value::Null
                }
                "cardsInfo" => {
                    let cards: Vec<i64> = serde_json::from_value(
                        params
                            .get("cards")
                            .cloned()
                            .ok_or_else(|| anyhow!("multi cardsInfo: missing cards"))?,
                    )?;
                    serde_json::to_value(self.cards_info(&cards).await?)?
                }
                other => bail!("MockAnkiConnect: unsupported multi sub-action `{other}`"),
            };
            out.push(result);
        }
        Ok(out)
    }
}

// ---------- Factory ----------

pub fn get_anki_connect(endpoint: String, api_key: Option<String>) -> Box<dyn AnkiConnect> {
    if std::env::var_os("FLTS_MOCK_ANKICONNECT").is_some_and(|v| !v.is_empty()) {
        Box::new(MockAnkiConnect::new())
    } else {
        Box::new(HttpAnkiConnect::new(endpoint, api_key))
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
        let mock = MockAnkiConnect::new();
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
        let mock = MockAnkiConnect::new();
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
        let mock = MockAnkiConnect::new();
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
        let mock = MockAnkiConnect::new();
        let note_id = mock.add_note(sample_note("flts_test")).await.unwrap();
        // The note's two cards were assigned ids note_id+1 and note_id+2.
        let card_a = note_id + 1;
        mock.suspend_card(card_a);
        let info = mock.cards_info(&[card_a]).await.unwrap();
        assert_eq!(info.len(), 1);
        assert!(info[0].is_suspended());
    }

    #[tokio::test]
    async fn mock_multi_dispatches_subactions_in_order() {
        let mock = MockAnkiConnect::new();
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
}
