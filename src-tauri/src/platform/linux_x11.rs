use std::time::Duration;

use enigo::Key;

use super::common;
use super::{CaptureError, PlatformCapabilities, PlatformCapture};

/// How long to wait for the PRIMARY owner to answer before giving up and falling
/// back to the simulate-copy path.
const PRIMARY_TIMEOUT: Duration = Duration::from_millis(250);

pub struct LinuxX11Capture;

impl PlatformCapture for LinuxX11Capture {
    fn grab_selected_text(&self) -> Result<String, CaptureError> {
        // Fast path unique to X11: highlighting text already fills PRIMARY, so there is
        // nothing to simulate and no timing delay to absorb. Falls back to the shared
        // Ctrl+C path when PRIMARY is empty (some apps do not export it).
        match read_primary_selection() {
            Ok(text) if !text.trim().is_empty() => return Ok(text),
            _ => {}
        }
        common::simulate_copy_and_read(Key::Control)
    }

    fn capture_region(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, CaptureError> {
        common::capture_region_xcap(x, y, width, height)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            os: "linux".into(),
            session: "x11".into(),
            backend: "Linux/X11 — PRIMARY selection with Ctrl+C fallback, XCB capture".into(),
            supports_text_selection: true,
            supports_screen_capture: true,
            notes: vec![
                "Highlighted text is read straight from the X11 PRIMARY selection, so no \
                 copy is simulated and the clipboard is left untouched."
                    .into(),
                "If an application does not export PRIMARY, the app falls back to a \
                 simulated Ctrl+C and restores your clipboard afterwards."
                    .into(),
            ],
            needs_accessibility_prompt: false,
        }
    }
}

fn read_primary_selection() -> Result<String, CaptureError> {
    let clipboard = x11_clipboard::Clipboard::new()
        .map_err(|e| CaptureError::Backend(format!("could not open the X11 display: {e}")))?;

    let atoms = &clipboard.getter.atoms;
    let bytes = clipboard
        .load(
            atoms.primary,
            atoms.utf8_string,
            atoms.property,
            PRIMARY_TIMEOUT,
        )
        .map_err(|e| CaptureError::Backend(format!("PRIMARY selection unavailable: {e}")))?;

    String::from_utf8(bytes)
        .map_err(|e| CaptureError::Backend(format!("PRIMARY selection was not UTF-8: {e}")))
}
