//! Fullscreen crop overlays — one per monitor, so a drag works on any screen. The
//! overlay reports the drag rectangle in its own CSS pixels and Rust converts to global
//! physical coordinates using the window's real geometry, which keeps mixed-DPI setups
//! correct without the webview knowing anything about scaling.
//!
//! Where the backend can, the screen is captured *before* the overlays appear and shown
//! inside them as a frozen frame, the way Win+Shift+S does it. Cropping live does not
//! survive video: Chrome stops painting a page it considers covered, so a YouTube tab
//! went blank the moment a fullscreen overlay went over it — and a subtitle would have
//! changed by the time the drag finished anyway.
//!
//! The overlay windows are built once — shortly after startup — and then hidden and
//! shown, never closed. Building a WebView2 window and loading its page costs hundreds
//! of milliseconds per monitor, and doing that on every press is what made Alt+S feel
//! slow once the overlay also had to wait for its frame before appearing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::platform::common;
use crate::state::AppState;

pub const LABEL_PREFIX: &str = "crop-";
pub const DISMISS_EVENT: &str = "crop:dismiss";
/// Tells an overlay a new crop has started: fetch the frame, then report ready.
pub const OPEN_EVENT: &str = "crop:open";

/// How long after startup the overlays are built, so they do not compete with it.
const PREWARM_DELAY: Duration = Duration::from_secs(3);

/// Bumped per Alt+S, so a timer or a late "ready" from an earlier crop cannot reveal an
/// overlay for the current one.
static SESSION: AtomicU64 = AtomicU64::new(0);
/// When the current crop was asked for, and the session that has already been logged.
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
static LOGGED_SESSION: AtomicU64 = AtomicU64::new(0);

/// A frozen overlay stays hidden until its webview has the frame on screen, so the
/// user never sees it half-built. This is the backstop if that signal never comes.
const READY_FALLBACK: Duration = Duration::from_millis(1200);

/// The screen under one overlay, captured before the overlay appeared.
pub struct FrozenFrame {
    /// The monitor the overlay covers, in global physical pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub image: image::RgbaImage,
    /// `image` as PNG for the overlay to display. Handed over once, then dropped.
    pub png: Vec<u8>,
}

/// Captures every monitor. Must run before any overlay is open, and off the main
/// thread. A monitor that cannot be captured is simply missing from the result, and
/// its overlay falls back to cropping live.
pub fn freeze_all(app: &AppHandle) -> HashMap<String, FrozenFrame> {
    let mut frames = HashMap::new();
    let Some(state) = app.try_state::<AppState>() else {
        return frames;
    };
    let Ok(monitors) = app.available_monitors() else {
        return frames;
    };

    for (index, monitor) in monitors.iter().enumerate() {
        let position = *monitor.position();
        let size = *monitor.size();

        let captured = state
            .platform
            .capture_frame(position.x, position.y, size.width, size.height)
            .and_then(|image| common::encode_png_fast(&image).map(|png| (image, png)));

        match captured {
            Ok((image, png)) => {
                frames.insert(
                    label_for(index),
                    FrozenFrame {
                        x: position.x,
                        y: position.y,
                        width: size.width,
                        height: size.height,
                        image,
                        png,
                    },
                );
            }
            Err(e) => state
                .logger
                .info(format!("monitor {index} will be cropped live: {e}")),
        }
    }
    frames
}

/// Builds the overlay windows ahead of the first Alt+S. Called once, after startup.
pub fn prewarm(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PREWARM_DELAY).await;
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(e) = ensure_windows(&handle) {
                if let Some(state) = handle.try_state::<AppState>() {
                    state.logger.warn(format!("could not prepare the crop overlay: {e}"));
                }
            }
        });
    });
}

/// Marks the start of a new crop: the moment Alt+S was handled.
pub fn begin_session() {
    SESSION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut started) = STARTED.lock() {
        *started = Some(Instant::now());
    }
}

/// One hidden overlay per monitor, placed over it. Reuses the windows already built and
/// closes any left over from a monitor that has since been unplugged.
fn ensure_windows(app: &AppHandle) -> tauri::Result<Vec<WebviewWindow>> {
    let monitors = app.available_monitors()?;
    let mut windows = Vec::with_capacity(monitors.len());

    for (index, monitor) in monitors.iter().enumerate() {
        let label = label_for(index);
        let window = match app.get_webview_window(&label) {
            Some(window) => window,
            None => WebviewWindowBuilder::new(
                app,
                &label,
                WebviewUrl::App("crop-overlay.html".into()),
            )
            .title("Select a region")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .visible(false)
            .build()?,
        };

        // Physical placement: the builder's inner_size/position are logical, which
        // would be wrong the moment two monitors have different scale factors. Redone
        // every time, since monitors can be rearranged between uses.
        let position = *monitor.position();
        let size = *monitor.size();
        window.set_position(PhysicalPosition::new(position.x, position.y))?;
        window.set_size(PhysicalSize::new(size.width, size.height))?;
        windows.push(window);
    }

    for (label, window) in app.webview_windows() {
        let stale = label
            .strip_prefix(LABEL_PREFIX)
            .and_then(|i| i.parse::<usize>().ok())
            .is_some_and(|i| i >= monitors.len());
        if stale {
            let _ = window.close();
        }
    }
    Ok(windows)
}

pub fn open_all(app: &AppHandle) -> tauri::Result<usize> {
    let windows = ensure_windows(app)?;
    let frozen: Vec<String> = app
        .try_state::<AppState>()
        .and_then(|state| state.frozen_frames.lock().ok().map(|f| f.keys().cloned().collect()))
        .unwrap_or_default();

    let mut focused = false;
    for window in &windows {
        // A frozen overlay shows itself once its frame is in — see `reveal`. A live one
        // shows straight away, and only the first takes focus so Esc has somewhere to go.
        if !frozen.iter().any(|label| label == window.label()) {
            window.show()?;
            if !focused {
                let _ = window.set_focus();
                focused = true;
            }
        }
        let _ = window.emit_to(window.label(), OPEN_EVENT, ());
    }

    if !frozen.is_empty() {
        let app = app.clone();
        let session = SESSION.load(Ordering::SeqCst);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(READY_FALLBACK).await;
            if SESSION.load(Ordering::SeqCst) != session {
                return;
            }
            for window in app.webview_windows().values() {
                if is_overlay(window.label()) && !window.is_visible().unwrap_or(true) {
                    reveal(&app, window);
                }
            }
        });
    }

    Ok(windows.len())
}

/// Whether a crop is in progress — what a freshly loaded overlay asks before fetching.
pub fn is_active(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|state| state.overlay_is_open())
        .unwrap_or(false)
}

/// An overlay's frozen frame as PNG bytes; empty when it has none. A copy rather than a
/// hand-over: a reused overlay can ask twice (its load and the open event can race).
pub fn frame_png(app: &AppHandle, label: &str) -> Vec<u8> {
    app.try_state::<AppState>()
        .and_then(|state| {
            let frames = state.frozen_frames.lock().ok()?;
            Some(frames.get(label)?.png.clone())
        })
        .unwrap_or_default()
}

/// Puts a frozen overlay on screen. The one under the cursor takes focus, since that is
/// where the user is about to drag and where Esc should land.
pub fn reveal(app: &AppHandle, window: &WebviewWindow) {
    // A late "ready" from a crop that has already been cancelled or committed.
    if !is_active(app) {
        return;
    }
    let _ = window.show();

    let session = SESSION.load(Ordering::SeqCst);
    if LOGGED_SESSION.swap(session, Ordering::SeqCst) != session {
        let started = STARTED.lock().ok().and_then(|s| *s);
        if let (Some(started), Some(state)) = (started, app.try_state::<AppState>()) {
            state.logger.info(format!(
                "crop overlay on screen {}ms after the hotkey",
                started.elapsed().as_millis()
            ));
        }
    }

    let under_cursor = match (app.cursor_position(), window.outer_position(), window.outer_size()) {
        (Ok(cursor), Ok(origin), Ok(size)) => {
            let (x, y) = (cursor.x as i32, cursor.y as i32);
            x >= origin.x
                && y >= origin.y
                && x < origin.x + size.width as i32
                && y < origin.y + size.height as i32
        }
        // Without a cursor position, any overlay is better than none having focus.
        _ => true,
    };
    if under_cursor {
        let _ = window.set_focus();
    }
}

/// Hides every overlay, keeping the windows for the next crop.
pub fn close_all(app: &AppHandle) {
    // Tell every overlay to stop reporting before hiding them, so a second pointerup
    // cannot fire a duplicate capture.
    let _ = app.emit(DISMISS_EVENT, ());

    for window in app.webview_windows().values() {
        if window.label().starts_with(LABEL_PREFIX) {
            let _ = window.hide();
        }
    }
}

pub fn is_overlay(label: &str) -> bool {
    label.starts_with(LABEL_PREFIX)
}

fn label_for(index: usize) -> String {
    format!("{LABEL_PREFIX}{index}")
}
