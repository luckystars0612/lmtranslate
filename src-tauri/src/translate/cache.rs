//! In-memory, per-session translation cache (Milestone 7).
//!
//! Re-translating the same selection is common — you dismiss the popup, look away, then
//! hit the hotkey on the same text again — and a cache hit turns a multi-second round
//! trip into an instant result.
//!
//! Keyed on the exact source text rather than the plan's SHA-256 of it: with real keys
//! a collision is impossible by construction, and hashing a few kilobytes buys nothing
//! next to the network call it avoids. Image input is not cached, as the plan notes:
//! two crops are rarely byte-identical, so the hit rate would not pay for the memory.

use std::collections::{HashMap, VecDeque};

use super::types::TranslateResponse;

/// Enough to cover a working session's worth of repeated lookups.
const CAPACITY: usize = 64;

/// (source text, target language) — the language has to be part of the key or switching
/// language in the popup would return the previous language's translation.
type Key = (String, String);

#[derive(Default)]
pub struct TranslationCache {
    entries: HashMap<Key, TranslateResponse>,
    /// Insertion order, for eviction once `CAPACITY` is reached.
    order: VecDeque<Key>,
}

impl TranslationCache {
    pub fn get(&self, text: &str, target_lang: &str) -> Option<TranslateResponse> {
        self.entries
            .get(&(text.to_string(), target_lang.to_string()))
            .cloned()
    }

    pub fn put(&mut self, text: &str, target_lang: &str, response: &TranslateResponse) {
        let key = (text.to_string(), target_lang.to_string());

        if self.entries.contains_key(&key) {
            return;
        }
        while self.order.len() >= CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, response.clone());
    }

    /// Called when the provider, model or endpoint changes: results from the previous
    /// model must not be served as though they came from the new one.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::types::TranslatedSegment;

    fn response(translated: &str) -> TranslateResponse {
        TranslateResponse {
            segments: vec![TranslatedSegment {
                translated: translated.to_string(),
                ..Default::default()
            }],
            raw_model_output: String::new(),
            usage: None,
        }
    }

    #[test]
    fn round_trips_a_hit() {
        let mut cache = TranslationCache::default();
        cache.put("Hello", "Vietnamese", &response("Xin chào"));
        let hit = cache.get("Hello", "Vietnamese").unwrap();
        assert_eq!(hit.segments[0].translated, "Xin chào");
    }

    #[test]
    fn language_is_part_of_the_key() {
        let mut cache = TranslationCache::default();
        cache.put("Hello", "Vietnamese", &response("Xin chào"));
        assert!(cache.get("Hello", "Japanese").is_none());
    }

    #[test]
    fn evicts_the_oldest_beyond_capacity() {
        let mut cache = TranslationCache::default();
        for i in 0..CAPACITY + 10 {
            cache.put(&format!("text {i}"), "Vietnamese", &response("x"));
        }
        assert_eq!(cache.len(), CAPACITY);
        assert!(cache.get("text 0", "Vietnamese").is_none());
        assert!(cache.get(&format!("text {}", CAPACITY + 9), "Vietnamese").is_some());
    }

    #[test]
    fn clearing_drops_everything() {
        let mut cache = TranslationCache::default();
        cache.put("Hello", "Vietnamese", &response("Xin chào"));
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.get("Hello", "Vietnamese").is_none());
    }
}
