//! Single-call generator for per-chapter source-language summaries.
//!
//! The summary fuels cross-paragraph context during translation (see
//! `library/src/translator/gemini.rs` and `openai.rs`). One call per
//! chapter; the prior chapter's summary is fed in as context so summaries
//! form a chain that captures cumulative book state.

use std::time::Duration;

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
    ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    CreateChatCompletionResponse,
};
use gemini_rust::GenerationResponse;
use isolang::Language;
use log::{debug, info};
use tokio::time::timeout;

use crate::translator::{TranslationModel, TranslationProvider};

/// Generous ceiling for a non-streaming summary call. Summaries are short
/// outputs (~200 tokens) but a slow model on a long chapter input may take
/// a while; we don't want to retry aggressively because the caller will
/// just give up on the book.
const SUMMARY_REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

fn system_prompt(book_language: &Language) -> String {
    let lang = book_language.to_name();
    format!(
        "You are summarizing a chapter of a book for the purpose of providing context \
         to a future translator working into a foreign language. The translator's \
         goal: make consistent choices about characters, register, and recurring \
         vocabulary across the whole book.\n\n\
         Bias your summary HEAVILY toward facts the translator needs and AWAY from \
         plot retelling:\n\
         - Characters introduced or developed: name, apparent gender, social position, \
           distinguishing attributes (age, profession, dialect, relationship to other \
           characters). Note anything that would affect gendered/honorific \
           translation choices in inflected target languages.\n\
         - Narrator point of view and tense (first-person past, third-person \
           omniscient, etc.).\n\
         - Register and tone: formal, informal, archaic, poetic, conversational, \
           technical. Dialect or sociolect.\n\
         - Recurring proper nouns, place names, or invented terms specific to this \
           book's setting.\n\
         - Conventions a translator should follow (honorifics, dialect markers, \
           code-switching) when carried over from prior chapters.\n\
         \n\
         Do NOT retell the plot beat by beat. Skip generic action sequences. \
         150-250 words is a good target — shorter is fine if the chapter introduces \
         little new context.\n\
         \n\
         When the chapter depicts sensitive material — illicit drugs, violence, \
         sexual content, self-harm, prejudice, or other content-moderation \
         hot-buttons — describe the register, tone, and translation challenge in \
         general terms, but do NOT enumerate specific street slang, name specific \
         substances, define drug or criminal vocabulary, or produce glossary-style \
         lists of such terms with explanations. Example of what to say: \"the \
         chapter uses period-specific {lang} drug slang in informal teenage \
         register; the translator should match the era and the generational \
         tone.\" Example of what NOT to say: \"key terms include X (meaning Y), \
         Z (meaning W) …\" — that style of list is unsafe.\n\
         \n\
         The output must be in {lang} (the book's language). Plain prose, no \
         markdown, no headings."
    )
}

fn user_message(
    book_title: &str,
    chapter_title: Option<&str>,
    chapter_text: &str,
    prior_summary: Option<&str>,
) -> String {
    let mut out = String::with_capacity(chapter_text.len() + 1024);
    out.push_str("Book title: ");
    out.push_str(book_title);
    out.push_str("\n\n");

    if let Some(prior) = prior_summary
        && !prior.is_empty()
    {
        out.push_str("Summary of prior chapters (in order):\n");
        out.push_str(prior);
        out.push_str("\n\n");
    }

    out.push_str("Chapter");
    if let Some(title) = chapter_title
        && !title.is_empty()
    {
        out.push_str(": ");
        out.push_str(title);
    }
    out.push_str("\n\nText:\n");
    out.push_str(chapter_text);
    out
}

/// Run a single non-streaming summary call against the user's selected
/// model. Returns the plain-text summary in the book's language.
pub async fn generate_chapter_summary(
    provider: TranslationProvider,
    model: TranslationModel,
    api_key: &str,
    book_language: &Language,
    book_title: &str,
    chapter_title: Option<&str>,
    chapter_text: &str,
    prior_summary: Option<&str>,
) -> anyhow::Result<String> {
    let system = system_prompt(book_language);
    let user = user_message(book_title, chapter_title, chapter_text, prior_summary);

    debug!(
        "summary request: provider={provider:?} model={model:?} book={book_title:?} \
         chapter={chapter_title:?} chars(text/prior/sys)={}/{}/{}",
        chapter_text.len(),
        prior_summary.map(|s| s.len()).unwrap_or(0),
        system.len(),
    );

    match provider {
        TranslationProvider::Google => {
            let gemini_model = crate::translator::gemini::gemini_model(model)?;
            let client = crate::translator::gemini::gemini_client(api_key.to_string(), gemini_model)?;
            let response = timeout(
                SUMMARY_REQUEST_TIMEOUT,
                client
                    .generate_content()
                    .with_system_prompt(system)
                    .with_user_message(user)
                    .with_safety_settings(crate::translator::gemini::permissive_safety_settings())
                    .execute(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Gemini summary request timed out"))??;

            let text = response.text();
            if text.is_empty() {
                let diag = describe_empty_gemini_response(&response);
                anyhow::bail!("Gemini summary returned empty content ({diag})");
            }
            info!(
                "Gemini summary ok: response_id={:?} model={:?} \
                 tokens(prompt/cand/thoughts/total)={:?}/{:?}/{:?}/{:?}",
                response.response_id.as_deref(),
                response.model_version.as_deref(),
                response.usage_metadata.as_ref().and_then(|u| u.prompt_token_count),
                response.usage_metadata.as_ref().and_then(|u| u.candidates_token_count),
                response.usage_metadata.as_ref().and_then(|u| u.thoughts_token_count),
                response.usage_metadata.as_ref().and_then(|u| u.total_token_count),
            );
            Ok(text)
        }
        TranslationProvider::Openai => {
            let model_name = crate::translator::openai::openai_model_name(model)?;
            let client = crate::translator::openai::openai_client(api_key.to_string());

            let request = CreateChatCompletionRequestArgs::default()
                .model(model_name)
                .messages([
                    ChatCompletionRequestMessage::System(
                        ChatCompletionRequestSystemMessageArgs::default()
                            .content(system)
                            .build()?,
                    ),
                    ChatCompletionRequestMessage::User(
                        ChatCompletionRequestUserMessageArgs::default()
                            .content(user)
                            .build()?,
                    ),
                ])
                .build()?;

            let response = timeout(
                SUMMARY_REQUEST_TIMEOUT,
                client.chat().create(request),
            )
            .await
            .map_err(|_| anyhow::anyhow!("OpenAI summary request timed out"))??;

            let text = response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default();
            if text.is_empty() {
                let diag = describe_empty_openai_response(&response);
                anyhow::bail!("OpenAI summary returned empty content ({diag})");
            }
            info!(
                "OpenAI summary ok: id={:?} model={:?} \
                 tokens(prompt/completion/total)={:?}/{:?}/{:?}",
                response.id,
                response.model,
                response.usage.as_ref().map(|u| u.prompt_tokens),
                response.usage.as_ref().map(|u| u.completion_tokens),
                response.usage.as_ref().map(|u| u.total_tokens),
            );
            Ok(text)
        }
    }
}

/// Build a compact diagnostic string from a Gemini response whose `text()`
/// was empty, surfacing the fields that explain *why* nothing came back:
/// candidate count, finish_reason, the shape of `content.parts` (since
/// `text()` only reads parts\[0\] when it's `Part::Text`), elevated safety
/// ratings, prompt block reason, token usage (the thinking-budget smoking
/// gun for Gemini 2.5/3), and ids for support repros.
fn describe_empty_gemini_response(resp: &GenerationResponse) -> String {
    let mut bits: Vec<String> = Vec::new();
    bits.push(format!("candidates={}", resp.candidates.len()));
    if let Some(c) = resp.candidates.first() {
        bits.push(format!("finish_reason={:?}", c.finish_reason));
        let parts_shape = match c.content.parts.as_ref() {
            None => "parts=None".to_string(),
            Some(v) if v.is_empty() => "parts=[]".to_string(),
            Some(v) => {
                let names: Vec<&'static str> = v.iter().map(part_variant_name).collect();
                format!("parts={names:?}")
            },
        };
        bits.push(parts_shape);
        if let Some(ratings) = c.safety_ratings.as_ref() {
            let elevated: Vec<String> = ratings
                .iter()
                .filter(|r| {
                    matches!(
                        r.probability,
                        gemini_rust::HarmProbability::Medium | gemini_rust::HarmProbability::High
                    )
                })
                .map(|r| format!("{:?}={:?}", r.category, r.probability))
                .collect();
            if !elevated.is_empty() {
                bits.push(format!("safety=[{}]", elevated.join(",")));
            }
        }
    }
    if let Some(pf) = resp.prompt_feedback.as_ref()
        && let Some(br) = pf.block_reason.as_ref()
    {
        bits.push(format!("prompt_block_reason={br:?}"));
    }
    if let Some(um) = resp.usage_metadata.as_ref() {
        bits.push(format!(
            "tokens(prompt/cand/thoughts/total)={:?}/{:?}/{:?}/{:?}",
            um.prompt_token_count,
            um.candidates_token_count,
            um.thoughts_token_count,
            um.total_token_count,
        ));
    }
    if let Some(rid) = resp.response_id.as_deref() {
        bits.push(format!("response_id={rid}"));
    }
    if let Some(mv) = resp.model_version.as_deref() {
        bits.push(format!("model_version={mv}"));
    }
    bits.join(", ")
}

fn part_variant_name(p: &gemini_rust::Part) -> &'static str {
    use gemini_rust::Part;
    match p {
        Part::Text { thought: Some(true), .. } => "Text(thought)",
        Part::Text { .. } => "Text",
        Part::InlineData { .. } => "InlineData",
        Part::FunctionCall { .. } => "FunctionCall",
        Part::FunctionResponse { .. } => "FunctionResponse",
        Part::FileData { .. } => "FileData",
        Part::ExecutableCode { .. } => "ExecutableCode",
        Part::CodeExecutionResult { .. } => "CodeExecutionResult",
    }
}

/// Same idea for OpenAI: when `choices[0].message.content` is empty/missing,
/// surface choice count, finish_reason (`length`, `content_filter`, etc.),
/// any explicit `refusal` string, token usage, and ids.
fn describe_empty_openai_response(resp: &CreateChatCompletionResponse) -> String {
    let mut bits: Vec<String> = Vec::new();
    bits.push(format!("choices={}", resp.choices.len()));
    if let Some(c) = resp.choices.first() {
        bits.push(format!("finish_reason={:?}", c.finish_reason));
        bits.push(format!("content_is_some={}", c.message.content.is_some()));
        if let Some(refusal) = c.message.refusal.as_deref() {
            bits.push(format!("refusal={refusal:?}"));
        }
    }
    if let Some(u) = resp.usage.as_ref() {
        bits.push(format!(
            "tokens(prompt/completion/total)={}/{}/{}",
            u.prompt_tokens, u.completion_tokens, u.total_tokens,
        ));
    }
    bits.push(format!("id={:?}", resp.id));
    bits.push(format!("model={:?}", resp.model));
    bits.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_includes_prior_summary_when_present() {
        let msg = user_message(
            "Anna Karenina",
            Some("I"),
            "Chapter text here.",
            Some("Previously: characters introduced."),
        );
        assert!(msg.contains("Book title: Anna Karenina"));
        assert!(msg.contains("Summary of prior chapters"));
        assert!(msg.contains("Previously: characters introduced."));
        assert!(msg.contains("Chapter: I"));
        assert!(msg.contains("Chapter text here."));
    }

    #[test]
    fn user_message_skips_prior_summary_for_first_chapter() {
        let msg = user_message("Some Book", None, "First chapter text.", None);
        assert!(msg.contains("Book title: Some Book"));
        assert!(!msg.contains("Summary of prior chapters"));
        assert!(msg.contains("First chapter text."));
    }

    #[test]
    fn user_message_skips_prior_summary_when_empty_string() {
        let msg = user_message("X", None, "text", Some(""));
        assert!(!msg.contains("Summary of prior chapters"));
    }

    #[test]
    fn system_prompt_targets_book_language() {
        let p = system_prompt(&Language::Eng);
        assert!(p.contains("must be in English"));
    }
}
