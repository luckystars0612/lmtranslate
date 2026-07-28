//! The provider abstraction.
//!
//! MiniMax M3 is the only implementation in v1, but capture and UI code only ever see
//! this trait, so a second provider (or a self-hosted OpenAI-compatible endpoint) is a
//! new impl plus a row in `config::PROVIDERS`.
//!
//! The methods are `async` — unlike the sketch in the plan — because every real
//! implementation is an HTTP call and the popup must stay responsive while it runs.

use async_trait::async_trait;

use super::types::{ProviderError, TranslateRequest, TranslateResponse};

#[async_trait]
pub trait TranslateProvider: Send + Sync {
    async fn translate(&self, req: TranslateRequest) -> Result<TranslateResponse, ProviderError>;

    /// False for text-only models: the screenshot flow refuses early and says so.
    fn supports_image_input(&self) -> bool;

    /// A cheap round trip behind the Settings "Test connection" button.
    /// On success, returns a short human-readable summary of what answered.
    async fn validate(&self) -> Result<String, ProviderError>;
}
