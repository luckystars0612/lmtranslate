//! Wayland backend.
//!
//! This is a genuinely different implementation, not a variant of the X11 one. The
//! compositor sandboxes both input injection and screen capture, so:
//!
//! * There is no synthetic Ctrl+C. Instead we read the **primary selection**
//!   (`zwp_primary_selection_v1`), which highlighting already fills on GNOME and KDE
//!   and which needs no permission prompt. If the compositor does not expose it, the
//!   flow reports "not supported on this Wayland session" rather than hanging.
//! * Screen capture goes through the `xdg-desktop-portal` Screenshot portal, which
//!   shows the user the compositor's own consent dialog.

use std::io::Read;
use std::path::PathBuf;

use super::common;
use super::{CaptureError, PlatformCapabilities, PlatformCapture};

pub struct LinuxWaylandCapture;

impl PlatformCapture for LinuxWaylandCapture {
    fn grab_selected_text(&self) -> Result<String, CaptureError> {
        use wl_clipboard_rs::paste::{
            get_contents, ClipboardType, Error as PasteError, MimeType, Seat,
        };

        match get_contents(ClipboardType::Primary, Seat::Unspecified, MimeType::Text) {
            Ok((mut pipe, _mime)) => {
                let mut buffer = Vec::new();
                pipe.read_to_end(&mut buffer).map_err(|e| {
                    CaptureError::Backend(format!("could not read the selection: {e}"))
                })?;
                let text = String::from_utf8_lossy(&buffer).into_owned();
                if text.trim().is_empty() {
                    Err(CaptureError::EmptySelection)
                } else {
                    Ok(text)
                }
            }
            Err(PasteError::ClipboardEmpty) | Err(PasteError::NoMimeType) => {
                Err(CaptureError::EmptySelection)
            }
            Err(PasteError::PrimarySelectionUnsupported) => Err(CaptureError::Unsupported(
                "This Wayland compositor does not expose the primary selection, so \
                 LM Translate cannot read highlighted text here. Use the screenshot \
                 hotkey instead, or log into an X11 session."
                    .into(),
            )),
            Err(PasteError::MissingProtocol { name, .. }) => Err(CaptureError::Unsupported(
                format!(
                    "This Wayland compositor is missing the {name} protocol, so \
                     LM Translate cannot read highlighted text here. Use the screenshot \
                     hotkey instead."
                ),
            )),
            Err(e) => Err(CaptureError::Backend(format!(
                "could not read the Wayland selection: {e}"
            ))),
        }
    }

    fn capture_region(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, CaptureError> {
        if width == 0 || height == 0 {
            return Err(CaptureError::EmptySelection);
        }

        // The portal hands back a full-desktop screenshot; we crop it ourselves.
        let path = tauri::async_runtime::block_on(request_portal_screenshot())?;

        let full = image::open(&path)
            .map_err(|e| {
                CaptureError::Backend(format!("could not read the portal screenshot: {e}"))
            })?
            .to_rgba8();

        // Best effort cleanup: the portal writes into a cache dir it owns.
        let _ = std::fs::remove_file(&path);

        let crop_x = x.max(0) as u32;
        let crop_y = y.max(0) as u32;
        let crop_w = width.min(full.width().saturating_sub(crop_x));
        let crop_h = height.min(full.height().saturating_sub(crop_y));

        if crop_w == 0 || crop_h == 0 {
            return Err(CaptureError::EmptySelection);
        }

        let cropped = image::imageops::crop_imm(&full, crop_x, crop_y, crop_w, crop_h).to_image();
        common::encode_png(common::downscale(cropped))
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            os: "linux".into(),
            session: "wayland".into(),
            backend: "Linux/Wayland — primary selection, xdg-desktop-portal capture".into(),
            supports_text_selection: true,
            supports_screen_capture: true,
            notes: vec![
                "Wayland forbids synthetic keystrokes, so highlighted text is read from \
                 the primary selection instead. Compositors that do not implement it \
                 will report that select-and-translate is unavailable."
                    .into(),
                "Screen capture goes through xdg-desktop-portal, so your compositor may \
                 ask for permission the first time."
                    .into(),
                "Your clipboard is never modified on Wayland.".into(),
            ],
            needs_accessibility_prompt: false,
        }
    }
}

async fn request_portal_screenshot() -> Result<PathBuf, CaptureError> {
    use ashpd::desktop::screenshot::Screenshot;

    let response = Screenshot::request()
        .interactive(false)
        .send()
        .await
        .map_err(|e| {
            CaptureError::Unsupported(format!(
                "the screenshot portal is unavailable or refused the request: {e}"
            ))
        })?
        .response()
        .map_err(|e| {
            CaptureError::Unsupported(format!("the screenshot portal returned no image: {e}"))
        })?;

    let uri = response.uri().to_string();
    file_uri_to_path(&uri)
        .ok_or_else(|| CaptureError::Backend(format!("portal returned a non-file URI: {uri}")))
}

/// Minimal `file://` -> path conversion, so we do not pull in a URL crate for one call.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Drop an empty authority ("file:///tmp/x" -> "/tmp/x").
    let raw = rest.strip_prefix('/').map(|r| format!("/{r}")).unwrap_or_else(|| rest.to_string());
    Some(PathBuf::from(percent_decode(&raw)))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
