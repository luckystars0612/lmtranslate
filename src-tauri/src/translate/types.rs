//! The only vocabulary the capture flows and the popup speak.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum InputKind {
    Text(String),
    /// PNG, base64-encoded.
    ImageBase64(String),
}

impl InputKind {
    pub fn label(&self) -> &'static str {
        match self {
            InputKind::Text(_) => "text",
            InputKind::ImageBase64(_) => "image",
        }
    }

    /// Size in the unit that matters for that kind — characters or base64 bytes.
    pub fn magnitude(&self) -> usize {
        match self {
            InputKind::Text(t) => t.chars().count(),
            InputKind::ImageBase64(b) => b.len(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TranslateRequest {
    pub input: InputKind,
    pub target_lang: String,
}

/// Field names are snake_case on purpose: this is deserialised straight out of the
/// model's JSON reply, which uses the schema from the prompt templates.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TranslatedSegment {
    #[serde(default)]
    pub original: String,
    #[serde(default)]
    pub translated: String,
    /// Only present for image input.
    #[serde(default)]
    pub x_pct: Option<f32>,
    #[serde(default)]
    pub y_pct: Option<f32>,
    #[serde(default)]
    pub w_pct: Option<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct TranslateResponse {
    pub segments: Vec<TranslatedSegment>,
    pub raw_model_output: String,
    /// Token counts reported by the endpoint, logged so slow translations can be traced
    /// to the thing that actually causes them: generated tokens.
    pub usage: Option<String>,
}

impl TranslateResponse {
    pub fn joined_original(&self) -> String {
        join(self.segments.iter().map(|s| s.original.as_str()))
    }

    pub fn joined_translated(&self) -> String {
        join(self.segments.iter().map(|s| s.translated.as_str()))
    }
}

fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("no API key is set for this provider")]
    MissingApiKey,
    #[error("the endpoint rejected the API key")]
    Unauthorized,
    #[error("the endpoint is rate limiting this key")]
    RateLimited,
    #[error("the request took longer than {0} seconds")]
    Timeout(u64),
    #[error("could not reach the endpoint: {0}")]
    Network(String),
    #[error("the endpoint returned HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("the endpoint reported error {code}: {message}")]
    Api { code: i64, message: String },
    #[error("could not read the model's reply: {0}")]
    Unparseable(String),
    #[error("{0}")]
    Config(String),
}

impl ProviderError {
    /// (title, detail, hint) for the popup's error state.
    ///
    /// Every branch names what went wrong *and* what to do about it — an error that
    /// just says "translation failed" is not an acceptable end state.
    pub fn user_facing(&self) -> (String, String, String) {
        match self {
            ProviderError::MissingApiKey => (
                "No API key yet".into(),
                "This provider has no key saved.".into(),
                "Add one in Settings -> Provider, then press the hotkey again.".into(),
            ),
            ProviderError::Unauthorized => (
                "Invalid API key".into(),
                "The endpoint rejected the key that is saved for this provider.".into(),
                "Check it in Settings -> Provider and use \"Test connection\".".into(),
            ),
            ProviderError::RateLimited => (
                "Rate limited".into(),
                "The provider is throttling this key.".into(),
                "Wait a few seconds and press the hotkey again.".into(),
            ),
            ProviderError::Timeout(secs) => (
                "Timed out".into(),
                format!("No reply within {secs} seconds."),
                "Check your connection, or try a smaller selection.".into(),
            ),
            ProviderError::Network(detail) => (
                "Could not reach the endpoint".into(),
                detail.clone(),
                "Check your connection and the Base URL in Settings -> Provider.".into(),
            ),
            ProviderError::Http { status, body } => (
                format!("Endpoint returned HTTP {status}"),
                body.clone(),
                "Check the Base URL and model name in Settings -> Provider.".into(),
            ),
            ProviderError::Api { code, message } => (
                format!("Provider error {code}"),
                message.clone(),
                "Check the model name in Settings -> Provider.".into(),
            ),
            ProviderError::Unparseable(detail) => (
                "Unreadable reply".into(),
                detail.clone(),
                "The model did not answer in the expected format. Try again, or pick a \
                 different model in Settings -> Provider."
                    .into(),
            ),
            ProviderError::Config(detail) => (
                "Provider not configured".into(),
                detail.clone(),
                "Finish setting it up in Settings -> Provider.".into(),
            ),
        }
    }
}
