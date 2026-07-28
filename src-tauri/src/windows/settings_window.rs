use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "settings";

/// Opens Settings, or brings it forward if it is already open.
pub fn open(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.unminimize();
        window.show()?;
        window.set_focus()?;
        return Ok(());
    }

    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("settings.html".into()))
        .title("LM Translate — Settings")
        .inner_size(680.0, 780.0)
        .min_inner_size(520.0, 480.0)
        .resizable(true)
        .build()?;

    Ok(())
}
