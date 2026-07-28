//! A tiny append-only debug log, viewable and clearable from Settings.
//!
//! Only request/response *metadata* is ever written here — never the API key, and
//! never the text being translated.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Rotated (truncated) past this size so the file cannot grow without bound.
const MAX_BYTES: u64 = 512 * 1024;
/// How much of the tail the Settings viewer shows.
const VIEW_LINES: usize = 400;

pub struct Logger {
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl Logger {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("INFO ", message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("WARN ", message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.write("ERROR", message.as_ref());
    }

    fn write(&self, level: &str, message: &str) {
        let Ok(_guard) = self.write_lock.lock() else {
            return;
        };

        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        // Cheap rotation: once the file is oversized, keep only the recent tail.
        if std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
            if let Ok(existing) = std::fs::read_to_string(&self.path) {
                let tail: Vec<&str> = existing.lines().rev().take(VIEW_LINES).collect();
                let trimmed: String = tail
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = std::fs::write(&self.path, format!("{trimmed}\n"));
            }
        }

        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!("{stamp} {level} {message}\n");

        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// The tail of the log, newest last.
    pub fn read(&self) -> String {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return String::new();
        };
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(VIEW_LINES);
        lines[start..].join("\n")
    }

    pub fn clear(&self) {
        let _guard = self.write_lock.lock();
        let _ = std::fs::write(&self.path, "");
    }
}
