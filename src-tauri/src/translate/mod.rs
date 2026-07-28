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

pub fn build_from(
    meta: ProviderMeta,
    cfg: ProviderConfig,
) -> Result<Arc<dyn TranslateProvider>, ProviderError> {
    match meta.api {
        ApiShape::OpenAiChat => Ok(Arc::new(MiniMaxProvider::new(meta, cfg)?)),
        ApiShape::AnthropicMessages => Ok(Arc::new(AnthropicProvider::new(meta, cfg)?)),
    }
}
