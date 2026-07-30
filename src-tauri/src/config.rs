//! Settings load/save. A plain JSON file in the OS config dir — no plugin, so the
//! shape stays entirely ours and is easy to hand-edit when debugging.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const DEFAULT_HOTKEY_TEXT: &str = "Alt+D";
pub const DEFAULT_HOTKEY_REPLACE: &str = "Alt+F";
pub const DEFAULT_HOTKEY_SCREENSHOT: &str = "Alt+S";
pub const DEFAULT_TARGET_LANG: &str = "Vietnamese";
/// The rewrite direction: you have written something in your own language and want the
/// selection replaced by this language instead.
pub const DEFAULT_REPLACE_LANG: &str = "English";

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

/// Which HTTP contract a provider speaks. These are genuinely different request and
/// response shapes, not variants — see `translate::client` and `translate::anthropic`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiShape {
    /// `POST {base}/chat/completions`, reasoning inlined in the message content.
    OpenAiChat,
    /// `POST {base}/v1/messages`, reasoning carried in its own content block.
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMeta {
    pub id: &'static str,
    pub label: &'static str,
    pub default_base_url: &'static str,
    pub default_model: &'static str,
    pub supports_image: bool,
    pub api: ApiShape,
}

/// The list is what the UI binds to, so adding a provider is a row here plus a
/// `TranslateProvider` impl.
///
/// The Anthropic-shaped MiniMax endpoint is first, and so the default for a fresh
/// install, because it keeps the model's chain-of-thought in a separate content block
/// instead of inlining it in the answer — see `translate::anthropic` for why that
/// matters. The OpenAI-shaped entries remain because that contract is what every
/// self-hosted and proxy server speaks.
pub const PROVIDERS: &[ProviderMeta] = &[
    ProviderMeta {
        id: "minimax-anthropic",
        label: "MiniMax (Anthropic API)",
        default_base_url: "https://api.minimax.io/anthropic",
        default_model: "MiniMax-M3",
        supports_image: true,
        api: ApiShape::AnthropicMessages,
    },
    ProviderMeta {
        id: "minimax",
        label: "MiniMax (OpenAI-compatible API)",
        default_base_url: "https://api.minimax.io/v1",
        default_model: "MiniMax-M3",
        supports_image: true,
        api: ApiShape::OpenAiChat,
    },
    ProviderMeta {
        id: "openai-compatible",
        label: "OpenAI-compatible endpoint",
        default_base_url: "http://localhost:8000/v1",
        default_model: "",
        supports_image: true,
        api: ApiShape::OpenAiChat,
    },
    ProviderMeta {
        id: "anthropic-compatible",
        label: "Anthropic-compatible endpoint",
        default_base_url: "https://api.anthropic.com",
        default_model: "",
        supports_image: true,
        api: ApiShape::AnthropicMessages,
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
    /// The language the rewrite hotkey translates into, replacing the selection.
    pub replace_lang: String,
    pub hotkey_text: String,
    pub hotkey_replace: String,
    pub hotkey_screenshot: String,
    pub launch_on_login: bool,
    /// When false, a successful rewrite happens silently — no popup at all. Failures
    /// still surface one, because a silent no-op would be indistinguishable from a
    /// broken hotkey.
    pub rewrite_show_popup: bool,
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
            replace_lang: DEFAULT_REPLACE_LANG.to_string(),
            hotkey_text: DEFAULT_HOTKEY_TEXT.to_string(),
            hotkey_replace: DEFAULT_HOTKEY_REPLACE.to_string(),
            hotkey_screenshot: DEFAULT_HOTKEY_SCREENSHOT.to_string(),
            launch_on_login: false,
            rewrite_show_popup: true,
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
        if self.replace_lang.trim().is_empty() {
            self.replace_lang = defaults.replace_lang;
        }
        if self.hotkey_text.trim().is_empty() {
            self.hotkey_text = defaults.hotkey_text;
        }
        if self.hotkey_replace.trim().is_empty() {
            self.hotkey_replace = defaults.hotkey_replace;
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

/// The one folder this app writes to, named after the app and nothing else.
///
/// Deliberately not derived from the bundle identifier: `app_config_dir()` and
/// `app_log_dir()` append the whole reverse-DNS identifier, which puts vendor branding
/// into paths the user has to look at.
pub const APP_DIR: &str = "LMTranslate";

/// Folder names written by earlier builds, checked once so an upgrade does not lose a
/// saved API key. Safe to delete an entry here once nobody is upgrading from it.
const LEGACY_DIRS: &[&str] = &["vn.vinsoc.lmtranslate", "com.lmtranslate.desktop"];

fn app_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .config_dir()
        .map(|dir| dir.join(APP_DIR))
        .map_err(|e| format!("could not locate the config directory: {e}"))
}

pub fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_dir(app)?.join("settings.json"))
}

pub fn log_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_dir(app)?.join("logs").join("lmtranslate.log"))
}

/// Copies settings forward from a folder an older build used, if the new one has none.
/// Returns the path it migrated from, for the log.
pub fn migrate_legacy_settings(app: &AppHandle, target: &Path) -> Option<PathBuf> {
    if target.exists() {
        return None;
    }
    let base = app.path().config_dir().ok()?;

    for legacy in LEGACY_DIRS {
        let candidate = base.join(legacy).join("settings.json");
        if !candidate.exists() {
            continue;
        }
        if let Some(parent) = target.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return None;
            }
        }
        if std::fs::copy(&candidate, target).is_ok() {
            return Some(candidate);
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Container-level `#[serde(default)]` fills missing fields from `Settings::default()`,
    /// not from each field type's own `Default`. That distinction matters: a settings file
    /// written before `rewriteShowPopup` existed must keep showing the popup rather than
    /// silently switching rewrites to quiet mode, which `bool::default()` would do.
    #[test]
    fn missing_fields_come_from_the_struct_default() {
        let parsed: Settings = serde_json::from_str(r#"{"targetLang":"Japanese"}"#).unwrap();
        assert_eq!(parsed.target_lang, "Japanese");
        assert!(
            parsed.rewrite_show_popup,
            "an older settings file must keep showing the rewrite popup"
        );
        assert_eq!(parsed.hotkey_replace, DEFAULT_HOTKEY_REPLACE);
        assert_eq!(parsed.replace_lang, DEFAULT_REPLACE_LANG);
    }

    #[test]
    fn an_explicit_false_is_preserved() {
        let parsed: Settings =
            serde_json::from_str(r#"{"rewriteShowPopup":false}"#).unwrap();
        assert!(!parsed.rewrite_show_popup);
    }

    /// Blank strings are backfilled, but a deliberate `false` must survive normalisation.
    #[test]
    fn normalising_does_not_resurrect_the_popup() {
        let parsed: Settings = serde_json::from_str(r#"{"rewriteShowPopup":false}"#).unwrap();
        assert!(!parsed.normalized().rewrite_show_popup);
    }
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
