//! 0.11.0 (KT-328) — persistence for CLI worker control offers.
//!
//! A joined CLI worker cannot be launched atomically (an active session owns one
//! discussion; `wait_for_peer` only wakes a session already in the target room).
//! So provisioning a `cli` worker opens a durable OFFER addressed to the exact
//! target session in the origin room; only that session may accept it. This module
//! owns the offer lifecycle (migration 127 `task_execution_worker_offers`):
//! idempotent open, lazy-expiry-at-read, and CAS status transitions. The
//! acceptance handshake (session transfer + final checkpoint) is KT-328 tranche 2;
//! the Phase-E fork that calls [`open_worker_offer`] lives in
//! `crate::api::orchestration`.

use std::str::FromStr;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use crate::db::discussion_sessions::find_active_session;
use crate::models::{TaskExecutionWorkerOffer, WorkerOfferStatus};

const OFFER_COLS: &str = "id, task_execution_id, attempt_no, target_cli_session_id, \
    origin_discussion_id, child_discussion_id, status, expires_at, offer_message_id, \
    reason, accepted_at, declined_at, created_at, updated_at";

/// Parse a stored rfc3339 timestamp STRICTLY. We always write canonical UTC, so a
/// value that fails to parse is a corrupt row — surfaced as a hard conversion error
/// rather than silently defaulted to `now`. A corrupt `expires_at` reading as
/// "expires now" would quietly kill a live offer; corruption must be loud, same
/// severity as the status guard below.
fn parse_dt_strict(col: usize, value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                col,
                rusqlite::types::Type::Text,
                e.to_string().into(),
            )
        })
}

fn parse_opt_dt_strict(
    col: usize,
    value: Option<String>,
) -> rusqlite::Result<Option<DateTime<Utc>>> {
    value.map(|v| parse_dt_strict(col, v)).transpose()
}

/// Parse the stored status strictly: an out-of-domain value is a corrupt row,
/// surfaced as a hard conversion error rather than silently defaulted — an offer
/// whose status is unknown must not read as a live/acceptable one. The migration
/// CHECK makes a bad value unwritable; this is the defense-in-depth guard.
fn row_to_offer(row: &Row) -> rusqlite::Result<TaskExecutionWorkerOffer> {
    let status_raw: String = row.get(6)?;
    let status = WorkerOfferStatus::from_str(&status_raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })?;
    Ok(TaskExecutionWorkerOffer {
        id: row.get(0)?,
        task_execution_id: row.get(1)?,
        attempt_no: row.get::<_, i64>(2)? as u32,
        target_cli_session_id: row.get(3)?,
        origin_discussion_id: row.get(4)?,
        child_discussion_id: row.get(5)?,
        status,
        expires_at: parse_opt_dt_strict(7, row.get(7)?)?,
        offer_message_id: row.get(8)?,
        reason: row.get(9)?,
        accepted_at: parse_opt_dt_strict(10, row.get(10)?)?,
        declined_at: parse_opt_dt_strict(11, row.get(11)?)?,
        created_at: parse_dt_strict(12, row.get(12)?)?,
        updated_at: parse_dt_strict(13, row.get(13)?)?,
    })
}

/// Fields for opening a new offer. The opaque `id` is generated server-side (never
/// caller-supplied) so it cannot be guessed.
pub struct NewWorkerOffer<'a> {
    /// Optional pre-generated opaque id. `None` → generated internally (the ordinary
    /// path). `Some` lets a caller mint the id server-side FIRST, embed it in the
    /// control-offer message, then open the offer with that exact id inside the SAME
    /// atomic tx (KT-319 rework re-offer): the message body and the offer row can never
    /// disagree. Still server-minted (a random UUID), never caller-supplied over the wire.
    pub id: Option<&'a str>,
    pub task_execution_id: &'a str,
    pub attempt_no: u32,
    pub target_cli_session_id: i64,
    pub origin_discussion_id: &'a str,
    pub child_discussion_id: &'a str,
    /// Deadline evaluated at read (lazy expiry). A `DateTime<Utc>`, NOT a string, so
    /// the stored value is always canonical UTC (`+00:00`): the lexicographic
    /// `expires_at < now` compare can never be skewed by a caller-supplied offset
    /// (e.g. `...T09:00:00-05:00` = 14:00 UTC must not sort before `...T13:30:00Z`).
    pub expires_at: Option<DateTime<Utc>>,
    pub offer_message_id: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// Low-level insert of a fresh `pending` offer with a server-generated opaque id.
/// A live offer already present for this attempt or this target session surfaces the
/// partial-unique index as a raw constraint error (the structural backstop) — the
/// caller-facing entry point is [`open_worker_offer`], which detects both clashes
/// explicitly and returns a typed [`OpenOutcome`] instead.
pub fn insert_worker_offer(
    conn: &Connection,
    new: &NewWorkerOffer,
) -> Result<TaskExecutionWorkerOffer> {
    let id = new
        .id
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let now = Utc::now().to_rfc3339();
    let expires_at = new.expires_at.map(|dt| dt.to_rfc3339());
    conn.execute(
        "INSERT INTO task_execution_worker_offers (
             id, task_execution_id, attempt_no, target_cli_session_id,
             origin_discussion_id, child_discussion_id, status, expires_at,
             offer_message_id, reason, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7, ?8, ?9, ?10, ?10)",
        params![
            id,
            new.task_execution_id,
            new.attempt_no as i64,
            new.target_cli_session_id,
            new.origin_discussion_id,
            new.child_discussion_id,
            expires_at,
            new.offer_message_id,
            new.reason,
            now,
        ],
    )?;
    get_worker_offer(conn, &id)?
        .ok_or_else(|| anyhow::anyhow!("worker offer insert did not return a row"))
}

/// The offer by its opaque id.
pub fn get_worker_offer(conn: &Connection, id: &str) -> Result<Option<TaskExecutionWorkerOffer>> {
    let sql =
        format!("SELECT {OFFER_COLS} FROM task_execution_worker_offers WHERE id = ?1 LIMIT 1");
    Ok(conn.query_row(&sql, params![id], row_to_offer).optional()?)
}

/// The live (pending|accepting) offer for an execution attempt, if any. Two racing
/// live offers for one attempt are impossible (partial unique index), so this is at
/// most one row.
pub fn get_active_offer_for_attempt(
    conn: &Connection,
    task_execution_id: &str,
    attempt_no: u32,
) -> Result<Option<TaskExecutionWorkerOffer>> {
    let sql = format!(
        "SELECT {OFFER_COLS} FROM task_execution_worker_offers
          WHERE task_execution_id = ?1 AND attempt_no = ?2
            AND status IN ('pending', 'accepting')
          LIMIT 1"
    );
    Ok(conn
        .query_row(
            &sql,
            params![task_execution_id, attempt_no as i64],
            row_to_offer,
        )
        .optional()?)
}

/// Every offer of an execution, newest first — the auditable trail (re-offers,
/// declines, expiries).
pub fn list_offers_for_execution(
    conn: &Connection,
    task_execution_id: &str,
) -> Result<Vec<TaskExecutionWorkerOffer>> {
    let sql = format!(
        "SELECT {OFFER_COLS} FROM task_execution_worker_offers
          WHERE task_execution_id = ?1
          ORDER BY created_at DESC, id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![task_execution_id], row_to_offer)?;
    let mut offers = Vec::new();
    for offer in rows {
        offers.push(offer?);
    }
    Ok(offers)
}

/// Lazy expiry — evaluated AT READ, no scheduler. A live (pending|accepting) offer is
/// CAS'd to `expired` when it is no longer worth keeping, for EITHER reason:
///  - its deadline has passed (`expires_at < now`, only when a deadline is set); OR
///  - its target CLI session has LEFT the room (`discussion_sessions.status='left'`).
///
/// The session-gone leg is the KT-328 finding fix: in V1 offers carry no deadline, so
/// without it a session that closes/crashes (never accepts) would keep a `pending`
/// offer alive forever — wedging that session's single live-offer slot and keeping the
/// audited FK reference pinned. Only `'left'` triggers it: a `'paused'` session is
/// temporarily away (UI pause) and still reachable, so it must NOT expire the offer.
///
/// Idempotent (a second call on a terminal offer is a no-op). `now` is formatted to
/// canonical UTC here, so the lexicographic `expires_at < now` compare (both canonical
/// UTC) matches the codebase's existing time gating (e.g. dispatch `available_at`).
pub fn expire_offer_if_stale(
    conn: &Connection,
    id: &str,
    now: DateTime<Utc>,
) -> Result<Option<TaskExecutionWorkerOffer>> {
    let now = now.to_rfc3339();
    conn.execute(
        "UPDATE task_execution_worker_offers
            SET status = 'expired',
                updated_at = ?2,
                reason = COALESCE(
                    reason,
                    CASE
                      WHEN expires_at IS NOT NULL AND expires_at < ?2
                        THEN 'offer deadline passed'
                      ELSE 'target CLI session left the room'
                    END)
          WHERE id = ?1
            AND status IN ('pending', 'accepting')
            AND (
                  (expires_at IS NOT NULL AND expires_at < ?2)
               OR EXISTS (
                    SELECT 1 FROM discussion_sessions s
                     WHERE s.id = target_cli_session_id
                       AND s.status = 'left'
                  )
            )",
        params![id, now],
    )?;
    get_worker_offer(conn, id)
}

/// CAS an offer `from → to`, journaling the timestamps that matter (`accepted_at`
/// on accept, `declined_at` on decline) and optionally recording a reason. Returns
/// whether exactly this transition applied — a lost race (status moved beneath us)
/// returns `false` without mutating, so a concurrent double-accept has one winner.
pub fn transition_offer_status(
    conn: &Connection,
    id: &str,
    from: WorkerOfferStatus,
    to: WorkerOfferStatus,
    reason: Option<&str>,
) -> Result<bool> {
    if from == to {
        bail!(
            "worker offer transition requires distinct from/to ({})",
            from.as_str()
        );
    }
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE task_execution_worker_offers
            SET status = ?2,
                updated_at = ?3,
                reason = COALESCE(?4, reason),
                accepted_at = CASE WHEN ?2 = 'accepted' THEN ?3 ELSE accepted_at END,
                declined_at = CASE WHEN ?2 = 'declined' THEN ?3 ELSE declined_at END
          WHERE id = ?1 AND status = ?5",
        params![id, to.as_str(), now, reason, from.as_str()],
    )?;
    Ok(changed > 0)
}

/// Cancel-first (KT-319 DoD-9): CAS every still-live (`pending`|`accepting`) offer of
/// this execution to `cancelled` before a re-offer opens the next attempt. In the
/// realistic review loop the prior attempt's offer is already `accepted` (terminal), so
/// this is a no-op; it is the STRUCTURAL guarantee for the case a prior attempt's offer
/// is still live (a worker that never re-accepted). With no live offer left on the
/// target session, the follow-up `open_worker_offer` can only return `Opened` — it can
/// never report `SessionCommittedElsewhere` pointing the execution at itself, so
/// `OpenOutcome` stays two-variant. Returns how many offers were cancelled. MUST run in
/// the caller's re-offer tx so cancel + open are atomic.
pub fn cancel_live_offers_for_execution(
    conn: &Connection,
    task_execution_id: &str,
) -> Result<usize> {
    let now = Utc::now().to_rfc3339();
    let changed = conn.execute(
        "UPDATE task_execution_worker_offers
            SET status = 'cancelled',
                updated_at = ?2,
                reason = COALESCE(reason, 'superseded by the next review round re-offer')
          WHERE task_execution_id = ?1
            AND status IN ('pending', 'accepting')",
        params![task_execution_id, now],
    )?;
    Ok(changed)
}

/// Record the opaque control-offer message as provenance, once that message exists
/// in the SAME transaction (the `offer_message_id → messages(id)` FK then holds —
/// which is why the offer is inserted first with a NULL provenance, then updated).
/// Idempotent by construction: a resume that reattaches a live offer already carries
/// this id and never re-posts the message.
pub fn set_offer_message(conn: &Connection, offer_id: &str, message_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE task_execution_worker_offers
            SET offer_message_id = ?2, updated_at = ?3
          WHERE id = ?1",
        params![offer_id, message_id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// The business result of [`open_worker_offer`]. A `Result::Err` is a genuine DB
/// fault; `SessionCommittedElsewhere` is an *expected, actionable* state the Phase-E
/// fork turns into a `Blocked(awaiting_worker_acceptance)` execution with a
/// structured reason — never a `Failed` on an opaque `UNIQUE` string.
#[derive(Debug)]
pub enum OpenOutcome {
    /// A fresh or idempotently-reattached live offer.
    Opened(TaskExecutionWorkerOffer),
    /// The target session already holds a live offer for a *different* execution;
    /// `blocking.task_execution_id` names who holds it, for the structured reason.
    SessionCommittedElsewhere { blocking: TaskExecutionWorkerOffer },
}

/// The live (pending|accepting) offer held by `target_cli_session_id`, if any. At
/// most one row (session partial-unique index). Callers reach this only after the
/// idempotent same-(exec,attempt) reattach has been ruled out, so any hit is a
/// genuine "session committed elsewhere" — another execution, or (once KT-319 adds
/// re-offers) a still-live earlier attempt of this execution.
fn active_offer_for_session(
    conn: &Connection,
    target_cli_session_id: i64,
) -> Result<Option<TaskExecutionWorkerOffer>> {
    let sql = format!(
        "SELECT {OFFER_COLS} FROM task_execution_worker_offers
          WHERE target_cli_session_id = ?1
            AND status IN ('pending', 'accepting')
          LIMIT 1"
    );
    Ok(conn
        .query_row(&sql, params![target_cli_session_id], row_to_offer)
        .optional()?)
}

/// Idempotent open (KT-328 Phase-E fork). MUST run inside the caller's provisioning
/// transaction: the read-then-insert below is only atomic within one transaction —
/// two concurrent opens on a bare connection could both miss the live row and race
/// the insert. Called as part of the single provisioning tx (like Phase D/E), a
/// rollback of that tx also drops the offer.
///
/// All within the caller's tx:
///  1. reattach — lazy-expire any stale (past-deadline OR target-session-left) live
///     offer for this attempt; if a live one survives, return `Opened(existing)`
///     (a re-post is a no-op);
///  2. session guard — if the target session holds a live offer for a *different*
///     execution (lazy-expired first), return `SessionCommittedElsewhere` so the
///     caller can Block with a structured reason instead of tripping the raw session
///     UNIQUE index;
///  3. otherwise insert a fresh `pending` offer → `Opened(new)`.
///
/// The two partial-unique indexes remain the structural backstop (defense in depth):
/// the typed outcomes are the ordinary path, the constraints catch anything that
/// slips a pre-check.
pub fn open_worker_offer(conn: &Connection, new: &NewWorkerOffer) -> Result<OpenOutcome> {
    let now = Utc::now();
    // 1. Idempotent reattach for this attempt.
    if let Some(existing) =
        get_active_offer_for_attempt(conn, new.task_execution_id, new.attempt_no)?
    {
        if let Some(offer) = expire_offer_if_stale(conn, &existing.id, now)? {
            if offer.status.is_live() {
                return Ok(OpenOutcome::Opened(offer));
            }
        }
    }
    // 2. Session committed elsewhere? (Any live offer on the target session — the
    //    same-(exec,attempt) reattach was already ruled out in step 1.) Lazy-expire a
    //    stale blocker first (past-deadline OR its session left) so an abandoned
    //    execution's dead offer never wedges the session's slot.
    if let Some(blocker) = active_offer_for_session(conn, new.target_cli_session_id)? {
        if let Some(refreshed) = expire_offer_if_stale(conn, &blocker.id, now)? {
            if refreshed.status.is_live() {
                return Ok(OpenOutcome::SessionCommittedElsewhere {
                    blocking: refreshed,
                });
            }
        }
    }
    // 3. Fresh offer.
    Ok(OpenOutcome::Opened(insert_worker_offer(conn, new)?))
}

/// The business result of [`accept_worker_offer`] (KT-328 tranche 2, commit 1). Every
/// non-`Accepting` variant is a TYPED refusal that leaves acceptance state untouched
/// (beyond the documented read-time lazy-expiry, which is correct for anyone) — a bad or
/// racing caller never accepts on another session's behalf. `Result::Err` is a genuine
/// DB fault only, never an ordinary refusal.
#[derive(Debug)]
// Outcome enum: the success payload travels inline rather than boxed, so the
// nominal path pays no allocation.
#[allow(clippy::large_enum_variant)]
pub enum AcceptOutcome {
    /// The offer's EXACT target session accepted or resumed an already staged
    /// `accepting` offer. The offer is ready for the idempotent durable transfer +
    /// final checkpoint (KT-328 tranche 2, commit 2). Carries the refreshed offer.
    Accepting(TaskExecutionWorkerOffer),
    /// No offer with that opaque id.
    NotFound,
    /// The caller is not the session this offer targets — a different session (SAME
    /// provider included) or an unresolvable / left caller identity. Server-derived: the
    /// caller supplies only its durable `(agent, session)` pair, never a session id, so it
    /// cannot claim another session's offer.
    WrongAcceptor,
    /// The exact target session is live, but the bridge's separate durable binding is
    /// not owned by the offer origin (or, for a resumed transfer, its child). Refuse
    /// before the `pending → accepting` CAS so reconnect/explicit transfer can repair
    /// ownership without leaving a newly staged offer behind.
    BindingMismatch,
    /// The offer is terminal. `status` names the real current state so the exact target
    /// learns why it can no longer start/resume the saga.
    NotAcceptable { status: WorkerOfferStatus },
    /// The offer expired at read — its deadline passed or its target session left the
    /// room — so it is no longer acceptable by anyone.
    Expired,
}

/// Accept a CLI worker control offer by its opaque id, on behalf of the caller's EXACT
/// joined session (KT-328 tranche 2, commit 1). Identity is DERIVED SERVER-SIDE from the
/// live `(source_agent, source_session_id)` pair plus its separate bridge-derived durable
/// binding (DoD-2): the model supplies neither identity, so it cannot accept for another
/// session. The durable binding is checked before `pending → accepting`; an already
/// `accepting` offer is resumable by the same exact session because the following transfer
/// and checkpoint are idempotent. Every non-accept path is a typed [`AcceptOutcome`] that
/// mutates nothing beyond the honest read-time lazy-expiry.
///
/// Runs in one transaction so read → identity/binding checks → CAS cannot be split by a
/// racing accept. A duplicate from the exact same session may observe `accepting` and
/// resume; all downstream phases are idempotent. A different session is still refused
/// before any state-dependent detail is exposed.
pub fn accept_worker_offer(
    conn: &Connection,
    offer_id: &str,
    source_agent: &str,
    source_session_id: &str,
    source_binding_session_id: &str,
) -> Result<AcceptOutcome> {
    let tx = conn.unchecked_transaction()?;
    let outcome = accept_within_tx(
        &tx,
        offer_id,
        source_agent,
        source_session_id,
        source_binding_session_id,
    )?;
    tx.commit()?;
    Ok(outcome)
}

fn accept_within_tx(
    conn: &Connection,
    offer_id: &str,
    source_agent: &str,
    source_session_id: &str,
    source_binding_session_id: &str,
) -> Result<AcceptOutcome> {
    let Some(offer) = get_worker_offer(conn, offer_id)? else {
        return Ok(AcceptOutcome::NotFound);
    };
    // Lazy-expire at read (past-deadline OR target session left). A stale offer is not
    // acceptable by anyone — evaluated BEFORE caller resolution so even the now-left
    // target gets `Expired`, never `WrongAcceptor`.
    let offer = expire_offer_if_stale(conn, &offer.id, Utc::now())?.unwrap_or(offer);
    if offer.status == WorkerOfferStatus::Expired {
        return Ok(AcceptOutcome::Expired);
    }
    // Server-derived identity: resolve the caller's exact active session; it must BE the
    // offer's target. A different session (same provider included) or an unresolvable /
    // left caller is a `WrongAcceptor` — no mutation.
    let caller = find_active_session(conn, source_agent, source_session_id)?;
    let is_target = caller
        .as_ref()
        .is_some_and(|session| session.id == offer.target_cli_session_id);
    if !is_target {
        return Ok(AcceptOutcome::WrongAcceptor);
    }

    // The live session and the reload-stable source binding are deliberately separate
    // identities. Verify ownership before staging: otherwise a stale/rejoined bridge can
    // commit `pending → accepting`, fail the later transfer, and strand the offer. During
    // a resumed `accepting` saga either side is valid because phase 2 may already have
    // moved the binding before a crash.
    if matches!(
        offer.status,
        WorkerOfferStatus::Pending | WorkerOfferStatus::Accepting
    ) {
        let bound_disc = crate::db::disc_source::find_disc_by_source_session(
            conn,
            source_agent,
            source_binding_session_id,
        )?;
        let binding_ready = match offer.status {
            WorkerOfferStatus::Pending => {
                bound_disc.as_deref() == Some(offer.origin_discussion_id.as_str())
            }
            WorkerOfferStatus::Accepting => bound_disc.as_deref().is_some_and(|disc_id| {
                disc_id == offer.origin_discussion_id.as_str()
                    || disc_id == offer.child_discussion_id.as_str()
            }),
            _ => false,
        };
        if !binding_ready {
            return Ok(AcceptOutcome::BindingMismatch);
        }
    }

    // A same-session retry from `accepting` resumes the saga. Concurrent duplicate calls
    // may therefore both enter the following phases, whose binding move and checkpoint
    // are explicitly idempotent; refusing the retry would make a crash permanent.
    if offer.status == WorkerOfferStatus::Accepting {
        return Ok(AcceptOutcome::Accepting(offer));
    }

    // CAS pending → accepting. Terminal states remain typed refusals.
    if transition_offer_status(
        conn,
        &offer.id,
        WorkerOfferStatus::Pending,
        WorkerOfferStatus::Accepting,
        None,
    )? {
        let refreshed = get_worker_offer(conn, &offer.id)?
            .ok_or_else(|| anyhow::anyhow!("worker offer vanished after accept CAS"))?;
        Ok(AcceptOutcome::Accepting(refreshed))
    } else {
        let current = get_worker_offer(conn, &offer.id)?
            .ok_or_else(|| anyhow::anyhow!("worker offer vanished during accept"))?;
        Ok(AcceptOutcome::NotAcceptable {
            status: current.status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::orchestration::{launch_single_task, set_execution_sub_discussion};
    use crate::models::{LaunchSingleTaskInput, OrchestrationActor, PlanningActorKind};
    use rusqlite::Connection;

    const PARENT: &str = "d-parent";
    const CHILD: &str = "d-child";
    const SESSION_PK: i64 = 1;

    /// A minimal migrated DB with one project, a parent + child discussion, a joined
    /// CLI session and a launched execution — the FK chain a real offer needs.
    struct Fixture {
        conn: Connection,
        exec_id: String,
    }

    fn backend_actor() -> OrchestrationActor {
        OrchestrationActor {
            kind: PlanningActorKind::Backend,
            id: Some("test".into()),
            session_id: None,
            source_message_id: None,
        }
    }

    fn base_conn(session_id: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO projects (id, name, path, created_at, updated_at) \
             VALUES ('p1', 'P', '/p', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO discussions (id, title, created_at, updated_at) \
             VALUES (?1, 'Parent', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            params![PARENT],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO discussions (id, title, created_at, updated_at) \
             VALUES (?1, 'Child', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            params![CHILD],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (?1, ?2, 'ClaudeCode', ?3, 'peer', 'active', '2026-01-01T00:00:00Z')",
            params![SESSION_PK, PARENT, session_id],
        )
        .unwrap();
        crate::db::disc_source::bind_to_source(&conn, PARENT, "ClaudeCode", session_id).unwrap();
        conn
    }

    fn seed_task(conn: &Connection, task_id: &str, number: i64) {
        conn.execute(
            "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
             VALUES (?1, ?2, 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            params![task_id, number],
        )
        .unwrap();
    }

    fn launch(conn: &Connection, task_id: &str) -> String {
        let exec_id = launch_single_task(
            conn,
            &LaunchSingleTaskInput::new(task_id, PARENT),
            &backend_actor(),
        )
        .unwrap()
        .execution
        .id;
        set_execution_sub_discussion(conn, &exec_id, CHILD).unwrap();
        exec_id
    }

    fn seed(session_id: &str) -> Fixture {
        let conn = base_conn(session_id);
        seed_task(&conn, "t1", 1);
        let exec_id = launch(&conn, "t1");
        Fixture { conn, exec_id }
    }

    fn new_offer(fx: &Fixture, expires_at: Option<DateTime<Utc>>) -> NewWorkerOffer<'_> {
        NewWorkerOffer {
            id: None,
            task_execution_id: &fx.exec_id,
            attempt_no: 0,
            target_cli_session_id: SESSION_PK,
            origin_discussion_id: PARENT,
            child_discussion_id: CHILD,
            expires_at,
            offer_message_id: None,
            reason: None,
        }
    }

    fn dt(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn opened(outcome: OpenOutcome) -> TaskExecutionWorkerOffer {
        match outcome {
            OpenOutcome::Opened(offer) => offer,
            other => panic!("expected Opened, got {other:?}"),
        }
    }

    #[test]
    fn open_is_idempotent_per_attempt() {
        let fx = seed("s-1");
        let first = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        let second = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        // Re-post reattaches the SAME live offer, not a second row.
        assert_eq!(first.id, second.id);
        assert_eq!(first.status, WorkerOfferStatus::Pending);
        assert_eq!(
            list_offers_for_execution(&fx.conn, &fx.exec_id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn deleting_a_room_holding_the_target_session_cascades_the_offer_and_never_wedges() {
        // Proven scenario: the offer's origin+child rooms stay ALIVE, but its target
        // CLI session has moved into a THIRD room (a post-acceptance transfer, then
        // reused elsewhere by the human). Deleting that third room hard-deletes the
        // session (discussion_sessions.disc_id ON DELETE CASCADE). Under the old
        // RESTRICT this failed with a raw FK error and the room became undeletable;
        // under ON DELETE CASCADE the now-meaningless offer is swept and the delete
        // succeeds. Differential proof: run this test on the pre-fix schema and it
        // fails on the delete with a FOREIGN KEY constraint.
        let fx = seed("s-moved");
        let offer = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        // A terminal (accepted) offer, exactly the "historical offer still pointing
        // at the session" the reviewer described.
        assert!(transition_offer_status(
            &fx.conn,
            &offer.id,
            WorkerOfferStatus::Pending,
            WorkerOfferStatus::Accepted,
            None,
        )
        .unwrap());

        // A THIRD room, and the target session now lives there — NOT in origin/child.
        fx.conn
            .execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('d-third', 'Third', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        fx.conn
            .execute(
                "UPDATE discussion_sessions SET disc_id = 'd-third' WHERE id = ?1",
                params![SESSION_PK],
            )
            .unwrap();

        // Deleting the third room must SUCCEED (no wedged room, no raw FK error) ...
        let deleted = crate::db::discussions::delete_discussion(&fx.conn, "d-third").unwrap();
        assert!(deleted, "the third room must be deletable");
        // ... the referenced session was cascade-deleted with its room ...
        let sessions: i64 = fx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM discussion_sessions WHERE id = ?1",
                params![SESSION_PK],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sessions, 0, "target session should be gone");
        // ... the now-meaningless offer was swept with it ...
        assert!(
            get_worker_offer(&fx.conn, &offer.id).unwrap().is_none(),
            "offer must cascade with its target session"
        );
        // ... and the offer's origin room is untouched.
        let origin: i64 = fx
            .conn
            .query_row(
                "SELECT COUNT(*) FROM discussions WHERE id = ?1",
                params![PARENT],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(origin, 1, "origin room must remain");
    }

    #[test]
    fn open_reports_session_committed_elsewhere() {
        // The ordinary "my CLI is already busy on another task" case must come back
        // as a TYPED outcome the Phase-E fork can Block on — not a raw UNIQUE error.
        let fx = seed("s-2");
        let first = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        // A DIFFERENT execution targeting the SAME session.
        seed_task(&fx.conn, "t2", 2);
        let other_exec = launch(&fx.conn, "t2");
        let clash = NewWorkerOffer {
            id: None,
            task_execution_id: &other_exec,
            attempt_no: 0,
            target_cli_session_id: SESSION_PK,
            origin_discussion_id: PARENT,
            child_discussion_id: CHILD,
            expires_at: None,
            offer_message_id: None,
            reason: None,
        };
        match open_worker_offer(&fx.conn, &clash).unwrap() {
            OpenOutcome::SessionCommittedElsewhere { blocking } => {
                // The typed payload names exactly who holds the session.
                assert_eq!(blocking.id, first.id);
                assert_eq!(blocking.task_execution_id, fx.exec_id);
            }
            other => panic!("expected SessionCommittedElsewhere, got {other:?}"),
        }
        // And no orphan row was inserted for the second execution.
        assert!(list_offers_for_execution(&fx.conn, &other_exec)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn raw_insert_enforces_session_uniqueness_as_backstop() {
        // Bypassing open_worker_offer's typed guard, the partial-unique index is the
        // structural backstop — and specifically a CONSTRAINT violation, not any error.
        let fx = seed("s-2b");
        insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        seed_task(&fx.conn, "t2", 2);
        let other_exec = launch(&fx.conn, "t2");
        let clash = NewWorkerOffer {
            id: None,
            task_execution_id: &other_exec,
            attempt_no: 0,
            target_cli_session_id: SESSION_PK,
            origin_discussion_id: PARENT,
            child_discussion_id: CHILD,
            expires_at: None,
            offer_message_id: None,
            reason: None,
        };
        let err = insert_worker_offer(&fx.conn, &clash).unwrap_err();
        let sqlite = err
            .downcast_ref::<rusqlite::Error>()
            .expect("clash should surface a rusqlite error");
        assert!(
            matches!(
                sqlite,
                rusqlite::Error::SqliteFailure(e, _)
                    if e.code == rusqlite::ErrorCode::ConstraintViolation
            ),
            "expected a UNIQUE constraint backstop, got {sqlite:?}"
        );
    }

    #[test]
    fn open_is_undone_by_transaction_rollback() {
        // The open must be a rollback-safe part of the caller's provisioning tx: if
        // the tx aborts, the offer leaves no orphan row.
        let mut fx = seed("s-7");
        let exec_id = fx.exec_id.clone();
        {
            let tx = fx.conn.transaction().unwrap();
            let outcome = open_worker_offer(
                &tx,
                &NewWorkerOffer {
                    id: None,
                    task_execution_id: &exec_id,
                    attempt_no: 0,
                    target_cli_session_id: SESSION_PK,
                    origin_discussion_id: PARENT,
                    child_discussion_id: CHILD,
                    expires_at: None,
                    offer_message_id: None,
                    reason: None,
                },
            )
            .unwrap();
            assert!(matches!(outcome, OpenOutcome::Opened(_)));
            // tx dropped without commit → rollback.
        }
        assert!(list_offers_for_execution(&fx.conn, &exec_id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn lazy_expiry_flips_a_past_deadline_offer_at_read() {
        let fx = seed("s-3");
        // A past deadline in a NON-UTC offset: the DateTime input normalizes it to
        // canonical UTC, so the lexicographic compare is correct regardless.
        let past = dt("2000-01-01T00:00:00-05:00");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, Some(past))).unwrap();
        assert_eq!(offer.status, WorkerOfferStatus::Pending);
        let after = expire_offer_if_stale(&fx.conn, &offer.id, Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(after.status, WorkerOfferStatus::Expired);
        // Idempotent: a second read does not re-touch a terminal offer.
        let again = expire_offer_if_stale(&fx.conn, &offer.id, Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(again.status, WorkerOfferStatus::Expired);
        // And a fresh open after the deadline opens a NEW pending offer.
        let reopened = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        assert_ne!(reopened.id, offer.id);
        assert_eq!(reopened.status, WorkerOfferStatus::Pending);
    }

    #[test]
    fn future_deadline_is_not_expired() {
        let fx = seed("s-4");
        // A future deadline whose LEXICOGRAPHIC string would sort "early" if its
        // offset leaked (`...-14:00`), proving the DateTime normalization defends it.
        let future = dt("2099-01-01T00:00:00-14:00");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, Some(future))).unwrap();
        let after = expire_offer_if_stale(&fx.conn, &offer.id, Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(after.status, WorkerOfferStatus::Pending);
    }

    #[test]
    fn offer_expires_at_read_when_target_session_leaves() {
        // KT-328 finding: a CLI worker that never accepts — terminal closed, human
        // gone, crash — must NOT wedge its target session's single live-offer slot
        // forever. In V1 offers carry no deadline, so the release must key off the real
        // condition: the target session left the room. NO deadline is set here.
        let fx = seed("s-left");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        assert_eq!(offer.status, WorkerOfferStatus::Pending);
        assert!(offer.expires_at.is_none());
        // While the session is active, the offer holds the session's live-offer slot.
        assert!(active_offer_for_session(&fx.conn, SESSION_PK)
            .unwrap()
            .is_some());
        // The target session leaves the room (soft-close).
        fx.conn
            .execute(
                "UPDATE discussion_sessions SET status = 'left' WHERE id = ?1",
                params![SESSION_PK],
            )
            .unwrap();
        // At read the offer expires — with NO deadline — and records the reason.
        let after = expire_offer_if_stale(&fx.conn, &offer.id, Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(after.status, WorkerOfferStatus::Expired);
        assert_eq!(
            after.reason.as_deref(),
            Some("target CLI session left the room")
        );
        // The session's live-offer slot is now released: a fresh open for a DIFFERENT
        // execution on the same session no longer reports SessionCommittedElsewhere.
        assert!(active_offer_for_session(&fx.conn, SESSION_PK)
            .unwrap()
            .is_none());
    }

    #[test]
    fn paused_target_session_does_not_expire_the_offer() {
        // A 'paused' session is temporarily away (UI pause) and still reachable — only
        // a 'left' session releases the offer. Guards against over-expiring on the
        // broader `status != 'active'` predicate.
        let fx = seed("s-paused");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        fx.conn
            .execute(
                "UPDATE discussion_sessions SET status = 'paused' WHERE id = ?1",
                params![SESSION_PK],
            )
            .unwrap();
        let after = expire_offer_if_stale(&fx.conn, &offer.id, Utc::now())
            .unwrap()
            .unwrap();
        assert_eq!(after.status, WorkerOfferStatus::Pending);
    }

    #[test]
    fn cas_transition_has_one_winner() {
        let fx = seed("s-5");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        // pending → accepting wins once; a second attempt from `pending` loses.
        assert!(transition_offer_status(
            &fx.conn,
            &offer.id,
            WorkerOfferStatus::Pending,
            WorkerOfferStatus::Accepting,
            None
        )
        .unwrap());
        assert!(!transition_offer_status(
            &fx.conn,
            &offer.id,
            WorkerOfferStatus::Pending,
            WorkerOfferStatus::Accepting,
            None
        )
        .unwrap());
        // accepting → accepted stamps accepted_at.
        assert!(transition_offer_status(
            &fx.conn,
            &offer.id,
            WorkerOfferStatus::Accepting,
            WorkerOfferStatus::Accepted,
            None
        )
        .unwrap());
        let done = get_worker_offer(&fx.conn, &offer.id).unwrap().unwrap();
        assert_eq!(done.status, WorkerOfferStatus::Accepted);
        assert!(done.accepted_at.is_some());
    }

    #[test]
    fn corrupt_status_surfaces_instead_of_defaulting() {
        let fx = seed("s-6");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        // Bypass the CHECK to simulate a downgraded/externally-corrupted row.
        fx.conn
            .execute("PRAGMA ignore_check_constraints = ON", [])
            .unwrap();
        fx.conn
            .execute(
                "UPDATE task_execution_worker_offers SET status = 'martian' WHERE id = ?1",
                params![offer.id],
            )
            .unwrap();
        assert!(get_worker_offer(&fx.conn, &offer.id).is_err());
    }

    #[test]
    fn corrupt_expires_at_surfaces_instead_of_defaulting() {
        // A corrupt timestamp must not read as "now" (which would silently kill a
        // live offer); it surfaces as a hard row error — same severity as status.
        let fx = seed("s-9");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        fx.conn
            .execute(
                "UPDATE task_execution_worker_offers SET expires_at = 'not-a-date' WHERE id = ?1",
                params![offer.id],
            )
            .unwrap();
        assert!(get_worker_offer(&fx.conn, &offer.id).is_err());
    }

    // ---- KT-328 tranche 2, commit 1: acceptance (server-derived identity) ----

    const SECOND_SESSION_PK: i64 = 2;

    /// A second joined CLI session of the SAME provider (ClaudeCode) in the origin room,
    /// so "two CLIs, same provider — only the target may accept" is exercisable.
    fn seed_second_session(conn: &Connection, session_id: &str) {
        conn.execute(
            "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
             VALUES (?1, ?2, 'ClaudeCode', ?3, 'peer', 'active', '2026-01-01T00:00:00Z')",
            params![SECOND_SESSION_PK, PARENT, session_id],
        )
        .unwrap();
    }

    fn accepting(outcome: AcceptOutcome) -> TaskExecutionWorkerOffer {
        match outcome {
            AcceptOutcome::Accepting(offer) => offer,
            other => panic!("expected Accepting, got {other:?}"),
        }
    }

    #[test]
    fn accept_by_the_target_session_wins_and_stages_accepting() {
        let fx = seed("s-accept");
        let offer = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        // The caller supplies ONLY its durable (agent, session) pair — never the PK. The
        // backend derives its session and confirms it IS the offer's target.
        let staged = accepting(
            accept_worker_offer(&fx.conn, &offer.id, "ClaudeCode", "s-accept", "s-accept").unwrap(),
        );
        assert_eq!(staged.id, offer.id);
        assert_eq!(staged.status, WorkerOfferStatus::Accepting);
        // `accepting` is a staging state, not `accepted`: accepted_at stays None until the
        // final checkpoint (commit 2) — no external effect yet.
        assert!(staged.accepted_at.is_none());
        assert_eq!(
            get_worker_offer(&fx.conn, &offer.id)
                .unwrap()
                .unwrap()
                .status,
            WorkerOfferStatus::Accepting
        );
    }

    #[test]
    fn accept_by_a_different_same_provider_session_is_wrong_acceptor() {
        // Two ClaudeCode CLIs joined: the offer targets session 1; session 2 (SAME
        // provider, different session_id) must NOT be able to accept it.
        let fx = seed("s-target");
        seed_second_session(&fx.conn, "s-other");
        let offer = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        let out =
            accept_worker_offer(&fx.conn, &offer.id, "ClaudeCode", "s-other", "s-target").unwrap();
        assert!(matches!(out, AcceptOutcome::WrongAcceptor), "got {out:?}");
        // No mutation: still pending, and the REAL target can still accept.
        assert_eq!(
            get_worker_offer(&fx.conn, &offer.id)
                .unwrap()
                .unwrap()
                .status,
            WorkerOfferStatus::Pending
        );
        assert!(matches!(
            accept_worker_offer(&fx.conn, &offer.id, "ClaudeCode", "s-target", "s-target",)
                .unwrap(),
            AcceptOutcome::Accepting(_)
        ));
    }

    #[test]
    fn stale_durable_binding_is_refused_before_pending_offer_is_staged() {
        let fx = seed("live-target");
        let offer = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());

        // The live target session is still parked in PARENT, but its bridge-derived
        // reload-stable identity belongs elsewhere. This is the real KT-424 failure:
        // the old code committed `accepting` before discovering the transfer mismatch.
        crate::db::disc_source::transfer_source_binding(
            &fx.conn,
            PARENT,
            CHILD,
            "ClaudeCode",
            "live-target",
        )
        .unwrap();

        let outcome = accept_worker_offer(
            &fx.conn,
            &offer.id,
            "ClaudeCode",
            "live-target",
            "live-target",
        )
        .unwrap();
        assert!(
            matches!(outcome, AcceptOutcome::BindingMismatch),
            "stale binding must be actionable without staging, got {outcome:?}"
        );
        assert_eq!(
            get_worker_offer(&fx.conn, &offer.id)
                .unwrap()
                .unwrap()
                .status,
            WorkerOfferStatus::Pending,
            "binding mismatch must not consume the resumable pending state"
        );
    }

    #[test]
    fn accept_of_an_unresolvable_caller_is_wrong_acceptor_not_error() {
        // A caller identity that resolves to no active session is a typed WrongAcceptor,
        // never a DB error and never a silent success — no faking the target.
        let fx = seed("s-target2");
        let offer = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        let out = accept_worker_offer(
            &fx.conn,
            &offer.id,
            "ClaudeCode",
            "ghost-session",
            "s-target2",
        )
        .unwrap();
        assert!(matches!(out, AcceptOutcome::WrongAcceptor), "got {out:?}");
        assert_eq!(
            get_worker_offer(&fx.conn, &offer.id)
                .unwrap()
                .unwrap()
                .status,
            WorkerOfferStatus::Pending
        );
    }

    #[test]
    fn accepting_offer_resumes_for_the_exact_session_while_terminal_state_is_reported() {
        let fx = seed("s-busy");
        let offer = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        // A prior accept already moved it to `accepting`. The next call from the SAME
        // exact target resumes the idempotent saga instead of wedging forever.
        assert!(transition_offer_status(
            &fx.conn,
            &offer.id,
            WorkerOfferStatus::Pending,
            WorkerOfferStatus::Accepting,
            None,
        )
        .unwrap());
        assert!(matches!(
            accept_worker_offer(&fx.conn, &offer.id, "ClaudeCode", "s-busy", "s-busy",).unwrap(),
            AcceptOutcome::Accepting(_)
        ));
        // A settled (accepted) offer also reports its real state.
        assert!(transition_offer_status(
            &fx.conn,
            &offer.id,
            WorkerOfferStatus::Accepting,
            WorkerOfferStatus::Accepted,
            None,
        )
        .unwrap());
        match accept_worker_offer(&fx.conn, &offer.id, "ClaudeCode", "s-busy", "s-busy").unwrap() {
            AcceptOutcome::NotAcceptable { status } => {
                assert_eq!(status, WorkerOfferStatus::Accepted);
            }
            other => panic!("expected NotAcceptable(accepted), got {other:?}"),
        }
    }

    #[test]
    fn accept_of_a_stale_offer_expires_and_reports_expired() {
        // Target session left before accepting: the offer lazy-expires AT READ and accept
        // returns Expired — evaluated before caller resolution, so even the now-left
        // target gets Expired, never WrongAcceptor.
        let fx = seed("s-leaver");
        let offer = insert_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap();
        fx.conn
            .execute(
                "UPDATE discussion_sessions SET status = 'left' WHERE id = ?1",
                params![SESSION_PK],
            )
            .unwrap();
        let out =
            accept_worker_offer(&fx.conn, &offer.id, "ClaudeCode", "s-leaver", "s-leaver").unwrap();
        assert!(matches!(out, AcceptOutcome::Expired), "got {out:?}");
        assert_eq!(
            get_worker_offer(&fx.conn, &offer.id)
                .unwrap()
                .unwrap()
                .status,
            WorkerOfferStatus::Expired
        );
    }

    #[test]
    fn accept_of_an_unknown_offer_is_not_found() {
        let fx = seed("s-unknown");
        let out = accept_worker_offer(
            &fx.conn,
            "no-such-offer",
            "ClaudeCode",
            "s-unknown",
            "s-unknown",
        )
        .unwrap();
        assert!(matches!(out, AcceptOutcome::NotFound), "got {out:?}");
    }

    /// Cancel-first (KT-319 DoD-9): cancels every LIVE offer of an execution and leaves terminal
    /// ones untouched — the structural guarantee that a re-offer for the next attempt can only
    /// `Opened`, because no live offer of this execution survives to trip the session's
    /// live-offer uniqueness.
    #[test]
    fn cancel_live_offers_cancels_live_and_leaves_terminal() {
        let fx = seed("s-cancel");
        // Attempt 0: open, then settle to `accepted` (a terminal offer that must survive).
        let a0 = opened(open_worker_offer(&fx.conn, &new_offer(&fx, None)).unwrap());
        transition_offer_status(
            &fx.conn,
            &a0.id,
            WorkerOfferStatus::Pending,
            WorkerOfferStatus::Accepted,
            None,
        )
        .unwrap();
        // Attempt 1: a still-live (pending) offer on the same session (legal now a0 is terminal).
        let mut n1 = new_offer(&fx, None);
        n1.attempt_no = 1;
        let a1 = opened(open_worker_offer(&fx.conn, &n1).unwrap());

        let n = cancel_live_offers_for_execution(&fx.conn, &fx.exec_id).unwrap();
        assert_eq!(n, 1, "exactly the one live offer was cancelled");
        assert_eq!(
            get_worker_offer(&fx.conn, &a1.id).unwrap().unwrap().status,
            WorkerOfferStatus::Cancelled,
            "the live offer is cancelled"
        );
        assert_eq!(
            get_worker_offer(&fx.conn, &a0.id).unwrap().unwrap().status,
            WorkerOfferStatus::Accepted,
            "the terminal (accepted) offer is untouched"
        );
        // Idempotent: a second sweep finds nothing live.
        assert_eq!(
            cancel_live_offers_for_execution(&fx.conn, &fx.exec_id).unwrap(),
            0
        );
    }
}
