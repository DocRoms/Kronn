//! Origin-agnostic launch state machine shared by discussion-authored
//! (`discussion_actions`, KT-476) and Live-Page-authored (`live_page_actions`,
//! KT-538) Kronn action proposals.
//!
//! One CAS/claim, refresh and completion implementation backs both origins
//! so the launch guarantees — atomic claim, the 5-minute fail-closed window
//! for a launch interrupted before a run id was published, plaintext
//! scrubbing of manually supplied overrides, and the provenance
//! overridability rule — cannot drift between the two tables. Each origin
//! module owns its own row shape (the message/page anchor columns) and
//! converts the shared slice into/out of `ActionCore` around these calls;
//! see `discussion_actions.rs` and `live_page_actions.rs` for the thin
//! adapters.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

use super::discussion_actions::{
    DiscussionActionState, DiscussionActionValue, DiscussionActionValueProvenance,
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum ActionTable {
    Discussion,
    /// KT-678 — a Page's declaration row is an offer and never carries
    /// execution state; the state machine below only ever runs against one of
    /// its launches. A discussion keeps both roles on one row because a fence
    /// is posted once, in one message, and carries one intention.
    LivePageLaunch,
}

impl ActionTable {
    fn name(self) -> &'static str {
        match self {
            Self::Discussion => "discussion_actions",
            Self::LivePageLaunch => "live_page_action_launches",
        }
    }
}

/// The slice of an action row the shared launch state machine reads and
/// mutates, independent of whether the row is anchored to a discussion
/// message or a Live Page action block.
#[derive(Debug, Clone)]
pub(crate) struct ActionCore {
    pub(crate) id: String,
    pub(crate) state: DiscussionActionState,
    pub(crate) values: Vec<DiscussionActionValue>,
    pub(crate) shared_run_id: Option<String>,
    /// The discussion a Quick Prompt launch opened. It has no shared run: its
    /// execution is that discussion's first agent turn.
    pub(crate) result_discussion_id: Option<String>,
    pub(crate) diagnostic: Option<String>,
    pub(crate) launched_at: Option<String>,
    pub(crate) finished_at: Option<String>,
    pub(crate) updated_at: String,
}

pub struct ActionCompletion {
    pub state: DiscussionActionState,
    pub shared_run_id: Option<String>,
    pub result_discussion_id: Option<String>,
    pub deep_link: Option<String>,
    pub diagnostic: Option<String>,
}

pub(crate) fn state_db_str(state: DiscussionActionState) -> &'static str {
    match state {
        DiscussionActionState::Proposed => "proposed",
        DiscussionActionState::Launching => "launching",
        DiscussionActionState::Running => "running",
        DiscussionActionState::Succeeded => "succeeded",
        DiscussionActionState::Failed => "failed",
        DiscussionActionState::Cancelled => "cancelled",
        DiscussionActionState::PreflightFailed => "preflight_failed",
    }
}

/// Whether a reconciliation may write what it worked out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconcile {
    /// Project in memory only. The caller holds the read-only companion
    /// connection (ADR-001 O2), which refuses writes by its file handle.
    Projected,
    /// Persist, because the caller holds the write connection.
    Persisted,
}

/// Brings an action's state up to date with the run behind it.
///
/// Reconciling and persisting used to be the same act, and `list`/`get` called
/// it through the read-only connection. Before any CTA had been launched every
/// row was `proposed`, so this returned early, nothing was written and the
/// endpoint answered — which is why it looked healthy. From the first launch
/// onwards each listing attempted the UPDATE and was refused with
/// `Unable to list Page actions: attempt to write a readonly database`.
///
/// The page then stayed broken for everyone, and self-sustainingly so: the
/// reconciliation that would have moved the state on is exactly what was
/// refused. Measured on a live instance — a run that succeeded in 2.3 s left
/// its action stuck `running`, and the page down, for 2 h 39.
///
/// The logic stays in one place; only the writing is now the caller's to grant.
/// A projected read still returns the reconciled truth.
///
/// What runs behind a launch depends on its target: a shared run for a Quick
/// API, Quick Exec or workflow, the first agent turn of the result discussion
/// for a Quick Prompt. A launch with neither is either still being claimed or
/// has lost what it produced.
pub(crate) fn reconcile(
    conn: &Connection,
    table: ActionTable,
    core: &mut ActionCore,
    mode: Reconcile,
) -> Result<()> {
    if !matches!(
        core.state,
        DiscussionActionState::Launching | DiscussionActionState::Running
    ) {
        return Ok(());
    }
    if core.shared_run_id.is_none() {
        if let Some(discussion_id) = core.result_discussion_id.clone() {
            return reconcile_from_agent_turn(conn, table, core, mode, &discussion_id);
        }
        if core.state == DiscussionActionState::Running {
            // Running means a completion recorded what it produced; both links
            // are `ON DELETE SET NULL`, so nothing left means it was deleted.
            let diagnostic = "Ce que ce lancement avait produit n’existe plus.".to_string();
            return settle(
                conn,
                table,
                core,
                mode,
                DiscussionActionState::Failed,
                Some(diagnostic),
                None,
            );
        }
    }
    let Some(run_id) = core.shared_run_id.clone() else {
        // The launch claim is the idempotency boundary. If the backend stops
        // after claiming but before publishing a durable run/discussion id,
        // never retry the external action automatically: that could
        // duplicate side effects. Surface a bounded, actionable terminal
        // state instead.
        let stale = core
            .launched_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|launched| {
                Utc::now()
                    .signed_duration_since(launched.with_timezone(&Utc))
                    .num_minutes()
                    >= 5
            });
        if core.state == DiscussionActionState::Launching && stale {
            let now = Utc::now().to_rfc3339();
            let diagnostic = "The launch was interrupted before Kronn published its run. It was not retried automatically to avoid duplicate side effects.".to_string();
            if mode == Reconcile::Persisted {
                conn.execute(
                    &format!(
                        "UPDATE {} SET state = 'failed', diagnostic = ?2,
                         finished_at = ?3, updated_at = ?3 WHERE id = ?1 AND state = 'launching'",
                        table.name()
                    ),
                    params![core.id, diagnostic, now],
                )?;
            }
            core.state = DiscussionActionState::Failed;
            core.diagnostic = Some(diagnostic);
            core.finished_at = Some(now.clone());
            core.updated_at = now;
        }
        return Ok(());
    };
    let Some(run) = crate::db::shared_runs::lifecycle(conn, &run_id)? else {
        return Ok(());
    };
    use crate::models::SharedRunStatus;
    let next = match run.status {
        SharedRunStatus::Queued | SharedRunStatus::Running => DiscussionActionState::Running,
        SharedRunStatus::Success => DiscussionActionState::Succeeded,
        SharedRunStatus::Failed | SharedRunStatus::Timeout => DiscussionActionState::Failed,
        SharedRunStatus::Cancelled => DiscussionActionState::Cancelled,
        SharedRunStatus::PreflightFailed => DiscussionActionState::PreflightFailed,
    };
    if next == core.state && run.diagnostic == core.diagnostic {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    let finished_at = (!matches!(next, DiscussionActionState::Running))
        .then(|| run.finished_at.unwrap_or_else(Utc::now).to_rfc3339());
    if mode == Reconcile::Persisted {
        conn.execute(
            &format!(
                "UPDATE {} SET state = ?2, diagnostic = ?3,
                 finished_at = ?4, updated_at = ?5 WHERE id = ?1",
                table.name()
            ),
            params![
                core.id,
                state_db_str(next),
                run.diagnostic,
                finished_at,
                now
            ],
        )?;
    }
    core.state = next;
    core.diagnostic = run.diagnostic;
    core.finished_at = finished_at;
    core.updated_at = now;
    Ok(())
}

/// A Quick Prompt launch has succeeded when its agent has answered, not when
/// its discussion was created: the card and the Page button used to say
/// "done" while the agent was still reading the ticket, and kept saying it if
/// the agent then failed. The launch's own turn is the discussion's first; the
/// ones a human asks for afterwards are that discussion's business.
fn reconcile_from_agent_turn(
    conn: &Connection,
    table: ActionTable,
    core: &mut ActionCore,
    mode: Reconcile,
    discussion_id: &str,
) -> Result<()> {
    let turn: Option<(String, Option<String>, String)> = conn
        .query_row(
            "SELECT status, last_error, COALESCE(completed_at, updated_at)
             FROM agent_dispatch_jobs WHERE discussion_id = ?1
             ORDER BY created_at ASC, id ASC LIMIT 1",
            params![discussion_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    // Not queued yet: the launch is still starting, nothing to settle.
    let Some((status, error, at)) = turn else {
        return Ok(());
    };
    use crate::db::agent_dispatch::DispatchStatus;
    let (next, diagnostic) = match DispatchStatus::parse(&status)? {
        DispatchStatus::Pending | DispatchStatus::Running => (DiscussionActionState::Running, None),
        DispatchStatus::Completed => (DiscussionActionState::Succeeded, None),
        DispatchStatus::Failed => (
            DiscussionActionState::Failed,
            Some(error.unwrap_or_else(|| "L’agent n’a pas pu répondre.".into())),
        ),
        DispatchStatus::Cancelled => (DiscussionActionState::Cancelled, None),
    };
    let finished_at = (next != DiscussionActionState::Running).then_some(at);
    settle(conn, table, core, mode, next, diagnostic, finished_at)
}

/// Record a reconciled state, in memory always and in the row when allowed.
fn settle(
    conn: &Connection,
    table: ActionTable,
    core: &mut ActionCore,
    mode: Reconcile,
    next: DiscussionActionState,
    diagnostic: Option<String>,
    finished_at: Option<String>,
) -> Result<()> {
    if next == core.state && diagnostic == core.diagnostic {
        return Ok(());
    }
    let now = Utc::now().to_rfc3339();
    let finished_at =
        finished_at.or_else(|| (next != DiscussionActionState::Running).then(|| now.clone()));
    if mode == Reconcile::Persisted {
        conn.execute(
            &format!(
                "UPDATE {} SET state = ?2, diagnostic = ?3, finished_at = ?4, updated_at = ?5
                 WHERE id = ?1 AND state IN ('launching', 'running')",
                table.name()
            ),
            params![core.id, state_db_str(next), diagnostic, finished_at, now],
        )?;
    }
    core.state = next;
    core.diagnostic = diagnostic;
    core.finished_at = finished_at;
    core.updated_at = now;
    Ok(())
}

/// Idempotent no-op cancel of a still-`proposed` row.
pub(crate) fn cancel(conn: &Connection, table: ActionTable, id: &str) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        &format!(
            "UPDATE {} SET state = 'cancelled', finished_at = ?2,
             updated_at = ?2 WHERE id = ?1 AND state = 'proposed'",
            table.name()
        ),
        params![id, now],
    )?;
    Ok(())
}

/// Claim a row for launch. Precondition: caller has already verified
/// `core.state == Proposed` in the same transaction (a non-proposed row must
/// never reach this function — the caller returns `Existing` itself, exactly
/// mirroring KT-476's original single-table implementation). Validates and
/// applies `supplied` overrides, scrubs any manually supplied plaintext back
/// out before persisting, and CASes the state to `launching`.
///
/// Returns the resolved runtime variables (in memory only — never derive
/// these from `core.values`, which is the copy replayed to every future GET)
/// on a genuine claim, or `None` if the CAS lost a race (defensive: under the
/// current single-writer-mutex `Database`, this cannot actually happen since
/// nothing else can interleave inside one transaction, but the check is kept
/// to avoid silently trusting an UPDATE that matched zero rows).
pub(crate) fn claim_launch(
    transaction: &rusqlite::Transaction<'_>,
    table: ActionTable,
    core: &mut ActionCore,
    supplied: &HashMap<String, String>,
    // Whether the proposal's target still resolves, checked by the origin
    // module against the same contract preflight used.
    target_still_exists: bool,
) -> Result<Option<HashMap<String, String>>> {
    debug_assert_eq!(core.state, DiscussionActionState::Proposed);
    // Preflight ran when the proposal was ingested; the click can come much
    // later, and a transcript or a published page outlives the Quick Exec it
    // names. Claiming here would hand the caller variables for a target that
    // no longer exists — a launch with nothing behind it.
    if !target_still_exists {
        let now = Utc::now().to_rfc3339();
        core.state = DiscussionActionState::PreflightFailed;
        core.diagnostic = Some("La cible n’existe plus ou n’est pas accessible.".into());
        core.updated_at = now.clone();
        transaction.execute(
            &format!(
                "UPDATE {} SET state = ?2, diagnostic = ?3, updated_at = ?4 WHERE id = ?1",
                table.name()
            ),
            params![
                &core.id,
                state_db_str(core.state),
                core.diagnostic.as_deref(),
                &now
            ],
        )?;
        return Ok(None);
    }
    for (name, supplied_value) in supplied {
        let Some(value) = core.values.iter_mut().find(|value| value.name == *name) else {
            anyhow::bail!("unknown action variable `{name}`");
        };
        // A `project_env`/`kronn_context`/`dynamic_binding` value is
        // read-only UNLESS its own declaration opted into an audited manual
        // override — mirrors `execution_variables::resolve()`'s
        // `allow_manual_override` contract. `dynamic_binding` is always
        // "overridable" in this narrow sense: it has no other path to a
        // value, since it is never proposed with one — the origin module
        // (`live_page_actions::claim_launch`) is the only caller allowed to
        // populate `supplied` for such a field, and only with a value it
        // just resolved itself from the real dataset/page row.
        let overridable = value.allow_manual_override
            || matches!(
                value.provenance,
                DiscussionActionValueProvenance::UserInput
                    | DiscussionActionValueProvenance::AgentSuggestion
                    | DiscussionActionValueProvenance::DynamicBinding
            );
        if !overridable {
            anyhow::bail!("action variable `{name}` is resolved by Kronn and is read-only");
        }
        value.value = Some(supplied_value.clone());
        if value.provenance == DiscussionActionValueProvenance::AgentSuggestion
            && value.suggested_value.as_deref() != Some(supplied_value.as_str())
        {
            value.provenance = DiscussionActionValueProvenance::UserInput;
        }
    }
    for value in &core.values {
        if value.required
            && matches!(
                value.provenance,
                DiscussionActionValueProvenance::UserInput
                    | DiscussionActionValueProvenance::AgentSuggestion
            )
            && value.value.as_deref().unwrap_or_default().trim().is_empty()
        {
            anyhow::bail!("required action variable `{}` is missing", value.name);
        }
    }

    // The runtime value is handed to the executor in memory only, for this
    // one launch. Kronn's encrypted, retention-bound execution-variable
    // snapshot is the ONE place a manually supplied value may be durably
    // stored — never this row's `values_json`, which every future GET
    // replays verbatim to the client.
    let variables: HashMap<String, String> = core
        .values
        .iter()
        .filter_map(|value| {
            value
                .value
                .as_ref()
                .map(|v| (value.name.clone(), v.clone()))
        })
        .collect();
    for value in &mut core.values {
        if value.allow_manual_override
            || matches!(
                value.provenance,
                DiscussionActionValueProvenance::UserInput
                    | DiscussionActionValueProvenance::AgentSuggestion
                    | DiscussionActionValueProvenance::DynamicBinding
            )
        {
            value.value = None;
        }
    }

    let now = Utc::now().to_rfc3339();
    let changed = transaction.execute(
        &format!(
            "UPDATE {} SET state = 'launching', values_json = ?2,
             launched_at = ?3, updated_at = ?3 WHERE id = ?1 AND state = 'proposed'",
            table.name()
        ),
        params![core.id, serde_json::to_string(&core.values)?, now],
    )?;
    core.state = DiscussionActionState::Launching;
    core.launched_at = Some(now.clone());
    core.updated_at = now;
    if changed == 1 {
        Ok(Some(variables))
    } else {
        Ok(None)
    }
}

pub(crate) fn complete(
    conn: &Connection,
    table: ActionTable,
    id: &str,
    completion: ActionCompletion,
) -> Result<()> {
    if matches!(
        completion.state,
        DiscussionActionState::Proposed | DiscussionActionState::Launching
    ) {
        anyhow::bail!("invalid terminal action transition");
    }
    let now = Utc::now().to_rfc3339();
    let finished_at = (completion.state != DiscussionActionState::Running).then_some(now.clone());
    conn.execute(
        &format!(
            "UPDATE {} SET state = ?2, shared_run_id = ?3,
             result_discussion_id = ?4, deep_link = ?5, diagnostic = ?6,
             finished_at = ?7, updated_at = ?8
             WHERE id = ?1 AND state IN ('launching','running')",
            table.name()
        ),
        params![
            id,
            state_db_str(completion.state),
            completion.shared_run_id,
            completion.result_discussion_id,
            completion.deep_link,
            completion.diagnostic,
            finished_at,
            now,
        ],
    )?;
    Ok(())
}
