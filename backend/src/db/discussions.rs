use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{*, ModelTier};

// ─── Discussions ────────────────────────────────────────────────────────────

/// Count total discussions (for pagination).
pub fn count_discussions(conn: &Connection) -> Result<u32> {
    let count: u32 = conn.query_row("SELECT COUNT(*) FROM discussions", [], |row| row.get(0))?;
    Ok(count)
}

/// Checkpoint the in-flight agent response. Called periodically from
/// `make_agent_stream` (every ~30s or ~100 chunks) so a backend restart
/// preserves what the agent has produced so far.
///
/// - Setting a non-null `partial` sets `partial_response_started_at` to NOW()
///   only if the column is currently NULL (first checkpoint of this run).
///   Subsequent checkpoints preserve the original timestamp — critical for
///   `recover_partial_responses` to place the recovered Agent message
///   chronologically before any later user message posted after restart.
/// - Setting `None` clears BOTH columns (normal completion).
pub fn set_partial_response(conn: &Connection, disc_id: &str, partial: Option<&str>) -> Result<()> {
    match partial {
        Some(text) => {
            conn.execute(
                "UPDATE discussions \
                 SET partial_response = ?2, \
                     partial_response_started_at = COALESCE(partial_response_started_at, ?3) \
                 WHERE id = ?1",
                params![disc_id, text, Utc::now().to_rfc3339()],
            )?;
        }
        None => {
            conn.execute(
                "UPDATE discussions \
                 SET partial_response = NULL, partial_response_started_at = NULL \
                 WHERE id = ?1",
                params![disc_id],
            )?;
        }
    }
    Ok(())
}

/// Recover any in-flight agent responses that were checkpointed but never
/// completed (process killed mid-stream). Called once at backend boot:
/// for each `discussion` with non-null `partial_response`, we save the
/// partial as an Agent message with a clear "interrupted" footer, then
/// clear the checkpoint.
///
/// Returns the list of recovered discussion ids so the caller can broadcast
/// a `PartialResponseRecovered` WS event — the frontend uses it to refetch
/// the affected discs + toast the user that their in-flight agents were
/// interrupted.
///
/// Chronology: each recovered message is timestamped with the discussion's
/// `partial_response_started_at` (when the agent began producing output),
/// NOT Utc::now(). Without this, a user who didn't see the partial
/// (frontend not yet connected) and resent their prompt would see the
/// recovered message appear AFTER their 2nd user message — confusing, and
/// the bug reported 2026-04-13.
///
/// Companion to the orphan workflow_runs scan in main.rs — together they
/// guarantee no fake-Running state survives a crash.
/// Mark/unmark a discussion as owing an agent run. Set at
/// enqueue (batch child creation, human message that will auto-reply);
/// cleared when the agent delivers. See `reconcile_awaiting_agents`.
pub fn set_awaiting_agent(conn: &Connection, disc_id: &str, awaiting: bool) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET awaiting_agent = ?2 WHERE id = ?1",
        rusqlite::params![disc_id, awaiting as i64],
    )?;
    Ok(())
}

/// Boot reconcile n°3. Finds discussions that were owed an agent
/// run which never produced any durable trace (queued batch child, or an
/// auto-reply never spawned) and were orphaned by a restart. Must run AFTER
/// `recover_partial_responses` (which converts in-flight partials into Agent
/// messages) so a disc that WAS streaming is already "answered" and skipped
/// here. Appends an interruption notice, then clears the flag — it NEVER
/// re-spawns (an interruption may be deliberate: the user shut the machine).
/// If the notice insert fails the flag is kept so the next boot retries
/// (retry can't duplicate: nothing was persisted). Returns the disc ids
/// marked, for a boot broadcast/toast.
pub fn reconcile_awaiting_agents(conn: &Connection) -> Result<Vec<String>> {
    // Candidates: flagged, and no partial left to recover. The last-message
    // check below is the precise guard (skip anything already answered).
    let candidates: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM discussions \
             WHERE awaiting_agent = 1 AND partial_response IS NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    const FOOTER: &str = "⏸️ **Réponse interrompue** — le backend a redémarré avant qu'une \
        réponse de l'agent soit enregistrée pour cette discussion (interruption possiblement \
        volontaire). Relancez si besoin.\n\n\
        *Note pour l'agent relancé : ce message est un marqueur système, pas une réponse. \
        Ignore-le et exécute la demande du dernier message utilisateur ci-dessus — si c'est \
        un protocole à phases, reprends à la première phase non complétée visible dans \
        l'historique.*";

    let mut marked = Vec::with_capacity(candidates.len());
    for disc_id in candidates {
        // Defensive: only act if the last message is still a User prompt with
        // no Agent answer after it. Anything else (already answered, or a
        // partial just recovered into an Agent message) → just clear the flag.
        let last_role: Option<String> = conn
            .query_row(
                "SELECT role FROM messages WHERE discussion_id = ?1 \
                 ORDER BY sort_order DESC LIMIT 1",
                rusqlite::params![&disc_id],
                |r| r.get(0),
            )
            .optional()?;
        let owed = matches!(last_role.as_deref(), Some("User"));
        if owed {
            let msg = DiscussionMessage {
                model: None,
                lint_report: None,
                id: uuid::Uuid::new_v4().to_string(),
                role: MessageRole::Agent,
                content: FOOTER.to_string(),
                agent_type: None,
                timestamp: Utc::now(),
                tokens_used: 0,
                auth_mode: None,
                model_tier: None,
                cost_usd: None,
                author_pseudo: None,
                author_avatar_email: None,
                source_msg_id: None,
                duration_ms: None,
            };
            // If the notice can't be persisted, KEEP awaiting_agent=1 so the
            // next boot retries: no notice landed, so retrying can't duplicate
            // one, whereas clearing here loses the user's only signal forever.
            // Worst case of a persistent failure is one warn line per boot —
            // observable, unlike a silently dropped owed-run marker.
            if let Err(e) = insert_message(conn, &disc_id, &msg) {
                tracing::warn!("reconcile_awaiting_agents: failed to append notice for {}: {}", disc_id, e);
                continue;
            }
            marked.push(disc_id.clone());
        }
        if let Err(e) = set_awaiting_agent(conn, &disc_id, false) {
            tracing::warn!("reconcile_awaiting_agents: failed to clear flag for {}: {}", disc_id, e);
        }
    }
    Ok(marked)
}

pub fn recover_partial_responses(conn: &Connection) -> Result<Vec<String>> {
    let triples: Vec<(String, String, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT id, partial_response, partial_response_started_at \
             FROM discussions WHERE partial_response IS NOT NULL"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.filter_map(|r| r.ok()).collect()
    };

    if triples.is_empty() {
        return Ok(Vec::new());
    }

    const FOOTER: &str = "\n\n---\n⚠️ **Réflexion interrompue** — le backend a été redémarré pendant que cet agent répondait. \
        Voici ce qu'il avait écrit jusque-là. Relancez la discussion pour reprendre.";

    let mut recovered = Vec::with_capacity(triples.len());
    for (disc_id, partial, started_at_str) in triples {
        let content = format!("{}{}", partial.trim_end(), FOOTER);
        // Use the checkpoint's start time so the recovered message sits
        // BEFORE any later user message. Fall back to now() only if the
        // column is empty (shouldn't happen after migration 032, but
        // defensive since legacy rows might have partial_response set
        // without a start timestamp).
        let ts = started_at_str
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let msg = DiscussionMessage {
            model: None,
            lint_report: None,
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Agent,
            content,
            agent_type: None,
            timestamp: ts,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None, source_msg_id: None, duration_ms: None,
        };
        match insert_message(conn, &disc_id, &msg) {
            Ok(_) => {
                if let Err(e) = set_partial_response(conn, &disc_id, None) {
                    tracing::warn!("Cleared partial after recovery but failed to wipe column for {}: {}", disc_id, e);
                }
                recovered.push(disc_id);
            }
            Err(e) => {
                tracing::warn!("Failed to recover partial for disc {}: {}", disc_id, e);
            }
        }
    }
    Ok(recovered)
}

/// Check if a discussion currently has an in-flight partial response
/// checkpoint (i.e. an agent started answering but hasn't finished).
/// Used by the POST message handler to refuse duplicate sends while a
/// previous partial is still pending recovery.
pub fn has_pending_partial(conn: &Connection, disc_id: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM discussions \
         WHERE id = ?1 AND partial_response IS NOT NULL",
        params![disc_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Column list shared by every `SELECT ... FROM discussions d` that maps rows
/// via [`map_discussion_row`]. Keep the order in sync with the indices read there.
const DISC_SELECT_COLS: &str = "d.id, d.project_id, d.title, d.agent, d.language, d.participants_json,
                d.created_at, d.updated_at, d.archived, d.skill_ids_json,
                d.message_count,
                d.profile_ids_json, d.directive_ids_json,
                d.workspace_mode, d.workspace_path, d.worktree_branch,
                d.summary_cache, d.summary_up_to_msg_idx, d.model_tier,
                d.pin_first_message,
                d.shared_id, d.shared_with_json, d.workflow_run_id,
                d.pinned,
                d.test_mode_restore_branch, d.test_mode_stash_ref,
                d.summary_strategy, d.introspection_call_count,
                d.source_agent, d.source_session_id, d.imported_at, d.diverged_at,
                d.model,
                (SELECT COUNT(*) FROM messages m
                   WHERE m.discussion_id = d.id AND m.role != 'System') AS non_system_count,
                d.awaiting_agent";

/// Map one `discussions` row (selected via [`DISC_SELECT_COLS`]) into a
/// [`Discussion`] without its messages (those are loaded separately).
fn map_discussion_row(row: &rusqlite::Row) -> rusqlite::Result<Discussion> {
    let agent_str: String = row.get(3)?;
    let participants_str: String = row.get(5)?;
    let skill_ids_str: String = row.get::<_, String>(9).unwrap_or_else(|_| "[]".into());
    let profile_ids_str: String = row.get::<_, String>(11).unwrap_or_else(|_| "[]".into());
    let directive_ids_str: String = row.get::<_, String>(12).unwrap_or_else(|_| "[]".into());

    Ok(Discussion {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        agent: parse_agent_type(&agent_str),
        language: row.get(4)?,
        participants: serde_json::from_str(&participants_str).unwrap_or_default(),
        messages: vec![],
        message_count: row.get::<_, u32>(10).unwrap_or(0),
        // Index 33 — trailing computed col from DISC_SELECT_COLS (subquery
        // counting non-System messages), now after d.model at 32. Used by the
        // unread badge so tool breadcrumbs don't inflate the "à lire" counter.
        non_system_message_count: row.get::<_, u32>(33).unwrap_or(0),
        skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
        profile_ids: serde_json::from_str(&profile_ids_str).unwrap_or_default(),
        directive_ids: serde_json::from_str(&directive_ids_str).unwrap_or_default(),
        archived: row.get::<_, i32>(8).unwrap_or(0) != 0,
        pinned: row.get::<_, i32>(23).unwrap_or(0) != 0,
        workspace_mode: row.get::<_, String>(13).unwrap_or_else(|_| "Direct".into()),
        workspace_path: row.get::<_, Option<String>>(14).unwrap_or(None),
        worktree_branch: row.get::<_, Option<String>>(15).unwrap_or(None),
        tier: parse_model_tier(&row.get::<_, String>(18).unwrap_or_else(|_| "default".into())),
        model: row.get::<_, Option<String>>(32).unwrap_or(None),
        pin_first_message: row.get::<_, i32>(19).unwrap_or(0) != 0,
        summary_cache: row.get::<_, Option<String>>(16).unwrap_or(None),
        summary_up_to_msg_idx: row.get::<_, Option<u32>>(17).unwrap_or(None),
        shared_id: row.get::<_, Option<String>>(20).unwrap_or(None),
        shared_with: serde_json::from_str(&row.get::<_, String>(21).unwrap_or_else(|_| "[]".into())).unwrap_or_default(),
        workflow_run_id: row.get::<_, Option<String>>(22).unwrap_or(None),
        awaiting_agent: row.get::<_, i32>(34).unwrap_or(0) != 0,
        test_mode_restore_branch: row.get::<_, Option<String>>(24).unwrap_or(None),
        test_mode_stash_ref: row.get::<_, Option<String>>(25).unwrap_or(None),
        summary_strategy: parse_summary_strategy(row.get::<_, String>(26).unwrap_or_else(|_| "Auto".into()).as_str()),
        introspection_call_count: row.get::<_, u32>(27).unwrap_or(0),
        created_at: parse_dt(row.get::<_, String>(6)?),
        updated_at: parse_dt(row.get::<_, String>(7)?),
    })
}

pub fn list_discussions(conn: &Connection) -> Result<Vec<Discussion>> {
    list_discussions_paginated(conn, None, None)
}

/// Discussions spawned by a batch / workflow run (linked via `workflow_run_id`).
/// Ordered oldest-first so MCP callers see them in creation order. Messages
/// are not loaded (use `get_discussion` for a single disc's body).
pub fn list_discussions_by_run(conn: &Connection, run_id: &str) -> Result<Vec<Discussion>> {
    let sql = format!(
        "SELECT {} FROM discussions d WHERE d.workflow_run_id = ?1 ORDER BY d.created_at ASC",
        DISC_SELECT_COLS
    );
    let mut stmt = conn.prepare(&sql)?;
    let discussions: Vec<Discussion> = stmt
        .query_map(params![run_id], map_discussion_row)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(discussions)
}

pub fn list_discussions_paginated(conn: &Connection, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Discussion>> {
    let sql = format!(
        "SELECT {} FROM discussions d ORDER BY d.updated_at DESC{}",
        DISC_SELECT_COLS,
        match (limit, offset) {
            (Some(l), Some(o)) => format!(" LIMIT {} OFFSET {}", l, o),
            (Some(l), None) => format!(" LIMIT {}", l),
            _ => String::new(),
        }
    );
    let mut stmt = conn.prepare(&sql)?;

    let discussions: Vec<Discussion> = stmt
        .query_map([], map_discussion_row)?
        .filter_map(|r| r.ok())
        .collect();

    // Don't load messages for the list view — messages are only loaded
    // for individual discussions via get_discussion(). With 200+ discussions
    // each having 50+ messages, loading all messages here is a performance bomb.
    // message_count is populated via SQL subquery for display purposes.

    Ok(discussions)
}

/// Like list_discussions but also loads all messages (used for export).
pub fn list_discussions_with_messages(conn: &Connection) -> Result<Vec<Discussion>> {
    let mut discussions = list_discussions(conn)?;

    let all_messages = list_all_messages(conn)?;
    for disc in &mut discussions {
        if let Some(msgs) = all_messages.get(&disc.id) {
            disc.messages = msgs.clone();
            disc.message_count = disc.messages.len() as u32;
            disc.non_system_message_count = disc
                .messages
                .iter()
                .filter(|m| !matches!(m.role, crate::models::MessageRole::System))
                .count() as u32;
        }
    }

    Ok(discussions)
}

pub fn get_discussion(conn: &Connection, id: &str) -> Result<Option<Discussion>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, agent, language, participants_json,
                created_at, updated_at, archived, skill_ids_json, profile_ids_json, directive_ids_json,
                workspace_mode, workspace_path, worktree_branch,
                summary_cache, summary_up_to_msg_idx, model_tier, pin_first_message,
                shared_id, shared_with_json, workflow_run_id, pinned,
                test_mode_restore_branch, test_mode_stash_ref,
                summary_strategy, introspection_call_count,
                source_agent, source_session_id, imported_at, diverged_at,
                model, awaiting_agent
         FROM discussions WHERE id = ?1"
    )?;

    let disc = stmt.query_row(params![id], |row| {
        let agent_str: String = row.get(3)?;
        let participants_str: String = row.get(5)?;
        let skill_ids_str: String = row.get::<_, String>(9).unwrap_or_else(|_| "[]".into());
        let profile_ids_str: String = row.get::<_, String>(10).unwrap_or_else(|_| "[]".into());
        let directive_ids_str: String = row.get::<_, String>(11).unwrap_or_else(|_| "[]".into());

        Ok(Discussion {
            id: row.get(0)?,
            project_id: row.get(1)?,
            title: row.get(2)?,
            agent: parse_agent_type(&agent_str),
            language: row.get(4)?,
            participants: serde_json::from_str(&participants_str).unwrap_or_default(),
            messages: vec![],
            message_count: 0, non_system_message_count: 0,
            skill_ids: serde_json::from_str(&skill_ids_str).unwrap_or_default(),
            profile_ids: serde_json::from_str(&profile_ids_str).unwrap_or_default(),
            directive_ids: serde_json::from_str(&directive_ids_str).unwrap_or_default(),
            archived: row.get::<_, i32>(8).unwrap_or(0) != 0,
            pinned: row.get::<_, i32>(22).unwrap_or(0) != 0,
            workspace_mode: row.get::<_, String>(12).unwrap_or_else(|_| "Direct".into()),
            workspace_path: row.get::<_, Option<String>>(13).unwrap_or(None),
            worktree_branch: row.get::<_, Option<String>>(14).unwrap_or(None),
            tier: parse_model_tier(&row.get::<_, String>(17).unwrap_or_else(|_| "default".into())),
            model: row.get::<_, Option<String>>(31).unwrap_or(None),
            pin_first_message: row.get::<_, i32>(18).unwrap_or(0) != 0,
            summary_cache: row.get::<_, Option<String>>(15).unwrap_or(None),
            summary_up_to_msg_idx: row.get::<_, Option<u32>>(16).unwrap_or(None),
            shared_id: row.get::<_, Option<String>>(19).unwrap_or(None),
            shared_with: serde_json::from_str(&row.get::<_, String>(20).unwrap_or_else(|_| "[]".into())).unwrap_or_default(),
            workflow_run_id: row.get::<_, Option<String>>(21).unwrap_or(None),
            awaiting_agent: row.get::<_, i32>(32).unwrap_or(0) != 0,
            test_mode_restore_branch: row.get::<_, Option<String>>(23).unwrap_or(None),
            test_mode_stash_ref: row.get::<_, Option<String>>(24).unwrap_or(None),
            summary_strategy: parse_summary_strategy(row.get::<_, String>(25).unwrap_or_else(|_| "Auto".into()).as_str()),
            introspection_call_count: row.get::<_, u32>(26).unwrap_or(0),
            created_at: parse_dt(row.get::<_, String>(6)?),
            updated_at: parse_dt(row.get::<_, String>(7)?),
        })
    }).ok();

    if let Some(mut d) = disc {
        d.messages = list_messages(conn, &d.id)?;
        d.message_count = d.messages.len() as u32;
        d.non_system_message_count = d
            .messages
            .iter()
            .filter(|m| !matches!(m.role, crate::models::MessageRole::System))
            .count() as u32;
        Ok(Some(d))
    } else {
        Ok(None)
    }
}

pub fn insert_discussion(conn: &Connection, disc: &Discussion) -> Result<()> {
    conn.execute(
        "INSERT INTO discussions (id, project_id, title, agent, language, participants_json, created_at, updated_at, archived, pinned, skill_ids_json, profile_ids_json, directive_ids_json, workspace_mode, workspace_path, worktree_branch, model_tier, pin_first_message, shared_id, shared_with_json, workflow_run_id, test_mode_restore_branch, test_mode_stash_ref, model)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        params![
            disc.id,
            disc.project_id,
            disc.title,
            format_agent_type(&disc.agent),
            disc.language,
            serde_json::to_string(&disc.participants)?,
            disc.created_at.to_rfc3339(),
            disc.updated_at.to_rfc3339(),
            disc.archived as i32,
            disc.pinned as i32,
            serde_json::to_string(&disc.skill_ids)?,
            serde_json::to_string(&disc.profile_ids)?,
            serde_json::to_string(&disc.directive_ids)?,
            disc.workspace_mode,
            disc.workspace_path,
            disc.worktree_branch,
            format_model_tier(&disc.tier),
            disc.pin_first_message as i32,
            disc.shared_id,
            serde_json::to_string(&disc.shared_with)?,
            disc.workflow_run_id,
            disc.test_mode_restore_branch,
            disc.test_mode_stash_ref,
            disc.model,
        ],
    )?;
    Ok(())
}

/// Ensure a local mirror of a shared discussion exists, returning its LOCAL
/// disc id (existing or freshly created). Idempotent on `shared_id`. The title
/// is stored as `"<title> (shared by <from_pseudo>)"` to match the WS-invite
/// creation path (`api::ws::handle_discussion_invite`) so both routes converge
/// on the same local representation.
///
/// Used by the cross-instance "join by code" flow: `claim-by-token` returns
/// `shared_id` + `title` in its HTTP response, so the joiner creates the
/// mirror directly here instead of waiting for the WS `DiscussionInvite` to
/// arrive (fragile under NAT / WS lag). A late WS invite then finds the disc
/// already present and is a no-op. Races with that invite are absorbed: if the
/// insert trips the UNIQUE `shared_id` index, we re-resolve the existing row.
pub fn ensure_mirror_by_shared_id(
    conn: &Connection,
    shared_id: &str,
    title: &str,
    from_pseudo: &str,
) -> Result<String> {
    if let Some(existing) = find_discussion_by_shared_id(conn, shared_id)? {
        return Ok(existing);
    }
    let now = Utc::now();
    let disc = Discussion {
        awaiting_agent: false,
        id: uuid::Uuid::new_v4().to_string(),
        project_id: None,
        title: format!("{title} (shared by {from_pseudo})"),
        agent: AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: Some(shared_id.to_string()),
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    match insert_discussion(conn, &disc) {
        Ok(()) => Ok(disc.id),
        Err(e) => {
            // Lost a race with the WS invite (UNIQUE shared_id) — re-resolve.
            if let Some(existing) = find_discussion_by_shared_id(conn, shared_id)? {
                Ok(existing)
            } else {
                Err(e)
            }
        }
    }
}

/// F9 — whether this disc is "human-only" (no agent runner ever spawns).
/// Read directly off the column (like `diverged_at`) so we don't have to thread
/// the flag through the big `Discussion` struct + all its query sites.
pub fn disc_is_no_agent(conn: &Connection, disc_id: &str) -> Result<bool> {
    let v: Option<i64> = conn
        .query_row(
            "SELECT no_agent FROM discussions WHERE id = ?1",
            params![disc_id],
            |row| row.get(0),
        )
        .ok();
    Ok(v.unwrap_or(0) != 0)
}

/// Set/clear the F9 human-only flag on a disc. Returns true if the row existed.
pub fn set_disc_no_agent(conn: &Connection, disc_id: &str, no_agent: bool) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET no_agent = ?2, updated_at = ?3 WHERE id = ?1",
        params![disc_id, no_agent as i32, Utc::now().to_rfc3339()],
    )?;
    Ok(affected > 0)
}

pub fn delete_discussion(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM discussions WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

pub fn update_discussion(conn: &Connection, id: &str, title: Option<&str>, archived: Option<bool>, pinned: Option<bool>, project_id: Option<Option<&str>>) -> Result<bool> {
    update_discussion_fields(conn, id, title, archived, pinned, None, None, None, project_id)
}

pub fn update_discussion_skill_ids(conn: &Connection, id: &str, skill_ids: &[String]) -> Result<bool> {
    update_discussion_fields(conn, id, None, None, None, Some(skill_ids), None, None, None)
}

pub fn update_discussion_profile_ids(conn: &Connection, id: &str, profile_ids: &[String]) -> Result<bool> {
    update_discussion_fields(conn, id, None, None, None, None, Some(profile_ids), None, None)
}

pub fn update_discussion_tier(conn: &Connection, id: &str, tier: &ModelTier) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET model_tier = ?1, updated_at = ?2 WHERE id = ?3",
        params![format_model_tier(tier), Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn update_discussion_agent(conn: &Connection, id: &str, agent: &AgentType) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET agent = ?1, updated_at = ?2 WHERE id = ?3",
        params![format_agent_type(agent), Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

pub fn update_discussion_directive_ids(conn: &Connection, id: &str, directive_ids: &[String]) -> Result<bool> {
    update_discussion_fields(conn, id, None, None, None, None, None, Some(directive_ids), None)
}

pub fn update_discussion_summary_strategy(conn: &Connection, id: &str, strategy: crate::models::SummaryStrategy) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET summary_strategy = ?1, updated_at = ?2 WHERE id = ?3",
        params![format_summary_strategy(strategy), Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

/// Update workspace_path and worktree_branch for a discussion (used after worktree creation).
pub fn update_discussion_workspace(conn: &Connection, id: &str, workspace_path: &str, worktree_branch: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET workspace_path = ?1, worktree_branch = ?2, updated_at = ?3 WHERE id = ?4",
        params![workspace_path, worktree_branch, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

/// Set or clear the two test-mode tracking fields together.
///
/// `restore_branch = Some(...)` + `stash_ref = Option<...>`: user just entered
/// test mode — we remember the branch to go back to and optionally the stash
/// we pushed. Pass `(None, None)` on `exit` to return to normal worktree
/// operation.
pub fn update_discussion_test_mode(
    conn: &Connection,
    id: &str,
    restore_branch: Option<&str>,
    stash_ref: Option<&str>,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET test_mode_restore_branch = ?1, test_mode_stash_ref = ?2, updated_at = ?3 WHERE id = ?4",
        params![restore_branch, stash_ref, Utc::now().to_rfc3339(), id],
    )?;
    Ok(affected > 0)
}

#[allow(clippy::too_many_arguments)]
fn update_discussion_fields(conn: &Connection, id: &str, title: Option<&str>, archived: Option<bool>, pinned: Option<bool>, skill_ids: Option<&[String]>, profile_ids: Option<&[String]>, directive_ids: Option<&[String]>, project_id: Option<Option<&str>>) -> Result<bool> {
    let mut sets = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(t) = title {
        sets.push("title = ?");
        values.push(Box::new(t.to_string()));
    }
    if let Some(a) = archived {
        sets.push("archived = ?");
        values.push(Box::new(a as i32));
    }
    if let Some(p) = pinned {
        sets.push("pinned = ?");
        values.push(Box::new(p as i32));
    }
    if let Some(pid) = project_id {
        sets.push("project_id = ?");
        values.push(Box::new(pid.map(|s| s.to_string())));
    }
    if let Some(s) = skill_ids {
        sets.push("skill_ids_json = ?");
        values.push(Box::new(serde_json::to_string(s).unwrap_or_else(|_| "[]".into())));
    }
    if let Some(p) = profile_ids {
        sets.push("profile_ids_json = ?");
        values.push(Box::new(serde_json::to_string(p).unwrap_or_else(|_| "[]".into())));
    }
    if let Some(d) = directive_ids {
        sets.push("directive_ids_json = ?");
        values.push(Box::new(serde_json::to_string(d).unwrap_or_else(|_| "[]".into())));
    }

    if sets.is_empty() {
        return Ok(false);
    }

    sets.push("updated_at = ?");
    values.push(Box::new(Utc::now().to_rfc3339()));

    values.push(Box::new(id.to_string()));

    let sql = format!(
        "UPDATE discussions SET {} WHERE id = ?",
        sets.join(", ")
    );

    let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let affected = conn.execute(&sql, params.as_slice())?;
    Ok(affected > 0)
}

pub fn update_discussion_timestamp(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET updated_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

pub fn update_discussion_participants(conn: &Connection, id: &str, participants: &[AgentType]) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET participants_json = ?1, updated_at = ?2 WHERE id = ?3",
        params![serde_json::to_string(participants)?, Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

// ─── Messages ───────────────────────────────────────────────────────────────

/// Load all messages grouped by discussion_id in a single query (avoids N+1).
fn list_all_messages(conn: &Connection) -> Result<std::collections::HashMap<String, Vec<DiscussionMessage>>> {
    let mut stmt = conn.prepare(
        "SELECT discussion_id, id, role, content, agent_type, timestamp, tokens_used, auth_mode, model_tier, cost_usd, duration_ms, lint_report, model
         FROM messages ORDER BY sort_order, timestamp"
    )?;

    let mut map: std::collections::HashMap<String, Vec<DiscussionMessage>> = std::collections::HashMap::new();
    let rows = stmt.query_map([], |row| {
        let disc_id: String = row.get(0)?;
        let role_str: String = row.get(2)?;
        let agent_type_str: Option<String> = row.get(4)?;

        Ok((disc_id, DiscussionMessage {
            id: row.get(1)?,
            role: parse_role(&role_str),
            content: row.get(3)?,
            agent_type: agent_type_str.map(|s| parse_agent_type(&s)),
            timestamp: parse_dt(row.get::<_, String>(5)?),
            tokens_used: row.get::<_, i64>(6).unwrap_or(0) as u64,
            auth_mode: row.get(7)?,
            model_tier: row.get::<_, Option<String>>(8).unwrap_or(None),
            cost_usd: row.get::<_, Option<f64>>(9).unwrap_or(None),
            author_pseudo: None,
            author_avatar_email: None, source_msg_id: None,
            duration_ms: row.get::<_, Option<i64>>(10).unwrap_or(None).map(|d| d as u64),
            lint_report: row.get::<_, Option<String>>(11).unwrap_or(None)
                .and_then(|s| serde_json::from_str(&s).ok()),
            model: row.get::<_, Option<String>>(12).unwrap_or(None),
        }))
    })?;

    for row in rows.filter_map(|r| r.ok()) {
        map.entry(row.0).or_default().push(row.1);
    }

    Ok(map)
}

pub fn list_messages(conn: &Connection, discussion_id: &str) -> Result<Vec<DiscussionMessage>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, agent_type, timestamp, tokens_used, auth_mode, model_tier, cost_usd, author_pseudo, author_avatar_email, source_msg_id, duration_ms, lint_report, model
         FROM messages WHERE discussion_id = ?1
         ORDER BY sort_order, timestamp"
    )?;

    let messages = stmt.query_map(params![discussion_id], |row| {
        let role_str: String = row.get(1)?;
        let agent_type_str: Option<String> = row.get(3)?;

        Ok(DiscussionMessage {
            id: row.get(0)?,
            role: parse_role(&role_str),
            content: row.get(2)?,
            agent_type: agent_type_str.map(|s| parse_agent_type(&s)),
            timestamp: parse_dt(row.get::<_, String>(4)?),
            tokens_used: row.get::<_, i64>(5).unwrap_or(0) as u64,
            auth_mode: row.get(6)?,
            model_tier: row.get::<_, Option<String>>(7).unwrap_or(None),
            cost_usd: row.get::<_, Option<f64>>(8).unwrap_or(None),
            author_pseudo: row.get::<_, Option<String>>(9).unwrap_or(None),
            author_avatar_email: row.get::<_, Option<String>>(10).unwrap_or(None),
            source_msg_id: row.get::<_, Option<String>>(11).unwrap_or(None),
            duration_ms: row.get::<_, Option<i64>>(12).unwrap_or(None).map(|d| d as u64),
            lint_report: row.get::<_, Option<String>>(13).unwrap_or(None)
                .and_then(|s| serde_json::from_str(&s).ok()),
            model: row.get::<_, Option<String>>(14).unwrap_or(None),
        })
    })?.filter_map(|r| r.ok()).collect();

    Ok(messages)
}

/// For each SHARED discussion, the pair `(shared_id, latest_message_ts_millis)`.
/// `latest_message_ts_millis` is 0 when the disc has no messages yet. Used on
/// peer (re)connect to ask "send me everything newer than this" per shared disc
/// (the F4 catch-up). Timestamps are stored as RFC3339 strings, lexically
/// sortable, so `MAX(timestamp)` yields the newest; we parse it to millis.
pub fn list_shared_sync_points(conn: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT d.shared_id, MAX(m.timestamp)
           FROM discussions d
           LEFT JOIN messages m ON m.discussion_id = d.id
          WHERE d.shared_id IS NOT NULL
          GROUP BY d.id, d.shared_id",
    )?;
    let rows = stmt.query_map([], |row| {
        let shared_id: String = row.get(0)?;
        let max_ts: Option<String> = row.get(1)?;
        Ok((shared_id, max_ts))
    })?;
    let mut out = Vec::new();
    for r in rows.filter_map(|r| r.ok()) {
        let since = r.1.map(parse_dt).map(|dt| dt.timestamp_millis()).unwrap_or(0);
        out.push((r.0, since));
    }
    Ok(out)
}

/// stab-3 — timestamp of the LAST message (any role): the reset anchor of
/// the cold backoff ramp (`reset_on_peer_message`).
pub fn last_message_at(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    // Latest by sort_order AND on the reception clock (Copilot + Codex,
    // PR 118): a federated message can arrive stamped in the past — the
    // reset contract is about when THIS instance received it, so the anchor
    // reads `received_at` (072; COALESCE covers pre-migration rows) on the
    // newest row by sort_order, riding the (discussion_id, sort_order)
    // index. A query error is a REAL SQL failure and must reach the caller.
    let ts: Option<String> = conn
        .query_row(
            "SELECT COALESCE(received_at, timestamp) FROM messages
              WHERE discussion_id = ?1
              ORDER BY sort_order DESC LIMIT 1",
            params![discussion_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(ts
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc)))
}

/// stab-3 — timestamp of the LAST User message in a disc, the anchor of
/// the human attention lease (pacing hot/cold regime).
pub fn last_user_message_at(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    // Latest User message on the reception clock — same rationale as
    // `last_message_at`: an old-stamped federated User message must still
    // RENEW the lease, so the anchor is `received_at` on the newest User
    // row by sort_order.
    let ts: Option<String> = conn
        .query_row(
            "SELECT COALESCE(received_at, timestamp) FROM messages
              WHERE discussion_id = ?1 AND role = 'User'
              ORDER BY sort_order DESC LIMIT 1",
            params![discussion_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(ts
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&chrono::Utc)))
}

/// Returns the `sort_order` assigned to the inserted message — callers that
/// long-poll (`disc_wait_for_peer`) need their REAL position, not an estimate
/// (stab-1: estimated positions drifted under concurrent posters and made
/// agents silently skip messages).
pub fn insert_message(conn: &Connection, discussion_id: &str, msg: &DiscussionMessage) -> Result<i64> {
    // Get the next sort_order for this discussion
    let next_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages WHERE discussion_id = ?1",
        params![discussion_id],
        |row| row.get(0),
    )?;

    let lint_report_json = msg
        .lint_report
        .as_ref()
        .and_then(|r| serde_json::to_string(r).ok());

    conn.execute(
        "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, sort_order, tokens_used, auth_mode, model_tier, cost_usd, author_pseudo, author_avatar_email, source_msg_id, duration_ms, lint_report, model, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            msg.id,
            discussion_id,
            format_role(&msg.role),
            msg.content,
            msg.agent_type.as_ref().map(format_agent_type),
            msg.timestamp.to_rfc3339(),
            next_order,
            msg.tokens_used as i64,
            msg.auth_mode,
            msg.model_tier,
            msg.cost_usd,
            msg.author_pseudo,
            msg.author_avatar_email,
            msg.source_msg_id,
            msg.duration_ms.map(|d| d as i64),
            lint_report_json,
            msg.model,
            // Reception clock (072): THIS instance's now, never the author's
            // timestamp — the pacing anchors depend on it.
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;

    conn.execute(
        "UPDATE discussions SET message_count = message_count + 1 WHERE id = ?1",
        params![discussion_id],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    Ok(next_order)
}

/// 0.8.5 — Stamp the QP lineage on a discussion that was spawned by a
/// QP launch. Sets `originating_qp_id` + `originating_qp_version`,
/// which the metrics aggregator GROUPs BY when computing per-version
/// avg tokens / duration / cost. Safe to call multiple times on the
/// same discussion — last write wins (used by the compare-agents
/// flow where every child gets the same lineage).
pub fn set_originating_qp(conn: &Connection, disc_id: &str, qp_id: &str, version_index: u32) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET originating_qp_id = ?2, originating_qp_version = ?3 WHERE id = ?1",
        params![disc_id, qp_id, version_index as i64],
    )?;
    Ok(())
}

/// Find a discussion by its shared_id (cross-Kronn replicated ID).
pub fn find_discussion_by_shared_id(conn: &Connection, shared_id: &str) -> Result<Option<String>> {
    let id = conn.query_row(
        "SELECT id FROM discussions WHERE shared_id = ?1",
        params![shared_id],
        |row| row.get::<_, String>(0),
    ).ok();
    Ok(id)
}

/// Update shared_id and shared_with for a discussion.
pub fn update_discussion_sharing(conn: &Connection, discussion_id: &str, shared_id: &str, shared_with: &[String]) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE discussions SET shared_id = ?1, shared_with_json = ?2, updated_at = ?3 WHERE id = ?4",
        params![shared_id, serde_json::to_string(shared_with)?, Utc::now().to_rfc3339(), discussion_id],
    )?;
    Ok(affected > 0)
}

pub fn delete_last_agent_messages(conn: &Connection, discussion_id: &str) -> Result<u64> {
    // Delete trailing non-User messages (Agent + System) from the end
    let affected = conn.execute(
        "DELETE FROM messages WHERE discussion_id = ?1 AND sort_order > (
            SELECT COALESCE(MAX(sort_order), -1) FROM messages
            WHERE discussion_id = ?1 AND role = 'User'
        )",
        params![discussion_id],
    )?;

    // Recount to keep message_count accurate after bulk delete
    conn.execute(
        "UPDATE discussions SET message_count = (
            SELECT COUNT(*) FROM messages WHERE discussion_id = ?1
         ) WHERE id = ?1",
        params![discussion_id],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    Ok(affected as u64)
}

pub fn edit_last_user_message(conn: &Connection, discussion_id: &str, content: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE messages SET content = ?1, timestamp = ?2
         WHERE discussion_id = ?3 AND role = 'User'
         AND sort_order = (SELECT MAX(sort_order) FROM messages WHERE discussion_id = ?3 AND role = 'User')",
        params![content, Utc::now().to_rfc3339(), discussion_id],
    )?;

    update_discussion_timestamp(conn, discussion_id)?;
    // Invalidate cached summary since conversation content changed
    let _ = invalidate_summary_cache(conn, discussion_id);
    Ok(affected > 0)
}

/// Save a conversation summary cache for a discussion.
pub fn update_summary_cache(conn: &Connection, discussion_id: &str, summary: &str, up_to_msg_idx: u32) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET summary_cache = ?1, summary_up_to_msg_idx = ?2 WHERE id = ?3",
        params![summary, up_to_msg_idx, discussion_id],
    )?;
    Ok(())
}

/// Invalidate summary cache (e.g., when messages are edited or deleted).
pub fn invalidate_summary_cache(conn: &Connection, discussion_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET summary_cache = NULL, summary_up_to_msg_idx = NULL WHERE id = ?1",
        params![discussion_id],
    )?;
    // Also drop every ranged summary so the agent's next disc_summarize
    // call doesn't hand back a now-stale slice.
    conn.execute(
        "DELETE FROM disc_summary_ranges WHERE discussion_id = ?1",
        params![discussion_id],
    )?;
    Ok(())
}

/// Increment the per-disc tool-call counter. Called from each
/// `disc_introspection` endpoint. Best-effort: a write error is logged
/// but doesn't fail the user-facing tool call.
pub fn bump_introspection_count(conn: &Connection, discussion_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE discussions SET introspection_call_count = introspection_call_count + 1 WHERE id = ?1",
        params![discussion_id],
    )?;
    Ok(())
}

/// Lookup a cached ranged summary for `(disc_id, from_idx, to_idx)`.
/// Returns `(summary, tokens_used)` on hit, `None` on miss.
pub fn get_ranged_summary(
    conn: &Connection,
    discussion_id: &str,
    from_idx: u32,
    to_idx: u32,
) -> Result<Option<(String, u64)>> {
    let row: Option<(String, i64)> = conn.query_row(
        "SELECT summary, tokens_used FROM disc_summary_ranges
         WHERE discussion_id = ?1 AND from_idx = ?2 AND to_idx = ?3",
        params![discussion_id, from_idx, to_idx],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    Ok(row.map(|(s, t)| (s, t as u64)))
}

/// Persist a generated ranged summary. Replaces an existing entry for
/// the same `(disc, from, to)` triple (UPSERT via REPLACE) so an explicit
/// `force_refresh: true` overwrite works without an extra DELETE.
pub fn upsert_ranged_summary(
    conn: &Connection,
    discussion_id: &str,
    from_idx: u32,
    to_idx: u32,
    summary: &str,
    tokens_used: u64,
    model_name: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO disc_summary_ranges
            (discussion_id, from_idx, to_idx, summary, tokens_used, model_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            discussion_id,
            from_idx,
            to_idx,
            summary,
            tokens_used as i64,
            model_name,
            chrono::Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn update_message_tokens(conn: &Connection, message_id: &str, tokens_used: u64, auth_mode: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE messages SET tokens_used = ?1, auth_mode = ?2 WHERE id = ?3",
        params![tokens_used as i64, auth_mode, message_id],
    )?;
    Ok(())
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to parse datetime '{}': {}, using now()", s, e);
            Utc::now()
        })
}

fn parse_agent_type(s: &str) -> AgentType {
    match s {
        "ClaudeCode" => AgentType::ClaudeCode,
        "Codex" => AgentType::Codex,
        "Vibe" => AgentType::Vibe,
        "GeminiCli" => AgentType::GeminiCli,
        "Kiro" => AgentType::Kiro,
        "CopilotCli" => AgentType::CopilotCli,
        "Ollama" => AgentType::Ollama,
        _ => AgentType::Custom,
    }
}

fn format_agent_type(a: &AgentType) -> String {
    match a {
        AgentType::ClaudeCode => "ClaudeCode".into(),
        AgentType::Codex => "Codex".into(),
        AgentType::Vibe => "Vibe".into(),
        AgentType::GeminiCli => "GeminiCli".into(),
        AgentType::Kiro => "Kiro".into(),
        AgentType::CopilotCli => "CopilotCli".into(),
        AgentType::Ollama => "Ollama".into(),
        AgentType::Custom => "Custom".into(),
    }
}

fn parse_model_tier(s: &str) -> ModelTier {
    match s {
        "economy" => ModelTier::Economy,
        "reasoning" => ModelTier::Reasoning,
        _ => ModelTier::Default,
    }
}

fn format_model_tier(t: &ModelTier) -> &'static str {
    match t {
        ModelTier::Economy => "economy",
        ModelTier::Default => "default",
        ModelTier::Reasoning => "reasoning",
    }
}

fn parse_summary_strategy(s: &str) -> crate::models::SummaryStrategy {
    match s {
        "OnDemand" => crate::models::SummaryStrategy::OnDemand,
        "Off" => crate::models::SummaryStrategy::Off,
        // Default + any unknown value (forward-compat for OLD rows or
        // future variants that haven't shipped yet) → Auto.
        _ => crate::models::SummaryStrategy::Auto,
    }
}

fn format_summary_strategy(s: crate::models::SummaryStrategy) -> &'static str {
    match s {
        crate::models::SummaryStrategy::Auto => "Auto",
        crate::models::SummaryStrategy::OnDemand => "OnDemand",
        crate::models::SummaryStrategy::Off => "Off",
    }
}

fn parse_role(s: &str) -> MessageRole {
    match s {
        "User" => MessageRole::User,
        "Agent" => MessageRole::Agent,
        "System" => MessageRole::System,
        _ => MessageRole::System,
    }
}

fn format_role(r: &MessageRole) -> &'static str {
    match r {
        MessageRole::User => "User",
        MessageRole::Agent => "Agent",
        MessageRole::System => "System",
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Context Files CRUD
// ═══════════════════════════════════════════════════════════════════════════════

#[allow(clippy::too_many_arguments)]
pub fn insert_context_file(
    conn: &Connection,
    id: &str,
    discussion_id: &str,
    filename: &str,
    mime_type: &str,
    original_size: u64,
    extracted_text: &str,
    disk_path: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO context_files (id, discussion_id, filename, mime_type, original_size, extracted_text, extracted_size, disk_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![id, discussion_id, filename, mime_type, original_size as i64, extracted_text, extracted_text.len() as i64, disk_path],
    )?;
    Ok(())
}

/// True if a context file with this id already exists locally. Idempotency
/// guard for the F8 federated-file fetch (the host's file_id is reused on the
/// peer, so a re-received FileAttached is a no-op).
pub fn context_file_exists(conn: &Connection, file_id: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM context_files WHERE id = ?1",
        rusqlite::params![file_id],
        |row| row.get(0),
    )
}

/// Fetch a single context file by id (incl. `disk_path`). Used by the F8
/// `fetch-file` endpoint to stream a federated attachment's bytes to a peer.
pub fn get_context_file(conn: &Connection, file_id: &str) -> rusqlite::Result<Option<crate::models::ContextFile>> {
    conn.query_row(
        "SELECT id, discussion_id, filename, mime_type, original_size, extracted_size, disk_path, message_id, created_at
         FROM context_files WHERE id = ?1",
        rusqlite::params![file_id],
        map_context_file_row,
    ).optional()
}

/// Insert a context file received from a peer (F8), pinned to a specific
/// message. Mirrors `insert_context_file` but sets `message_id` directly and
/// reuses the host's `file_id` so it dedups across instances. `extracted_text`
/// is empty — the binary lives on disk and the local agent reads it by path.
#[allow(clippy::too_many_arguments)]
pub fn insert_federated_context_file(
    conn: &Connection,
    id: &str,
    discussion_id: &str,
    message_id: &str,
    filename: &str,
    mime_type: &str,
    size: u64,
    disk_path: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO context_files (id, discussion_id, filename, mime_type, original_size, extracted_text, extracted_size, disk_path, message_id)
         VALUES (?1, ?2, ?3, ?4, ?5, '', 0, ?6, ?7)",
        rusqlite::params![id, discussion_id, filename, mime_type, size as i64, disk_path, message_id],
    )?;
    Ok(())
}

pub fn list_context_files(conn: &Connection, discussion_id: &str) -> rusqlite::Result<Vec<crate::models::ContextFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, discussion_id, filename, mime_type, original_size, extracted_size, disk_path, message_id, created_at
         FROM context_files WHERE discussion_id = ?1 ORDER BY created_at"
    )?;
    let rows = stmt.query_map(rusqlite::params![discussion_id], map_context_file_row)?
        .filter_map(|r| r.ok()).collect();
    Ok(rows)
}

/// Row → ContextFile mapper shared by the disc-wide and per-message list queries.
/// Both SELECT the same column order: ... disk_path, message_id, created_at.
fn map_context_file_row(row: &rusqlite::Row) -> rusqlite::Result<crate::models::ContextFile> {
    Ok(crate::models::ContextFile {
        id: row.get(0)?,
        discussion_id: row.get(1)?,
        filename: row.get(2)?,
        mime_type: row.get(3)?,
        original_size: row.get::<_, i64>(4).unwrap_or(0) as u64,
        extracted_size: row.get::<_, i64>(5).unwrap_or(0) as u64,
        disk_path: row.get(6)?,
        message_id: row.get(7)?,
        created_at: row.get::<_, String>(8)
            .map(|s| chrono::NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S")
                .unwrap_or_default().and_utc())
            .unwrap_or_else(|_| Utc::now()),
    })
}

/// Files pinned to a single message (0.8.8). Used by the per-message bubble
/// render and the `disc_get_message` MCP tool so an agent navigating to an old
/// message knows what was attached to it.
pub fn list_context_files_for_message(conn: &Connection, message_id: &str) -> rusqlite::Result<Vec<crate::models::ContextFile>> {
    let mut stmt = conn.prepare(
        "SELECT id, discussion_id, filename, mime_type, original_size, extracted_size, disk_path, message_id, created_at
         FROM context_files WHERE message_id = ?1 ORDER BY created_at"
    )?;
    let rows = stmt.query_map(rusqlite::params![message_id], map_context_file_row)?
        .filter_map(|r| r.ok()).collect();
    Ok(rows)
}

/// Pin every still-pending (message_id IS NULL) context file of a discussion to
/// a freshly-sent message, so they render in that message's bubble instead of
/// staying sticky in the composer. Returns the count linked. Idempotent for a
/// given send: a second call links nothing because the rows are no longer NULL.
pub fn link_pending_context_files_to_message(
    conn: &Connection,
    discussion_id: &str,
    message_id: &str,
) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "UPDATE context_files SET message_id = ?2 WHERE discussion_id = ?1 AND message_id IS NULL",
        rusqlite::params![discussion_id, message_id],
    )?;
    Ok(n)
}

pub fn count_context_files(conn: &Connection, discussion_id: &str) -> rusqlite::Result<usize> {
    conn.query_row(
        "SELECT COUNT(*) FROM context_files WHERE discussion_id = ?1",
        rusqlite::params![discussion_id],
        |row| row.get::<_, i64>(0).map(|n| n as usize),
    )
}

pub fn delete_context_file(conn: &Connection, discussion_id: &str, file_id: &str) -> rusqlite::Result<bool> {
    let affected = conn.execute(
        "DELETE FROM context_files WHERE id = ?1 AND discussion_id = ?2",
        rusqlite::params![file_id, discussion_id],
    )?;
    Ok(affected > 0)
}

/// Get all context files for prompt injection (text + image references).
pub fn get_context_files_for_prompt(conn: &Connection, discussion_id: &str) -> rusqlite::Result<Vec<crate::core::context_files::ContextEntry>> {
    let mut stmt = conn.prepare(
        "SELECT filename, extracted_text, disk_path FROM context_files WHERE discussion_id = ?1 ORDER BY created_at"
    )?;
    let rows = stmt.query_map(rusqlite::params![discussion_id], |row| {
        Ok(crate::core::context_files::ContextEntry {
            filename: row.get(0)?,
            text: row.get(1)?,
            disk_path: row.get(2)?,
        })
    })?.filter_map(|r| r.ok()).collect();
    Ok(rows)
}

#[cfg(test)]
#[path = "discussions_test.rs"]
mod discussions_test;
