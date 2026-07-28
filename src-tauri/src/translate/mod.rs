pub mod client;
pub mod prompts;
pub mod provider;
pub mod types;

use std::sync::Arc;

use crate::config::{ProviderConfig, ProviderMeta, Settings};

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
    // Both entries in `config::PROVIDERS` speak the OpenAI-compatible chat shape today.
    Ok(Arc::new(MiniMaxProvider::new(meta, cfg)?))
}
