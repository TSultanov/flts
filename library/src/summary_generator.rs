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
};
use isolang::Language;
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
                    .execute(),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Gemini summary request timed out"))??;

            let text = response.text();
            if text.is_empty() {
                anyhow::bail!("Gemini summary returned empty content");
            }
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
                .into_iter()
                .next()
                .and_then(|c| c.message.content)
                .unwrap_or_default();
            if text.is_empty() {
                anyhow::bail!("OpenAI summary returned empty content");
            }
            Ok(text)
        }
    }
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
