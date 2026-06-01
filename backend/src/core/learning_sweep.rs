//! 0.9.0 — Continual Learning staleness cron (spec §8, decision D9).
//!
//! A `tokio::interval` loop (hourly) that marks pending/validated learnings
//! `stale` once they haven't been (re)validated for 7 days — baseline
//! `COALESCE(last_validated_at, created_at)`. No deletion: stale is a nudge the
//! UI surfaces ("8 months old — still valid?"). Event-driven was rejected at
//! design (no Tauri FileWatcher); the hourly tick is sufficient for a soft signal.
//!
//! Spawned at boot in BOTH binaries (`backend/src/main.rs` AND
//! `desktop/src-tauri/src/main.rs`) — the feature lives in the lib but the spawn
//! is per-binary, so forgetting one means no sweep in that target.

use crate::db::Database;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use std::time::Duration;

const SWEEP_INTERVAL_SECS: u64 = 3600;
const STALE_AFTER_DAYS: i64 = 7;

pub struct LearningSweep {
    db: Arc<Database>,
}

impl LearningSweep {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Run forever: recover stranded `promoting` rows ONCE at boot (crash
    /// recovery — see `db::learnings::recover_stranded_promoting`), then tick
    /// hourly, sweep, log failures (never crash the loop).
    pub async fn start(self: Arc<Self>) {
        match self
            .db
            .with_conn(crate::db::learnings::recover_stranded_promoting)
            .await
        {
            Ok(n) if n > 0 => {
                tracing::warn!(target: "learning_sweep", "recovered {n} stranded 'promoting' learning(s) → pending")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(target: "learning_sweep", "promoting-recovery failed: {e}"),
        }
        let mut tick = tokio::time::interval(Duration::from_secs(SWEEP_INTERVAL_SECS));
        loop {
            tick.tick().await;
            if let Err(e) = self.sweep_once().await {
                tracing::warn!(target: "learning_sweep", "sweep failed: {e}");
            }
        }
    }

    /// One sweep pass. Returns the number of rows newly marked stale.
    pub async fn sweep_once(&self) -> anyhow::Result<usize> {
        let cutoff = (Utc::now() - ChronoDuration::days(STALE_AFTER_DAYS)).to_rfc3339();
        let n = self
            .db
            .with_conn(move |conn| {
                crate::db::learnings::mark_stale_before(conn, &cutoff).map_err(|e| anyhow::anyhow!("{e}"))
            })
            .await?;
        if n > 0 {
            tracing::info!(target: "learning_sweep", "marked {n} learning(s) stale");
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::learnings as db_learnings;
    use crate::models::learnings::*;

    fn old_pending(id: &str, created_at: &str) -> Learning {
        Learning {
            id: id.into(),
            claim: format!("claim {id}"),
            evidence: vec![Evidence { kind: "user".into(), reference: "user:2000-01-01".into(), quote: None }],
            kind: LearningKind::Preference,
            status: LearningStatus::Pending,
            scope: None,
            confidence: None,
            faithfulness: None,
            discussion_id: None,
            project_id: None,
            source_agent: None,
            promoted_target: None,
            created_at: created_at.into(),
            last_validated_at: None,
            validated_by: None,
        }
    }

    #[tokio::test]
    async fn sweep_marks_only_rows_past_the_7day_cutoff() {
        let db = Arc::new(Database::open_in_memory().expect("in-memory db"));
        // Insert one ancient + one fresh pending (no FK parents needed — both null).
        db.with_conn(|conn| {
            db_learnings::insert(conn, &old_pending("ancient", "2000-01-01T00:00:00+00:00"))?;
            let mut fresh = old_pending("fresh", "2000-01-01T00:00:00+00:00");
            fresh.created_at = Utc::now().to_rfc3339();
            db_learnings::insert(conn, &fresh)?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .unwrap();

        let sweep = LearningSweep::new(db.clone());
        let n = sweep.sweep_once().await.unwrap();
        assert_eq!(n, 1, "only the ancient pending crosses the 7-day cutoff");

        let (ancient, fresh) = db
            .with_conn(|conn| {
                Ok::<_, anyhow::Error>((
                    db_learnings::get(conn, "ancient")?.unwrap().status,
                    db_learnings::get(conn, "fresh")?.unwrap().status,
                ))
            })
            .await
            .unwrap();
        assert_eq!(ancient, LearningStatus::Stale);
        assert_eq!(fresh, LearningStatus::Pending);
    }
}
