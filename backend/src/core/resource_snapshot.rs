//! Run-scoped snapshot cache for skills, directives and profiles.
//!
//! `core::skills`, `core::directives` and `core::profiles` always resolve a
//! resource id against whatever is on disk right now. That's correct for a
//! one-shot lookup, but a workflow run can span several steps executed over
//! minutes or hours: if a resource is edited or deleted between two steps of
//! the SAME run, later steps must keep seeing what the run already loaded,
//! not the new state (ADR-005 slice 1, KT-847).
//!
//! `RunSnapshotCache` pins each `(run_id, resource_id)` pair to the first
//! resolved value the run saw. `release` drops every entry for a run once it
//! has finished, so the cache doesn't grow unbounded across the process
//! lifetime — see `workflows::runner::execute_run` for the RAII guard that
//! calls it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub struct RunSnapshotCache<T> {
    entries: OnceLock<Mutex<HashMap<(String, String), T>>>,
}

impl<T: Clone> RunSnapshotCache<T> {
    pub const fn new() -> Self {
        Self {
            entries: OnceLock::new(),
        }
    }

    fn store(&self) -> &Mutex<HashMap<(String, String), T>> {
        self.entries.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// Returns the value this run already pinned for `id`, if any.
    /// Otherwise resolves it through `resolve` and pins the result for the
    /// rest of the run — a later call with the same `(run_id, id)` returns
    /// this same value even if `resolve` would now return something else.
    pub fn get_or_resolve(
        &self,
        run_id: &str,
        id: &str,
        resolve: impl FnOnce() -> Option<T>,
    ) -> Option<T> {
        let key = (run_id.to_string(), id.to_string());
        {
            let entries = self.store().lock().unwrap_or_else(|p| p.into_inner());
            if let Some(cached) = entries.get(&key) {
                return Some(cached.clone());
            }
        }
        let resolved = resolve()?;
        let mut entries = self.store().lock().unwrap_or_else(|p| p.into_inner());
        let pinned = entries.entry(key).or_insert(resolved);
        Some(pinned.clone())
    }

    /// Drops every entry pinned to `run_id`. Call once that run has
    /// finished so its snapshot doesn't stay pinned in memory forever.
    pub fn release(&self, run_id: &str) {
        let mut entries = self.store().lock().unwrap_or_else(|p| p.into_inner());
        entries.retain(|(pinned_run, _), _| pinned_run != run_id);
    }
}

impl<T: Clone> Default for RunSnapshotCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Releases every skill/directive/profile snapshot pinned to `run_id` when
/// dropped — mirrors `CancelGuard` (see `lib.rs`). Insert one at the top of
/// a workflow run's execution call so its snapshots don't outlive it.
pub struct RunSnapshotGuard {
    run_id: String,
}

impl RunSnapshotGuard {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
        }
    }
}

impl Drop for RunSnapshotGuard {
    fn drop(&mut self) {
        crate::core::skills::release_skills_snapshot(&self.run_id);
        crate::core::directives::release_directives_snapshot(&self.run_id);
        crate::core::profiles::release_profiles_snapshot(&self.run_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_resolution_is_pinned_for_the_run() {
        let cache: RunSnapshotCache<String> = RunSnapshotCache::new();
        let first = cache.get_or_resolve("run-1", "a", || Some("v1".to_string()));
        assert_eq!(first, Some("v1".to_string()));

        // A "resolve" that would now return something else must be ignored:
        // the run already pinned "v1".
        let second = cache.get_or_resolve("run-1", "a", || Some("v2".to_string()));
        assert_eq!(second, Some("v1".to_string()));
    }

    #[test]
    fn a_deleted_resource_still_resolves_from_the_pin() {
        let cache: RunSnapshotCache<String> = RunSnapshotCache::new();
        let _ = cache.get_or_resolve("run-1", "a", || Some("v1".to_string()));

        // "Deleted" = resolve now returns None. The pin still answers.
        let after_delete = cache.get_or_resolve("run-1", "a", || None);
        assert_eq!(after_delete, Some("v1".to_string()));
    }

    #[test]
    fn different_runs_are_independent() {
        let cache: RunSnapshotCache<String> = RunSnapshotCache::new();
        let _ = cache.get_or_resolve("run-1", "a", || Some("v1".to_string()));
        let other_run = cache.get_or_resolve("run-2", "a", || Some("v2".to_string()));
        assert_eq!(other_run, Some("v2".to_string()));
    }

    #[test]
    fn release_drops_only_that_run() {
        let cache: RunSnapshotCache<String> = RunSnapshotCache::new();
        let _ = cache.get_or_resolve("run-1", "a", || Some("v1".to_string()));
        let _ = cache.get_or_resolve("run-2", "a", || Some("v2".to_string()));

        cache.release("run-1");

        // run-1 is gone: a fresh resolve happens again.
        let after_release = cache.get_or_resolve("run-1", "a", || Some("v1-fresh".to_string()));
        assert_eq!(after_release, Some("v1-fresh".to_string()));

        // run-2 is untouched.
        let still_pinned = cache.get_or_resolve("run-2", "a", || Some("v2-changed".to_string()));
        assert_eq!(still_pinned, Some("v2".to_string()));
    }

    #[test]
    fn unresolvable_id_yields_none_and_pins_nothing() {
        let cache: RunSnapshotCache<String> = RunSnapshotCache::new();
        let missing = cache.get_or_resolve("run-1", "nope", || None);
        assert_eq!(missing, None);

        // A later resolve for the same id still runs `resolve` — nothing
        // was pinned for a lookup that found nothing.
        let now_found = cache.get_or_resolve("run-1", "nope", || Some("found".to_string()));
        assert_eq!(now_found, Some("found".to_string()));
    }
}
