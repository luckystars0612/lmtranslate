//! The small bubble that appears next to a fresh mouse selection and offers to translate
//! it, for when a click is easier than reaching for the hotkey.
//!
//! It must never take focus: the click on it is followed by a synthetic Ctrl+C that has
//! to reach the application holding the selection. So the window is built unfocusable
//! (WS_EX_NOACTIVATE on Windows) and shown without activation, and
//! `flows::focus_selection_owner` hands focus back regardless if something activated
//! it anyway.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindowBuilder,
};

use crate::flows::{self, TextMode};
use crate::platform::PointerEvent;
use crate::state::AppState;

pub const LABEL: &str = "bubble";
pub const SHOW_EVENT: &str = "bubble:show";

/// The pill plus the 8px shadow gutter the stylesheet reserves around it, logical px.
const WIDTH: f64 = 188.0;
const HEIGHT: f64 = 50.0;
/// Offset from where the button was released, so the bubble does not sit under the
/// cursor and hide the end of the selection.
const OFFSET_X: f64 = 4.0;
const OFFSET_Y: f64 = 12.0;
/// How long an untouched bubble stays.
const LINGER: Duration = Duration::from_millis(4000);
/// How long it stays once the pointer has been over it and left.
const LINGER_AFTER_HOVER: Duration = Duration::from_millis(1200);

/// Where the bubble is while it is on screen, and a counter that cancels stale timers.
#[derive(Default)]
pub struct BubbleState {
    shown: Mutex<Option<Rect>>,
    generation: AtomicU64,
}

/// Global physical pixels.
#[derive(Clone, Copy, Debug)]
struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

/// Starts watching the mouse. A no-op on backends that cannot.
pub fn start(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let handle = app.clone();
    if state
        .platform
        .watch_pointer(Box::new(move |event| on_pointer(&handle, event)))
    {
        state.logger.info("watching the mouse for selections");
    }
}

/// Called on the watcher's thread for every press and every likely selection.
fn on_pointer(app: &AppHandle, event: PointerEvent) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    match event {
        PointerEvent::Pressed { x, y } => {
            let shown = state.bubble.shown.lock().ok().and_then(|slot| *slot);
            // A press on the bubble is the user using it, not dismissing it.
            if shown.is_some_and(|rect| !rect.contains(x, y)) {
                hide(app);
            }
        }
        PointerEvent::Selected { x, y, window } => {
            // Read per event, so the setting applies the moment it is saved.
            if !state.settings_snapshot().selection_bubble {
                return;
            }
            if let Ok(mut slot) = state.target_window.lock() {
                *slot = Some(window);
            }
            show(app, x, y);
        }
    }
}

/// Puts the bubble next to `x`/`y` (global, physical).
fn show(app: &AppHandle, x: i32, y: i32) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let generation = state.bubble.generation.fetch_add(1, Ordering::SeqCst) + 1;

    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(state) = handle.try_state::<AppState>() else {
            return;
        };
        // Hidden again (or superseded) before this got to run.
        if state.bubble.generation.load(Ordering::SeqCst) != generation {
            return;
        }

        let window = match handle.get_webview_window(LABEL) {
            Some(window) => window,
            None => match build(&handle) {
                Ok(window) => window,
                Err(e) => {
                    state
                        .logger
                        .warn(format!("could not open the selection bubble: {e}"));
                    return;
                }
            },
        };

        let Some(rect) = place(&handle, x, y) else {
            return;
        };
        let _ = window.set_position(PhysicalPosition::new(rect.x, rect.y));
        let _ = window.set_size(PhysicalSize::new(rect.width as u32, rect.height as u32));
        // Built unfocused, so tao shows it with SW_SHOWNOACTIVATE every time.
        let _ = window.show();
        let _ = window.emit_to(LABEL, SHOW_EVENT, ());

        if let Ok(mut slot) = state.bubble.shown.lock() {
            *slot = Some(rect);
        };
    });

    schedule_hide(app, generation, LINGER);
}

/// Takes the bubble off screen. Returns whether it was showing.
pub fn hide(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    let was_shown = state
        .bubble
        .shown
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .is_some();
    state.bubble.generation.fetch_add(1, Ordering::SeqCst);

    if was_shown {
        let _ = app.run_on_main_thread({
            let app = app.clone();
            move || {
                if let Some(window) = app.get_webview_window(LABEL) {
                    let _ = window.hide();
                }
            }
        });
    }
    was_shown
}

/// `hide`, then wait until the bubble is really gone from the screen. Only safe to call
/// from a non-main thread; the screenshot flow needs it so the bubble is not frozen
/// into the frame.
pub fn hide_and_settle(app: &AppHandle) {
    if !hide(app) {
        return;
    }
    // `hide` queued its work on the main thread; a no-op queued behind it returns once
    // that work is done.
    let (tx, rx) = mpsc::channel();
    if app.run_on_main_thread(move || {
        let _ = tx.send(());
    })
    .is_ok()
    {
        let _ = rx.recv_timeout(Duration::from_millis(500));
    }
    std::thread::sleep(Duration::from_millis(40));
}

/// The pointer entered (`true`) or left the bubble. Hovering keeps it; leaving starts
/// a shorter countdown.
pub fn hover(app: &AppHandle, inside: bool) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let generation = state.bubble.generation.fetch_add(1, Ordering::SeqCst) + 1;
    if !inside {
        schedule_hide(app, generation, LINGER_AFTER_HOVER);
    }
}

/// A button on the bubble was clicked.
pub fn act(app: &AppHandle, action: &str) {
    hide(app);
    match action {
        "translate" => flows::start_text_flow(app.clone(), TextMode::Popup),
        "rewrite" => flows::start_text_flow(app.clone(), TextMode::Replace),
        _ => {}
    }
}

fn schedule_hide(app: &AppHandle, generation: u64, delay: Duration) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(delay).await;
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        if state.bubble.generation.load(Ordering::SeqCst) == generation {
            hide(&app);
        }
    });
}

fn build(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("bubble.html".into()))
        .title("Translate selection")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        // Both matter — see the module comment.
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()
}

/// Below-right of the release point, flipped and clamped into the monitor's work area.
fn place(app: &AppHandle, x: i32, y: i32) -> Option<Rect> {
    let monitor = app
        .monitor_from_point(x as f64, y as f64)
        .ok()
        .flatten()
        .or(app.primary_monitor().ok().flatten())?;

    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let width = (WIDTH * scale).round() as i32;
    let height = (HEIGHT * scale).round() as i32;

    let min_x = area.position.x;
    let min_y = area.position.y;
    let max_x = (area.position.x + area.size.width as i32 - width).max(min_x);
    let max_y = (area.position.y + area.size.height as i32 - height).max(min_y);

    let mut left = x + (OFFSET_X * scale).round() as i32;
    if left > max_x {
        left = x - (OFFSET_X * scale).round() as i32 - width;
    }
    let mut top = y + (OFFSET_Y * scale).round() as i32;
    if top > max_y {
        top = y - (OFFSET_Y * scale).round() as i32 - height;
    }

    Some(Rect {
        x: left.clamp(min_x, max_x),
        y: top.clamp(min_y, max_y),
        width,
        height,
    })
}
