//! Settings load/save. A plain JSON file in the OS config dir — no plugin, so the
//! shape stays entirely ours and is easy to hand-edit when debugging.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const DEFAULT_HOTKEY_TEXT: &str = "Alt+D";
pub const DEFAULT_HOTKEY_SCREENSHOT: &str = "Alt+S";
pub const DEFAULT_TARGET_LANG: &str = "Vietnamese";

/// Ceiling for a single translation call, popup error state included.
pub const REQUEST_TIMEOUT_SECS: u64 = 45;

/// Offered in both the Settings combo box and the popup's language dropdown.
/// Kept here rather than inline in the UI so there is one source of truth.
pub const COMMON_LANGUAGES: &[&str] = &[
    "Vietnamese",
    "English",
    "Simplified Chinese",
    "Traditional Chinese",
    "Japanese",
    "Korean",
    "Thai",
    "French",
    "German",
    "Spanish",
    "Portuguese",
    "Italian",
    "Russian",
    "Arabic",
    "Hindi",
    "Indonesian",
];

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMeta {
    pub id: &'static str,
    pub label: &'static str,
    pub default_base_url: &'static str,
    pub default_model: &'static str,
    pub supports_image: bool,
}

/// v1 ships one real implementation, but the list is the thing the UI binds to, so
/// adding a provider is a matter of adding a row plus a `TranslateProvider` impl.
pub const PROVIDERS: &[ProviderMeta] = &[
    ProviderMeta {
        id: "minimax",
        label: "MiniMax",
        default_base_url: "https://api.minimax.io/v1",
        default_model: "MiniMax-M3",
        supports_image: true,
    },
    ProviderMeta {
        id: "openai-compatible",
        label: "OpenAI-compatible endpoint",
        default_base_url: "http://localhost:8000/v1",
        default_model: "",
        supports_image: true,
    },
];

pub fn provider_meta(id: &str) -> ProviderMeta {
    PROVIDERS
        .iter()
        .copied()
        .find(|p| p.id == id)
        .unwrap_or(PROVIDERS[0])
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub provider: String,
    /// Per-provider credentials, so switching provider does not lose the other key.
    pub providers: BTreeMap<String, ProviderConfig>,
    pub target_lang: String,
    pub hotkey_text: String,
    pub hotkey_screenshot: String,
    pub launch_on_login: bool,
    /// 0 means "stay until dismissed".
    pub popup_auto_close_ms: u64,
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        let providers = PROVIDERS
            .iter()
            .map(|p| {
                (
                    p.id.to_string(),
                    ProviderConfig {
                        api_key: String::new(),
                        base_url: p.default_base_url.to_string(),
                        model: p.default_model.to_string(),
                    },
                )
            })
            .collect();

        Self {
            provider: PROVIDERS[0].id.to_string(),
            providers,
            target_lang: DEFAULT_TARGET_LANG.to_string(),
            hotkey_text: DEFAULT_HOTKEY_TEXT.to_string(),
            hotkey_screenshot: DEFAULT_HOTKEY_SCREENSHOT.to_string(),
            launch_on_login: false,
            popup_auto_close_ms: 0,
            theme: "system".to_string(),
        }
    }
}

impl Settings {
    /// Fills in anything blank or unrecognised so the rest of the app can trust the values.
    pub fn normalized(mut self) -> Self {
        let defaults = Settings::default();

        if provider_meta(&self.provider).id != self.provider {
            self.provider = defaults.provider.clone();
        }

        for meta in PROVIDERS {
            let entry = self.providers.entry(meta.id.to_string()).or_default();
            if entry.base_url.trim().is_empty() {
                entry.base_url = meta.default_base_url.to_string();
            }
            if entry.model.trim().is_empty() {
                entry.model = meta.default_model.to_string();
            }
            entry.base_url = entry.base_url.trim().to_string();
            entry.model = entry.model.trim().to_string();
        }

        if self.target_lang.trim().is_empty() {
            self.target_lang = defaults.target_lang;
        }
        if self.hotkey_text.trim().is_empty() {
            self.hotkey_text = defaults.hotkey_text;
        }
        if self.hotkey_screenshot.trim().is_empty() {
            self.hotkey_screenshot = defaults.hotkey_screenshot;
        }
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = defaults.theme;
        }
        self
    }

    pub fn active_meta(&self) -> ProviderMeta {
        provider_meta(&self.provider)
    }

    pub fn active_provider_config(&self) -> ProviderConfig {
        let meta = self.active_meta();
        self.providers
            .get(&self.provider)
            .cloned()
            .unwrap_or(ProviderConfig {
                api_key: String::new(),
                base_url: meta.default_base_url.to_string(),
                model: meta.default_model.to_string(),
            })
    }
}

pub fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|dir| dir.join("settings.json"))
        .map_err(|e| format!("could not locate the config directory: {e}"))
}

pub fn log_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_log_dir()
        .map(|dir| dir.join("lmtranslate.log"))
        .map_err(|e| format!("could not locate the log directory: {e}"))
}

pub fn load(path: &Path) -> Result<Settings, String> {
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| format!("could not read {path:?}: {e}"))?;
    // Windows editors (and PowerShell's `Set-Content -Encoding utf8`) like to prepend a
    // byte-order mark, which serde_json rejects outright. Hand-editing this file should
    // not silently reset every setting.
    let raw = raw.trim_start_matches('\u{feff}');
    let parsed: Settings =
        serde_json::from_str(raw).map_err(|e| format!("{path:?} is not valid settings: {e}"))?;
    Ok(parsed.normalized())
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {parent:?}: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("could not serialise settings: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("could not write {path:?}: {e}"))
}
