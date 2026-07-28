use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tauri_plugin_global_shortcut::Shortcut;

use crate::config::Settings;
use crate::logger::Logger;
use crate::platform::PlatformCapture;
use crate::translate::cache::TranslationCache;
use crate::translate::provider::TranslateProvider;
use crate::translate::types::InputKind;
use crate::windows::popup_window::{Anchor, PopupState};

#[derive(Default)]
pub struct RegisteredHotkeys {
    pub text: Option<Shortcut>,
    pub screenshot: Option<Shortcut>,
}

pub struct AppState {
    pub config_path: PathBuf,
    pub settings: RwLock<Settings>,
    pub platform: Box<dyn PlatformCapture>,
    pub provider: RwLock<Arc<dyn TranslateProvider>>,
    pub logger: Logger,
    pub hotkeys: Mutex<RegisteredHotkeys>,

    /// Latest popup payload. The popup pulls this on load, so an event emitted before
    /// the webview is ready is never lost.
    pub popup: Mutex<PopupState>,
    pub popup_anchor: Mutex<Option<Anchor>>,
    /// Retained so the popup's language dropdown can re-run the same input.
    pub last_input: Mutex<Option<InputKind>>,
    /// Text translations from this session, so a repeat is instant.
    pub cache: Mutex<TranslationCache>,

    /// Debounce: one translation in flight at a time.
    in_flight: AtomicBool,
    overlay_open: AtomicBool,
    /// Bumped per flow; async tasks check it before touching a popup that has since
    /// been replaced or closed.
    generation: AtomicU64,
    /// Set once the user interacts with the popup, which cancels the auto-close timer.
    popup_kept: AtomicBool,
    /// Focus-loss should only close the popup after it has actually held focus once.
    popup_focused_once: AtomicBool,
}

impl AppState {
    pub fn new(
        config_path: PathBuf,
        settings: Settings,
        platform: Box<dyn PlatformCapture>,
        provider: Arc<dyn TranslateProvider>,
        logger: Logger,
    ) -> Self {
        Self {
            config_path,
            settings: RwLock::new(settings),
            platform,
            provider: RwLock::new(provider),
            logger,
            hotkeys: Mutex::new(RegisteredHotkeys::default()),
            popup: Mutex::new(PopupState::default()),
            popup_anchor: Mutex::new(None),
            last_input: Mutex::new(None),
            cache: Mutex::new(TranslationCache::default()),
            in_flight: AtomicBool::new(false),
            overlay_open: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            popup_kept: AtomicBool::new(false),
            popup_focused_once: AtomicBool::new(false),
        }
    }

    pub fn settings_snapshot(&self) -> Settings {
        self.settings
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn provider_snapshot(&self) -> Option<Arc<dyn TranslateProvider>> {
        self.provider.read().ok().map(|p| p.clone())
    }

    /// True if this caller won the race and now owns the in-flight slot.
    pub fn begin_flight(&self) -> bool {
        self.in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn end_flight(&self) {
        self.in_flight.store(false, Ordering::SeqCst);
    }

    pub fn begin_overlay(&self) -> bool {
        self.overlay_open
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    pub fn end_overlay(&self) {
        self.overlay_open.store(false, Ordering::SeqCst);
    }

    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.generation() == generation
    }

    pub fn reset_popup_flags(&self) {
        self.popup_kept.store(false, Ordering::SeqCst);
        self.popup_focused_once.store(false, Ordering::SeqCst);
    }

    pub fn keep_popup_open(&self) {
        self.popup_kept.store(true, Ordering::SeqCst);
    }

    pub fn popup_was_kept(&self) -> bool {
        self.popup_kept.load(Ordering::SeqCst)
    }

    pub fn note_popup_focus(&self, focused: bool) -> bool {
        if focused {
            self.popup_focused_once.store(true, Ordering::SeqCst);
            false
        } else {
            self.popup_focused_once.load(Ordering::SeqCst)
        }
    }
}
