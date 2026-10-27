use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use enigo::Key;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetAncestor, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, IsWindow, SendMessageTimeoutW, SetForegroundWindow,
    SetWindowsHookExW, WindowFromPoint, GA_ROOT, HC_ACTION, HTCLIENT, LLMHF_INJECTED, MSG,
    MSLLHOOKSTRUCT, SMTO_ABORTIFHUNG, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_NCHITTEST, WM_RBUTTONDOWN,
};

use super::common;
use super::{CaptureError, PlatformCapabilities, PlatformCapture, PointerEvent, WindowHandle};

/// An unassigned virtual-key code. AutoHotkey uses the same one for the same job: a
/// keystroke no application reacts to, which still counts as "something was pressed
/// while Alt was down".
const VK_MENU_MASK: u16 = 0xE8;

/// How long `activate_window` waits for the target to actually come to the front.
const ACTIVATE_TIMEOUT: Duration = Duration::from_millis(300);
/// After it has, how long to give it to put keyboard focus back on the control that
/// holds the selection before a chord is sent at it.
const ACTIVATE_SETTLE: Duration = Duration::from_millis(40);

/// A press that moves less than this before release is a click, not a drag.
const DRAG_THRESHOLD_PX: i32 = 8;
/// Two clicks further apart than this are not a double-click.
const MULTI_CLICK_SLOP_PX: i32 = 6;
/// Upper bound for the hit test, so a hung application cannot stall the watcher.
const HIT_TEST_TIMEOUT_MS: u32 = 50;

/// Window classes where a drag is never a text selection: the desktop and the taskbar.
const IGNORED_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

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

    fn capture_frame(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<image::RgbaImage, CaptureError> {
        common::capture_frame_xcap(x, y, width, height)
    }

    fn replace_selection(&self, text: &str) -> Result<(), CaptureError> {
        common::replace_selection(Key::Control, text)
    }

    fn foreground_window(&self) -> Option<WindowHandle> {
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() || is_ours(window) {
            return None;
        }
        Some(window as WindowHandle)
    }

    fn activate_window(&self, window: WindowHandle) -> bool {
        let window = window as HWND;
        unsafe {
            if IsWindow(window) == 0 {
                return false;
            }
            if GetForegroundWindow() == window {
                return true;
            }
            // Allowed because we are the foreground process at this point: either our
            // popup has focus, or the user has just clicked our bubble.
            SetForegroundWindow(window);
        }

        let deadline = Instant::now() + ACTIVATE_TIMEOUT;
        while Instant::now() < deadline {
            if unsafe { GetForegroundWindow() } == window {
                std::thread::sleep(ACTIVATE_SETTLE);
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// RegisterHotKey swallows the hotkey's letter but not its modifier, so the
    /// foreground application sees Alt go down and, when the user lets go, come back up
    /// with nothing in between. That is a lone Alt tap: Chrome and Edge answer it by
    /// focusing their menu button, Firefox by showing its menu bar. The synthetic
    /// Ctrl+C that follows then lands on a menu instead of the page, and the user gets
    /// "No text selected" with their text plainly highlighted — but only when they
    /// happened to release Alt before the copy went out, which is why it looked random.
    ///
    /// Tapping a key nobody listens to while Alt is still held makes the eventual
    /// release non-lone. The same goes for the Windows key and the Start menu.
    fn mask_hotkey_modifiers(&self) {
        let key = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_MENU_MASK,
                    wScan: 0,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [key(0), key(KEYEVENTF_KEYUP)];
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
    }

    fn watch_pointer(&self, sink: Box<dyn Fn(PointerEvent) + Send + Sync>) -> bool {
        let (tx, rx) = mpsc::channel();
        if RAW_POINTER.set(tx).is_err() {
            // Already watching; a second hook would report every click twice.
            return false;
        }

        let (installed_tx, installed_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("pointer-hook".into())
            .spawn(move || unsafe {
                let hook = SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(mouse_hook),
                    GetModuleHandleW(std::ptr::null()),
                    0,
                );
                let _ = installed_tx.send(!hook.is_null());
                if hook.is_null() {
                    return;
                }
                // A low-level hook is called on the installing thread, through its
                // message loop, so this thread has to keep pumping for the app's life.
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
            });
        if spawned.is_err() || !installed_rx.recv().unwrap_or(false) {
            return false;
        }

        std::thread::Builder::new()
            .name("pointer-watch".into())
            .spawn(move || interpret(rx, sink))
            .is_ok()
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            os: "windows".into(),
            session: "win32".into(),
            backend: "Windows — synthetic Ctrl+C, Desktop Duplication capture".into(),
            supports_text_selection: true,
            supports_screen_capture: true,
            supports_replace: true,
            supports_selection_bubble: true,
            notes: vec![
                "Your clipboard is saved before the copy and restored right after.".into(),
            ],
            needs_accessibility_prompt: false,
        }
    }
}

fn is_ours(window: HWND) -> bool {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
        pid == GetCurrentProcessId()
    }
}

// ── Mouse watcher ──────────────────────────────────────────────────────────

/// What the hook forwards. The hook itself must return within a few hundred
/// milliseconds or Windows silently removes it, so it only copies the event out;
/// everything that can block happens on the interpreting thread.
#[derive(Clone, Copy)]
enum RawPointer {
    Down { x: i32, y: i32, time: u32 },
    Up { x: i32, y: i32, time: u32 },
    Other { x: i32, y: i32 },
}

static RAW_POINTER: OnceLock<mpsc::Sender<RawPointer>> = OnceLock::new();

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam as *const MSLLHOOKSTRUCT);
        if info.flags & LLMHF_INJECTED == 0 {
            let (x, y, time) = (info.pt.x, info.pt.y, info.time);
            let raw = match wparam as u32 {
                WM_LBUTTONDOWN => Some(RawPointer::Down { x, y, time }),
                WM_LBUTTONUP => Some(RawPointer::Up { x, y, time }),
                WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    Some(RawPointer::Other { x, y })
                }
                _ => None,
            };
            if let (Some(raw), Some(tx)) = (raw, RAW_POINTER.get()) {
                let _ = tx.send(raw);
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}

/// Turns raw button traffic into "that looked like a text selection".
///
/// Two gestures count: a left-button drag, and a double or triple click (word and
/// line selection). Neither proves text was selected — dragging a scrollbar or a file
/// looks the same — so `selection_target` weeds out the cases it can identify, and the
/// bubble itself is small and goes away on the next click.
fn interpret(rx: mpsc::Receiver<RawPointer>, sink: Box<dyn Fn(PointerEvent) + Send + Sync>) {
    let double_click_ms = unsafe { GetDoubleClickTime() };
    let mut press: Option<(i32, i32, u32)> = None;
    let mut last_click: Option<(i32, i32, u32)> = None;
    let mut clicks = 0u32;

    for raw in rx {
        match raw {
            RawPointer::Down { x, y, time } => {
                press = Some((x, y, time));
                sink(PointerEvent::Pressed { x, y });
            }
            RawPointer::Other { x, y } => {
                press = None;
                sink(PointerEvent::Pressed { x, y });
            }
            RawPointer::Up { x, y, time } => {
                let Some((px, py, pressed_at)) = press.take() else {
                    continue;
                };

                let dragged = (x - px).abs() >= DRAG_THRESHOLD_PX
                    || (y - py).abs() >= DRAG_THRESHOLD_PX;

                let follows_click = last_click.is_some_and(|(cx, cy, released_at)| {
                    pressed_at.wrapping_sub(released_at) <= double_click_ms
                        && (px - cx).abs() <= MULTI_CLICK_SLOP_PX
                        && (py - cy).abs() <= MULTI_CLICK_SLOP_PX
                });

                if dragged {
                    clicks = 0;
                    last_click = None;
                } else {
                    clicks = if follows_click { clicks + 1 } else { 1 };
                    last_click = Some((x, y, time));
                }

                if dragged || clicks >= 2 {
                    if let Some(window) = selection_target(x, y) {
                        sink(PointerEvent::Selected { x, y, window });
                    }
                }
            }
        }
    }
}

/// The application a gesture released at `x`/`y` was made in, if it plausibly selected
/// text there.
fn selection_target(x: i32, y: i32) -> Option<WindowHandle> {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null() || is_ours(foreground) {
            return None;
        }

        // The release has to land in the focused application, or the copy that a click
        // on the bubble sends would go somewhere else.
        let under = WindowFromPoint(POINT { x, y });
        if under.is_null() || GetAncestor(under, GA_ROOT) != foreground {
            return None;
        }

        if IGNORED_CLASSES.contains(&class_name(foreground).as_str()) {
            return None;
        }

        // Title bars and borders: the drag moved or resized the window.
        let point = ((x as u16 as u32) | ((y as u16 as u32) << 16)) as LPARAM;
        let mut hit = 0usize;
        let answered = SendMessageTimeoutW(
            under,
            WM_NCHITTEST,
            0,
            point,
            SMTO_ABORTIFHUNG,
            HIT_TEST_TIMEOUT_MS,
            &mut hit,
        );
        if answered != 0 && hit as u32 != HTCLIENT {
            return None;
        }

        Some(foreground as WindowHandle)
    }
}

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 64];
    let len = unsafe { GetClassNameW(window, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}
