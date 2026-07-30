use enigo::Key;

use super::common;
use super::{CaptureError, PlatformCapabilities, PlatformCapture};

pub struct MacosCapture;

impl PlatformCapture for MacosCapture {
    fn grab_selected_text(&self) -> Result<String, CaptureError> {
        // Cmd+C rather than Ctrl+C.
        common::simulate_copy_and_read(Key::Meta)
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

    fn replace_selection(&self, text: &str) -> Result<(), CaptureError> {
        common::replace_selection(Key::Meta, text)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            os: "macos".into(),
            session: "quartz".into(),
            backend: "macOS — synthetic Cmd+C, CoreGraphics capture".into(),
            supports_text_selection: true,
            supports_screen_capture: true,
            supports_replace: true,
            notes: vec![
                "Sending Cmd+C needs the Accessibility permission; capture needs Screen \
                 Recording. Grant both in System Settings > Privacy & Security."
                    .into(),
                "Your clipboard is saved before the copy and restored right after.".into(),
            ],
            needs_accessibility_prompt: true,
        }
    }
}
