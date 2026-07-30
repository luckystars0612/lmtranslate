use enigo::Key;

use super::common;
use super::{CaptureError, PlatformCapabilities, PlatformCapture};

pub struct WindowsCapture;

impl PlatformCapture for WindowsCapture {
    fn grab_selected_text(&self) -> Result<String, CaptureError> {
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

    fn replace_selection(&self, text: &str) -> Result<(), CaptureError> {
        common::replace_selection(Key::Control, text)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            os: "windows".into(),
            session: "win32".into(),
            backend: "Windows — synthetic Ctrl+C, Desktop Duplication capture".into(),
            supports_text_selection: true,
            supports_screen_capture: true,
            supports_replace: true,
            notes: vec![
                "Your clipboard is saved before the copy and restored right after.".into(),
            ],
            needs_accessibility_prompt: false,
        }
    }
}
