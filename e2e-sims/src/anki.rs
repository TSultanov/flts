//! AnkiConnect simulator: a real-HTTP port of `library::anki::connect::MockAnkiConnect`.
//! Happy-path protocol only — failure injection lives in the rules/fault layer.

use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

const ANKI_CONNECT_VERSION: u32 = 6;

#[derive(Debug)]
struct Note {
    fields: BTreeMap<String, String>,
    tags: Vec<String>,
    deck: String,
    model: String,
}

#[derive(Debug)]
struct Card {
    note_id: i64,
    queue: i64,
    interval: i64,
    factor: i64,
    data: Option<Value>,
}

#[derive(Debug)]
struct State {
    next_id: i64,
    version: u32,
    models: HashMap<String, i64>,
    decks: HashMap<String, i64>,
    notes: HashMap<i64, Note>,
    cards: HashMap<i64, Card>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            next_id: 1,
            version: ANKI_CONNECT_VERSION,
            models: HashMap::new(),
            decks: HashMap::new(),
            notes: HashMap::new(),
            cards: HashMap::new(),
        }
    }
}

#[derive(Debug, Default)]
pub struct AnkiSimState {
    inner: Mutex<State>,
}

/// Action outcome: `Ok` is the bare result value, `Err` the AnkiConnect error string.
type ActionResult = Result<Value, String>;

impl AnkiSimState {
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = State::default();
    }

    /// `{"decks": [name], "notes": [{deck, model, fields, tags}]}`; both optional.
    pub fn seed(&self, v: Value) -> Result<(), String> {
        let obj = v.as_object().ok_or("seed: expected an object")?;
        if let Some(decks) = obj.get("decks") {
            let decks = decks.as_array().ok_or("seed: decks must be an array")?;
            for d in decks {
                let name = d.as_str().ok_or("seed: deck names must be strings")?;
                self.create_deck(name);
            }
        }
        if let Some(notes) = obj.get("notes") {
            let notes = notes.as_array().ok_or("seed: notes must be an array")?;
            for n in notes {
                let deck = n
                    .get("deck")
                    .and_then(Value::as_str)
                    .ok_or("seed: note.deck missing")?;
                let model = n.get("model").and_then(Value::as_str).unwrap_or_default();
                let fields = parse_fields(n.get("fields"))?;
                let tags = parse_tags(n.get("tags"))?;
                self.create_deck(deck);
                self.add_note(deck, model, fields, tags)
                    .map_err(|e| format!("seed: {e}"))?;
            }
        }
        Ok(())
    }

    fn create_deck(&self, name: &str) -> i64 {
        let mut s = self.inner.lock().unwrap();
        if let Some(id) = s.decks.get(name) {
            return *id;
        }
        let id = s.next_id;
        s.next_id += 1;
        s.decks.insert(name.to_owned(), id);
        id
    }

    fn create_model(&self, name: &str) -> i64 {
        let mut s = self.inner.lock().unwrap();
        if let Some(id) = s.models.get(name) {
            return *id;
        }
        let id = s.next_id;
        s.next_id += 1;
        s.models.insert(name.to_owned(), id);
        id
    }

    fn find_notes(&self, query: &str) -> Result<Vec<i64>, String> {
        let tag = query
            .strip_prefix("tag:")
            .ok_or("only `tag:<value>` queries are supported")?;
        let s = self.inner.lock().unwrap();
        let mut hits: Vec<i64> = s
            .notes
            .iter()
            .filter(|(_, n)| n.tags.iter().any(|t| t == tag))
            .map(|(id, _)| *id)
            .collect();
        hits.sort_unstable();
        Ok(hits)
    }

    /// Allocates the note id then two card ids, as `MockAnkiConnect` does.
    fn add_note(
        &self,
        deck: &str,
        model: &str,
        fields: BTreeMap<String, String>,
        tags: Vec<String>,
    ) -> Result<i64, String> {
        let mut s = self.inner.lock().unwrap();
        if !s.decks.contains_key(deck) {
            return Err(format!("deck was not found: {deck}"));
        }
        let note_id = s.next_id;
        s.next_id += 3;
        for card_id in [note_id + 1, note_id + 2] {
            s.cards.insert(
                card_id,
                Card {
                    note_id,
                    queue: 0,
                    interval: 0,
                    factor: 0,
                    data: None,
                },
            );
        }
        s.notes.insert(
            note_id,
            Note {
                fields,
                tags,
                deck: deck.to_owned(),
                model: model.to_owned(),
            },
        );
        Ok(note_id)
    }

    fn update_note_fields(
        &self,
        note_id: i64,
        fields: BTreeMap<String, String>,
    ) -> Result<(), String> {
        let mut s = self.inner.lock().unwrap();
        let deck = s
            .notes
            .get(&note_id)
            .ok_or_else(|| format!("note was not found: {note_id}"))?
            .deck
            .clone();
        if !s.decks.contains_key(&deck) {
            return Err(format!("deck was not found: {deck}"));
        }
        let stored = s.notes.get_mut(&note_id).expect("held under same lock");
        stored.fields.extend(fields);
        Ok(())
    }

    fn cards_info(&self, ids: &[i64]) -> Value {
        let s = self.inner.lock().unwrap();
        let out: Vec<Value> = ids
            .iter()
            .filter_map(|id| {
                s.cards.get(id).map(|c| {
                    json!({
                        "cardId": id,
                        "note": c.note_id,
                        "queue": c.queue,
                        "interval": c.interval,
                        "factor": c.factor,
                        "data": c.data,
                    })
                })
            })
            .collect();
        Value::Array(out)
    }

    fn notes_info(&self, ids: &[i64]) -> Value {
        let s = self.inner.lock().unwrap();
        let out: Vec<Value> = ids
            .iter()
            .filter_map(|id| {
                s.notes.get(id).map(|n| {
                    let mut cards: Vec<i64> = s
                        .cards
                        .iter()
                        .filter_map(|(cid, c)| (c.note_id == *id).then_some(*cid))
                        .collect();
                    cards.sort_unstable();
                    json!({
                        "noteId": id,
                        "modelName": n.model,
                        "cards": cards,
                        "tags": n.tags,
                        "fields": fields_envelope(&n.fields),
                    })
                })
            })
            .collect();
        Value::Array(out)
    }

    fn dispatch(&self, action: &str, params: &Value) -> ActionResult {
        match action {
            "version" => Ok(json!(self.inner.lock().unwrap().version)),
            "modelNamesAndIds" => Ok(json!(self.inner.lock().unwrap().models)),
            "deckNamesAndIds" => Ok(json!(self.inner.lock().unwrap().decks)),
            "createDeck" => {
                let name = str_param(params, "deck")?;
                Ok(json!(self.create_deck(name)))
            }
            "createModel" => {
                let name = str_param(params, "modelName")?;
                Ok(json!({ "id": self.create_model(name) }))
            }
            "findNotes" => {
                let query = str_param(params, "query")?;
                Ok(json!(self.find_notes(query)?))
            }
            "addNote" => {
                let note = params.get("note").ok_or("addNote: missing note")?;
                let deck = note
                    .get("deckName")
                    .and_then(Value::as_str)
                    .ok_or("addNote: missing deckName")?;
                let model = note
                    .get("modelName")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let fields = parse_fields(note.get("fields"))?;
                let tags = parse_tags(note.get("tags"))?;
                Ok(json!(self.add_note(deck, model, fields, tags)?))
            }
            "updateNoteFields" => {
                let note = params.get("note").ok_or("updateNoteFields: missing note")?;
                let id = note
                    .get("id")
                    .and_then(Value::as_i64)
                    .ok_or("updateNoteFields: missing id")?;
                self.update_note_fields(id, parse_fields(note.get("fields"))?)?;
                Ok(Value::Null)
            }
            "cardsInfo" => Ok(self.cards_info(&id_list(params, "cards")?)),
            "notesInfo" => Ok(self.notes_info(&id_list(params, "notes")?)),
            "multi" => {
                let actions = params
                    .get("actions")
                    .and_then(Value::as_array)
                    .ok_or("multi: missing actions")?;
                let mut out = Vec::with_capacity(actions.len());
                for sub in actions {
                    let name = sub.get("action").and_then(Value::as_str).unwrap_or("");
                    let sub_params = sub.get("params").cloned().unwrap_or(Value::Null);
                    // Sub-errors are packaged in-band; the batch itself still succeeds.
                    out.push(match self.dispatch(name, &sub_params) {
                        Ok(v) => v,
                        Err(e) => json!({ "result": null, "error": e }),
                    });
                }
                Ok(Value::Array(out))
            }
            other => Err(format!("unsupported action: {other}")),
        }
    }
}

/// AnkiConnect returns note fields as `{name: {value, order}}`.
fn fields_envelope(fields: &BTreeMap<String, String>) -> Value {
    let mut out = serde_json::Map::new();
    for (i, (k, v)) in fields.iter().enumerate() {
        out.insert(k.clone(), json!({ "value": v, "order": i }));
    }
    Value::Object(out)
}

fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing `{key}`"))
}

fn id_list(params: &Value, key: &str) -> Result<Vec<i64>, String> {
    let arr = params
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing `{key}`"))?;
    arr.iter()
        .map(|v| v.as_i64().ok_or_else(|| format!("`{key}`: not an integer")))
        .collect()
}

fn parse_fields(v: Option<&Value>) -> Result<BTreeMap<String, String>, String> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    serde_json::from_value(v.clone()).map_err(|e| format!("bad fields: {e}"))
}

fn parse_tags(v: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(v) = v else {
        return Ok(Vec::new());
    };
    serde_json::from_value(v.clone()).map_err(|e| format!("bad tags: {e}"))
}

pub fn anki_router() -> (Router, Arc<AnkiSimState>) {
    let sim = Arc::new(AnkiSimState::default());
    let handler_sim = sim.clone();
    let router = Router::new().route(
        "/",
        post(move |body: Option<Json<Value>>| {
            let sim = handler_sim.clone();
            async move {
                let env = body.map(|Json(v)| v).unwrap_or(Value::Null);
                let action = env.get("action").and_then(Value::as_str).unwrap_or("");
                let params = env.get("params").cloned().unwrap_or(Value::Null);
                Json(match sim.dispatch(action, &params) {
                    Ok(result) => json!({ "result": result, "error": null }),
                    Err(error) => json!({ "result": null, "error": error }),
                })
            }
        }),
    );
    (router, sim)
}
