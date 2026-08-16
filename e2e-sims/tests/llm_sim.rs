use async_openai::types::chat::{
    CreateChatCompletionResponse, CreateChatCompletionStreamResponse, FinishReason as OaFinish,
};
use e2e_sims::llm::llm_router;
use e2e_sims::server::{SimState, serve};
use gemini_rust::{CachedContent, FinishReason, GenerationResponse};
use serde_json::{Value, json};
use std::sync::Arc;

async fn start() -> (String, reqwest::Client) {
    let (inner, sim) = llm_router();
    let (reset, seed) = (sim.clone(), sim.clone());
    let state = Arc::new(SimState::new(
        Box::new(move || reset.reset()),
        Box::new(move |v| seed.seed(v)),
    ));
    let (port, _h) = serve(inner, state).await.unwrap();
    (format!("http://127.0.0.1:{port}"), reqwest::Client::new())
}

async fn seed(c: &reqwest::Client, base: &str, body: Value) {
    let r = c
        .post(format!("{base}/_sim/seed"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
}

fn translation() -> Value {
    json!({
        "s": [{
            "ft": "The sun rose over the mountains.",
            "wl": [
                {"o": "El", "t": ["the"], "n": null, "p": false,
                 "g": {"lf": "el", "lt": "the", "pos": "article_definite",
                       "pl": null, "pe": null, "te": null, "ca": null, "ot": null}},
                {"o": "sol", "t": ["sun"], "n": null, "p": false,
                 "g": {"lf": "sol", "lt": "sun", "pos": "common_noun",
                       "pl": "singular", "pe": null, "te": null, "ca": null, "ot": null}},
                {"o": ".", "t": [], "n": null, "p": true,
                 "g": {"lf": ".", "lt": ".", "pos": "punctuation",
                       "pl": null, "pe": null, "te": null, "ca": null, "ot": null}}
            ]
        }]
    })
}

fn script_seed(chunks: u64) -> Value {
    json!({
        "scripts": [{
            "matchSubstring": "El sol",
            "translation": translation(),
            "stream": true,
            "chunks": chunks,
        }],
        "fallback": "minimal",
    })
}

/// `data:` payloads of an SSE body, in order.
fn sse_events(body: &str) -> Vec<String> {
    body.split("\n\n")
        .filter_map(|block| {
            block
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .map(str::to_owned)
        })
        .collect()
}

async fn gemini_generate(c: &reqwest::Client, base: &str, body: Value) -> (u16, String) {
    let r = c
        .post(format!(
            "{base}/v1beta/models/gemini-2.5-flash:generateContent"
        ))
        .json(&body)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

async fn gemini_stream(c: &reqwest::Client, base: &str, body: Value) -> (u16, String) {
    let r = c
        .post(format!(
            "{base}/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse"
        ))
        .json(&body)
        .send()
        .await
        .unwrap();
    (r.status().as_u16(), r.text().await.unwrap())
}

fn user_request(text: &str) -> Value {
    json!({"contents": [{"role": "user", "parts": [{"text": text}]}]})
}

#[tokio::test]
async fn gemini_non_stream_returns_scripted_translation() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(5)).await;

    let (status, body) = gemini_generate(&c, &base, user_request("El sol salió")).await;
    assert_eq!(status, 200, "{body}");

    let parsed: GenerationResponse = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed.candidates[0].finish_reason, Some(FinishReason::Stop));
    let text: Value = serde_json::from_str(&parsed.text()).unwrap();
    assert_eq!(text, translation());

    let usage = parsed.usage_metadata.unwrap();
    assert!(usage.prompt_token_count.unwrap() > 0);
    assert!(usage.candidates_token_count.unwrap() > 0);
    assert_eq!(
        usage.total_token_count.unwrap(),
        usage.prompt_token_count.unwrap() + usage.candidates_token_count.unwrap()
    );
}

#[tokio::test]
async fn gemini_stream_splits_script_into_requested_chunks() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(5)).await;

    let (status, body) = gemini_stream(&c, &base, user_request("El sol salió")).await;
    assert_eq!(status, 200, "{body}");

    let events = sse_events(&body);
    assert_eq!(events.len(), 5, "{body}");

    let mut assembled = String::new();
    for (i, ev) in events.iter().enumerate() {
        let chunk: GenerationResponse = serde_json::from_str(ev).unwrap();
        assembled.push_str(&chunk.text());
        let last = i + 1 == events.len();
        assert_eq!(
            chunk.candidates[0].finish_reason.is_some(),
            last,
            "chunk {i}: {ev}"
        );
        assert_eq!(chunk.usage_metadata.is_some(), last, "chunk {i}: {ev}");
    }
    assert_eq!(
        serde_json::from_str::<Value>(&assembled).unwrap(),
        translation()
    );
}

#[tokio::test]
async fn openai_stream_terminates_with_done() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(3)).await;

    let r = c
        .post(format!("{base}/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-5.2",
            "stream": true,
            "messages": [{"role": "user", "content": "Translate this paragraph: El sol salió"}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();

    let events = sse_events(&body);
    assert_eq!(events.last().unwrap(), "[DONE]", "{body}");

    let mut assembled = String::new();
    let mut finish = None;
    for ev in events.iter().take(events.len() - 1) {
        let chunk: CreateChatCompletionStreamResponse = serde_json::from_str(ev).unwrap();
        assert_eq!(chunk.object, "chat.completion.chunk");
        let choice = &chunk.choices[0];
        if let Some(text) = &choice.delta.content {
            assembled.push_str(text);
        }
        if let Some(fr) = choice.finish_reason {
            finish = Some(fr);
        }
    }
    assert_eq!(finish, Some(OaFinish::Stop));
    assert_eq!(
        serde_json::from_str::<Value>(&assembled).unwrap(),
        translation()
    );
}

#[tokio::test]
async fn openai_non_stream_returns_message_content() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(3)).await;

    let r = c
        .post(format!("{base}/chat/completions"))
        .json(&json!({
            "model": "glm-5.2",
            "messages": [{"role": "user", "content": "Translate this paragraph: El sol salió"}],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let parsed: CreateChatCompletionResponse =
        serde_json::from_str(&r.text().await.unwrap()).unwrap();

    let choice = &parsed.choices[0];
    assert_eq!(choice.finish_reason, Some(OaFinish::Stop));
    assert_eq!(parsed.object, "chat.completion");
    assert_eq!(parsed.model, "glm-5.2");
    let content: Value = serde_json::from_str(choice.message.content.as_ref().unwrap()).unwrap();
    assert_eq!(content, translation());

    let usage = parsed.usage.unwrap();
    assert!(usage.prompt_tokens > 0 && usage.completion_tokens > 0);
    assert_eq!(
        usage.total_tokens,
        usage.prompt_tokens + usage.completion_tokens
    );
}

#[tokio::test]
async fn cached_contents_create_delete_then_use_returns_404() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(3)).await;

    let r = c
        .post(format!("{base}/v1beta/cachedContents"))
        .json(&json!({
            "model": "models/gemini-2.5-flash",
            "displayName": "flts-1-spa-eng-book-c0",
            "systemInstruction": {"parts": [{"text": "be a translator"}]},
            "contents": [{"role": "user", "parts": [{"text": "chapter text"}]}],
            "ttl": "3600s",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let created: CachedContent = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(created.name, "cachedContents/sim-1");
    assert_eq!(
        created.display_name.as_deref(),
        Some("flts-1-spa-eng-book-c0")
    );
    assert!(created.usage_metadata.total_token_count > 0);

    // Referencing the live cache works.
    let mut req = user_request("El sol salió");
    req["cachedContent"] = json!(created.name);
    let (status, _) = gemini_stream(&c, &base, req.clone()).await;
    assert_eq!(status, 200);

    let r = c
        .delete(format!("{base}/v1beta/{}", created.name))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // Using it after deletion is a Google-shaped 404.
    let (status, body) = gemini_stream(&c, &base, req).await;
    assert_eq!(status, 404, "{body}");
    let err: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(err["error"]["code"], 404);
    assert_eq!(err["error"]["status"], "NOT_FOUND");
    assert!(
        err["error"]["message"].as_str().unwrap().contains("sim-1"),
        "{body}"
    );

    // So is deleting it twice.
    let r = c
        .delete(format!("{base}/v1beta/{}", created.name))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn unmatched_request_falls_back_to_a_minimal_translation() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(3)).await;

    let (status, body) = gemini_generate(&c, &base, user_request("nothing matches this")).await;
    assert_eq!(status, 200, "{body}");
    let parsed: GenerationResponse = serde_json::from_str(&body).unwrap();
    let fallback: Value = serde_json::from_str(&parsed.text()).unwrap();

    // Required top-level keys of `paragraph_translation_schema()`, all the way
    // down to the grammar block the importer needs.
    let sentence = &fallback["s"][0];
    assert!(sentence["ft"].is_string());
    let word = &sentence["wl"][0];
    assert!(word["o"].is_string());
    assert!(word["t"].is_array());
    assert!(word["p"].is_boolean());
    for k in ["lf", "lt", "pos"] {
        assert!(word["g"][k].is_string(), "missing g.{k}: {fallback}");
    }
}

#[tokio::test]
async fn fallback_also_serves_openai_and_survives_reset() {
    let (base, c) = start().await;
    seed(&c, &base, script_seed(3)).await;

    let r = c.post(format!("{base}/_sim/reset")).send().await.unwrap();
    assert_eq!(r.status(), 200);

    // Scripts are gone, but the scripted text now gets the fallback, not an error.
    let (status, body) = gemini_generate(&c, &base, user_request("El sol salió")).await;
    assert_eq!(status, 200, "{body}");
    let parsed: GenerationResponse = serde_json::from_str(&body).unwrap();
    let text: Value = serde_json::from_str(&parsed.text()).unwrap();
    assert_ne!(text, translation());
    assert!(text["s"][0]["wl"][0]["o"].is_string());

    let r = c
        .post(format!("{base}/chat/completions"))
        .json(&json!({
            "model": "deepseek-v4-flash",
            "messages": [{"role": "user", "content": "El sol salió"}],
        }))
        .send()
        .await
        .unwrap();
    let parsed: CreateChatCompletionResponse =
        serde_json::from_str(&r.text().await.unwrap()).unwrap();
    let content: Value =
        serde_json::from_str(parsed.choices[0].message.content.as_ref().unwrap()).unwrap();
    assert!(content["s"][0]["wl"][0]["g"]["pos"].is_string());
}

#[tokio::test]
async fn seed_rejects_malformed_input() {
    let (base, c) = start().await;
    for bad in [
        json!([]),
        json!({"scripts": {}}),
        json!({"scripts": [{"translation": {}}]}),
        json!({"scripts": [{"matchSubstring": "x"}]}),
        json!({"scripts": [], "fallback": "wat"}),
    ] {
        let r = c
            .post(format!("{base}/_sim/seed"))
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400, "accepted {bad}");
    }
}

#[tokio::test]
async fn first_matching_script_wins() {
    let (base, c) = start().await;
    seed(
        &c,
        &base,
        json!({"scripts": [
            {"matchSubstring": "sol", "translation": {"s": [{"ft": "first", "wl": []}]}},
            {"matchSubstring": "El sol", "translation": {"s": [{"ft": "second", "wl": []}]}},
        ]}),
    )
    .await;

    let (_, body) = gemini_generate(&c, &base, user_request("El sol salió")).await;
    let parsed: GenerationResponse = serde_json::from_str(&body).unwrap();
    let text: Value = serde_json::from_str(&parsed.text()).unwrap();
    assert_eq!(text["s"][0]["ft"], "first");
}
