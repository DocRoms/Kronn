//! Process-wide memo of derived content, keyed by a fingerprint of its input.
//!
//! Masking the secrets of a resource runs a dozen regexes over every string it
//! holds, and the result is a pure function of that content. A listing that
//! renders the same automations on every read would pay it again each time, so
//! the masked result is kept under the SHA-256 of what produced it: the same
//! content is never masked twice, and changed content simply gets a new key.
//!
//! The memo lives in memory only. A changed masking rule ships with a new
//! binary, hence a new process, so an entry can never outlive the rules that
//! produced it.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// SHA-256 (hex) of `parts`, each length-prefixed so that two different
/// splits of the same bytes (`["ab", "c"]` and `["a", "bc"]`) never collide.
pub fn fingerprint(parts: &[&[u8]]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// How often a lookup was answered from the memo, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoStats {
    pub hits: u64,
    pub misses: u64,
}

struct Entry<T> {
    value: Arc<T>,
    weight: usize,
}

struct State<T> {
    entries: HashMap<String, Entry<T>>,
    /// Insertion order, oldest first: the next entries to evict.
    order: VecDeque<String>,
    used: usize,
}

/// A bounded memo. `budget` caps the summed weight of the entries (bytes, as
/// far as the caller's weigher is honest): the oldest entries leave first, and
/// a value heavier than the whole budget is computed but never kept.
pub struct ContentMemo<T> {
    budget: usize,
    state: Mutex<State<T>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl<T> ContentMemo<T> {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            state: Mutex::new(State {
                entries: HashMap::new(),
                order: VecDeque::new(),
                used: 0,
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    pub fn stats(&self) -> MemoStats {
        MemoStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T>> {
        // A panic while holding the lock cannot leave the maps half-updated in
        // a way that matters: the worst case is a missing entry, recomputed.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The value kept under `key`, or `compute()` stored under it. The
    /// computation runs outside the lock, so two racing misses both compute
    /// (the result is identical) rather than one blocking every reader.
    pub fn get_or_try_insert<E>(
        &self,
        key: &str,
        weigh: impl FnOnce(&T) -> usize,
        compute: impl FnOnce() -> Result<T, E>,
    ) -> Result<Arc<T>, E> {
        if let Some(entry) = self.lock().entries.get(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::clone(&entry.value));
        }
        self.misses.fetch_add(1, Ordering::Relaxed);
        let value = Arc::new(compute()?);
        let weight = weigh(&value);
        if weight <= self.budget {
            let mut state = self.lock();
            if !state.entries.contains_key(key) {
                while state.used + weight > self.budget {
                    let Some(oldest) = state.order.pop_front() else {
                        break;
                    };
                    if let Some(evicted) = state.entries.remove(&oldest) {
                        state.used -= evicted.weight;
                    }
                }
                state.used += weight;
                state.order.push_back(key.to_string());
                state.entries.insert(
                    key.to_string(),
                    Entry {
                        value: Arc::clone(&value),
                        weight,
                    },
                );
            }
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The memo weighs a `&T`, and `T` is `String` in these tests.
    #[allow(clippy::ptr_arg)]
    fn text_weight(value: &String) -> usize {
        value.len()
    }

    #[test]
    fn the_same_key_is_computed_once() {
        let memo = ContentMemo::new(1024);
        let mut runs = 0;
        for _ in 0..3 {
            let value = memo
                .get_or_try_insert::<()>("k", text_weight, || {
                    runs += 1;
                    Ok("masked".to_string())
                })
                .unwrap();
            assert_eq!(*value, "masked");
        }
        assert_eq!(runs, 1, "a second lookup must not recompute");
        assert_eq!(memo.stats(), MemoStats { hits: 2, misses: 1 });
    }

    #[test]
    fn a_failed_computation_is_not_kept() {
        let memo = ContentMemo::new(1024);
        assert!(memo
            .get_or_try_insert("k", text_weight, || Err::<String, _>("boom"))
            .is_err());
        let value = memo
            .get_or_try_insert::<()>("k", text_weight, || Ok("fine".to_string()))
            .unwrap();
        assert_eq!(*value, "fine");
    }

    #[test]
    fn the_oldest_entries_leave_first_when_the_budget_is_full() {
        let memo = ContentMemo::new(10);
        for key in ["a", "b", "c"] {
            memo.get_or_try_insert::<()>(key, text_weight, || Ok("xxxx".to_string()))
                .unwrap();
        }
        // Three entries of weight 4 do not fit in 10: "a" was evicted.
        let before = memo.stats();
        memo.get_or_try_insert::<()>("c", text_weight, || Ok("xxxx".to_string()))
            .unwrap();
        assert_eq!(memo.stats().hits, before.hits + 1, "the newest is kept");
        memo.get_or_try_insert::<()>("a", text_weight, || Ok("xxxx".to_string()))
            .unwrap();
        assert_eq!(memo.stats().misses, before.misses + 1, "the oldest is gone");
    }

    #[test]
    fn a_value_heavier_than_the_budget_is_returned_but_never_kept() {
        let memo = ContentMemo::new(3);
        let mut runs = 0;
        for _ in 0..2 {
            let value = memo
                .get_or_try_insert::<()>("big", text_weight, || {
                    runs += 1;
                    Ok("too large".to_string())
                })
                .unwrap();
            assert_eq!(*value, "too large");
        }
        assert_eq!(runs, 2);
    }

    #[test]
    fn fingerprints_follow_the_content_and_its_boundaries() {
        assert_eq!(fingerprint(&[b"ab", b"c"]), fingerprint(&[b"ab", b"c"]));
        assert_ne!(fingerprint(&[b"ab", b"c"]), fingerprint(&[b"a", b"bc"]));
        assert_ne!(fingerprint(&[b"ab"]), fingerprint(&[b"ab", b""]));
        assert_eq!(fingerprint(&[b"x"]).len(), 64);
    }
}
