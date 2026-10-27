//! Everything the three webviews can ask Rust to do.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::config::{self, ProviderConfig, ProviderMeta, Settings};
use crate::flows;
use crate::hotkeys::{self, Slot};
use crate::platform::{common, PlatformCapabilities};
use crate::secrets;
use crate::speech;
use crate::state::AppState;
use crate::translate::{self, types::ProviderError};
use crate::windows::{crop_overlay, popup_window, selection_bubble, settings_window};

/// A pass/fail/warn result with a sentence the user can act on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// "ok" | "warn" | "bad" | "busy"
    pub tone: String,
    pub message: String,
}

impl Check {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            tone: "ok".into(),
            message: message.into(),
        }
    }
    fn warn(message: impl Into<String>) -> Self {
        Self {
            tone: "warn".into(),
            message: message.into(),
        }
    }
    fn bad(message: impl Into<String>) -> Self {
        Self {
            tone: "bad".into(),
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformReport {
    #[serde(flatten)]
    capabilities: PlatformCapabilities,
    config_path: String,
    log_path: String,
    /// Where the API keys are kept, e.g. "Windows Credential Manager".
    key_storage: String,
}

// ── Settings ───────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings_snapshot()
}

#[tauri::command]
pub fn list_providers() -> Vec<ProviderMeta> {
    config::PROVIDERS.to_vec()
}

/// The translation models to offer for a provider. The key and Base URL come from the
/// form rather than saved settings, so the list follows what is being typed.
#[tauri::command]
pub async fn list_models(
    provider: String,
    base_url: String,
    api_key: String,
) -> Result<Vec<String>, String> {
    let meta = config::provider_meta(&provider);
    translate::list_models(
        meta,
        ProviderConfig {
            api_key,
            base_url: fallback(&base_url, meta.default_base_url),
            model: String::new(),
        },
    )
    .await
}

#[tauri::command]
pub fn list_speech_models() -> Vec<String> {
    config::SPEECH_MODELS.iter().map(|s| s.to_string()).collect()
}

#[tauri::command]
pub fn list_languages() -> Vec<String> {
    config::COMMON_LANGUAGES
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Persists settings and applies every side effect they imply: rebuild the provider,
/// re-register both hotkeys, update launch-on-login.
#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    incoming: Settings,
) -> Result<Settings, String> {
    let settings = incoming.normalized();

    config::save(&state.config_path, &settings)?;

    // Cached results came from the previous provider/model, so they must not be served
    // as though they came from the new one.
    if let Ok(mut cache) = state.cache.lock() {
        let dropped = cache.len();
        cache.clear();
        if dropped > 0 {
            state
                .logger
                .info(format!("dropped {dropped} cached translation(s)"));
        }
    }

    match translate::build_provider(&settings) {
        Ok(provider) => {
            if let Ok(mut slot) = state.provider.write() {
                *slot = provider;
            }
        }
        Err(e) => state
            .logger
            .warn(format!("could not rebuild the provider: {e}")),
    }

    if let Ok(mut slot) = state.settings.write() {
        *slot = settings.clone();
    }

    for problem in hotkeys::apply(&app, &settings) {
        state.logger.warn(&problem);
    }

    apply_autostart(&app, settings.launch_on_login);
    if !settings.selection_bubble {
        selection_bubble::hide(&app);
    }

    state.logger.info(format!(
        "settings saved (provider={}, target={}, autoClose={}ms, bubble={})",
        settings.provider,
        settings.target_lang,
        settings.popup_auto_close_ms,
        settings.selection_bubble
    ));

    Ok(settings)
}

#[tauri::command]
pub fn check_hotkey(app: AppHandle, accel: String, slot: String) -> Check {
    let Some(slot) = Slot::parse(&slot) else {
        return Check::bad("Unknown hotkey slot.");
    };
    match hotkeys::probe(&app, &accel, slot) {
        (true, message) => Check::ok(message),
        (false, message) => Check::bad(message),
    }
}

/// Releases every global shortcut while the Settings hotkey picker is recording.
///
/// Without this the picker can never capture a combination that is already registered
/// globally — including LM Translate's own defaults — because the OS delivers it to
/// the owner rather than to the focused webview.
#[tauri::command]
pub fn begin_hotkey_capture(app: AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister_all();
}

/// Puts the real hotkeys back when recording ends, whatever the outcome.
#[tauri::command]
pub fn end_hotkey_capture(app: AppHandle, state: State<'_, AppState>) {
    let settings = state.settings_snapshot();
    for problem in hotkeys::apply(&app, &settings) {
        state.logger.warn(&problem);
    }
}

#[tauri::command]
pub async fn test_connection(
    provider: String,
    api_key: String,
    base_url: String,
    model: String,
) -> Check {
    let meta = config::provider_meta(&provider);

    let cfg = ProviderConfig {
        api_key,
        base_url: fallback(&base_url, meta.default_base_url),
        model: fallback(&model, meta.default_model),
    };

    let host = cfg
        .base_url
        .split("://")
        .nth(1)
        .unwrap_or(&cfg.base_url)
        .split('/')
        .next()
        .unwrap_or("the endpoint")
        .to_string();

    let built = match translate::build_from(meta, cfg) {
        Ok(p) => p,
        Err(e) => return Check::bad(e.to_string()),
    };

    match built.validate().await {
        Ok(message) => Check::ok(message),
        Err(ProviderError::MissingApiKey) => Check::bad("Enter an API key first."),
        Err(ProviderError::Unauthorized) => {
            Check::bad(format!("{host} rejected this API key."))
        }
        Err(ProviderError::RateLimited) => Check::warn(format!(
            "{host} accepted the key but is rate limiting it right now."
        )),
        Err(ProviderError::Http { status: 404, .. }) => Check::bad(format!(
            "No chat endpoint at that Base URL — {host} returned 404."
        )),
        // A 4xx other than 401/404 means we got through and were authenticated; the
        // request itself was refused, most often because of the model name.
        Err(ProviderError::Http { status, body }) if (400..500).contains(&status) => {
            Check::warn(format!(
                "Reached {host} and the key was accepted, but it rejected the test \
                 request (HTTP {status}): {body}"
            ))
        }
        Err(ProviderError::Api { code, message }) => {
            Check::warn(format!("{host} answered with error {code}: {message}"))
        }
        Err(e) => Check::bad(e.to_string()),
    }
}

fn fallback(value: &str, default: &str) -> String {
    if value.trim().is_empty() {
        default.to_string()
    } else {
        value.trim().to_string()
    }
}

#[tauri::command]
pub fn platform_report(state: State<'_, AppState>) -> PlatformReport {
    PlatformReport {
        capabilities: state.platform.capabilities(),
        config_path: state.config_path.display().to_string(),
        log_path: state.logger.path().display().to_string(),
        key_storage: secrets::storage_name().to_string(),
    }
}

/// "system" | "light" | "dark" — the crop overlay needs it and nothing else.
#[tauri::command]
pub fn app_theme(state: State<'_, AppState>) -> String {
    state.settings_snapshot().theme
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    settings_window::open(&app).map_err(|e| e.to_string())
}

// ── Popup ──────────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_popup_state(app: AppHandle) -> popup_window::PopupState {
    popup_window::snapshot(&app)
}

#[tauri::command]
pub fn close_popup(app: AppHandle, state: State<'_, AppState>) {
    // Bump the generation so an in-flight request does not reopen or repaint a popup
    // the user has already dismissed.
    state.next_generation();
    popup_window::close(&app);
}

#[tauri::command]
pub fn keep_popup_open(state: State<'_, AppState>) {
    state.keep_popup_open();
}

#[tauri::command]
pub fn resize_popup(app: AppHandle, height: f64) {
    popup_window::resize_to(&app, height);
}

#[tauri::command]
pub fn retranslate(app: AppHandle, target_lang: String) -> Result<(), String> {
    flows::retranslate(app, target_lang)
}

#[tauri::command]
pub fn copy_text(text: String) -> Result<(), String> {
    common::put_on_clipboard(text)
}

// ── Crop overlay ───────────────────────────────────────────────────────────

/// Coordinates are CSS pixels inside the reporting overlay; Rust converts them.
#[tauri::command]
pub fn crop_commit(
    app: AppHandle,
    window: tauri::WebviewWindow,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) {
    flows::handle_crop(app, window.label().to_string(), x, y, width, height);
}

#[tauri::command]
pub fn crop_cancel(app: AppHandle) {
    flows::cancel_crop(app);
}

/// The frozen screen behind this overlay as PNG bytes, or nothing when it crops live.
/// Raw bytes rather than JSON: a 4K frame would otherwise be serialised number by number.
#[tauri::command]
pub fn crop_frame(app: AppHandle, window: tauri::WebviewWindow) -> tauri::ipc::Response {
    tauri::ipc::Response::new(crop_overlay::take_frame_png(&app, window.label()))
}

/// The overlay has its frame painted and can be shown.
#[tauri::command]
pub fn crop_ready(app: AppHandle, window: tauri::WebviewWindow) {
    crop_overlay::reveal(&app, &window);
}

// ── Read aloud ─────────────────────────────────────────────────────────────

/// Reads `text` aloud, sending clips through `on_event` as they are ready. `lang` is
/// one of the app's language names, or null to let the voice work it out.
#[tauri::command]
pub fn speak(
    app: AppHandle,
    text: String,
    lang: Option<String>,
    preview: Option<speech::Preview>,
    on_event: tauri::ipc::Channel<speech::SpeechEvent>,
) -> Result<(), String> {
    speech::speak(app, text, lang, preview, on_event)
}

#[tauri::command]
pub async fn list_voices(
    app: AppHandle,
    engine: String,
    lang: String,
) -> Result<speech::VoiceList, String> {
    speech::list_voices(&app, &engine, &lang).await
}

#[tauri::command]
pub fn list_azure_regions() -> Vec<speech::VoiceOption> {
    speech::azure::REGIONS
        .iter()
        .map(|(id, label)| speech::VoiceOption {
            id: id.to_string(),
            label: label.to_string(),
        })
        .collect()
}

#[tauri::command]
pub fn stop_speaking(state: State<'_, AppState>) {
    state.speech.cancel();
}

#[tauri::command]
pub fn speech_offline_status(app: AppHandle) -> speech::local::OfflineStatus {
    speech::local::status(&app)
}

#[tauri::command]
pub async fn speech_offline_install(app: AppHandle) -> Result<(), String> {
    speech::local::install(app).await
}

#[tauri::command]
pub fn speech_offline_remove(app: AppHandle) -> Result<(), String> {
    speech::local::remove(&app)
}

// ── Selection bubble ───────────────────────────────────────────────────────

/// "translate" | "rewrite"
#[tauri::command]
pub fn bubble_action(app: AppHandle, action: String) {
    selection_bubble::act(&app, &action);
}

#[tauri::command]
pub fn bubble_hover(app: AppHandle, inside: bool) {
    selection_bubble::hover(&app, inside);
}

// ── Debug log ──────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_log(state: State<'_, AppState>) -> String {
    state.logger.read()
}

#[tauri::command]
pub fn clear_log(state: State<'_, AppState>) {
    state.logger.clear();
    state.logger.info("log cleared from Settings");
}

#[tauri::command]
pub fn reveal_log(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = state.logger.path().to_path_buf();
    if !path.exists() {
        // Nothing has been logged yet; make the file so the reveal has a target.
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, "");
    }

    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_accessibility_settings(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;
        return app
            .opener()
            .open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                None::<&str>,
            )
            .map_err(|e| e.to_string());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}

// ── Shared helper ──────────────────────────────────────────────────────────

pub fn apply_autostart(app: &AppHandle, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;

    let manager = app.autolaunch();

    // Only touch the registry / launch agent when the state actually differs.
    // Calling disable() on an already-disabled entry reports a spurious
    // "the system cannot find the file specified".
    if manager.is_enabled().unwrap_or(false) == enabled {
        return;
    }

    let outcome = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };

    if let Err(e) = outcome {
        if let Some(state) = app.try_state::<AppState>() {
            state
                .logger
                .warn(format!("could not update launch-on-login: {e}"));
        }
    }
}
