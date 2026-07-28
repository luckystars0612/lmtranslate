//! MiniMax M3 client, spoken over the OpenAI-compatible `/chat/completions` shape.
//!
//! The same implementation backs the "OpenAI-compatible endpoint" provider — the only
//! difference is which base URL and model name come out of the settings.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::{ProviderConfig, ProviderMeta, REQUEST_TIMEOUT_SECS};

use super::prompts;
use super::provider::TranslateProvider;
use super::types::{
    InputKind, ProviderError, TranslateRequest, TranslateResponse, TranslatedSegment,
};

/// Upper bound on generated tokens; the per-request budget is scaled down from here.
const MAX_TOKENS: u32 = 8192;

pub struct MiniMaxProvider {
    http: reqwest::Client,
    cfg: ProviderConfig,
    label: &'static str,
    supports_image: bool,
}

impl MiniMaxProvider {
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

    /// Host only — safe to put in the log and in UI copy.
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

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.cfg.base_url.trim_end_matches('/')
        )
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
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(self.cfg.api_key.trim())
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
                    body: truncate(&text, 400),
                },
            });
        }

        Ok(text)
    }

    /// One user-turn chat completion. `content` is either a string (text mode) or an
    /// array of content parts (image mode).
    async fn chat(
        &self,
        content: Value,
        max_tokens: u32,
    ) -> Result<(String, Option<String>), ProviderError> {
        self.preflight()?;

        let body = json!({
            "model": self.cfg.model,
            "temperature": prompts::TEMPERATURE,
            // Bounded so a model that starts rambling cannot stall the popup for the
            // full request timeout.
            "max_tokens": max_tokens,
            // Honoured where supported; the prompts also demand JSON and the parser
            // has a brace-matching fallback, so it is safe when it is ignored.
            "response_format": { "type": "json_object" },
            "messages": [
                { "role": "system", "content": prompts::SYSTEM_PROMPT },
                { "role": "user", "content": content },
            ],
        });

        let raw = self.post(body).await?;
        extract_message_content(&raw)
    }
}

/// Output budget for a request.
///
/// The generated token count is what determines how long the user waits, so it is
/// scaled to the input rather than left at a flat maximum. Text mode now returns only
/// the translation, so its output tracks the input length closely; a screenshot can
/// contain an unknown amount of text and gets the full budget.
pub(super) fn max_tokens_for(input: &InputKind, ceiling: u32) -> u32 {
    match input {
        InputKind::Text(text) => {
            // ~3 tokens per source character is generous even for scripts that expand.
            let estimate = text.chars().count().saturating_mul(3).clamp(256, ceiling as usize);
            estimate as u32
        }
        InputKind::ImageBase64(_) => ceiling,
    }
}

#[async_trait]
impl TranslateProvider for MiniMaxProvider {
    async fn translate(&self, req: TranslateRequest) -> Result<TranslateResponse, ProviderError> {
        let (content, fallback_original) = match &req.input {
            InputKind::Text(text) => (
                json!(prompts::text_prompt(&req.target_lang, text)),
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
                    json!([
                        { "type": "text", "text": prompts::image_prompt(&req.target_lang) },
                        {
                            "type": "image_url",
                            "image_url": { "url": format!("data:image/png;base64,{b64}") }
                        }
                    ]),
                    String::new(),
                )
            }
        };

        let budget = max_tokens_for(&req.input, MAX_TOKENS);
        let (text, usage) = self.chat(content, budget).await?;
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
            "temperature": 0,
            "messages": [{ "role": "user", "content": "ping" }],
        });

        let raw = self.post(body).await?;
        // A 2xx that carries a provider-level error still means the key was read.
        check_provider_error(&serde_json::from_str::<Value>(&raw).unwrap_or(Value::Null))?;

        Ok(format!(
            "{} answered as {}.",
            self.endpoint_host(),
            self.cfg.model
        ))
    }
}

/// Shared with `super::anthropic`: transport failures look the same either way.
pub(super) fn map_transport_error(e: reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Timeout(REQUEST_TIMEOUT_SECS)
    } else {
        ProviderError::Network(e.to_string())
    }
}

pub(super) fn truncate(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max).collect();
    format!("{head}…")
}

/// MiniMax reports application-level failures inside an HTTP 200 body.
fn check_provider_error(value: &Value) -> Result<(), ProviderError> {
    let Some(base) = value.get("base_resp") else {
        return Ok(());
    };
    let code = base.get("status_code").and_then(Value::as_i64).unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let message = base
        .get("status_msg")
        .and_then(Value::as_str)
        .unwrap_or("no detail given")
        .to_string();

    Err(match code {
        1004 | 1039 => ProviderError::Unauthorized,
        1002 | 1008 => ProviderError::RateLimited,
        other => ProviderError::Api {
            code: other,
            message,
        },
    })
}

/// Returns the assistant text and, when the endpoint reports it, a token-usage summary.
fn extract_message_content(body: &str) -> Result<(String, Option<String>), ProviderError> {
    let value: Value = serde_json::from_str(body).map_err(|e| {
        ProviderError::Unparseable(format!("the endpoint did not return JSON ({e})"))
    })?;

    check_provider_error(&value)?;

    let usage = summarise_usage(&value, "prompt_tokens", "completion_tokens");

    let message = value
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .ok_or_else(|| {
            ProviderError::Unparseable(format!(
                "the reply had no choices ({})",
                truncate(body, 200)
            ))
        })?;

    let text = match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        // Some OpenAI-compatible servers return content as an array of parts.
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => {
            return Err(ProviderError::Unparseable(
                "the reply carried no message content".into(),
            ))
        }
    };

    Ok((text, usage))
}

/// "in=123 out=456" from whichever field names the contract uses.
pub(super) fn summarise_usage(value: &Value, input_key: &str, output_key: &str) -> Option<String> {
    let usage = value.get("usage")?;
    let input = usage.get(input_key).and_then(Value::as_i64);
    let output = usage.get(output_key).and_then(Value::as_i64);
    match (input, output) {
        (None, None) => None,
        _ => Some(format!(
            "in={} out={}",
            input.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
            output.map(|v| v.to_string()).unwrap_or_else(|| "?".into()),
        )),
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    segments: Vec<TranslatedSegment>,
}

/// Best-effort read of the model's JSON, degrading to a single raw segment rather
/// than failing the whole translation.
pub fn parse_segments(raw: &str, fallback_original: &str) -> Vec<TranslatedSegment> {
    let cleaned = strip_reasoning(raw);
    let unfenced = strip_fence(cleaned);

    let mut candidates: Vec<String> = vec![cleaned.to_string()];
    if unfenced != cleaned {
        candidates.push(unfenced.to_string());
    }
    // A reasoning model puts its answer *after* its deliberation, and the deliberation
    // often quotes the schema. Prefer the last balanced block for that reason.
    let mut blocks = json_objects(unfenced);
    blocks.reverse();
    candidates.extend(blocks);

    for candidate in candidates {
        if let Ok(envelope) = serde_json::from_str::<Envelope>(&candidate) {
            let segments: Vec<TranslatedSegment> = envelope
                .segments
                .into_iter()
                .filter(|s| !s.translated.trim().is_empty())
                .collect();
            if !segments.is_empty() {
                return segments;
            }
        }
    }

    // Note this falls back to the *cleaned* text: if the JSON cannot be read at all,
    // the popup still must never show the model's private reasoning.
    vec![TranslatedSegment {
        original: fallback_original.to_string(),
        translated: unfenced.trim().to_string(),
        ..Default::default()
    }]
}

/// Removes a reasoning model's chain-of-thought preamble.
///
/// MiniMax-M3 is a reasoning model. Well-behaved deployments return the deliberation in
/// a separate `reasoning_content` field, which `extract_message_content` already ignores;
/// others inline it in `content` wrapped in `<think>` tags. That has to come off before
/// anything else, because the deliberation discusses the required JSON schema and so is
/// full of braces — which is enough to defeat any brace-matching scan of the whole reply.
fn strip_reasoning(raw: &str) -> &str {
    const CLOSERS: [&str; 3] = ["</think>", "</thinking>", "</reasoning>"];
    const OPENERS: [&str; 3] = ["<think>", "<thinking>", "<reasoning>"];

    let mut out = raw.trim();

    // Everything up to the *last* close tag is deliberation.
    for closer in CLOSERS {
        if let Some(index) = out.rfind(closer) {
            out = out[index + closer.len()..].trim();
        }
    }

    // An opener with no closer means the reply was cut off mid-thought; there is no
    // answer in it, and returning the thought would be worse than returning nothing.
    for opener in OPENERS {
        if let Some(index) = out.find(opener) {
            out = out[..index].trim();
        }
    }

    out
}

/// Every balanced top-level `{...}` block, in order, ignoring braces inside strings.
fn json_objects(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut blocks = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        if chars[start] != '{' {
            start += 1;
            continue;
        }

        let mut depth = 0usize;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;

        for (offset, &c) in chars[start..].iter().enumerate() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_string = false;
                }
                continue;
            }
            match c {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + offset);
                        break;
                    }
                }
                _ => {}
            }
        }

        match end {
            Some(index) => {
                blocks.push(chars[start..=index].iter().collect());
                start = index + 1;
            }
            // Unbalanced from here on, so there is nothing further to find.
            None => break,
        }
    }

    blocks
}

fn strip_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    // Drop an optional language tag on the opening fence.
    let rest = rest.split_once('\n').map(|(_, body)| body).unwrap_or(rest);
    rest.trim_end()
        .strip_suffix("```")
        .unwrap_or(rest)
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_clean_reply() {
        let raw = r#"{"segments":[{"original":"Hello","translated":"Xin chào"}]}"#;
        let segments = parse_segments(raw, "Hello");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].translated, "Xin chào");
    }

    #[test]
    fn parses_a_fenced_reply() {
        let raw = "```json\n{\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}\n```";
        let segments = parse_segments(raw, "Hi");
        assert_eq!(segments[0].translated, "Chào");
    }

    #[test]
    fn parses_json_buried_in_prose() {
        let raw = "Sure! Here you go:\n{\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}\nHope that helps.";
        let segments = parse_segments(raw, "Hi");
        assert_eq!(segments[0].translated, "Chào");
    }

    #[test]
    fn braces_inside_strings_do_not_end_the_block() {
        let raw = r#"{"segments":[{"original":"a}b","translated":"c}d"}]}"#;
        let segments = parse_segments(raw, "a}b");
        assert_eq!(segments[0].translated, "c}d");
    }

    #[test]
    fn keeps_image_position_hints() {
        let raw = r#"{"segments":[{"original":"A","translated":"B","x_pct":10.5,"y_pct":20,"w_pct":30}]}"#;
        let segments = parse_segments(raw, "");
        assert_eq!(segments[0].x_pct, Some(10.5));
        assert_eq!(segments[0].y_pct, Some(20.0));
    }

    #[test]
    fn falls_back_to_a_single_raw_segment() {
        let segments = parse_segments("Xin chào", "Hello");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].original, "Hello");
        assert_eq!(segments[0].translated, "Xin chào");
    }

    #[test]
    fn strips_a_reasoning_block() {
        let raw = "<think>The user wants Vietnamese. Let me translate.</think>\n\
                   {\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}";
        let segments = parse_segments(raw, "Hi");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].translated, "Chào");
    }

    /// The regression that put chain-of-thought in the popup: the deliberation quotes
    /// the schema, so a brace scan over the whole reply starts inside the reasoning and
    /// never balances.
    #[test]
    fn strips_reasoning_that_quotes_the_schema() {
        let raw = "<think>\nI should return it as a single segment.\n\
                   Format: {\"segments\": [{\"original\"\n</think>\n\
                   {\"segments\":[{\"original\":\"Monitor and block\",\"translated\":\"Giám sát và chặn\"}]}";
        let segments = parse_segments(raw, "Monitor and block");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].translated, "Giám sát và chặn");
        assert!(!segments[0].translated.contains("think"));
    }

    #[test]
    fn prefers_the_answer_over_a_decoy_earlier_in_the_reply() {
        let raw = "Example of the shape: {\"segments\":[{\"original\":\"x\",\"translated\":\"DECOY\"}]}\n\
                   Here is the real answer:\n\
                   {\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}";
        let segments = parse_segments(raw, "Hi");
        assert_eq!(segments[0].translated, "Chào");
    }

    #[test]
    fn a_fenced_reply_after_reasoning_still_parses() {
        let raw = "<think>thinking {braces} here</think>\n```json\n\
                   {\"segments\":[{\"original\":\"Hi\",\"translated\":\"Chào\"}]}\n```";
        let segments = parse_segments(raw, "Hi");
        assert_eq!(segments[0].translated, "Chào");
    }

    /// Truncated mid-thought: there is no answer, and leaking the thought would be
    /// worse than reporting nothing. An empty translation becomes an error state.
    #[test]
    fn unclosed_reasoning_never_leaks() {
        let raw = "<think>Let me work out how to phrase this";
        let segments = parse_segments(raw, "Hi");
        assert_eq!(segments.len(), 1);
        assert!(segments[0].translated.is_empty());
    }

    #[test]
    fn the_raw_fallback_is_never_the_reasoning() {
        let raw = "<think>deliberating</think>Chào buổi sáng";
        let segments = parse_segments(raw, "Good morning");
        assert_eq!(segments[0].translated, "Chào buổi sáng");
    }

    #[test]
    fn detects_a_provider_error_inside_a_200() {
        let body = r#"{"base_resp":{"status_code":1004,"status_msg":"bad key"}}"#;
        let err = extract_message_content(body).unwrap_err();
        assert!(matches!(err, ProviderError::Unauthorized));
    }
}
