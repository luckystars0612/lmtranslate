//! Bits shared by the backends that behave alike: the simulate-copy dance used by
//! Windows, macOS and X11, and the `xcap` capture used by Windows and macOS.

use std::time::{Duration, Instant};

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings as EnigoSettings};
use image::ImageEncoder;

use super::CaptureError;

/// Parked on the clipboard before the synthetic copy so we can tell "the app copied
/// nothing" apart from "the app copied exactly what was already there".
/// Invisible separators keep it from ever showing up as real content.
const SENTINEL: &str = "\u{2063}lmtranslate\u{2063}";

/// The plan suggests a flat 80-120ms wait. Polling instead is strictly better: it
/// returns as soon as the target app has written the clipboard (usually well under
/// 80ms) and it can tell an empty selection apart from a slow one.
const COPY_TIMEOUT: Duration = Duration::from_millis(600);
const POLL_INTERVAL: Duration = Duration::from_millis(15);

/// Time given to the foreground app to process the modifier key-ups below before the
/// copy chord goes out.
const MODIFIER_SETTLE: Duration = Duration::from_millis(40);

/// Longest edge of the image sent to the model. Caps the base64 payload without
/// hurting OCR on realistic crops.
const MAX_IMAGE_EDGE: u32 = 1920;

/// Save the clipboard, send the copy chord, read what landed, put the clipboard back.
pub fn simulate_copy_and_read(modifier: Key) -> Result<String, CaptureError> {
    let mut clipboard = Clipboard::new()
        .map_err(|e| CaptureError::Backend(format!("the clipboard is unavailable: {e}")))?;

    let saved = clipboard.get_text().ok();

    clipboard
        .set_text(SENTINEL)
        .map_err(|e| CaptureError::Backend(format!("could not stage the clipboard: {e}")))?;

    let send = || -> Result<(), CaptureError> {
        let mut enigo = Enigo::new(&EnigoSettings::default()).map_err(|e| {
            CaptureError::Unsupported(format!("cannot send synthetic keystrokes here: {e}"))
        })?;

        // The hotkey that got us here is still physically held: a global shortcut fires
        // on key-*down*, and the default binding is Alt+D. Two things follow, and the
        // order below is load-bearing for both.
        //
        // 1. Alt must come up before the copy chord, or the foreground application sees
        //    Ctrl+Alt+C, which is not "copy" anywhere — nothing reaches the clipboard
        //    and the flow reports an empty selection.
        // 2. Alt must NOT come up on its own. A lone Alt keystroke is what opens the
        //    window's menu on Windows, and that moves keyboard focus off the control
        //    holding the selection — which breaks the copy just as thoroughly.
        //
        // Pressing the copy modifier first makes the Alt release non-lone, satisfying
        // both. Measured: with Alt held, chord-only and release-then-chord both copy
        // nothing; this ordering copies correctly.
        enigo
            .key(modifier, Direction::Press)
            .map_err(|e| CaptureError::Backend(e.to_string()))?;

        let stuck: &[Key] = if matches!(modifier, Key::Meta) {
            &[Key::Alt, Key::Shift, Key::Control]
        } else {
            &[Key::Alt, Key::Shift, Key::Meta]
        };
        for held in stuck {
            let _ = enigo.key(*held, Direction::Release);
        }
        std::thread::sleep(MODIFIER_SETTLE);
        let tapped = enigo.key(Key::Unicode('c'), Direction::Click);
        // Always let the modifier back up, even if the tap failed.
        let _ = enigo.key(modifier, Direction::Release);
        tapped.map_err(|e| CaptureError::Backend(e.to_string()))
    };

    if let Err(e) = send() {
        restore_clipboard(saved);
        return Err(e);
    }

    let deadline = Instant::now() + COPY_TIMEOUT;
    let mut copied: Option<String> = None;
    while Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        if let Ok(text) = clipboard.get_text() {
            if text != SENTINEL {
                copied = Some(text);
                break;
            }
        }
    }

    restore_clipboard(saved);

    match copied {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(CaptureError::EmptySelection),
    }
}

/// Never permanently clobber the user's clipboard.
fn restore_clipboard(saved: Option<String>) {
    #[cfg(not(target_os = "linux"))]
    {
        if let Ok(mut clipboard) = Clipboard::new() {
            match saved {
                Some(text) => {
                    let _ = clipboard.set_text(text);
                }
                None => {
                    let _ = clipboard.clear();
                }
            }
        }
    }

    // On X11/Wayland the clipboard is owner-based: whoever set it must stay alive to
    // serve it. `.wait()` blocks until another app takes ownership, so it gets its own
    // detached thread rather than stalling the capture.
    #[cfg(target_os = "linux")]
    {
        std::thread::spawn(move || {
            use arboard::SetExtLinux;
            if let Ok(mut clipboard) = Clipboard::new() {
                match saved {
                    Some(text) => {
                        let _ = clipboard.set().wait().text(text);
                    }
                    None => {
                        let _ = clipboard.clear();
                    }
                }
            }
        });
    }
}

/// Writes text to the clipboard — backs the popup's "Copy translation" button.
pub fn put_on_clipboard(text: String) -> Result<(), String> {
    #[cfg(not(target_os = "linux"))]
    {
        let mut clipboard =
            Clipboard::new().map_err(|e| format!("the clipboard is unavailable: {e}"))?;
        clipboard
            .set_text(text)
            .map_err(|e| format!("could not write to the clipboard: {e}"))
    }

    // Same ownership rule as `restore_clipboard`: the setter has to stay alive to
    // serve the selection, so it lives on its own thread.
    #[cfg(target_os = "linux")]
    {
        use arboard::SetExtLinux;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || match Clipboard::new() {
            Ok(mut clipboard) => {
                let _ = tx.send(Ok(()));
                let _ = clipboard.set().wait().text(text);
            }
            Err(e) => {
                let _ = tx.send(Err(format!("the clipboard is unavailable: {e}")));
            }
        });
        rx.recv_timeout(Duration::from_millis(1000))
            .unwrap_or_else(|_| Err("the clipboard did not respond".into()))
    }
}

/// Direct capture through `xcap`.
///
/// Windows and macOS only: see the Cargo.toml note on why Linux does not use xcap.
/// `linux_x11.rs` and `linux_wayland.rs` provide the equivalent for their sessions.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn capture_region_xcap(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, CaptureError> {
    if width == 0 || height == 0 {
        return Err(CaptureError::EmptySelection);
    }

    let centre_x = x + (width / 2) as i32;
    let centre_y = y + (height / 2) as i32;

    let monitor = xcap::Monitor::from_point(centre_x, centre_y).map_err(|e| {
        CaptureError::Backend(format!("no monitor contains {centre_x},{centre_y}: {e}"))
    })?;

    let backend = |e: xcap::XCapError| CaptureError::Backend(e.to_string());
    let monitor_x = monitor.x().map_err(backend)?;
    let monitor_y = monitor.y().map_err(backend)?;
    let monitor_w = monitor.width().map_err(backend)?;
    let monitor_h = monitor.height().map_err(backend)?;

    // xcap wants monitor-relative coordinates, and refuses regions that overhang.
    let rel_x = (x - monitor_x).max(0) as u32;
    let rel_y = (y - monitor_y).max(0) as u32;
    let rel_w = width.min(monitor_w.saturating_sub(rel_x));
    let rel_h = height.min(monitor_h.saturating_sub(rel_y));

    if rel_w == 0 || rel_h == 0 {
        return Err(CaptureError::EmptySelection);
    }

    let image = monitor
        .capture_region(rel_x, rel_y, rel_w, rel_h)
        .map_err(|e| CaptureError::Backend(format!("screen capture failed: {e}")))?;

    encode_png(downscale(image))
}

/// Keeps the base64 payload sane for large crops.
pub fn downscale(image: image::RgbaImage) -> image::RgbaImage {
    let (w, h) = (image.width(), image.height());
    let longest = w.max(h);
    if longest <= MAX_IMAGE_EDGE {
        return image;
    }
    let ratio = MAX_IMAGE_EDGE as f32 / longest as f32;
    let target_w = ((w as f32 * ratio).round() as u32).max(1);
    let target_h = ((h as f32 * ratio).round() as u32).max(1);
    image::imageops::resize(
        &image,
        target_w,
        target_h,
        image::imageops::FilterType::CatmullRom,
    )
}

pub fn encode_png(image: image::RgbaImage) -> Result<Vec<u8>, CaptureError> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| CaptureError::Backend(format!("could not encode the capture: {e}")))?;
    Ok(out)
}
