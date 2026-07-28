//! Hotkey registration and dispatch.
//!
//! Registration is deliberately re-run wholesale whenever settings change: unregister
//! everything, then register what the settings now say. That keeps the registered set
//! and the stored set from ever drifting apart.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::config::Settings;
use crate::flows;
use crate::state::{AppState, RegisteredHotkeys};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Text,
    Screenshot,
}

impl Slot {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "text" => Some(Slot::Text),
            "screenshot" => Some(Slot::Screenshot),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Slot::Text => "select-and-translate",
            Slot::Screenshot => "screenshot-and-translate",
        }
    }
}

pub fn parse(accelerator: &str) -> Result<Shortcut, String> {
    accelerator
        .parse::<Shortcut>()
        .map_err(|e| format!("\"{accelerator}\" is not a valid shortcut ({e})."))
}

/// Registers both hotkeys, returning a human-readable problem per hotkey that failed.
pub fn apply(app: &AppHandle, settings: &Settings) -> Vec<String> {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();

    let mut problems = Vec::new();
    let mut registered = RegisteredHotkeys::default();

    for (slot, accelerator) in [
        (Slot::Text, &settings.hotkey_text),
        (Slot::Screenshot, &settings.hotkey_screenshot),
    ] {
        match parse(accelerator) {
            Ok(shortcut) => match shortcuts.register(shortcut) {
                Ok(()) => match slot {
                    Slot::Text => registered.text = Some(shortcut),
                    Slot::Screenshot => registered.screenshot = Some(shortcut),
                },
                Err(e) => problems.push(format!(
                    "{accelerator} could not be registered for {} — another application \
                     probably owns it ({e}).",
                    slot.label()
                )),
            },
            Err(message) => problems.push(message),
        }
    }

    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut slot) = state.hotkeys.lock() {
            *slot = registered;
        }
    }

    problems
}

/// The plugin hands every registered shortcut to one handler, so match it back to a slot.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) {
    if state != ShortcutState::Pressed {
        return;
    }

    let Some(app_state) = app.try_state::<AppState>() else {
        return;
    };

    let slot = {
        let Ok(registered) = app_state.hotkeys.lock() else {
            return;
        };
        if registered.text.as_ref() == Some(shortcut) {
            Some(Slot::Text)
        } else if registered.screenshot.as_ref() == Some(shortcut) {
            Some(Slot::Screenshot)
        } else {
            None
        }
    };

    match slot {
        Some(Slot::Text) => flows::start_text_flow(app.clone()),
        Some(Slot::Screenshot) => flows::start_screenshot_flow(app.clone()),
        None => {}
    }
}

/// Backs the Settings hotkey picker: is this combination usable right now?
pub fn probe(app: &AppHandle, accelerator: &str, slot: Slot) -> (bool, String) {
    let shortcut = match parse(accelerator) {
        Ok(s) => s,
        Err(message) => return (false, message),
    };

    let settings = app
        .try_state::<AppState>()
        .map(|s| s.settings_snapshot())
        .unwrap_or_default();

    let other = match slot {
        Slot::Text => &settings.hotkey_screenshot,
        Slot::Screenshot => &settings.hotkey_text,
    };
    if parse(other).ok() == Some(shortcut) {
        return (
            false,
            "That combination is already used by the other LM Translate hotkey.".into(),
        );
    }

    let shortcuts = app.global_shortcut();

    // Already ours for this slot — nothing to test.
    let currently_ours = match slot {
        Slot::Text => &settings.hotkey_text,
        Slot::Screenshot => &settings.hotkey_screenshot,
    };
    if parse(currently_ours).ok() == Some(shortcut) {
        return (true, "Available — this is the current shortcut.".into());
    }

    // The only reliable conflict test is to try taking it, so take it and give it back.
    match shortcuts.register(shortcut) {
        Ok(()) => {
            let _ = shortcuts.unregister(shortcut);
            (true, "Available.".into())
        }
        Err(e) => (
            false,
            format!("Another application already owns that combination ({e})."),
        ),
    }
}
