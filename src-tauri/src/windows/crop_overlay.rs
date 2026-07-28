//! Fullscreen transparent crop overlays — one per monitor, so a drag works on any
//! screen. The overlay reports the drag rectangle in its own CSS pixels and Rust
//! converts to global physical coordinates using the window's real geometry, which
//! keeps mixed-DPI setups correct without the webview knowing anything about scaling.

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder,
};

pub const LABEL_PREFIX: &str = "crop-";
pub const DISMISS_EVENT: &str = "crop:dismiss";

pub fn open_all(app: &AppHandle) -> tauri::Result<usize> {
    let monitors = app.available_monitors()?;
    let mut opened = 0;

    for (index, monitor) in monitors.iter().enumerate() {
        let label = format!("{LABEL_PREFIX}{index}");
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
        window.show()?;

        // Only the first overlay takes focus, so Esc has somewhere to land.
        if opened == 0 {
            let _ = window.set_focus();
        }
        opened += 1;
    }

    Ok(opened)
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
