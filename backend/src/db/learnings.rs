//! 0.9.0 — Continual Learning persistence (table `learnings` + `learning_rejections`).
//!
//! Mirrors the `db/disc_source.rs` style: narrow slice of the schema, all helpers
//! share the `learnings` row mapping. Enums round-trip as their DB strings via
//! `models::learnings::*::{as_str, from_db}`. Evidence is stored as a JSON array
//! in `evidence_json`. See `docs/research/continual-learning-0.9.0-spec.md`.

use crate::models::learnings::*;
use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection, Row};

const COLS: &str = "id, claim, evidence_json, kind, status, scope, confidence, \
    faithfulness, discussion_id, project_id, source_agent, promoted_target, \
    created_at, last_validated_at, validated_by";

fn row_to_learning(row: &Row) -> rusqlite::Result<Learning> {
    let id_for_log: String = row.get(0)?;
    let evidence_json: String = row.get(2)?;
    // A non-empty evidence_json that fails to parse = DB corruption — log it
    // loudly instead of silently degrading to an empty list (which would mask
    // the corruption and violate the "evidence[] non-empty" invariant).
    let evidence: Vec<Evidence> = match serde_json::from_str(&evidence_json) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "continual_learning",
                "learning {id_for_log}: corrupt evidence_json ({e}) — treating as empty",
            );
            Vec::new()
        }
    };
    let kind_s: String = row.get(3)?;
    let status_s: String = row.get(4)?;
    let scope_s: Option<String> = row.get(5)?;
    let faith_s: Option<String> = row.get(7)?;
    Ok(Learning {
        id: row.get(0)?,
        claim: row.get(1)?,
        evidence,
        kind: LearningKind::from_db(&kind_s).unwrap_or(LearningKind::Inference),
        status: LearningStatus::from_db(&status_s).unwrap_or(LearningStatus::Pending),
        scope: scope_s.as_deref().and_then(LearningScope::from_db),
        confidence: row.get(6)?,
        faithfulness: faith_s.as_deref().and_then(Faithfulness::from_db),
        discussion_id: row.get(8)?,
        project_id: row.get(9)?,
        source_agent: row.get(10)?,
        promoted_target: row.get(11)?,
        created_at: row.get(12)?,
        last_validated_at: row.get(13)?,
        validated_by: row.get(14)?,
    })
}

/// Insert a new pending learning. `evidence` MUST be non-empty (the caller —
/// the API handler — enforces this; we double-check to keep the invariant local).
pub fn insert(conn: &Connection, l: &Learning) -> Result<()> {
    if l.evidence.is_empty() {
        return Err(anyhow!("learning evidence[] must be non-empty"));
    }
    let evidence_json = serde_json::to_string(&l.evidence)?;
    conn.execute(
        "INSERT INTO learnings (id, claim, evidence_json, kind, status, scope, \
         confidence, faithfulness, discussion_id, project_id, source_agent, \
         promoted_target, created_at, last_validated_at, validated_by) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![
            l.id,
            l.claim,
            evidence_json,
            l.kind.as_str(),
            l.status.as_str(),
            l.scope.map(|s| s.as_str()),
            l.confidence,
            l.faithfulness.map(|f| f.as_str()),
            l.discussion_id,
            l.project_id,
            l.source_agent,
            l.promoted_target,
            l.created_at,
            l.last_validated_at,
            l.validated_by,
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Learning>> {
    let sql = format!("SELECT {COLS} FROM learnings WHERE id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map(params![id], row_to_learning)?;
    match rows.next() {
        Some(r) => Ok(Some(r?)),
        None => Ok(None),
    }
}

/// List learnings, optionally filtered by status and/or project. Newest first.
pub fn list(
    conn: &Connection,
    status: Option<LearningStatus>,
    project_id: Option<&str>,
) -> Result<Vec<Learning>> {
    let mut sql = format!("SELECT {COLS} FROM learnings WHERE 1=1");
    if status.is_some() {
        sql.push_str(" AND status = ?1");
    }
    if project_id.is_some() {
        sql.push_str(if status.is_some() { " AND project_id = ?2" } else { " AND project_id = ?1" });
    }
    sql.push_str(" ORDER BY created_at DESC");
    let mut stmt = conn.prepare(&sql)?;
    let map = |rows: rusqlite::MappedRows<_>| -> Result<Vec<Learning>> {
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    let out = match (status, project_id) {
        (Some(s), Some(p)) => map(stmt.query_map(params![s.as_str(), p], row_to_learning)?)?,
        (Some(s), None) => map(stmt.query_map(params![s.as_str()], row_to_learning)?)?,
        (None, Some(p)) => map(stmt.query_map(params![p], row_to_learning)?)?,
        (None, None) => map(stmt.query_map([], row_to_learning)?)?,
    };
    Ok(out)
}

/// Count of pending candidates (for the global badge).
pub fn count_pending(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM learnings WHERE status = 'pending'",
        [],
        |r| r.get(0),
    )?)
}

/// Pending candidates for one discussion (manual-archive modal).
pub fn disc_pending(conn: &Connection, disc_id: &str) -> Result<Vec<Learning>> {
    let sql = format!(
        "SELECT {COLS} FROM learnings WHERE discussion_id = ?1 AND status = 'pending' \
         ORDER BY created_at DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![disc_id], row_to_learning)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Set the Gate-2 faithfulness verdict (posture B: stored, surfaced, never blocks).
pub fn set_faithfulness(conn: &Connection, id: &str, f: Faithfulness) -> Result<()> {
    conn.execute(
        "UPDATE learnings SET faithfulness = ?2 WHERE id = ?1",
        params![id, f.as_str()],
    )?;
    Ok(())
}

/// Phase 1 of promotion — atomically claim a PENDING row (`pending → promoting`)
/// via CAS. Returns true iff this caller won the claim. Blocks concurrent
/// validate (second claim 0-rows) AND reject (reject requires pending). Must be
/// called BEFORE the file write.
pub fn claim_for_promotion(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE learnings SET status = 'promoting' WHERE id = ?1 AND status = 'pending'",
        params![id],
    )?;
    Ok(n == 1)
}

/// Phase 2 (success) — `promoting → promoted` + route scope/target. Conditional
/// on `status='promoting'` so it only finalizes a row WE claimed.
pub fn finalize_promotion(
    conn: &Connection,
    id: &str,
    scope: LearningScope,
    promoted_target: Option<&str>,
    validated_by: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE learnings SET status = 'promoted', scope = ?2, promoted_target = ?3, \
         validated_by = ?4, last_validated_at = ?5 WHERE id = ?1 AND status = 'promoting'",
        params![id, scope.as_str(), promoted_target, validated_by, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Phase 2 (failure) — `promoting → pending`, so a failed file write doesn't
/// strand the row (a retry of validate can re-claim it).
pub fn revert_promotion(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE learnings SET status = 'pending' WHERE id = ?1 AND status = 'promoting'",
        params![id],
    )?;
    Ok(())
}

/// Crash-recovery — revert every `promoting` row back to `pending`. Called ONCE
/// at boot: a `promoting` row at startup is necessarily stranded (the process
/// that claimed it died before finalize/revert), and there's no in-flight
/// validate to race with. Re-validation is idempotent (`promote_to_file` keys on
/// `lc_id`), so a half-written file self-heals. Returns rows recovered.
pub fn recover_stranded_promoting(conn: &Connection) -> Result<usize> {
    let n = conn.execute(
        "UPDATE learnings SET status = 'pending' WHERE status = 'promoting'",
        [],
    )?;
    Ok(n)
}

/// Reject — CAS on PENDING only. Returns true iff a pending row was rejected
/// (false ⇒ already promoted/promoting/rejected → caller refuses, no
/// reject-after-promote, no validate/reject desync).
pub fn reject(conn: &Connection, id: &str) -> Result<bool> {
    let n = conn.execute(
        "UPDATE learnings SET status = 'rejected' WHERE id = ?1 AND status = 'pending'",
        params![id],
    )?;
    Ok(n == 1)
}

/// Mark stale every pending/validated row not (re)validated since `cutoff`
/// (RFC-3339). Baseline = COALESCE(last_validated_at, created_at) — a pending
/// candidate untouched for 7d gets the nudge too (spec §8). Returns rows touched.
pub fn mark_stale_before(conn: &Connection, cutoff_rfc3339: &str) -> Result<usize> {
    let n = conn.execute(
        "UPDATE learnings SET status = 'stale' \
         WHERE status IN ('pending','validated') \
           AND COALESCE(last_validated_at, created_at) < ?1",
        params![cutoff_rfc3339],
    )?;
    Ok(n)
}

/// Record a rejection by claim hash; returns the new cumulative count
/// (safeguard #6a: caller auto-rejects once it reaches the threshold).
pub fn record_rejection(conn: &Connection, claim_hash: &str, reason: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO learning_rejections (claim_hash, reason, count, last_at) \
         VALUES (?1, ?2, 1, ?3) \
         ON CONFLICT(claim_hash) DO UPDATE SET count = count + 1, reason = ?2, last_at = ?3",
        params![claim_hash, reason, Utc::now().to_rfc3339()],
    )?;
    Ok(conn.query_row(
        "SELECT count FROM learning_rejections WHERE claim_hash = ?1",
        params![claim_hash],
        |r| r.get(0),
    )?)
}

pub fn rejection_count(conn: &Connection, claim_hash: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT count FROM learning_rejections WHERE claim_hash = ?1",
            params![claim_hash],
            |r| r.get(0),
        )
        .unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // FK targets — in prod these tables exist (earlier migrations); the
        // isolated test DB needs minimal stubs so the 063 CREATE resolves them.
        conn.execute_batch(
            "CREATE TABLE projects(id TEXT PRIMARY KEY); \
             CREATE TABLE discussions(id TEXT PRIMARY KEY);",
        )
        .unwrap();
        conn.execute_batch(include_str!("sql/063_continual_learning.sql")).unwrap();
        // Seed the parent rows the samples reference (FK enforcement is on).
        conn.execute_batch(
            "INSERT INTO projects(id) VALUES('p1'); \
             INSERT INTO discussions(id) VALUES('d1');",
        )
        .unwrap();
        conn
    }

    fn sample(id: &str, claim: &str) -> Learning {
        Learning {
            id: id.into(),
            claim: claim.into(),
            evidence: vec![Evidence {
                kind: "file".into(),
                reference: "src/foo.rs:42".into(),
                quote: Some("fn foo() {}".into()),
            }],
            kind: LearningKind::Fact,
            status: LearningStatus::Pending,
            scope: None,
            confidence: Some(0.8),
            faithfulness: None,
            discussion_id: Some("d1".into()),
            project_id: Some("p1".into()),
            source_agent: Some("ClaudeCode".into()),
            promoted_target: None,
            created_at: Utc::now().to_rfc3339(),
            last_validated_at: None,
            validated_by: None,
        }
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let c = mem_db();
        let l = sample("l1", "uses pnpm strict");
        insert(&c, &l).unwrap();
        let got = get(&c, "l1").unwrap().unwrap();
        assert_eq!(got.claim, "uses pnpm strict");
        assert_eq!(got.kind, LearningKind::Fact);
        assert_eq!(got.evidence.len(), 1);
        assert_eq!(got.evidence[0].reference, "src/foo.rs:42");
        assert_eq!(got.confidence, Some(0.8));
    }

    #[test]
    fn insert_rejects_empty_evidence() {
        let c = mem_db();
        let mut l = sample("l2", "no evidence");
        l.evidence.clear();
        assert!(insert(&c, &l).is_err(), "empty evidence[] must be refused");
    }

    #[test]
    fn dedup_unique_index_blocks_same_kind_scope_claim() {
        let c = mem_db();
        insert(&c, &sample("l3", "same claim")).unwrap();
        let mut dup = sample("l4", "same claim"); // same kind (Fact), same scope (None)
        assert!(insert(&c, &dup).is_err(), "dedup unique index must block");
        // different kind → allowed
        dup.kind = LearningKind::Preference;
        assert!(insert(&c, &dup).is_ok());
    }

    #[test]
    fn rejected_row_does_not_block_reproposal() {
        // Partial unique index (WHERE status != 'rejected') — a rejected claim
        // can be re-proposed so negative-learning can accumulate (the dedup no
        // longer pre-empts it).
        let c = mem_db();
        insert(&c, &sample("r1", "same claim")).unwrap();
        assert!(reject(&c, "r1").unwrap(), "pending row rejected");
        assert!(insert(&c, &sample("r2", "same claim")).is_ok(), "rejected must not block re-proposal");
        // ...but two NON-rejected of the same key are still blocked.
        assert!(insert(&c, &sample("r3", "same claim")).is_err(), "two active dups still blocked");
    }

    #[test]
    fn count_pending_and_disc_pending() {
        let c = mem_db();
        insert(&c, &sample("a", "c1")).unwrap();
        insert(&c, &sample("b", "c2")).unwrap();
        assert_eq!(count_pending(&c).unwrap(), 2);
        assert_eq!(disc_pending(&c, "d1").unwrap().len(), 2);
        assert_eq!(disc_pending(&c, "other").unwrap().len(), 0);
    }

    #[test]
    fn two_phase_promotion_claims_then_finalizes() {
        let c = mem_db();
        insert(&c, &sample("v", "validate me")).unwrap();
        assert!(claim_for_promotion(&c, "v").unwrap(), "claim wins on pending");
        assert_eq!(get(&c, "v").unwrap().unwrap().status, LearningStatus::Promoting);
        // a second claim loses (already promoting), and reject is blocked too.
        assert!(!claim_for_promotion(&c, "v").unwrap(), "double-claim loses");
        assert!(!reject(&c, "v").unwrap(), "reject blocked while promoting");
        finalize_promotion(&c, "v", LearningScope::Project, Some("docs/learnings.md"), "human").unwrap();
        let got = get(&c, "v").unwrap().unwrap();
        assert_eq!(got.status, LearningStatus::Promoted);
        assert_eq!(got.scope, Some(LearningScope::Project));
        assert_eq!(got.promoted_target.as_deref(), Some("docs/learnings.md"));
        assert!(got.last_validated_at.is_some());
        assert_eq!(count_pending(&c).unwrap(), 0);
    }

    #[test]
    fn revert_promotion_returns_to_pending() {
        let c = mem_db();
        insert(&c, &sample("rv", "revert me")).unwrap();
        assert!(claim_for_promotion(&c, "rv").unwrap());
        revert_promotion(&c, "rv").unwrap();
        assert_eq!(get(&c, "rv").unwrap().unwrap().status, LearningStatus::Pending);
        // re-claimable after revert (failed-write self-heal).
        assert!(claim_for_promotion(&c, "rv").unwrap());
    }

    #[test]
    fn recover_stranded_promoting_reverts_to_pending() {
        let c = mem_db();
        insert(&c, &sample("s1", "stuck one")).unwrap();
        insert(&c, &sample("s2", "stuck two")).unwrap();
        claim_for_promotion(&c, "s1").unwrap(); // → promoting (simulate crash mid-promotion)
        // s2 stays pending; only the promoting one is recovered.
        assert_eq!(recover_stranded_promoting(&c).unwrap(), 1);
        assert_eq!(get(&c, "s1").unwrap().unwrap().status, LearningStatus::Pending);
        assert_eq!(get(&c, "s2").unwrap().unwrap().status, LearningStatus::Pending);
        // recovered row is re-claimable (self-heal on re-validate).
        assert!(claim_for_promotion(&c, "s1").unwrap());
    }

    #[test]
    fn reject_is_conditional_on_pending() {
        let c = mem_db();
        insert(&c, &sample("rj", "reject me")).unwrap();
        assert!(reject(&c, "rj").unwrap(), "first reject ok");
        assert!(!reject(&c, "rj").unwrap(), "second reject is a no-op (already rejected)");
    }

    #[test]
    fn faithfulness_is_stored_not_blocking() {
        let c = mem_db();
        insert(&c, &sample("f", "loose claim")).unwrap();
        set_faithfulness(&c, "f", Faithfulness::Contradiction).unwrap();
        // still pending — posture B never auto-rejects
        let got = get(&c, "f").unwrap().unwrap();
        assert_eq!(got.faithfulness, Some(Faithfulness::Contradiction));
        assert_eq!(got.status, LearningStatus::Pending);
    }

    #[test]
    fn mark_stale_uses_coalesce_baseline() {
        let c = mem_db();
        let mut old = sample("old", "ancient pending");
        old.created_at = "2000-01-01T00:00:00+00:00".into();
        insert(&c, &old).unwrap();
        let fresh = sample("fresh", "recent pending");
        insert(&c, &fresh).unwrap();
        let touched = mark_stale_before(&c, "2020-01-01T00:00:00+00:00").unwrap();
        assert_eq!(touched, 1, "only the ancient pending goes stale");
        assert_eq!(get(&c, "old").unwrap().unwrap().status, LearningStatus::Stale);
        assert_eq!(get(&c, "fresh").unwrap().unwrap().status, LearningStatus::Pending);
    }

    #[test]
    fn negative_learning_counter_increments() {
        let c = mem_db();
        let h = "hash-of-claim";
        assert_eq!(record_rejection(&c, h, "wrong").unwrap(), 1);
        assert_eq!(record_rejection(&c, h, "wrong again").unwrap(), 2);
        assert_eq!(record_rejection(&c, h, "still wrong").unwrap(), 3);
        assert_eq!(rejection_count(&c, h).unwrap(), 3);
        assert_eq!(rejection_count(&c, "never-seen").unwrap(), 0);
    }
}
