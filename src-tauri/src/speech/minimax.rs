//! MiniMax text-to-audio (`/v1/t2a_v2`).
//!
//! Requested as a stream: measured against the same text, the streamed response
//! finishes about a second sooner than the plain one. The stream's last event carries
//! the whole clip again, so that is the one kept rather than stitching the pieces.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{Clip, SpeechError};
use crate::config::Settings;

const DEFAULT_HOST: &str = "https://api.minimax.io";
const CHINA_HOST: &str = "https://api.minimaxi.com";

pub struct MiniMaxSpeech {
    http: reqwest::Client,
    host: &'static str,
    key: String,
    model: String,
}

impl MiniMaxSpeech {
    pub fn from_settings(settings: &Settings, http: reqwest::Client) -> Result<Self, SpeechError> {
        let (key, base_url) = super::minimax_key(settings);
        if key.is_empty() {
            return Err(SpeechError::NotConfigured("no MiniMax API key".into()));
        }
        let host = if base_url.contains("minimaxi.com") {
            CHINA_HOST
        } else {
            DEFAULT_HOST
        };
        Ok(Self {
            http,
            host,
            key,
            model: settings.speech_minimax_model.clone(),
        })
    }

    /// `boost` tells the model which language to pronounce, so a voice picked from one
    /// language's list still reads another correctly.
    pub async fn synthesize(&self, text: &str, voice: &str, boost: &str) -> Result<Clip, SpeechError> {
        let body = json!({
            "model": self.model,
            "text": text,
            "stream": true,
            "language_boost": boost,
            "voice_setting": { "voice_id": voice, "speed": 1, "vol": 1, "pitch": 0 },
            "audio_setting": { "sample_rate": 32000, "bitrate": 128000, "format": "mp3", "channel": 1 },
        });

        let response = self
            .http
            .post(format!("{}/v1/t2a_v2", self.host))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await
            .map_err(|e| SpeechError::Network(format!("could not reach MiniMax: {e}")))?;

        let status = response.status();
        let raw = response
            .text()
            .await
            .map_err(|e| SpeechError::Network(format!("MiniMax stopped mid-answer: {e}")))?;
        if !status.is_success() {
            return Err(SpeechError::Failed(format!(
                "MiniMax answered HTTP {}: {}",
                status.as_u16(),
                snippet(&raw)
            )));
        }

        // A refusal comes back as one plain JSON object, not as a stream.
        let events: Vec<Value> = if raw.trim_start().starts_with('{') {
            vec![serde_json::from_str(&raw).unwrap_or(Value::Null)]
        } else {
            raw.lines()
                .filter_map(|line| line.trim().strip_prefix("data:"))
                .filter_map(|data| serde_json::from_str(data.trim()).ok())
                .collect()
        };

        for event in &events {
            check_base_resp(event)?;
        }

        let audio = events
            .iter()
            .rev()
            .find(|e| e["data"]["status"].as_i64() == Some(2))
            .and_then(|e| e["data"]["audio"].as_str())
            .filter(|hex| !hex.is_empty())
            .ok_or_else(|| SpeechError::Failed("MiniMax returned no audio".into()))?;

        let bytes = decode_hex(audio)
            .ok_or_else(|| SpeechError::Failed("MiniMax returned malformed audio".into()))?;
        Ok(Clip {
            mime: "audio/mpeg",
            bytes,
        })
    }
}

impl MiniMaxSpeech {
    /// The account's system voices, from `/v1/get_voice`.
    pub async fn list_voices(&self) -> Result<Vec<MiniMaxVoice>, SpeechError> {
        let response = self
            .http
            .post(format!("{}/v1/get_voice", self.host))
            .bearer_auth(&self.key)
            .json(&json!({ "voice_type": "system" }))
            .send()
            .await
            .map_err(|e| SpeechError::Network(format!("could not reach MiniMax: {e}")))?;
        let body: Value = response
            .json()
            .await
            .map_err(|e| SpeechError::Failed(format!("MiniMax sent an unreadable voice list: {e}")))?;
        check_base_resp(&body)?;
        serde_json::from_value(body["system_voice"].clone())
            .map_err(|e| SpeechError::Failed(format!("MiniMax sent an unreadable voice list: {e}")))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MiniMaxVoice {
    pub voice_id: String,
    #[serde(default)]
    pub voice_name: String,
    #[serde(default)]
    pub description: Vec<String>,
}

/// MiniMax reports most failures inside a 200 response, in `base_resp`.
fn check_base_resp(event: &Value) -> Result<(), SpeechError> {
    let code = event["base_resp"]["status_code"].as_i64().unwrap_or(0);
    if code == 0 {
        return Ok(());
    }
    let message = event["base_resp"]["status_msg"].as_str().unwrap_or("unknown error");
    Err(match code {
        1004 | 2049 => SpeechError::Rejected(format!("MiniMax rejected the API key ({message}).")),
        1002 | 1039 => SpeechError::Rejected(format!("MiniMax is rate limiting this key ({message}).")),
        1008 | 2056 => SpeechError::Rejected(format!(
            "The MiniMax plan has no speech quota left right now ({message})."
        )),
        _ => SpeechError::Failed(format!("MiniMax error {code}: {message}")),
    })
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

fn snippet(body: &str) -> String {
    body.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_errors_are_reported_as_such() {
        let event = json!({ "base_resp": { "status_code": 2056, "status_msg": "usage limit exceeded" } });
        let error = check_base_resp(&event).unwrap_err();
        assert!(matches!(error, SpeechError::Rejected(_)));
        assert!(error.to_string().contains("quota"));
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0, 255, 16]));
        assert_eq!(decode_hex("0"), None);
    }
}
