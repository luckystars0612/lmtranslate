pub mod anthropic;
pub mod cache;
pub mod client;
pub mod prompts;
pub mod provider;
pub mod types;

use std::sync::Arc;

use crate::config::{ApiShape, ProviderConfig, ProviderMeta, Settings};

use anthropic::AnthropicProvider;
use client::MiniMaxProvider;
use provider::TranslateProvider;
use types::ProviderError;

/// Builds the active provider from settings. Called at startup and again whenever the
/// provider section of Settings changes.
pub fn build_provider(settings: &Settings) -> Result<Arc<dyn TranslateProvider>, ProviderError> {
    build_from(settings.active_meta(), settings.active_provider_config())
}

/// The models Settings offers for a provider: its built-in list where it has one,
/// otherwise whatever the endpoint's `/models` says it serves.
pub async fn list_models(meta: ProviderMeta, cfg: ProviderConfig) -> Result<Vec<String>, String> {
    if !meta.models.is_empty() {
        return Ok(meta.models.iter().map(|m| m.to_string()).collect());
    }

    let base = cfg.base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Enter the Base URL first.".into());
    }
    let key = cfg.api_key.trim();
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(concat!("lmtranslate/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;

    let request = match meta.api {
        ApiShape::OpenAiChat => http.get(format!("{base}/models")).bearer_auth(key),
        ApiShape::AnthropicMessages => {
            // Same base-URL tolerance as the Messages endpoint: with or without "/v1".
            let base = base.strip_suffix("/messages").unwrap_or(base);
            let url = if base.ends_with("/v1") {
                format!("{base}/models")
            } else {
                format!("{base}/v1/models")
            };
            http.get(url)
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .bearer_auth(key)
        }
    };

    let response = request
        .send()
        .await
        .map_err(|e| format!("Could not reach the endpoint: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "The endpoint did not list its models (HTTP {}). Type the model name instead.",
            status.as_u16()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("The model list was unreadable: {e}"))?;
    let models: Vec<String> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(str::to_string))
        .collect();
    if models.is_empty() {
        return Err("The endpoint listed no models. Type the model name instead.".into());
    }
    Ok(models)
}

pub fn build_from(
    meta: ProviderMeta,
    cfg: ProviderConfig,
) -> Result<Arc<dyn TranslateProvider>, ProviderError> {
    match meta.api {
        ApiShape::OpenAiChat => Ok(Arc::new(MiniMaxProvider::new(meta, cfg)?)),
        ApiShape::AnthropicMessages => Ok(Arc::new(AnthropicProvider::new(meta, cfg)?)),
    }
}
