//! The Anthropic Messages contract — `POST {base}/v1/messages`.
//!
//! Why a second transport exists at all: on the OpenAI-compatible endpoint MiniMax-M3
//! inlines its chain-of-thought into the assistant message as `<think>...</think>`, so
//! the only defence is text surgery on the reply — and when that surgery failed, the raw
//! reasoning reached the popup. The Anthropic contract models reasoning as a *distinct
//! content block type*, so reading only `type: "text"` blocks makes that leak
//! structurally impossible rather than merely handled. Same reason the system prompt is
//! a top-level field here instead of a pseudo-message.
//!
//! Two contract details that differ from the OpenAI shape and are easy to get wrong:
//! `max_tokens` is required, and there is no `response_format` — JSON discipline comes
//! from the prompt plus the shared parser.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::{ProviderConfig, ProviderMeta, REQUEST_TIMEOUT_SECS};

use super::client::{
    map_transport_error, max_tokens_for, parse_segments, summarise_usage, truncate,
};
use super::prompts;
use super::provider::TranslateProvider;
use super::types::{InputKind, ProviderError, TranslateRequest, TranslateResponse};

/// Pinned per the Anthropic API's versioning requirement.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Required by the contract. Generous enough for a screenful of translated segments.
const MAX_TOKENS: u32 = 8192;

pub struct AnthropicProvider {
    http: reqwest::Client,
    cfg: ProviderConfig,
    label: &'static str,
    supports_image: bool,
}

impl AnthropicProvider {
    pub fn new(meta: ProviderMeta, cfg: ProviderConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .user_agent(concat!("lmtranslate/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        Ok(Self {
            http,
            cfg,
            label: meta.label,
            supports_image: meta.supports_image,
        })
    }

    pub fn endpoint_host(&self) -> String {
        self.cfg
            .base_url
            .split("://")
            .nth(1)
            .unwrap_or(&self.cfg.base_url)
            .split('/')
            .next()
            .unwrap_or("the endpoint")
            .to_string()
    }

    /// Tolerates a base URL given with or without the version segment, since the same
    /// field is used for `https://api.minimax.io/anthropic` and for gateways that
    /// already include `/v1`.
    fn endpoint(&self) -> String {
        let base = self.cfg.base_url.trim_end_matches('/');
        if base.ends_with("/messages") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        }
    }

    fn preflight(&self) -> Result<(), ProviderError> {
        if self.cfg.base_url.trim().is_empty() {
            return Err(ProviderError::Config(format!(
                "{} has no Base URL.",
                self.label
            )));
        }
        if self.cfg.model.trim().is_empty() {
            return Err(ProviderError::Config(format!(
                "{} has no model name.",
                self.label
            )));
        }
        if self.cfg.api_key.trim().is_empty() {
            return Err(ProviderError::MissingApiKey);
        }
        Ok(())
    }

    async fn post(&self, body: Value) -> Result<String, ProviderError> {
        let key = self.cfg.api_key.trim();

        let response = self
            .http
            .post(self.endpoint())
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            // Anthropic itself authenticates with x-api-key, but compatibility gateways
            // are split on whether they read that or a bearer token, so send both.
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .map_err(map_transport_error)?;

        let status = response.status();
        let text = response.text().await.map_err(map_transport_error)?;

        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => ProviderError::Unauthorized,
                429 => ProviderError::RateLimited,
                other => ProviderError::Http {
                    status: other,
                    body: error_message(&text).unwrap_or_else(|| truncate(&text, 400)),
                },
            });
        }

        Ok(text)
    }

    async fn message(
        &self,
        content: Value,
        max_tokens: u32,
    ) -> Result<(String, Option<String>), ProviderError> {
        self.preflight()?;

        let body = json!({
            "model": self.cfg.model,
            // Required by this contract, unlike the OpenAI one.
            "max_tokens": max_tokens,
            "temperature": prompts::TEMPERATURE,
            // A real top-level field here, not a fake first message.
            "system": prompts::SYSTEM_PROMPT,
            "messages": [{ "role": "user", "content": content }],
        });

        let raw = self.post(body).await?;
        extract_text(&raw)
    }
}

#[async_trait]
impl TranslateProvider for AnthropicProvider {
    async fn translate(&self, req: TranslateRequest) -> Result<TranslateResponse, ProviderError> {
        let (content, fallback_original) = match &req.input {
            InputKind::Text(text) => (
                json!([{ "type": "text", "text": prompts::text_prompt(&req.target_lang, text) }]),
                text.clone(),
            ),
            InputKind::ImageBase64(b64) => {
                if !self.supports_image {
                    return Err(ProviderError::Config(format!(
                        "{} is configured as text-only, so it cannot read a screenshot.",
                        self.label
                    )));
                }
                (
                    // Image before text: this contract reads a trailing instruction
                    // as applying to the image above it.
                    json!([
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": b64,
                            }
                        },
                        { "type": "text", "text": prompts::image_prompt(&req.target_lang) }
                    ]),
                    String::new(),
                )
            }
        };

        let budget = max_tokens_for(&req.input, MAX_TOKENS);
        let (text, usage) = self.message(content, budget).await?;
        Ok(TranslateResponse {
            segments: parse_segments(&text, &fallback_original),
            raw_model_output: text,
            usage,
        })
    }

    fn supports_image_input(&self) -> bool {
        self.supports_image
    }

    async fn validate(&self) -> Result<String, ProviderError> {
        self.preflight()?;

        let body = json!({
            "model": self.cfg.model,
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": "ping" }],
        });

        let raw = self.post(body).await?;
        if let Some(message) = error_message(&raw) {
            return Err(ProviderError::Api {
                code: 0,
                message,
            });
        }

        Ok(format!(
            "{} answered as {} over the Anthropic API.",
            self.endpoint_host(),
            self.cfg.model
        ))
    }
}

/// Concatenates the `text` blocks and nothing else.
///
/// This is the whole point of using this contract: `thinking`, `redacted_thinking` and
/// `tool_use` blocks are simply not `text`, so the model's reasoning cannot end up in
/// the translation no matter what the model decides to emit.
fn extract_text(body: &str) -> Result<(String, Option<String>), ProviderError> {
    let value: Value = serde_json::from_str(body).map_err(|e| {
        ProviderError::Unparseable(format!("the endpoint did not return JSON ({e})"))
    })?;

    if let Some(message) = error_message(body) {
        return Err(ProviderError::Api { code: 0, message });
    }

    let usage = summarise_usage(&value, "input_tokens", "output_tokens");

    let blocks = value
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ProviderError::Unparseable(format!(
                "the reply had no content blocks ({})",
                truncate(body, 200)
            ))
        })?;

    let text: String = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");

    if text.trim().is_empty() {
        // A reply that is all reasoning and no answer usually means max_tokens ran out.
        let stop = value
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(ProviderError::Unparseable(format!(
            "the reply carried no text block (stop_reason: {stop})"
        )));
    }

    Ok((text, usage))
}

/// Pulls `error.message` out of this contract's error envelope.
fn error_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let error = value.get("error")?;
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    let kind = error.get("type").and_then(Value::as_str).unwrap_or("error");
    Some(format!("{kind}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base_url: &str) -> AnthropicProvider {
        AnthropicProvider::new(
            crate::config::provider_meta("minimax-anthropic"),
            ProviderConfig {
                api_key: "k".into(),
                base_url: base_url.into(),
                model: "MiniMax-M3".into(),
            },
        )
        .unwrap()
    }

    #[test]
    fn builds_the_endpoint_from_a_bare_base() {
        assert_eq!(
            provider("https://api.minimax.io/anthropic").endpoint(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn does_not_double_the_version_segment() {
        assert_eq!(
            provider("https://api.minimax.io/anthropic/v1").endpoint(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
        assert_eq!(
            provider("https://api.minimax.io/anthropic/v1/messages/").endpoint(),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn reads_only_text_blocks() {
        let body = r#"{"content":[
            {"type":"text","text":"{\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}"}
        ]}"#;
        let (text, _usage) = extract_text(body).unwrap();
        assert!(text.contains("Chào"));
    }

    /// The structural guarantee: reasoning is a different block type, so it is dropped
    /// without any text surgery.
    #[test]
    fn drops_thinking_blocks_entirely() {
        let body = r#"{"content":[
            {"type":"thinking","thinking":"The user wants Vietnamese. Schema: {\"segments\":[{"},
            {"type":"text","text":"{\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}"}
        ]}"#;
        let (text, _usage) = extract_text(body).unwrap();
        assert!(!text.contains("The user wants"));

        let segments = parse_segments(&text, "Hi");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].translated, "Chào");
    }

    #[test]
    fn reports_a_reply_that_is_all_reasoning() {
        let body = r#"{"content":[{"type":"thinking","thinking":"..."}],"stop_reason":"max_tokens"}"#;
        let err = extract_text(body).unwrap_err();
        assert!(matches!(err, ProviderError::Unparseable(ref m) if m.contains("max_tokens")));
    }

    #[test]
    fn surfaces_the_error_envelope() {
        let body = r#"{"type":"error","error":{"type":"authentication_error","message":"bad key"}}"#;
        let err = extract_text(body).unwrap_err();
        assert!(matches!(err, ProviderError::Api { ref message, .. } if message.contains("bad key")));
    }
}
