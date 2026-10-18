//! Run either ignored test explicitly from the repository root:
//! ```text
//! FLTS_RUN_LIVE_LLM_DIAGNOSTICS=1 cargo test -p app --lib --features manual-llm-diagnostics gemini_rust_endpoint -- --ignored --nocapture
//! FLTS_RUN_LIVE_LLM_DIAGNOSTICS=1 cargo test -p app --lib --features manual-llm-diagnostics async_openai_openrouter_endpoint -- --ignored --nocapture
//! ```
//! Reads existing app credentials without changing settings; sends synthetic text only.
//! `FLTS_CONFIG_DIR` selects the app config directory. Optional model overrides:
//! `FLTS_DIAG_GEMINI_MODEL` and `FLTS_DIAG_OPENROUTER_MODEL`.

use anyhow::Context;
use library::translator::{
    TranslationProvider,
    catalog::effective_model_id,
    diagnostics::{DiagnosticEndpoint, run},
};

use super::{config::Config, resolve_config_dir};

#[tokio::test]
#[ignore = "LIVE, BILLABLE: requires FLTS_RUN_LIVE_LLM_DIAGNOSTICS=1"]
async fn gemini_rust_endpoint() -> anyhow::Result<()> {
    run_provider(TranslationProvider::Google, "FLTS_DIAG_GEMINI_MODEL").await
}

#[tokio::test]
#[ignore = "LIVE, BILLABLE: requires FLTS_RUN_LIVE_LLM_DIAGNOSTICS=1"]
async fn async_openai_openrouter_endpoint() -> anyhow::Result<()> {
    run_provider(
        TranslationProvider::Openrouter,
        "FLTS_DIAG_OPENROUTER_MODEL",
    )
    .await
}

async fn run_provider(provider: TranslationProvider, model_override: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("FLTS_RUN_LIVE_LLM_DIAGNOSTICS").as_deref() == Ok("1"),
        "manual live-call opt-in required: FLTS_RUN_LIVE_LLM_DIAGNOSTICS=1"
    );
    let path = resolve_config_dir(None)?.join("config.json");
    // Use the app's deserializer without Config::load's corrupt-file rename/default fallback.
    let config: Config = serde_json::from_reader(
        std::fs::File::open(&path)
            .with_context(|| format!("opening application config {}", path.display()))?,
    )
    .context("parsing application config (left unchanged)")?;
    println!("Configuration: {} (read-only)", path.display());
    let target_language = isolang::Language::from_639_3(&config.target_language_id)
        .context("invalid targetLanguageId in application config")?;
    let model = std::env::var(model_override)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            effective_model_id(
                provider,
                if config.translation_provider == provider {
                    &config.model
                } else {
                    ""
                },
            )
        });
    let api_keys = config.api_keys();
    let endpoint = DiagnosticEndpoint {
        provider,
        model,
        api_key: api_keys.for_provider(provider),
    };
    let secrets: Vec<&str> = [
        config.gemini_api_key.as_deref(),
        config.openai_api_key.as_deref(),
        config.deepseek_api_key.as_deref(),
        config.zai_api_key.as_deref(),
        config.openrouter_api_key.as_deref(),
        config.anki_api_key.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    run(&[endpoint], target_language, &secrets).await
}
