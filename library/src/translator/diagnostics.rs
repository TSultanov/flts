//! Opt-in live wire diagnostics. Called only by the app's ignored manual tests.

use std::time::{Duration, Instant};

use anyhow::{Context, bail, ensure};
use async_openai::types::chat::{
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    CreateChatCompletionResponse, CreateChatCompletionStreamResponse, FinishReason, ResponseFormat,
};
use gemini_rust::{Gemini, GenerationResponse, Model, ThinkingConfig};
use isolang::Language;
use reqwest::{Client, Url};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{
    TranslationProvider, Translator, ensure_complete, gemini, openai, paragraph_translation_schema,
};
use crate::book::translation_import::ParagraphTranslation;

const PARAGRAPH: &str = "El sol ilumina la ciudad.";
const LIMIT: usize = 16 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(180);

pub struct DiagnosticEndpoint<'a> {
    pub provider: TranslationProvider,
    pub model: String,
    pub api_key: Option<&'a str>,
}

struct Reporter {
    secrets: Vec<String>,
}

impl Reporter {
    fn new(secrets: &[&str]) -> Self {
        let mut values = Vec::new();
        for secret in secrets.iter().filter(|s| !s.is_empty()) {
            values.push((*secret).to_owned());
            let quoted = serde_json::to_string(secret).expect("strings serialize");
            values.push(quoted[1..quoted.len() - 1].to_owned());
            let encoded: String = secret.bytes().map(|b| format!("%{b:02X}")).collect();
            values.push(encoded);
        }
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        values.dedup();
        Self { secrets: values }
    }

    fn redact(&self, text: &str) -> String {
        self.secrets.iter().fold(text.to_owned(), |text, secret| {
            text.replace(secret, "[REDACTED]")
        })
    }

    fn line(&self, text: impl AsRef<str>) {
        println!("{}", self.redact(text.as_ref()));
    }
}

fn endpoint(provider: TranslationProvider, model: &str, stream: bool) -> anyhow::Result<Url> {
    let (base, suffix) = match provider {
        TranslationProvider::Google => (
            std::env::var("FLTS_GEMINI_BASE_URL")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "https://generativelanguage.googleapis.com/v1beta/".into()),
            format!(
                "models/{}:{}",
                model.strip_prefix("models/").unwrap_or(model),
                if stream {
                    "streamGenerateContent"
                } else {
                    "generateContent"
                }
            ),
        ),
        TranslationProvider::Openrouter => (
            openai::openai_compat_base_url(provider).context("missing OpenRouter base URL")?,
            "chat/completions".to_owned(),
        ),
        _ => bail!("diagnostics supports only Gemini and OpenRouter"),
    };
    let mut url = Url::parse(&base).context("invalid endpoint URL")?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "base URL must not contain credentials, query parameters, or a fragment"
    );
    url.set_path(&format!("{}/{suffix}", url.path().trim_end_matches('/')));
    if provider == TranslationProvider::Google && stream {
        url.query_pairs_mut().append_pair("alt", "sse");
    }
    Ok(url)
}

fn request_body(
    provider: TranslationProvider,
    model: &str,
    to: Language,
    stream: bool,
) -> anyhow::Result<Value> {
    match provider {
        TranslationProvider::Google => {
            // Use the app's generation settings without creating a server-side cache.
            let client = Gemini::with_model("diagnostic-placeholder", Model::Custom(model.into()))?;
            let request = client
                .generate_content()
                .with_system_prompt(gemini::GeminiTranslator::get_prompt(
                    Language::Spa.to_name(),
                    to.to_name(),
                ))
                .with_user_message(format!("Translate this paragraph: {PARAGRAPH}"))
                .with_response_mime_type("application/json")
                .with_response_schema(gemini::gemini_paragraph_schema())
                .with_thinking_config(ThinkingConfig {
                    thinking_budget: (model.strip_prefix("models/").unwrap_or(model)
                        == "gemini-2.5-flash")
                        .then_some(0),
                    include_thoughts: Some(false),
                    thinking_level: None,
                })
                .with_safety_settings(gemini::permissive_safety_settings())
                .build();
            Ok(serde_json::to_value(request)?)
        }
        TranslationProvider::Openrouter => {
            let instructions = openai::translation_instructions(
                Language::Spa,
                to,
                &paragraph_translation_schema(),
                true,
            );
            let request = CreateChatCompletionRequestArgs::default()
                .model(model)
                .messages([ChatCompletionRequestUserMessageArgs::default()
                    .content(format!(
                        "{instructions}\n\nTranslate this paragraph: {PARAGRAPH}"
                    ))
                    .build()?
                    .into()])
                .response_format(ResponseFormat::JsonObject)
                .stream(stream)
                .build()?;
            Ok(serde_json::to_value(request)?)
        }
        _ => bail!("unsupported diagnostic provider"),
    }
}

/// Run independent JSON and SSE probes for each selected endpoint.
/// No retries, configuration writes, library writes, or automatic startup hooks.
pub async fn run(
    endpoints: &[DiagnosticEndpoint<'_>],
    target_language: Language,
    secrets: &[&str],
) -> anyhow::Result<()> {
    let mut all_secrets = secrets.to_vec();
    all_secrets.extend(endpoints.iter().filter_map(|e| e.api_key));
    let report = Reporter::new(&all_secrets);
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(DEADLINE)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    report.line(format!(
        "Manual LLM diagnostics: synthetic Spanish -> {}; paragraph: {PARAGRAPH:?}",
        target_language.to_name()
    ));
    report.line("Single-attempt JSON and SSE probes for the selected SDK path. Raw responses precede SDK parsing. No books or app settings are modified.");
    report.line("Each probe is limited to 180 seconds and 16 MiB. Authorization headers are never printed; configured secrets are redacted throughout.");
    let total = endpoints.len() * 2;
    let mut failures = 0;
    for endpoint in endpoints {
        for stream in [false, true] {
            let label = format!(
                "{:?} / {} / {}",
                endpoint.provider,
                endpoint.model,
                if stream { "SSE" } else { "JSON" }
            );
            report.line(format!("\n========== {label} =========="));
            let result = probe(&client, endpoint, target_language, stream, &report).await;
            match result {
                Ok(()) => report.line(format!("PASS: {label}")),
                Err(err) => {
                    failures += 1;
                    report.line(format!(
                        "FAIL: {label}\nError chain: {err:#}\nDebug chain: {err:?}"
                    ));
                }
            }
        }
    }
    report.line(format!(
        "\nRESULT: {} passed, {failures} failed ({total} probes)",
        total - failures
    ));
    ensure!(
        failures == 0,
        "{failures} live LLM probe(s) failed; see redacted diagnostics above"
    );
    Ok(())
}

async fn probe(
    client: &Client,
    target: &DiagnosticEndpoint<'_>,
    language: Language,
    stream: bool,
    report: &Reporter,
) -> anyhow::Result<()> {
    let key = target
        .api_key
        .filter(|key| !key.trim().is_empty())
        .context("API key missing from application configuration")?;
    let url = endpoint(target.provider, &target.model, stream)?;
    let body = request_body(target.provider, &target.model, language, stream)?;
    report.line(format!(
        "POST {url}\nContent-Type: application/json\nRequest JSON:\n{}",
        serde_json::to_string_pretty(&body)?
    ));
    let mut request = client.post(url).json(&body).header(
        "accept",
        if stream {
            "text/event-stream"
        } else {
            "application/json"
        },
    );
    request = if target.provider == TranslationProvider::Google {
        request.header("x-goog-api-key", key)
    } else {
        request.bearer_auth(key)
    };
    let started = Instant::now();
    let mut response = request
        .send()
        .await
        .context("HTTP request failed before response headers")?;
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    report.line(format!(
        "HTTP {} (headers after {:?})",
        status,
        started.elapsed()
    ));
    for name in [
        "content-type",
        "content-length",
        "date",
        "server",
        "x-request-id",
        "request-id",
        "x-goog-request-id",
        "cf-ray",
        "retry-after",
    ] {
        if let Some(value) = response.headers().get(name) {
            report.line(format!(
                "{name}: {}",
                value.to_str().unwrap_or("<non-text>")
            ));
        }
    }
    let mut raw = Vec::new();
    let mut chunks = 0;
    let mut read_error = None;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                chunks += 1;
                if chunks == 1 {
                    report.line(format!("First body bytes after {:?}", started.elapsed()));
                }
                let keep = chunk.len().min(LIMIT - raw.len());
                raw.extend_from_slice(&chunk[..keep]);
                if keep < chunk.len() || raw.len() == LIMIT {
                    read_error = Some(anyhow::anyhow!(
                        "response reached the 16 MiB diagnostic limit"
                    ));
                    break;
                }
            }
            Ok(None) => break,
            Err(err) => {
                read_error = Some(anyhow::Error::new(err).context("reading response body"));
                break;
            }
        }
    }
    report.line(format!(
        "Body: {} bytes, {chunks} transport chunks, elapsed {:?}",
        raw.len(),
        started.elapsed()
    ));
    report.line(format!(
        "--- RAW RESPONSE BEGIN ---\n{}\n--- RAW RESPONSE END ---",
        String::from_utf8_lossy(&raw)
    ));
    if let Some(err) = read_error {
        return Err(err);
    }
    ensure!(
        status.is_success(),
        "HTTP request rejected with {status}; raw error body is above"
    );
    ensure!(
        content_type.contains(if stream {
            "text/event-stream"
        } else {
            "application/json"
        }),
        "unexpected Content-Type: {content_type:?}"
    );
    let raw = std::str::from_utf8(&raw).context("response is not UTF-8")?;
    let content = decode_response(target.provider, stream, raw, report).await?;
    report.line(format!("--- ASSEMBLED MODEL OUTPUT ---\n{content}"));
    let mut translation: ParagraphTranslation =
        decode(&content, "application ParagraphTranslation JSON")?;
    translation.normalize_html_entities();
    ensure_complete(PARAGRAPH, &translation)
        .context("translation parsed but failed application completeness check")?;
    report.line(
        "SDK decoding, translation JSON decoding, and application completeness check all passed.",
    );
    Ok(())
}

fn decode<T: DeserializeOwned>(text: &str, label: &str) -> anyhow::Result<T> {
    serde_json::from_str::<Value>(text).with_context(|| format!("{label}: malformed JSON"))?;
    serde_json::from_str(text).with_context(|| format!("{label}: valid JSON but incompatible with the expected Rust type (see line/column and raw body above)"))
}

fn sse_frames(raw: &str) -> anyhow::Result<Vec<String>> {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut frames = Vec::new();
    let mut data = Vec::new();
    for line in normalized.split_terminator('\n') {
        if line.is_empty() {
            if !data.is_empty() {
                frames.push(data.join("\n"));
                data.clear();
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        } else if line == "data" {
            data.push("");
        }
    }
    ensure!(data.is_empty(), "SSE ended with an unterminated data frame");
    ensure!(!frames.is_empty(), "response contains no SSE data frames");
    Ok(frames)
}

async fn decode_response(
    provider: TranslationProvider,
    stream: bool,
    raw: &str,
    report: &Reporter,
) -> anyhow::Result<String> {
    if !stream {
        return if provider == TranslationProvider::Google {
            let response: GenerationResponse = decode(raw, "gemini-rust GenerationResponse")?;
            report.line(format!(
                "Gemini model={:?} usage={:?} feedback={:?}",
                response.model_version, response.usage_metadata, response.prompt_feedback
            ));
            let finish = response
                .candidates
                .first()
                .and_then(|c| c.finish_reason.clone());
            ensure!(
                finish == Some(gemini_rust::FinishReason::Stop),
                "Gemini response did not finish normally: {finish:?}"
            );
            Ok(response.text())
        } else {
            let response: CreateChatCompletionResponse =
                decode(raw, "async-openai CreateChatCompletionResponse")?;
            report.line(format!(
                "OpenRouter id={} model={} usage={:?}",
                response.id, response.model, response.usage
            ));
            let choice = response
                .choices
                .first()
                .context("response has no choices")?;
            report.line(format!(
                "finish_reason={:?} refusal={:?}",
                choice.finish_reason, choice.message.refusal
            ));
            ensure!(
                choice.finish_reason == Some(FinishReason::Stop),
                "response did not finish normally"
            );
            choice
                .message
                .content
                .clone()
                .context("choice has no text content")
        };
    }
    let frames = sse_frames(raw)?;
    report.line(format!("SSE data frames: {}", frames.len()));
    let mut gemini_finish = None;
    let mut text = String::new();
    let mut done = false;
    let mut finish = None;
    for (index, frame) in frames.iter().enumerate() {
        if frame == "[DONE]" {
            done = true;
            report.line(format!("frame {index}: [DONE]"));
            continue;
        }
        let value: Value = decode(frame, &format!("SSE frame {index}"))?;
        report.line(format!(
            "frame {index}: event_type={:?}, JSON keys={:?}",
            value.get("event_type"),
            value.as_object().map(|o| o.keys().collect::<Vec<_>>())
        ));
        if provider == TranslationProvider::Google {
            let response: GenerationResponse = decode(
                frame,
                &format!("SSE frame {index}: gemini-rust GenerationResponse"),
            )?;
            report.line(format!(
                "Gemini usage={:?} feedback={:?}",
                response.usage_metadata, response.prompt_feedback
            ));
            if let Some(reason) = response
                .candidates
                .first()
                .and_then(|c| c.finish_reason.clone())
            {
                gemini_finish = Some(reason);
            }
            text.push_str(&response.text());
        } else {
            ensure!(!done, "received data after [DONE]");
            let response: CreateChatCompletionStreamResponse = decode(
                frame,
                &format!("SSE frame {index}: async-openai CreateChatCompletionStreamResponse"),
            )?;
            if let Some(usage) = response.usage {
                report.line(format!("usage={usage:?}"));
            }
            if let Some(choice) = response.choices.first() {
                if let Some(content) = &choice.delta.content {
                    text.push_str(content);
                }
                if choice.finish_reason.is_some() {
                    finish = choice.finish_reason;
                }
                if let Some(refusal) = &choice.delta.refusal {
                    report.line(format!("refusal={refusal:?}"));
                }
            }
        }
    }
    if provider == TranslationProvider::Google {
        report.line(format!("Gemini stream finish_reason={gemini_finish:?}"));
        ensure!(
            gemini_finish == Some(gemini_rust::FinishReason::Stop),
            "Gemini stream missing normal finish reason"
        );
        Ok(text)
    } else {
        report.line(format!(
            "OpenRouter stream finish_reason={finish:?}, done={done}"
        ));
        ensure!(
            done && finish == Some(FinishReason::Stop),
            "stream missing [DONE] or normal finish_reason"
        );
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_plain_and_json_escaped_credentials() {
        let reporter = Reporter::new(&["secret-123", "key\"with\\escapes"]);
        let input = format!(
            "secret-123 {}",
            serde_json::to_string("key\"with\\escapes").unwrap()
        );
        assert_eq!(reporter.redact(&input), "[REDACTED] \"[REDACTED]\"");
    }

    #[test]
    fn sse_handles_comments_crlf_and_multiline_data() {
        assert_eq!(sse_frames(": keepalive\r\nevent: step.delta\r\ndata: {\r\ndata: \"x\":1}\r\n\r\ndata: [DONE]\r\n\r\n").unwrap(), ["{\n\"x\":1}", "[DONE]"]);
        assert!(sse_frames("data: {}\n").is_err());
    }

    #[test]
    fn decoding_distinguishes_malformed_json_from_sdk_schema_mismatch() {
        let malformed = decode::<GenerationResponse>("not json", "Gemini response").unwrap_err();
        assert!(format!("{malformed:#}").contains("malformed JSON"));
        let incompatible = decode::<GenerationResponse>(
            r#"{"candidates":[{"finishReason":"UNKNOWN_REASON"}]}"#,
            "Gemini response",
        )
        .unwrap_err();
        let message = format!("{incompatible:#}");
        assert!(message.contains("valid JSON but incompatible"));
        assert!(message.contains("unknown variant"));
    }

    #[tokio::test]
    async fn gemini_capture_requires_normal_completion() {
        let report = Reporter::new(&[]);
        let chunk = serde_json::json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]}}]});
        let prefix = format!("data: {chunk}\n\n");
        assert!(
            decode_response(TranslationProvider::Google, true, &prefix, &report)
                .await
                .is_err()
        );
        let end = serde_json::json!({"candidates":[{"finishReason":"STOP"}]});
        assert_eq!(
            decode_response(
                TranslationProvider::Google,
                true,
                &format!("{prefix}data: {end}\n\n"),
                &report
            )
            .await
            .unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn openrouter_capture_checks_finish_reason_and_done() {
        let report = Reporter::new(&[]);
        let chunk = serde_json::json!({"id":"test", "object":"chat.completion.chunk", "created":0, "model":"test",
            "choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}]});
        let prefix = format!("data: {chunk}\n\n");
        assert!(
            decode_response(TranslationProvider::Openrouter, true, &prefix, &report)
                .await
                .is_err()
        );
        assert_eq!(
            decode_response(
                TranslationProvider::Openrouter,
                true,
                &format!("{prefix}data: [DONE]\n\n"),
                &report
            )
            .await
            .unwrap(),
            "hello"
        );
    }

    #[test]
    fn probes_match_application_request_modes() {
        for stream in [false, true] {
            let google = request_body(
                TranslationProvider::Google,
                "models/test-model",
                Language::Eng,
                stream,
            )
            .unwrap();
            assert_eq!(
                google["generationConfig"]["responseMimeType"],
                "application/json"
            );
            assert!(google["generationConfig"]["responseSchema"].is_object());
            assert!(google.get("cachedContent").is_none());
            let router = request_body(
                TranslationProvider::Openrouter,
                "provider/model",
                Language::Eng,
                stream,
            )
            .unwrap();
            assert_eq!(router["response_format"]["type"], "json_object");
            assert!(
                router["messages"][0]["content"]
                    .as_str()
                    .unwrap()
                    .contains("JSON schema for the response:")
            );
        }
    }
}
