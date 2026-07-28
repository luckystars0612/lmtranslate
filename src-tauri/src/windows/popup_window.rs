//! The one and only result window: loading, result and error states all live here.
//!
//! Two things matter more than they look:
//!
//! * It is built **unfocused**. Flow 1 sends a synthetic Ctrl+C to whatever is in the
//!   foreground; if the popup stole focus first, the copy would go to the popup.
//! * The latest state is kept in `AppState`, and the webview pulls it on load. That
//!   removes the race between "emit the state" and "the webview finished booting".

use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::config::{Settings, COMMON_LANGUAGES};
use crate::state::AppState;

pub const LABEL: &str = "popup";
pub const EVENT: &str = "popup:state";

/// 420px card + the 10px shadow gutter the popup stylesheet reserves on each side.
const WIDTH: f64 = 440.0;
const INITIAL_HEIGHT: f64 = 128.0;
/// Distance from the anchor point, in logical pixels.
const GAP: f64 = 18.0;
/// Keeps the popup off the very edge of the work area.
const MARGIN: f64 = 8.0;

/// A point in global, physical screen coordinates that the popup should sit next to.
#[derive(Clone, Copy, Debug)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PopupState {
    /// "loading" | "result" | "error"
    pub kind: String,
    pub target_lang: String,
    /// "text" | "image"
    pub source_kind: String,
    pub original: String,
    pub translated: String,
    pub error_title: String,
    pub error_detail: String,
    pub error_hint: String,
    pub can_retranslate: bool,
    /// True for errors the user can actually act on, which puts an "Open Settings"
    /// button on the popup instead of leaving them to find it in the tray.
    pub offer_settings: bool,
    pub theme: String,
    pub languages: Vec<String>,
}

impl Default for PopupState {
    fn default() -> Self {
        Self {
            kind: "loading".into(),
            target_lang: crate::config::DEFAULT_TARGET_LANG.into(),
            source_kind: "text".into(),
            original: String::new(),
            translated: String::new(),
            error_title: String::new(),
            error_detail: String::new(),
            error_hint: String::new(),
            can_retranslate: false,
            offer_settings: false,
            theme: "system".into(),
            languages: COMMON_LANGUAGES.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl PopupState {
    fn base(settings: &Settings, source_kind: &str) -> Self {
        let mut languages: Vec<String> = COMMON_LANGUAGES.iter().map(|s| s.to_string()).collect();
        // A language typed by hand in Settings still has to appear in the dropdown.
        if !languages.iter().any(|l| l == &settings.target_lang) {
            languages.insert(0, settings.target_lang.clone());
        }

        Self {
            target_lang: settings.target_lang.clone(),
            source_kind: source_kind.to_string(),
            theme: settings.theme.clone(),
            languages,
            ..Default::default()
        }
    }

    pub fn loading(settings: &Settings, source_kind: &str) -> Self {
        Self {
            kind: "loading".into(),
            ..Self::base(settings, source_kind)
        }
    }

    /// Loading, but with the captured source text already on screen.
    pub fn loading_with_original(settings: &Settings, source_kind: &str, original: String) -> Self {
        Self {
            kind: "loading".into(),
            original,
            ..Self::base(settings, source_kind)
        }
    }

    pub fn result(
        settings: &Settings,
        source_kind: &str,
        original: String,
        translated: String,
    ) -> Self {
        Self {
            kind: "result".into(),
            original,
            translated,
            can_retranslate: true,
            ..Self::base(settings, source_kind)
        }
    }

    pub fn error(
        settings: &Settings,
        source_kind: &str,
        title: impl Into<String>,
        detail: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Self {
            kind: "error".into(),
            error_title: title.into(),
            error_detail: detail.into(),
            error_hint: hint.into(),
            ..Self::base(settings, source_kind)
        }
    }

    /// Marks an error state as fixable in Settings.
    pub fn offering_settings(mut self) -> Self {
        self.offer_settings = true;
        self
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Opens (or re-anchors) the popup and puts `state` on screen.
pub fn show(app: &AppHandle, anchor: Anchor, state: PopupState) {
    store(app, &state);
    if let Some(app_state) = app.try_state::<AppState>() {
        if let Ok(mut slot) = app_state.popup_anchor.lock() {
            *slot = Some(anchor);
        }
        app_state.reset_popup_flags();
    }

    on_main(app, move |app| {
        match app.get_webview_window(LABEL) {
            Some(window) => {
                let _ = reposition(&app, &window);
                let _ = window.show();
            }
            None => {
                if let Err(e) = build(&app) {
                    eprintln!("could not open the popup window: {e}");
                }
            }
        }
        emit_current(&app);
    });
}

/// Updates an already-open popup in place.
pub fn update(app: &AppHandle, state: PopupState) {
    store(app, &state);
    let app = app.clone();
    on_main(&app, |app| emit_current(&app));
}

pub fn close(app: &AppHandle) {
    on_main(app, |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.close();
        }
    });
}

/// Closes the popup and waits for it to actually go away.
///
/// Flow 1 calls this before capturing: if a previous popup still holds focus, the
/// synthetic Ctrl+C would be delivered to us instead of to the user's app.
pub fn close_and_settle(app: &AppHandle) {
    let existed = app.get_webview_window(LABEL).is_some();
    if !existed {
        return;
    }

    on_main_blocking(app, |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.close();
        }
    });

    // Give the compositor a moment to hand focus back to the app underneath.
    std::thread::sleep(Duration::from_millis(70));
}

/// Called once the capture step is done, so Esc and the copy button work.
pub fn focus(app: &AppHandle) {
    on_main(app, |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.set_focus();
        }
    });
}

/// The webview measured its card; snap the window to it and re-clamp on screen.
pub fn resize_to(app: &AppHandle, height: f64) {
    let height = height.clamp(80.0, 620.0);
    on_main(app, move |app| {
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.set_size(LogicalSize::new(WIDTH, height));
            let _ = reposition(&app, &window);
        }
    });
}

pub fn snapshot(app: &AppHandle) -> PopupState {
    app.try_state::<AppState>()
        .and_then(|s| s.popup.lock().ok().map(|g| g.clone()))
        .unwrap_or_default()
}

// ── Internals ──────────────────────────────────────────────────────────────

fn store(app: &AppHandle, state: &PopupState) {
    if let Some(app_state) = app.try_state::<AppState>() {
        if let Ok(mut slot) = app_state.popup.lock() {
            *slot = state.clone();
        }
    }
}

fn emit_current(app: &AppHandle) {
    let _ = app.emit_to(LABEL, EVENT, snapshot(app));
}

fn on_main<F>(app: &AppHandle, task: F)
where
    F: FnOnce(AppHandle) + Send + 'static,
{
    let handle = app.clone();
    if let Err(e) = app.run_on_main_thread(move || task(handle)) {
        eprintln!("main-thread dispatch failed: {e}");
    }
}

/// Only safe to call from a non-main thread.
fn on_main_blocking<F>(app: &AppHandle, task: F)
where
    F: FnOnce(AppHandle) + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    let dispatched = app.run_on_main_thread(move || {
        task(handle);
        let _ = tx.send(());
    });
    if dispatched.is_ok() {
        let _ = rx.recv_timeout(Duration::from_millis(1500));
    }
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("popup.html".into()))
        .title("Translation")
        .inner_size(WIDTH, INITIAL_HEIGHT)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        // Never steal focus on open — see the module comment.
        .focused(false)
        // Built hidden so it is never painted at 0,0 before being anchored.
        .visible(false)
        .build()?;

    reposition(app, &window)?;
    window.show()?;

    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(focused) = event {
            let Some(state) = handle.try_state::<AppState>() else {
                return;
            };
            // Only close on focus loss once the popup has actually held focus,
            // otherwise the "built unfocused" state would close it immediately.
            if state.note_popup_focus(*focused) {
                close(&handle);
            }
        }
    });

    Ok(())
}

/// Places the popup next to its anchor, flipping and clamping so it always lands
/// fully inside the work area of whichever monitor the anchor is on.
fn reposition(app: &AppHandle, window: &WebviewWindow) -> tauri::Result<()> {
    let anchor = app
        .try_state::<AppState>()
        .and_then(|s| s.popup_anchor.lock().ok().and_then(|g| *g));
    let Some(anchor) = anchor else {
        return Ok(());
    };

    let monitor = app
        .monitor_from_point(anchor.x, anchor.y)?
        .or(app.primary_monitor()?);
    let Some(monitor) = monitor else {
        return Ok(());
    };

    let scale = monitor.scale_factor();
    // work_area excludes the taskbar / dock / panels.
    let area = monitor.work_area();
    let area_x = area.position.x as f64;
    let area_y = area.position.y as f64;
    let area_w = area.size.width as f64;
    let area_h = area.size.height as f64;

    let size = window.outer_size()?;
    let win_w = size.width as f64;
    let win_h = size.height as f64;

    let gap = GAP * scale;
    let margin = MARGIN * scale;

    let min_x = area_x + margin;
    let min_y = area_y + margin;
    let max_x = (area_x + area_w - win_w - margin).max(min_x);
    let max_y = (area_y + area_h - win_h - margin).max(min_y);

    // Prefer below-right of the anchor; flip to the other side when it would overflow.
    let mut x = anchor.x + gap;
    if x > max_x {
        x = anchor.x - gap - win_w;
    }
    let mut y = anchor.y + gap;
    if y > max_y {
        y = anchor.y - gap - win_h;
    }

    window.set_position(PhysicalPosition::new(
        x.clamp(min_x, max_x).round() as i32,
        y.clamp(min_y, max_y).round() as i32,
    ))
}

/// Where the popup should appear for Flow 1: at the mouse cursor.
pub fn cursor_anchor(app: &AppHandle) -> Anchor {
    if let Ok(position) = app.cursor_position() {
        return Anchor {
            x: position.x,
            y: position.y,
        };
    }
    // No cursor (rare, but possible on some Wayland setups): fall back to the
    // top-left of the primary work area.
    let fallback = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let area = m.work_area();
            (area.position.x as f64, area.position.y as f64)
        })
        .unwrap_or((80.0, 80.0));

    Anchor {
        x: fallback.0 + 80.0,
        y: fallback.1 + 80.0,
    }
}
