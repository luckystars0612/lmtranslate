//! API keys live in the operating system's credential store — Credential Manager on
//! Windows, the login Keychain on macOS, the Secret Service on Linux — never in
//! settings.json.
//!
//! In memory, `Settings` still carries the keys exactly as before, so nothing else in
//! the app needs to know. Only the two edges change: `hydrate` fills the keys in after
//! the file is read, and `persist` takes them out before it is written.
//!
//! A session with no usable store (a Linux desktop without GNOME Keyring or KWallet,
//! say) keeps the old behaviour rather than losing the keys, and Settings says so.

use std::path::Path;
use std::sync::OnceLock;

use keyring::{Entry, Error};

use crate::config::{self, Settings};
use crate::logger::Logger;

/// Every entry is filed under this service name, one per key.
const SERVICE: &str = "LMTranslate";

/// Where keys are kept, for Settings and the log.
pub fn storage_name() -> &'static str {
    if !is_available() {
        return "the settings file (no system keychain is available)";
    }
    if cfg!(target_os = "windows") {
        "Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else {
        "the system keyring (Secret Service)"
    }
}

/// Probed once: whether the store answers at all. "No such entry" counts as working.
pub fn is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        Entry::new(SERVICE, "availability-probe")
            .and_then(|entry| match entry.get_password() {
                Ok(_) | Err(Error::NoEntry) => Ok(()),
                Err(e) => Err(e),
            })
            .is_ok()
    })
}

/// Called right after settings.json is read. Any key still in the file — written by an
/// older version, or typed in by hand — is moved into the store and the file rewritten
/// without it. Then every key is filled in from the store.
pub fn hydrate(path: &Path, settings: &mut Settings, logger: &Logger) {
    if !is_available() {
        logger.warn("no system keychain available; API keys stay in settings.json");
        return;
    }

    let in_file = slots(settings)
        .into_iter()
        .filter(|(_, key)| !key.trim().is_empty())
        .count();
    if in_file > 0 {
        match config::save(path, settings) {
            Ok(()) => logger.info(format!(
                "moved {in_file} API key(s) out of settings.json into {}",
                storage_name()
            )),
            Err(e) => logger.warn(format!("could not move API keys out of settings.json: {e}")),
        }
    }

    for (account, key) in slots(settings) {
        if !key.is_empty() {
            continue;
        }
        match Entry::new(SERVICE, &account).and_then(|entry| entry.get_password()) {
            Ok(stored) => *key = stored,
            Err(Error::NoEntry) => {}
            Err(e) => logger.warn(format!("could not read the {account} key: {e}")),
        }
    }
}

/// The copy of `settings` that is safe to write to disk: every key handed to the store
/// (or removed from it, when cleared) and blanked in the copy.
pub fn persist(settings: &Settings) -> Result<Settings, String> {
    let mut on_disk = settings.clone();
    if !is_available() {
        return Ok(on_disk);
    }

    for (account, key) in slots(&mut on_disk) {
        let entry = Entry::new(SERVICE, &account)
            .map_err(|e| format!("could not open the {account} key entry: {e}"))?;
        let value = key.trim();
        if value.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(Error::NoEntry) => {}
                Err(e) => return Err(format!("could not remove the {account} key: {e}")),
            }
        } else {
            // Skip the write when nothing changed; on macOS every write can prompt.
            if entry.get_password().ok().as_deref() != Some(value) {
                entry
                    .set_password(value)
                    .map_err(|e| format!("could not store the {account} key: {e}"))?;
            }
        }
        key.clear();
    }
    Ok(on_disk)
}

/// Every API key in `Settings`, with the account name it is stored under. The one place
/// that knows which fields are secrets: a new key field must be added here.
fn slots(settings: &mut Settings) -> Vec<(String, &mut String)> {
    let mut slots: Vec<(String, &mut String)> = settings
        .providers
        .iter_mut()
        .map(|(id, cfg)| (format!("provider/{id}"), &mut cfg.api_key))
        .collect();
    slots.push(("speech/minimax".into(), &mut settings.speech_minimax_key));
    slots.push(("speech/azure".into(), &mut settings.azure_speech_key));
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn use_mock_store() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }

    /// The point of the module: what reaches the disk has no keys in it.
    #[test]
    fn the_written_copy_carries_no_keys() {
        use_mock_store();
        let mut settings = Settings::default();
        settings.providers.get_mut("minimax").unwrap().api_key = "sk-secret".into();
        settings.azure_speech_key = "azure-secret".into();

        let on_disk = persist(&settings).unwrap();
        let json = serde_json::to_string(&on_disk).unwrap();
        assert!(!json.contains("sk-secret"));
        assert!(!json.contains("azure-secret"));
        // The in-memory settings are untouched.
        assert_eq!(settings.azure_speech_key, "azure-secret");
    }

    #[test]
    fn every_key_field_is_covered() {
        let mut settings = Settings::default();
        let accounts: Vec<String> = slots(&mut settings).into_iter().map(|(a, _)| a).collect();
        for provider in config::PROVIDERS {
            assert!(accounts.contains(&format!("provider/{}", provider.id)));
        }
        assert!(accounts.contains(&"speech/minimax".to_string()));
        assert!(accounts.contains(&"speech/azure".to_string()));
    }
}
