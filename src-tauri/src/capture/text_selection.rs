//! Flow 1's capture step: hand off to whichever `PlatformCapture` was chosen at
//! startup, then sanity-check what came back.

use crate::platform::{CaptureError, PlatformCapture};

/// Long selections are truncated rather than refused — the user gets a translation of
/// what fits instead of an error.
pub const MAX_INPUT_CHARS: usize = 8000;

pub fn grab_selected_text(platform: &dyn PlatformCapture) -> Result<String, CaptureError> {
    let raw = platform.grab_selected_text()?;
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return Err(CaptureError::EmptySelection);
    }

    Ok(trimmed.chars().take(MAX_INPUT_CHARS).collect())
}
