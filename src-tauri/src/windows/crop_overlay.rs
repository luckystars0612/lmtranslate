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

use std::collections::HashMap;
use std::time::Duration;

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::platform::common;
use crate::state::AppState;

pub const LABEL_PREFIX: &str = "crop-";
pub const DISMISS_EVENT: &str = "crop:dismiss";

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

pub fn open_all(app: &AppHandle) -> tauri::Result<usize> {
    let monitors = app.available_monitors()?;
    let frozen: Vec<String> = app
        .try_state::<AppState>()
        .and_then(|state| state.frozen_frames.lock().ok().map(|f| f.keys().cloned().collect()))
        .unwrap_or_default();
    let mut opened = 0;

    for (index, monitor) in monitors.iter().enumerate() {
        let label = label_for(index);
        if app.get_webview_window(&label).is_some() {
            continue;
        }

        let window = WebviewWindowBuilder::new(
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
        .build()?;

        // Physical placement: the builder's inner_size/position are logical, which
        // would be wrong the moment two monitors have different scale factors.
        let position = *monitor.position();
        let size = *monitor.size();
        window.set_position(PhysicalPosition::new(position.x, position.y))?;
        window.set_size(PhysicalSize::new(size.width, size.height))?;

        // A frozen overlay shows itself once the frame is painted — see `reveal`.
        if !frozen.contains(&label) {
            window.show()?;
            // Only the first overlay takes focus, so Esc has somewhere to land.
            if opened == 0 {
                let _ = window.set_focus();
            }
        }
        opened += 1;
    }

    if !frozen.is_empty() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(READY_FALLBACK).await;
            for window in app.webview_windows().values() {
                if is_overlay(window.label()) && !window.is_visible().unwrap_or(true) {
                    reveal(&app, window);
                }
            }
        });
    }

    Ok(opened)
}

/// Hands an overlay its frozen frame as PNG bytes; empty when it has none.
pub fn take_frame_png(app: &AppHandle, label: &str) -> Vec<u8> {
    app.try_state::<AppState>()
        .and_then(|state| {
            let mut frames = state.frozen_frames.lock().ok()?;
            Some(std::mem::take(&mut frames.get_mut(label)?.png))
        })
        .unwrap_or_default()
}

/// Puts a frozen overlay on screen. The one under the cursor takes focus, since that is
/// where the user is about to drag and where Esc should land.
pub fn reveal(app: &AppHandle, window: &WebviewWindow) {
    let _ = window.show();

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

pub fn close_all(app: &AppHandle) {
    // Tell every overlay to stop reporting before tearing them down, so a second
    // pointerup cannot fire a duplicate capture.
    let _ = app.emit(DISMISS_EVENT, ());

    for window in app.webview_windows().values() {
        if window.label().starts_with(LABEL_PREFIX) {
            let _ = window.close();
        }
    }
}

pub fn is_overlay(label: &str) -> bool {
    label.starts_with(LABEL_PREFIX)
}

fn label_for(index: usize) -> String {
    format!("{LABEL_PREFIX}{index}")
}
