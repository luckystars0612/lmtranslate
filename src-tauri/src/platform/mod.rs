//! Platform backends.
//!
//! "Grab the current selection" and "capture a screen region" genuinely need different
//! code on Windows, macOS, X11 and Wayland. Hiding that behind one trait keeps
//! `capture/` and everything above it platform-agnostic.

pub mod common;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux_x11;

#[cfg(target_os = "linux")]
pub mod linux_wayland;

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("nothing was selected")]
    EmptySelection,
    /// The session genuinely cannot do this — surfaced to the user verbatim rather
    /// than failing silently. This is the Wayland path's normal outcome on some
    /// compositors.
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Backend(String),
}

/// What this particular session can actually do, shown in Settings -> Advanced.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub os: String,
    pub session: String,
    /// Human-readable one-liner, e.g. "Windows (SendInput + clipboard, xcap capture)".
    pub backend: String,
    pub supports_text_selection: bool,
    pub supports_screen_capture: bool,
    /// Whether the session allows pasting a translation back over the selection.
    /// False on Wayland, which forbids synthetic input entirely.
    pub supports_replace: bool,
    pub notes: Vec<String>,
    /// macOS only: enigo needs the Accessibility permission before it can send keys.
    pub needs_accessibility_prompt: bool,
}

pub trait PlatformCapture: Send + Sync {
    fn grab_selected_text(&self) -> Result<String, CaptureError>;

    /// `x`/`y` are global, physical screen coordinates. Returns PNG bytes.
    fn capture_region(&self, x: i32, y: i32, width: u32, height: u32)
        -> Result<Vec<u8>, CaptureError>;

    /// Pastes `text` over whatever is currently selected, replacing it in place.
    fn replace_selection(&self, text: &str) -> Result<(), CaptureError>;

    fn capabilities(&self) -> PlatformCapabilities;
}

/// Chosen once at startup and stored in `AppState`.
pub fn detect() -> Box<dyn PlatformCapture> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsCapture)
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacosCapture)
    }

    #[cfg(target_os = "linux")]
    {
        if is_wayland() {
            Box::new(linux_wayland::LinuxWaylandCapture)
        } else {
            Box::new(linux_x11::LinuxX11Capture)
        }
    }
}

#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    match std::env::var("XDG_SESSION_TYPE") {
        Ok(kind) if kind.eq_ignore_ascii_case("wayland") => true,
        Ok(kind) if kind.eq_ignore_ascii_case("x11") => false,
        // XDG_SESSION_TYPE is not always set; the socket variable is the tiebreaker.
        _ => std::env::var_os("WAYLAND_DISPLAY").is_some(),
    }
}
