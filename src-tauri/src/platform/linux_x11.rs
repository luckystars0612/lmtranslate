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
        capture_region_x11(x, y, width, height)
    }

    fn replace_selection(&self, text: &str) -> Result<(), CaptureError> {
        common::replace_selection(Key::Control, text)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            os: "linux".into(),
            session: "x11".into(),
            backend: "Linux/X11 — PRIMARY selection with Ctrl+C fallback, X11 GetImage capture"
                .into(),
            supports_text_selection: true,
            supports_screen_capture: true,
            supports_replace: true,
            supports_selection_bubble: false,
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

/// Region capture via a plain `GetImage` on the root window.
///
/// Deliberately not `xcap`: on Linux that crate depends unconditionally on `pipewire`,
/// whose `-sys` crate binds against the system SPA headers, which ties the build to a
/// very recent PipeWire and pulls in a wlroots screencopy stack this app never uses —
/// the Wayland backend goes through xdg-desktop-portal instead. `x11rb` is pure Rust,
/// needs no system library, and `GetImage` is all X11 capture actually requires.
fn capture_region_x11(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CaptureError> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder};

    if width == 0 || height == 0 {
        return Err(CaptureError::EmptySelection);
    }

    let (conn, screen_index) = x11rb::connect(None)
        .map_err(|e| CaptureError::Backend(format!("could not reach the X server: {e}")))?;

    let setup = conn.setup();
    let screen = setup
        .roots
        .get(screen_index)
        .ok_or_else(|| CaptureError::Backend("the X server reported no screen".into()))?;

    // Every monitor shares one root window on X11, and its coordinate space *is* the
    // global screen space — so the physical coordinates arrive usable, with no
    // per-monitor translation of the kind the Windows and macOS backends need.
    let root_w = screen.width_in_pixels;
    let root_h = screen.height_in_pixels;

    let clamped_x = x.clamp(0, root_w as i32);
    let clamped_y = y.clamp(0, root_h as i32);
    let clamped_w = width.min((root_w as i32 - clamped_x).max(0) as u32);
    let clamped_h = height.min((root_h as i32 - clamped_y).max(0) as u32);

    if clamped_w == 0 || clamped_h == 0 {
        return Err(CaptureError::EmptySelection);
    }

    let reply = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            screen.root,
            clamped_x as i16,
            clamped_y as i16,
            clamped_w as u16,
            clamped_h as u16,
            !0,
        )
        .map_err(|e| CaptureError::Backend(format!("GetImage was refused: {e}")))?
        .reply()
        .map_err(|e| CaptureError::Backend(format!("GetImage failed: {e}")))?;

    // ZPixmap at depth 24 or 32 is four bytes per pixel. Anything else is a visual this
    // code has no business guessing at.
    if reply.depth != 24 && reply.depth != 32 {
        return Err(CaptureError::Unsupported(format!(
            "this X11 display uses colour depth {}, which LM Translate cannot read",
            reply.depth
        )));
    }

    let pixels = clamped_w as usize * clamped_h as usize;
    if reply.data.len() < pixels * 4 {
        return Err(CaptureError::Backend(format!(
            "the X server returned {} bytes for a {clamped_w}x{clamped_h} region, expected {}",
            reply.data.len(),
            pixels * 4
        )));
    }

    // Byte order is the server's, not the client's: LSBFirst gives B,G,R,X and MSBFirst
    // gives X,R,G,B. Alpha is forced opaque because X leaves the fourth byte undefined
    // at depth 24.
    let msb_first = setup.image_byte_order == ImageOrder::MSB_FIRST;
    let mut rgba = Vec::with_capacity(pixels * 4);
    for px in reply.data.chunks_exact(4).take(pixels) {
        if msb_first {
            rgba.extend_from_slice(&[px[1], px[2], px[3], 255]);
        } else {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
    }

    let image = image::RgbaImage::from_raw(clamped_w, clamped_h, rgba).ok_or_else(|| {
        CaptureError::Backend("could not assemble the captured image".into())
    })?;

    common::encode_png(common::downscale(image))
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
