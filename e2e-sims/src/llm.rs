//! LLM simulator speaking both wire protocols the app uses: Gemini v1beta
//! (generate/stream/cachedContents) and OpenAI-compatible chat completions.
//! A script is chosen by the first `matchSubstring` found in the raw request
//! body; an unmatched request still gets a schema-valid translation, so a
//! missing script can never wedge the app.

use axum::{
    Json, Router,
    extract::Path,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Chunk count when a script doesn't pick one.
const DEFAULT_CHUNKS: usize = 4;
/// Rough tokens-per-character, good enough for plausible usage numbers.
const CHARS_PER_TOKEN: usize = 4;
const CREATE_TIME: &str = "2026-01-01T00:00:00Z";
const EXPIRE_TIME: &str = "2099-01-01T00:00:00Z";

#[derive(Debug)]
struct Script {
    match_substring: String,
    /// The model's verbatim output, already serialized.
    payload: String,
    stream: bool,
    chunks: usize,
}

#[derive(Debug)]
struct Cache {
    model: String,
    display_name: Option<String>,
    tokens: i64,
}

#[derive(Debug, Default)]
struct Inner {
    scripts: Vec<Script>,
    caches: BTreeMap<String, Cache>,
    next_cache: u64,
}

#[derive(Debug, Default)]
pub struct LlmSimState {
    inner: Mutex<Inner>,
}

impl LlmSimState {
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = Inner::default();
    }

    /// `{"scripts": [{matchSubstring, translation, stream?, chunks?}], "fallback": "minimal"}`.
    /// Replaces the script list wholesale.
    pub fn seed(&self, v: Value) -> Result<(), String> {
        let obj = v.as_object().ok_or("seed: expected an object")?;
        match obj.get("fallback") {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s == "minimal" => {}
            Some(other) => return Err(format!("seed: unsupported fallback {other}")),
        }
        let mut parsed = Vec::new();
        if let Some(scripts) = obj.get("scripts") {
            let scripts = scripts.as_array().ok_or("seed: scripts must be an array")?;
            for s in scripts {
                let match_substring = s
                    .get("matchSubstring")
                    .and_then(Value::as_str)
                    .ok_or("seed: script.matchSubstring must be a string")?
                    .to_owned();
                let translation = s
                    .get("translation")
                    .ok_or("seed: script.translation is required")?;
                let chunks = match s.get("chunks") {
                    None | Some(Value::Null) => DEFAULT_CHUNKS,
                    Some(c) => c
                        .as_u64()
                        .filter(|c| *c >= 1)
                        .ok_or("seed: script.chunks must be a positive integer")?
                        as usize,
                };
                let stream = match s.get("stream") {
                    None | Some(Value::Null) => true,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => return Err("seed: script.stream must be a boolean".into()),
                };
                parsed.push(Script {
                    match_substring,
                    payload: translation.to_string(),
                    stream,
                    chunks,
                });
            }
        }
        self.inner.lock().unwrap().scripts = parsed;
        Ok(())
    }

    /// Payload plus the chunk count a streaming response should use.
    fn respond(&self, request_body: &str) -> (String, usize) {
        let inner = self.inner.lock().unwrap();
        match inner
            .scripts
            .iter()
            .find(|s| request_body.contains(&s.match_substring))
        {
            Some(s) => (s.payload.clone(), if s.stream { s.chunks } else { 1 }),
            None => (minimal_translation().to_string(), 1),
        }
    }

    fn create_cache(&self, req: &Value) -> Value {
        let mut inner = self.inner.lock().unwrap();
        inner.next_cache += 1;
        let name = format!("cachedContents/sim-{}", inner.next_cache);
        let cache = Cache {
            model: req
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("models/gemini-2.5-flash")
                .to_owned(),
            display_name: req
                .get("displayName")
                .and_then(Value::as_str)
                .map(str::to_owned),
            tokens: tokens(req.to_string().len()),
        };
        let body = cache_json(&name, &cache);
        inner.caches.insert(name, cache);
        body
    }

    fn cache_json(&self, name: &str) -> Option<Value> {
        let inner = self.inner.lock().unwrap();
        inner.caches.get(name).map(|c| cache_json(name, c))
    }

    fn delete_cache(&self, name: &str) -> bool {
        self.inner.lock().unwrap().caches.remove(name).is_some()
    }

    fn has_cache(&self, name: &str) -> bool {
        self.inner.lock().unwrap().caches.contains_key(name)
    }

    fn list_caches(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        let items: Vec<Value> = inner
            .caches
            .iter()
            .map(|(name, c)| cache_json(name, c))
            .collect();
        json!({ "cachedContents": items })
    }
}

/// Minimal object the translation importer accepts: one sentence, one word,
/// with every field the strict schema marks required.
fn minimal_translation() -> Value {
    json!({
        "s": [{
            "ft": "sim",
            "wl": [{
                "o": "sim",
                "t": ["sim"],
                "n": null,
                "p": false,
                "g": {
                    "lf": "sim", "lt": "sim", "pos": "common_noun",
                    "pl": null, "pe": null, "te": null, "ca": null, "ot": null
                }
            }]
        }]
    })
}

fn tokens(chars: usize) -> i64 {
    (chars / CHARS_PER_TOKEN).max(1) as i64
}

/// `CachedContent` as gemini-rust deserializes it: rfc3339 timestamps and
/// `usageMetadata` are mandatory there.
fn cache_json(name: &str, c: &Cache) -> Value {
    json!({
        "name": name,
        "model": c.model,
        "createTime": CREATE_TIME,
        "updateTime": CREATE_TIME,
        "expireTime": EXPIRE_TIME,
        "usageMetadata": { "totalTokenCount": c.tokens },
        "displayName": c.display_name,
    })
}

/// Splits into exactly `n` pieces (fewer only when the text is shorter than
/// `n` chars), on char boundaries.
fn split_chunks(s: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let n = n.clamp(1, chars.len().max(1));
    let (base, rem) = (chars.len() / n, chars.len() % n);
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    for k in 0..n {
        let len = base + usize::from(k < rem);
        out.push(chars[i..i + len].iter().collect());
        i += len;
    }
    out
}

fn sse(events: Vec<String>) -> Response {
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        [(header::CACHE_CONTROL, "no-cache")],
        body,
    )
        .into_response()
}

fn google_error(code: u16, message: String, status: &str) -> Response {
    let http = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        http,
        Json(json!({"error": {"code": code, "message": message, "status": status}})),
    )
        .into_response()
}

fn cache_missing(name: &str) -> Response {
    google_error(
        404,
        format!("CachedContent not found (or permission denied): {name}"),
        "NOT_FOUND",
    )
}

fn gemini_candidate(text: &str, finish: bool) -> Value {
    let mut candidate = json!({
        "content": {"parts": [{"text": text}], "role": "model"},
        "index": 0,
    });
    if finish {
        candidate["finishReason"] = json!("STOP");
    }
    candidate
}

fn gemini_usage(prompt_chars: usize, out_chars: usize) -> Value {
    let (p, c) = (tokens(prompt_chars), tokens(out_chars));
    json!({
        "promptTokenCount": p,
        "candidatesTokenCount": c,
        "totalTokenCount": p + c,
        "cachedContentTokenCount": 0,
        "thoughtsTokenCount": 0,
    })
}

/// `cachedContent` referencing a cache we don't hold — the app's
/// evict-and-rebuild path keys off this 404.
fn missing_cache_ref(sim: &LlmSimState, body: &str) -> Option<String> {
    let req: Value = serde_json::from_str(body).ok()?;
    let name = req.get("cachedContent")?.as_str()?.to_owned();
    (!sim.has_cache(&name)).then_some(name)
}

fn gemini_handle(sim: &LlmSimState, model_action: &str, body: String) -> Response {
    let Some((model, action)) = model_action.rsplit_once(':') else {
        return google_error(404, format!("unknown route: {model_action}"), "NOT_FOUND");
    };
    if let Some(name) = missing_cache_ref(sim, &body) {
        return cache_missing(&name);
    }
    let (payload, chunks) = sim.respond(&body);
    let usage = gemini_usage(body.len(), payload.len());

    match action {
        "generateContent" => Json(json!({
            "candidates": [gemini_candidate(&payload, true)],
            "usageMetadata": usage,
            "modelVersion": model,
            "responseId": "sim-response",
        }))
        .into_response(),
        "streamGenerateContent" => {
            let pieces = split_chunks(&payload, chunks);
            let last = pieces.len() - 1;
            let events = pieces
                .iter()
                .enumerate()
                .map(|(i, piece)| {
                    let mut chunk = json!({
                        "candidates": [gemini_candidate(piece, i == last)],
                        "modelVersion": model,
                        "responseId": "sim-response",
                    });
                    if i == last {
                        chunk["usageMetadata"] = usage.clone();
                    }
                    chunk.to_string()
                })
                .collect();
            sse(events)
        }
        other => google_error(404, format!("unsupported action: {other}"), "NOT_FOUND"),
    }
}

fn openai_handle(sim: &LlmSimState, body: String) -> Response {
    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let model = req
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("sim-model")
        .to_owned();
    let streaming = req.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let (payload, chunks) = sim.respond(&body);
    let (p, c) = (tokens(body.len()), tokens(payload.len()));
    let usage = json!({"prompt_tokens": p, "completion_tokens": c, "total_tokens": p + c});

    if !streaming {
        return Json(json!({
            "id": "chatcmpl-sim",
            "object": "chat.completion",
            "created": 1_767_225_600u64,
            "model": model,
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": payload, "refusal": null},
                "finish_reason": "stop",
                "logprobs": null,
            }],
            "usage": usage,
            "service_tier": null,
            "system_fingerprint": null,
        }))
        .into_response();
    }

    let pieces = split_chunks(&payload, chunks);
    let last = pieces.len() - 1;
    let mut events: Vec<String> = pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| {
            json!({
                "id": "chatcmpl-sim",
                "object": "chat.completion.chunk",
                "created": 1_767_225_600u64,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {
                        "role": if i == 0 { json!("assistant") } else { Value::Null },
                        "content": piece,
                        "refusal": null,
                        "tool_calls": null,
                        "function_call": null,
                    },
                    "finish_reason": if i == last { json!("stop") } else { Value::Null },
                    "logprobs": null,
                }],
                "usage": if i == last { usage.clone() } else { Value::Null },
                "service_tier": null,
                "system_fingerprint": null,
            })
            .to_string()
        })
        .collect();
    events.push("[DONE]".to_owned());
    sse(events)
}

pub fn llm_router() -> (Router, Arc<LlmSimState>) {
    let sim = Arc::new(LlmSimState::default());

    let gemini = {
        let sim = sim.clone();
        post(
            move |Path(model_action): Path<String>, body: String| async move {
                gemini_handle(&sim, &model_action, body)
            },
        )
    };
    let create_cache = {
        let sim = sim.clone();
        post(move |body: String| async move {
            let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            Json(sim.create_cache(&req))
        })
    };
    let list_caches = {
        let sim = sim.clone();
        get(move || async move { Json(sim.list_caches()) })
    };
    // Get/patch/delete share one route: patch (TTL refresh) just echoes the record.
    let one_cache = {
        let (get_sim, patch_sim, del_sim) = (sim.clone(), sim.clone(), sim.clone());
        get(move |Path(id): Path<String>| async move { cache_lookup(&get_sim, &id) })
            .patch(move |Path(id): Path<String>| async move { cache_lookup(&patch_sim, &id) })
            .delete(move |Path(id): Path<String>| async move {
                let name = format!("cachedContents/{id}");
                if del_sim.delete_cache(&name) {
                    Json(json!({})).into_response()
                } else {
                    cache_missing(&name)
                }
            })
    };
    let openai = {
        let sim = sim.clone();
        post(move |body: String| async move { openai_handle(&sim, body) })
    };

    let router = Router::new()
        .route("/v1beta/models/{*model_action}", gemini.clone())
        .route("/models/{*model_action}", gemini)
        .route(
            "/v1beta/cachedContents",
            create_cache.clone().merge(list_caches.clone()),
        )
        .route("/cachedContents", create_cache.merge(list_caches))
        .route("/v1beta/cachedContents/{id}", one_cache.clone())
        .route("/cachedContents/{id}", one_cache)
        .route("/chat/completions", openai.clone())
        .route("/v1/chat/completions", openai);
    (router, sim)
}

fn cache_lookup(sim: &LlmSimState, id: &str) -> Response {
    let name = format!("cachedContents/{id}");
    match sim.cache_json(&name) {
        Some(v) => Json(v).into_response(),
        None => cache_missing(&name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_chunks_is_exact_and_lossless() {
        for n in 1..8 {
            let parts = split_chunks("abcdefghij", n);
            assert_eq!(parts.len(), n);
            assert_eq!(parts.concat(), "abcdefghij");
        }
        // Fewer chars than chunks: one char per chunk, never empty.
        let parts = split_chunks("ab", 5);
        assert_eq!(parts, vec!["a", "b"]);
        assert_eq!(split_chunks("", 3), vec![""]);
        // Multi-byte text stays valid UTF-8.
        assert_eq!(split_chunks("áé", 2), vec!["á", "é"]);
    }
}
