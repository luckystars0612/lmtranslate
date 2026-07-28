//! Flow 2's capture step: hand off to whichever `PlatformCapture` was chosen at
//! startup and base64-encode the PNG it produces.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::platform::{CaptureError, PlatformCapture};

/// Anything smaller than this is a stray click rather than a deliberate crop.
pub const MIN_REGION_PX: u32 = 6;

pub fn capture_region(
    platform: &dyn PlatformCapture,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<String, CaptureError> {
    if width < MIN_REGION_PX || height < MIN_REGION_PX {
        return Err(CaptureError::EmptySelection);
    }

    let png = platform.capture_region(x, y, width, height)?;
    Ok(STANDARD.encode(png))
}
