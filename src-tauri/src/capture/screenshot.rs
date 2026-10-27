//! Flow 2's capture step: hand off to whichever `PlatformCapture` was chosen at
//! startup and base64-encode the PNG it produces.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::platform::{common, CaptureError, PlatformCapture};
use crate::windows::crop_overlay::FrozenFrame;

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

/// The same crop, cut from a frame captured before the overlay went up. `x`/`y` are
/// global physical coordinates; the frame records which monitor it covers.
pub fn crop_frozen(
    frame: &FrozenFrame,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<String, CaptureError> {
    if width < MIN_REGION_PX || height < MIN_REGION_PX {
        return Err(CaptureError::EmptySelection);
    }

    // The capture is normally exactly the monitor's size; scaling covers a backend that
    // returns it at a different resolution.
    let scale_x = frame.image.width() as f64 / frame.width.max(1) as f64;
    let scale_y = frame.image.height() as f64 / frame.height.max(1) as f64;

    let left = (((x - frame.x) as f64 * scale_x).round().max(0.0) as u32).min(frame.image.width());
    let top = (((y - frame.y) as f64 * scale_y).round().max(0.0) as u32).min(frame.image.height());
    let crop_w = ((width as f64 * scale_x).round() as u32).min(frame.image.width() - left);
    let crop_h = ((height as f64 * scale_y).round() as u32).min(frame.image.height() - top);

    if crop_w == 0 || crop_h == 0 {
        return Err(CaptureError::EmptySelection);
    }

    let cropped = image::imageops::crop_imm(&frame.image, left, top, crop_w, crop_h).to_image();
    let png = common::encode_png(common::downscale(cropped))?;
    Ok(STANDARD.encode(png))
}
