//! Which finished executions have their checkout being released right now
//! (KT-1646).
//!
//! Removing a worktree runs Git and touches a disk, so it never runs while the
//! single database connection is held. The release is therefore three steps:
//! claim under the connection, remove outside it, record under the connection
//! again. Between the first and the last step the database says nothing about
//! the checkout, so the claim lives here, in the process that does the work: no
//! other release takes the same execution, and every path that would put the
//! checkout back to use (reassign, resume, extend the review rounds) refuses
//! while it is held.
//!
//! The claim is deliberately not durable. A crash drops it with the process, and
//! the next sweep re-judges the checkout from Git and the database, which is
//! exactly what a half-finished removal needs.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};

static CLAIMED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

/// A release's hold on one execution's checkout, dropped when the release has
/// recorded its outcome (or given up).
#[derive(Debug)]
pub struct ReleaseClaim {
    execution_id: String,
}

impl ReleaseClaim {
    /// The claim on `execution_id`, or `None` when a release already holds it.
    pub fn take(execution_id: &str) -> Option<Self> {
        CLAIMED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(execution_id.to_string())
            .then(|| Self {
                execution_id: execution_id.to_string(),
            })
    }
}

impl Drop for ReleaseClaim {
    fn drop(&mut self) {
        CLAIMED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.execution_id);
    }
}

/// Whether a release currently holds `execution_id`'s checkout.
pub fn is_claimed(execution_id: &str) -> bool {
    CLAIMED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(execution_id)
}

/// The refusal every path that would reuse the checkout gives while a release
/// holds it. The caller retries once the release has settled.
pub fn ensure_not_being_released(execution_id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        !is_claimed(execution_id),
        "execution {execution_id}'s checkout is being released; retry once the release has settled"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_is_exclusive_and_ends_when_dropped() {
        let id = "claim-exclusive";
        let claim = ReleaseClaim::take(id).expect("first claim");
        assert!(ReleaseClaim::take(id).is_none());
        assert!(is_claimed(id));
        assert!(ensure_not_being_released(id).is_err());
        drop(claim);
        assert!(!is_claimed(id));
        assert!(ensure_not_being_released(id).is_ok());
        assert!(ReleaseClaim::take(id).is_some());
    }
}
