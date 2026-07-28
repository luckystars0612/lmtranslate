//! `lmtranslate --self-test` — the parts of the milestone acceptance tests that can
//! run without an API key, a mouse or a real text selection.
//!
//! 1. Drives the popup through loading, result and error at all four corners of the
//!    work area (Milestone 3), logging the rectangle it actually landed on so the
//!    clamp-and-flip logic can be checked from the log alone.
//! 2. Probes the screen-capture backend (Milestone 5) with a small fixed region.
//! 3. Probes the text-selection backend (Milestone 4), which answers the real
//!    question on a new machine: does this session allow what the flow needs?
//!
//! Everything here goes through the same code paths the hotkeys use.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::capture;
use crate::state::AppState;
use crate::windows::popup_window::{self, Anchor, PopupState};

const LOADING_MS: u64 = 1100;
const RESULT_MS: u64 = 1600;
const ERROR_MS: u64 = 1400;

pub fn run(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let settings = state.settings_snapshot();
        drop(state);

        let anchors = corner_anchors(&app);

        for (name, anchor) in anchors {
            log(&app, &format!("self-test: anchor {name} at {},{}", anchor.x, anchor.y));

            popup_window::show(&app, anchor, PopupState::loading(&settings, "text"));
            tokio::time::sleep(Duration::from_millis(LOADING_MS)).await;
            report(&app, &format!("{name}/loading"));

            popup_window::update(
                &app,
                PopupState::result(
                    &settings,
                    "text",
                    "Guten Morgen, wie geht es Ihnen heute?".into(),
                    "Chào buổi sáng, hôm nay bạn thế nào?".into(),
                ),
            );
            tokio::time::sleep(Duration::from_millis(RESULT_MS)).await;
            report(&app, &format!("{name}/result"));

            popup_window::update(
                &app,
                PopupState::error(
                    &settings,
                    "text",
                    "Invalid API key",
                    "The endpoint rejected the key that is saved for this provider.",
                    "Check it in Settings -> Provider and use \"Test connection\".",
                ),
            );
            tokio::time::sleep(Duration::from_millis(ERROR_MS)).await;
            report(&app, &format!("{name}/error"));
        }

        popup_window::close(&app);

        probe_backends(&app).await;
        log(&app, "self-test: finished");
    });
}

/// Exercises the two `PlatformCapture` methods directly and reports what the session
/// actually permits.
async fn probe_backends(app: &AppHandle) {
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let state = handle.state::<AppState>();
        let platform = state.platform.as_ref();

        // Logged before each probe, not just after, so a hard crash inside a backend
        // still leaves a trail pointing at which one it was.
        state.logger.info("self-test: probing screen capture...");
        let capture = capture::screenshot::capture_region(platform, 0, 0, 240, 120)
            .map(|base64| format!("ok, {} base64 bytes", base64.len()))
            .unwrap_or_else(|e| format!("FAILED — {e}"));
        state
            .logger
            .info(format!("self-test: screen capture — {capture}"));

        // Sends a real Ctrl+C to whatever is focused, then puts the clipboard back.
        state.logger.info("self-test: probing text selection...");
        let selection = match capture::text_selection::grab_selected_text(platform) {
            Ok(text) => format!("ok, {} chars captured", text.chars().count()),
            Err(e) => format!("{e}"),
        };
        state
            .logger
            .info(format!("self-test: text selection — {selection}"));
    })
    .await;

    if let Err(e) = outcome {
        log(app, &format!("self-test: backend probe panicked — {e}"));
    }
}

/// Anchors just inside each corner, so three of the four force a flip.
fn corner_anchors(app: &AppHandle) -> Vec<(&'static str, Anchor)> {
    let Some(monitor) = app.primary_monitor().ok().flatten() else {
        return vec![("centre", Anchor { x: 200.0, y: 200.0 })];
    };

    let area = monitor.work_area();
    let left = area.position.x as f64;
    let top = area.position.y as f64;
    let right = left + area.size.width as f64;
    let bottom = top + area.size.height as f64;
    let inset = 40.0;

    log(
        app,
        &format!(
            "self-test: work area {left},{top} {}x{}",
            area.size.width, area.size.height
        ),
    );

    vec![
        ("top-left", Anchor { x: left + inset, y: top + inset }),
        ("top-right", Anchor { x: right - inset, y: top + inset }),
        ("bottom-right", Anchor { x: right - inset, y: bottom - inset }),
        ("bottom-left", Anchor { x: left + inset, y: bottom - inset }),
    ]
}

/// Logs where the popup actually ended up, so clamping can be verified numerically.
fn report(app: &AppHandle, step: &str) {
    let Some(window) = app.get_webview_window(popup_window::LABEL) else {
        log(app, &format!("self-test: {step} — NO POPUP WINDOW"));
        return;
    };
    match (window.outer_position(), window.outer_size()) {
        (Ok(position), Ok(size)) => log(
            app,
            &format!(
                "self-test: {step} rect={},{} {}x{}",
                position.x, position.y, size.width, size.height
            ),
        ),
        _ => log(app, &format!("self-test: {step} — geometry unavailable")),
    }
}

fn log(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.info(message);
    }
}
