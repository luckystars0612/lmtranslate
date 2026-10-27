//! Read-aloud: turns a translation (or its original) into audio.
//!
//! Settings picks the voice — MiniMax, Azure or the offline Kokoro voice — and, with
//! fallback on, the other kind stands behind it: the offline voice after a cloud one,
//! a configured cloud voice after the offline one. A cloud voice with no key is simply
//! skipped, so a fresh install with the offline voice downloaded reads offline. Text is cut into sentences and each one is sent to the webview as soon as
//! it is ready, so playback starts after the first sentence rather than the whole text.
//! Every engine synthesises far faster than real time, so the next clip is always ready
//! before the current one finishes playing.

pub mod azure;
pub mod local;
pub mod minimax;
pub mod voices;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};

use crate::config::Settings;
use crate::state::AppState;

/// Ceiling for one cloud request. A sentence takes one to three seconds.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer than any realistic read-aloud; past this the text is cut, not refused.
const MAX_CHARS: usize = 6000;
/// A clip this long is roughly fifteen seconds of speech: long enough that the next one
/// is always ready in time, short enough that the first one arrives quickly.
const MAX_CLIP_CHARS: usize = 260;
/// Fragments shorter than this are merged into their neighbour; one-word clips sound
/// choppy because each clip is read with no knowledge of the next.
const MIN_CLIP_CHARS: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum SpeechError {
    /// The engine is not set up (no key, not downloaded). Not worth logging loudly.
    #[error("{0}")]
    NotConfigured(String),
    /// The service answered and said no: bad key, quota, rate limit.
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Failed(String),
}

/// One synthesised piece of audio, in a format the webview's decoder understands.
pub struct Clip {
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// What the webview receives while a text is being read.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SpeechEvent {
    /// Which engine is speaking; `fallback` is set once the preferred one has failed.
    #[serde(rename_all = "camelCase")]
    Engine { name: String, fallback: bool },
    /// The next clip in order, base64-encoded.
    Clip { mime: String, audio: String },
    Done,
    Error { message: String },
}

/// Shared across reads: the HTTP client keeps its connections warm between sentences,
/// and the offline voice stays loaded between uses.
pub struct SpeechState {
    generation: AtomicU64,
    http: reqwest::Client,
    pub local: local::LocalState,
    /// Voice lists, fetched once per session: MiniMax's, and Azure's for one region.
    minimax_voices: Mutex<Option<Vec<minimax::MiniMaxVoice>>>,
    azure_voices: Mutex<Option<(String, Vec<azure::AzureVoice>)>>,
}

/// Settings' "Preview" button: read with exactly this engine and voice, no fallback.
#[derive(Clone, Debug, Deserialize)]
pub struct Preview {
    pub engine: String,
    pub voice: String,
}

/// One entry of a voice dropdown.
#[derive(Clone, Debug, Serialize)]
pub struct VoiceOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceList {
    /// The voice used when none is picked.
    pub default_id: String,
    pub voices: Vec<VoiceOption>,
}

impl Default for SpeechState {
    fn default() -> Self {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("lmtranslate/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            generation: AtomicU64::new(0),
            http,
            local: local::LocalState::default(),
            minimax_voices: Mutex::new(None),
            azure_voices: Mutex::new(None),
        }
    }
}

impl SpeechState {
    /// Stops whatever is being read. The running job notices at its next clip.
    pub fn cancel(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }
}

/// One way of producing speech, with the voice already settled.
enum Engine {
    MiniMax {
        engine: minimax::MiniMaxSpeech,
        voice: String,
        boost: &'static str,
    },
    Azure {
        engine: azure::AzureSpeech,
        voice: String,
    },
    Local {
        speaker: i32,
    },
}

impl Engine {
    fn name(&self) -> &'static str {
        match self {
            Engine::MiniMax { .. } => "MiniMax",
            Engine::Azure { .. } => "Azure",
            Engine::Local { .. } => "Offline voice",
        }
    }

    async fn synthesize(&self, app: &AppHandle, text: &str) -> Result<Clip, SpeechError> {
        match self {
            Engine::MiniMax {
                engine,
                voice,
                boost,
            } => engine.synthesize(text, voice, boost).await,
            Engine::Azure { engine, voice } => engine.synthesize(text, voice).await,
            Engine::Local { speaker } => {
                let app = app.clone();
                let text = text.to_string();
                let speaker = *speaker;
                tauri::async_runtime::spawn_blocking(move || {
                    local::synthesize(&app, &text, speaker)
                })
                .await
                .map_err(|e| SpeechError::Failed(format!("the offline voice crashed: {e}")))?
            }
        }
    }
}

/// Starts reading `text` aloud, replacing anything already being read. `language` is
/// one of the app's language names, or `None` to let the engine work it out.
pub fn speak(
    app: AppHandle,
    text: String,
    language: Option<String>,
    preview: Option<Preview>,
    channel: Channel<SpeechEvent>,
) -> Result<(), String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Err("The app is shutting down.".into());
    };
    let text: String = text.trim().chars().take(MAX_CHARS).collect();
    if text.is_empty() {
        return Err("There is nothing to read.".into());
    }
    let language = language.filter(|l| !l.trim().is_empty());
    let generation = state.speech.cancel();
    let settings = state.settings_snapshot();

    let engines = match preview {
        Some(preview) => vec![build(
            &app,
            &settings,
            &state.speech,
            &preview.engine,
            Some(&preview.voice),
            language.as_deref(),
            &text,
        )?],
        None => engine_chain(&app, &settings, &state.speech, language.as_deref(), &text)?,
    };

    tauri::async_runtime::spawn(read_aloud(app, generation, text, engines, channel));
    Ok(())
}

async fn read_aloud(
    app: AppHandle,
    generation: u64,
    text: String,
    engines: Vec<Engine>,
    channel: Channel<SpeechEvent>,
) {
    let clips = split_into_clips(&text);
    let mut current = 0;
    let started = Instant::now();
    log_info(
        &app,
        &format!(
            "speech start engine={} clips={} chars={}",
            engines[0].name(),
            clips.len(),
            text.chars().count()
        ),
    );
    let _ = channel.send(SpeechEvent::Engine {
        name: engines[0].name().into(),
        fallback: false,
    });

    for (index, clip_text) in clips.iter().enumerate() {
        loop {
            if !is_current(&app, generation) {
                return;
            }
            let engine = &engines[current];
            let clip_started = Instant::now();
            match engine.synthesize(&app, clip_text).await {
                Ok(clip) => {
                    if index == 0 {
                        log_info(
                            &app,
                            &format!(
                                "speech first clip via {} in {}ms",
                                engine.name(),
                                clip_started.elapsed().as_millis()
                            ),
                        );
                    }
                    if !is_current(&app, generation) {
                        return;
                    }
                    let _ = channel.send(SpeechEvent::Clip {
                        mime: clip.mime.into(),
                        audio: STANDARD.encode(clip.bytes),
                    });
                    break;
                }
                Err(error) => {
                    log_warn(
                        &app,
                        &format!("speech clip {index} failed via {}: {error}", engine.name()),
                    );
                    current += 1;
                    if current >= engines.len() {
                        let _ = channel.send(SpeechEvent::Error {
                            message: error.to_string(),
                        });
                        return;
                    }
                    // Stay on the fallback for the rest of the text, so the voice does
                    // not switch back and forth between sentences.
                    let _ = channel.send(SpeechEvent::Engine {
                        name: engines[current].name().into(),
                        fallback: true,
                    });
                }
            }
        }
    }

    log_info(
        &app,
        &format!("speech done in {}ms", started.elapsed().as_millis()),
    );
    let _ = channel.send(SpeechEvent::Done);
}

/// The chosen engine first, then — with fallback on — the other kind. Engines that are
/// not set up, or cannot read this language, are left out rather than tried and failed.
fn engine_chain(
    app: &AppHandle,
    settings: &Settings,
    speech: &SpeechState,
    language: Option<&str>,
    text: &str,
) -> Result<Vec<Engine>, String> {
    let primary = settings.speech_provider.as_str();
    let mut order = vec![primary];
    if settings.speech_fallback {
        if primary == "local" {
            order.extend(["minimax", "azure"]);
        } else {
            order.push("local");
        }
    }

    let mut chain = Vec::new();
    let mut skipped = Vec::new();
    for kind in order {
        match build(app, settings, speech, kind, None, language, text) {
            Ok(engine) => chain.push(engine),
            Err(reason) => skipped.push(reason),
        }
    }
    if chain.is_empty() {
        return Err(format!("No voice can read this. {}", skipped.join(" ")));
    }
    if !skipped.is_empty() {
        log_info(app, &format!("speech: skipped — {}", skipped.join(" ")));
    }
    Ok(chain)
}

/// Sets up one engine with the voice for this language: `voice` when given (a
/// preview), else the one picked in Settings, else the engine's default.
fn build(
    app: &AppHandle,
    settings: &Settings,
    speech: &SpeechState,
    kind: &str,
    voice: Option<&str>,
    language: Option<&str>,
    text: &str,
) -> Result<Engine, String> {
    let picked = voice.or_else(|| picked_voice(settings, kind, language));
    match kind {
        "minimax" => {
            let engine = minimax::MiniMaxSpeech::from_settings(settings, speech.http.clone())
                .map_err(|_| "MiniMax has no API key.".to_string())?;
            let (default, boost) = voices::minimax(language);
            Ok(Engine::MiniMax {
                engine,
                voice: picked.unwrap_or(default).to_string(),
                boost,
            })
        }
        "azure" => {
            let engine = azure::AzureSpeech::from_settings(settings, speech.http.clone())
                .map_err(|_| "Azure needs a key and a region.".to_string())?;
            let (default, _) = voices::azure(language);
            Ok(Engine::Azure {
                engine,
                voice: picked.unwrap_or(default).to_string(),
            })
        }
        "local" => {
            if !local::is_installed(app) {
                return Err("The offline voice is not downloaded.".into());
            }
            let speaker = voices::local_pick(picked, language, text)
                .ok_or("The offline voice reads only English and Chinese.")?;
            Ok(Engine::Local { speaker })
        }
        other => Err(format!("Unknown voice \"{other}\".")),
    }
}

/// The voice picked in Settings for this engine and language, if any. The original
/// side of a translation has no known language, so it always gets the default.
fn picked_voice<'a>(settings: &'a Settings, kind: &str, language: Option<&str>) -> Option<&'a str> {
    settings
        .speech_voices
        .get(&format!("{kind}:{}", language?))
        .map(String::as_str)
}

/// The voices Settings offers for `engine` and `language`.
pub async fn list_voices(app: &AppHandle, engine: &str, language: &str) -> Result<VoiceList, String> {
    let state = app.state::<AppState>();
    let settings = state.settings_snapshot();
    let speech = &state.speech;

    match engine {
        "minimax" => {
            let default_id = voices::minimax(Some(language)).0.to_string();
            let cached = speech.minimax_voices.lock().ok().and_then(|c| c.clone());
            let all = match cached {
                Some(all) => all,
                None => {
                    let client = minimax::MiniMaxSpeech::from_settings(&settings, speech.http.clone())
                        .map_err(|_| "Add a MiniMax API key to see its voices.".to_string())?;
                    let all = client.list_voices().await.map_err(|e| e.to_string())?;
                    if let Ok(mut cache) = speech.minimax_voices.lock() {
                        *cache = Some(all.clone());
                    }
                    all
                }
            };
            let prefix = voices::minimax_prefix(language).unwrap_or("English_");
            let voices = all
                .into_iter()
                .filter(|v| v.voice_id.starts_with(prefix))
                .map(|v| {
                    let name = if v.voice_name.is_empty() { v.voice_id.clone() } else { v.voice_name.clone() };
                    let about: String = v.description.first().map(|d| d.chars().take(70).collect()).unwrap_or_default();
                    VoiceOption {
                        label: if about.is_empty() { name } else { format!("{name} — {about}") },
                        id: v.voice_id,
                    }
                })
                .collect();
            Ok(VoiceList { default_id, voices })
        }
        "azure" => {
            let default_id = voices::azure(Some(language)).0.to_string();
            let client = azure::AzureSpeech::from_settings(&settings, speech.http.clone())
                .map_err(|_| "Add an Azure key and region to see its voices.".to_string())?;
            let cached = speech
                .azure_voices
                .lock()
                .ok()
                .and_then(|c| c.clone())
                .filter(|(region, _)| region == client.region())
                .map(|(_, all)| all);
            let all = match cached {
                Some(all) => all,
                None => {
                    let all = client.list_voices().await.map_err(|e| e.to_string())?;
                    if let Ok(mut cache) = speech.azure_voices.lock() {
                        *cache = Some((client.region().to_string(), all.clone()));
                    }
                    all
                }
            };
            let locales = voices::azure_locales(language);
            let mut voices: Vec<VoiceOption> = all
                .into_iter()
                .filter(|v| locales.contains(&v.locale.as_str()) || !v.secondary_locale_list.is_empty())
                .map(|v| {
                    let multilingual = if v.secondary_locale_list.is_empty() { "" } else { " · multilingual" };
                    VoiceOption {
                        label: format!("{} ({}, {}){multilingual}", v.local_name, v.locale, v.gender),
                        id: v.short_name,
                    }
                })
                .collect();
            // The language's own voices first, multilingual ones after.
            voices.sort_by_key(|v| v.label.contains("multilingual"));
            Ok(VoiceList { default_id, voices })
        }
        "local" => {
            let default_id = voices::local(Some(language), "").map(|id| id.to_string()).unwrap_or_default();
            Ok(VoiceList {
                default_id,
                voices: voices::local_voices(language)
                    .into_iter()
                    .map(|(id, label)| VoiceOption { id, label })
                    .collect(),
            })
        }
        other => Err(format!("Unknown voice \"{other}\".")),
    }
}

/// Cuts text into clips at sentence ends, never mid-word, merging fragments too short
/// to read naturally on their own.
pub fn split_into_clips(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = text.chars().collect();

    for (i, &c) in chars.iter().enumerate() {
        current.push(c);
        let next = chars.get(i + 1).copied();
        let ends_sentence = match c {
            '。' | '！' | '？' | '；' | '\n' => true,
            // "7.45", "e.g.x" and "file.txt" are not sentence ends.
            '.' | '!' | '?' | ';' => next.is_none_or(char::is_whitespace),
            _ => false,
        };
        if ends_sentence || current.chars().count() >= MAX_CLIP_CHARS {
            if !ends_sentence {
                // Over-long sentence: give back everything after the last soft break.
                let cut = current
                    .char_indices()
                    .rev()
                    .find(|(_, ch)| matches!(ch, ',' | '，' | '、' | ':' | '：' | ' '))
                    .map(|(at, ch)| at + ch.len_utf8())
                    .filter(|&at| at > 0 && at < current.len());
                if let Some(at) = cut {
                    let rest = current.split_off(at);
                    sentences.push(std::mem::replace(&mut current, rest));
                    continue;
                }
            }
            sentences.push(std::mem::take(&mut current));
        }
    }
    sentences.push(current);

    let mut clips: Vec<String> = Vec::new();
    for sentence in sentences {
        let sentence = sentence.trim();
        if !sentence.chars().any(char::is_alphanumeric) {
            continue;
        }
        match clips.last_mut() {
            Some(last)
                if last.chars().count() < MIN_CLIP_CHARS
                    && last.chars().count() + sentence.chars().count() < MAX_CLIP_CHARS =>
            {
                // Chinese runs sentences together; a space there would be read as a pause.
                if last.chars().last().is_some_and(|c| c.is_ascii()) {
                    last.push(' ');
                }
                last.push_str(sentence);
            }
            _ => clips.push(sentence.to_string()),
        }
    }
    clips
}

fn is_current(app: &AppHandle, generation: u64) -> bool {
    app.try_state::<AppState>()
        .map(|state| state.speech.is_current(generation))
        .unwrap_or(false)
}

/// Builds a 16-bit mono WAV, which every webview can decode.
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// The first non-empty MiniMax key: the one set for speech, else the translation key.
pub(crate) fn minimax_key(settings: &Settings) -> (String, String) {
    let own = settings.speech_minimax_key.trim();
    // The host follows the translation provider's Base URL, so a mainland-China account
    // (api.minimaxi.com) keeps working.
    let mut base_url = String::new();
    let mut key = own.to_string();
    for id in [settings.provider.as_str(), "minimax-anthropic", "minimax"] {
        if !id.starts_with("minimax") {
            continue;
        }
        if let Some(cfg) = settings.providers.get(id) {
            if base_url.is_empty() {
                base_url = cfg.base_url.clone();
            }
            if key.is_empty() && !cfg.api_key.trim().is_empty() {
                key = cfg.api_key.trim().to_string();
                base_url = cfg.base_url.clone();
            }
        }
    }
    (key, base_url)
}

fn log_info(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.info(message);
    }
}

fn log_warn(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.warn(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_sentence_ends_but_not_inside_numbers() {
        let clips = split_into_clips(
            "The train leaves at 7.45 today, which is early for everyone involved. \
             Please be on time! Thanks a lot for understanding, really.",
        );
        assert_eq!(clips.len(), 2, "{clips:?}");
        assert!(clips[0].contains("7.45"));
        // "Please be on time!" is short, so it rides along with what follows.
        assert!(clips[1].starts_with("Please be on time!"));
    }

    #[test]
    fn splits_chinese_at_full_width_punctuation() {
        let clips = split_into_clips("银行今天几点关门？我们用 GitHub 来管理代码，这个问题很重要。明天见。");
        assert!(clips.iter().all(|c| c.chars().count() <= MAX_CLIP_CHARS));
        assert_eq!(clips.join(""), "银行今天几点关门？我们用 GitHub 来管理代码，这个问题很重要。明天见。");
    }

    #[test]
    fn breaks_an_over_long_sentence_at_a_comma_or_space() {
        let long = "word, ".repeat(80);
        let clips = split_into_clips(&long);
        assert!(clips.len() > 1);
        assert!(clips.iter().all(|c| c.chars().count() <= MAX_CLIP_CHARS));
    }

    #[test]
    fn drops_punctuation_only_fragments() {
        assert_eq!(split_into_clips("... !!! ???"), Vec::<String>::new());
    }

    #[test]
    fn the_wav_header_matches_the_payload() {
        let wav = encode_wav(&[0.0, 0.5, -0.5], 24_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
    }
}
