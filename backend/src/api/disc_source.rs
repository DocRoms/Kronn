//! 0.8.4 (#294) — Cross-agent memory HTTP routes.
//!
//! 7 endpoints that let an external CLI agent (Claude Code, Cursor,
//! Codex, …) push its conversation history into Kronn so the SAME
//! discussion thread can be picked up by a DIFFERENT agent later.
//! Wired through `disc-introspection-mcp.py` so each route is also a
//! standard MCP tool reachable from any compatible agent runtime.
//!
//! Endpoints:
//!
//! - `POST /api/disc/create` — create a fresh disc, optionally bound
//!   to a source session.
//! - `POST /api/disc/append` — append messages, idempotent on
//!   `(disc_id, source_msg_id)`.
//! - `POST /api/disc/link` — bind an existing disc to a source session.
//! - `POST /api/disc/unlink` — release the binding.
//! - `GET  /api/disc/find_by_session` — lookup by
//!   (source_agent, source_session_id).
//! - `GET  /api/disc/search` — LIKE search across titles + content.
//! - `GET  /api/disc/load_other` — load N messages from a disc other
//!   than the current one.
//!
//! Each route returns the standard `ApiResponse<T>` envelope so the
//! MCP bridge can unwrap success/error uniformly.

use axum::{
    extract::{Query, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// Body of `POST /api/disc/create`. The triple `(source_agent,
/// source_session_id, project_id)` is enough to disambiguate: if a
/// disc already exists for the (agent, session) pair, we return its
/// id instead of creating a duplicate.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscCreateRequest {
    pub title: String,
    pub agent: AgentType,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    /// When set, the new disc is immediately bound to this
    /// (source_agent, source_session_id) pair.
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub source_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscCreateResponse {
    pub disc_id: String,
    /// `true` when a fresh row was inserted; `false` when an existing
    /// disc was returned because (source_agent, source_session_id)
    /// already mapped.
    pub created: bool,
}

/// `POST /api/disc/create`
pub async fn disc_create(
    State(state): State<AppState>,
    Json(req): Json<DiscCreateRequest>,
) -> Json<ApiResponse<DiscCreateResponse>> {
    // Idempotency: if a binding for this (agent, session) is already
    // open, return its disc rather than creating a duplicate. This is
    // what makes `disc_create` safe to call on every CLI session
    // bootstrap.
    if let (Some(src_agent), Some(src_sess)) = (req.source_agent.as_deref(), req.source_session_id.as_deref()) {
        let src_agent = src_agent.to_string();
        let src_sess = src_sess.to_string();
        let lookup = state.db.with_conn(move |conn| {
            crate::db::disc_source::find_disc_by_source_session(conn, &src_agent, &src_sess)
        }).await;
        if let Ok(Some(disc_id)) = lookup {
            return Json(ApiResponse::ok(DiscCreateResponse {
                disc_id,
                created: false,
            }));
        }
    }

    let now = Utc::now();
    let language = req.language.unwrap_or_else(|| "en".to_string());
    let disc_id = Uuid::new_v4().to_string();
    let agent = req.agent.clone();
    let disc = Discussion {
        id: disc_id.clone(),
        project_id: req.project_id.clone(),
        title: req.title.clone(),
        agent: agent.clone(),
        language,
        participants: vec![agent],
        messages: vec![],
        message_count: 0, non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".to_string(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Reasoning,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::default(),
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    let disc_for_insert = disc.clone();
    let inserted = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_discussion(conn, &disc_for_insert)?;
        Ok::<_, anyhow::Error>(())
    }).await;
    if let Err(e) = inserted {
        return Json(ApiResponse::err(format!("DB error inserting disc: {}", e)));
    }

    // Bind to source if requested. Failure to bind is fatal because
    // the caller is going to rely on `find_by_session` to find this
    // disc next time — silent skip would leave them orphaned.
    if let (Some(src_agent), Some(src_sess)) = (req.source_agent.clone(), req.source_session_id.clone()) {
        let disc_for_bind = disc_id.clone();
        let bind_result = state.db.with_conn(move |conn| {
            crate::db::disc_source::bind_to_source(conn, &disc_for_bind, &src_agent, &src_sess)
        }).await;
        if let Err(e) = bind_result {
            return Json(ApiResponse::err(format!("DB error binding source: {}", e)));
        }
    }

    Json(ApiResponse::ok(DiscCreateResponse {
        disc_id,
        created: true,
    }))
}

/// One message in a `disc_append` payload. `source_msg_id` is REQUIRED
/// because it's how the dedup pass works — without it we'd duplicate
/// every message on every reconnect.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendMessage {
    pub source_msg_id: String,
    pub role: MessageRole,
    pub content: String,
    #[serde(default)]
    pub agent_type: Option<AgentType>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendRequest {
    pub disc_id: String,
    pub messages: Vec<DiscAppendMessage>,
}

/// Compact lint feedback echoed to the POSTING agent (tool result), so it can
/// self-correct unverifiable `[src:]` citations in its next message. The full
/// report rides the stored message (UI badge), same as streaming replies.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppendLintSummary {
    pub fabricated_count: u32,
    pub unsourced_count: u32,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendResponse {
    pub appended: u32,
    pub skipped_as_duplicates: u32,
    /// When true, the disc has been edited inside Kronn since the
    /// last import — the caller should warn the user before pushing
    /// MORE messages (they might be applying stale state on top).
    pub diverged: bool,
    /// Present only for a live single Agent append whose lint had a signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub lint: Option<AppendLintSummary>,
    /// `sort_order` of the LAST appended message (stab-1). Long-polling
    /// callers must pass it as `since_sort_order` instead of estimating
    /// their position — estimates drift under concurrent posters and made
    /// agents silently skip messages. `None` when nothing was appended.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_sort_order: Option<i64>,
}

/// `POST /api/disc/append`
pub async fn disc_append(
    State(state): State<AppState>,
    Json(req): Json<DiscAppendRequest>,
) -> Json<ApiResponse<DiscAppendResponse>> {
    let did = req.disc_id.clone();
    let exists = state.db.with_conn(move |conn| {
        crate::db::discussions::get_discussion(conn, &did)
    }).await;
    let disc = match exists {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // 0.8.4 (#294) — `diverged_at` lives on the table but NOT on the
    // `Discussion` struct (see migration 054 + the model comment).
    // Read the column directly so we can warn the caller their import
    // is landing on a user-edited disc.
    let did_div = req.disc_id.clone();
    let diverged = state.db.with_conn(move |conn| {
        crate::db::disc_source::get_diverged_at(conn, &did_div)
    }).await.ok().flatten().is_some();
    // Lint-on-append (contract 2026-07-13): ONLY a live single Agent append
    // is linted — bulk imports, User/System messages and project-less discs
    // are exempt — and the insert is NEVER blocked. The full report rides the
    // stored message (UI badge); the summary rides the response (tool result)
    // so the posting agent can self-correct.
    let live_agent_append = req.messages.len() == 1
        && matches!(req.messages[0].role, MessageRole::Agent);
    let mut live_lint_report: Option<crate::core::anti_halluc::LintReport> = None;
    let mut lint_summary: Option<AppendLintSummary> = None;
    if live_agent_append && crate::core::anti_halluc::current_mode().is_active() {
        if let Some(pid) = disc.project_id.clone() {
            let roots = state.db.with_conn(move |conn| {
                let p = crate::db::projects::get_project(conn, &pid)?;
                Ok(p.map(|p| {
                    let linked = p.linked_repos.iter()
                        .map(|lr| lr.location.clone())
                        .filter(|loc| !loc.starts_with("http://") && !loc.starts_with("https://"))
                        .collect::<Vec<_>>();
                    (p.path, linked)
                }))
            }).await.ok().flatten();
            if let Some((project_path, linked)) = roots.filter(|(p, _)| !p.is_empty()) {
                live_lint_report = crate::core::anti_halluc::finalize_lint_report(
                    &req.messages[0].content,
                    None,
                    &project_path,
                    &linked,
                );
                // Echo a summary only when something actually FAILED — a
                // report with soft signals but zero failures would pair a
                // scary note with 0/0 counts (caught by live dogfooding).
                if let Some(ref r) = live_lint_report {
                    if r.fabricated_count > 0 || r.unsourced_count > 0 {
                        lint_summary = Some(AppendLintSummary {
                            fabricated_count: r.fabricated_count,
                            unsourced_count: r.unsourced_count,
                            note: "Some citations in your message could not be verified against the discussion's project tree — re-check the [src:] paths/lines and correct in your next message if needed.".into(),
                        });
                    }
                }
            }
        }
    }

    let mut appended = 0u32;
    let mut skipped = 0u32;
    let mut last_sort_order: Option<i64> = None;
    // Freshly-inserted messages, federated to peers after the loop IF this is a
    // single-message (live-turn) append on a shared disc — see the F3 gate below.
    let mut inserted_msgs: Vec<DiscussionMessage> = Vec::new();

    let did_for_loop = req.disc_id.clone();
    for incoming in req.messages.iter() {
        let did_check = did_for_loop.clone();
        let src_id_check = incoming.source_msg_id.clone();
        let already = state.db.with_conn(move |conn| {
            crate::db::disc_source::message_exists_for_source_id(conn, &did_check, &src_id_check)
        }).await.unwrap_or(false);
        if already {
            skipped += 1;
            continue;
        }

        let msg = DiscussionMessage {
            model: None,
            // Only a live single Agent append carries a report (loop runs once).
            lint_report: live_lint_report.take(),
            id: Uuid::new_v4().to_string(),
            role: incoming.role.clone(),
            content: incoming.content.clone(),
            agent_type: incoming.agent_type.clone(),
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: Some(incoming.source_msg_id.clone()),
            // 0.8.5 — imported messages don't carry an authoritative
            // wall-clock duration (the source CLI may not have tracked
            // it). Always NULL on import; metrics aggregator excludes
            // NULLs from the AVG so this doesn't skew per-version data.
            duration_ms: None,
        };
        let did_insert = did_for_loop.clone();
        let msg_clone = msg.clone();
        let insert_result = state.db.with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &did_insert, &msg_clone)
        }).await;
        match insert_result {
            Ok(sort_order) => last_sort_order = Some(sort_order),
            Err(e) => {
                return Json(ApiResponse::err(format!("DB error appending message: {}", e)))
            }
        }
        inserted_msgs.push(msg);
        appended += 1;
    }

    // Federate to peers via the shared helper (carries role + agent_type so an
    // agent reply lands as Agent, not User). F3: ONLY for a single-message
    // append — the live agent-turn case. Bulk transcript imports
    // (messages.len() > 1) are historical catch-up, not live chat: replaying N
    // frames would re-announce old turns AND can overflow the broadcast bus,
    // silently truncating the peer's copy.
    if req.messages.len() == 1 {
        if let Some(m) = inserted_msgs.first() {
            crate::api::federation::federate_message(&state, &req.disc_id, m).await;
        }
    }

    // Liveness heartbeat (migration 064). Posting is proof the agent is
    // alive — bump last_seen for each distinct agent_type that appended,
    // so `count_live_participants` (the double-responder guard) keeps
    // counting it as a live responder. Best-effort; a failure here must
    // not fail the append.
    if appended > 0 {
        let mut seen_agents = std::collections::HashSet::new();
        for incoming in req.messages.iter() {
            if let Some(at) = incoming.agent_type.clone() {
                let agent_type = format!("{at:?}");
                if seen_agents.insert(agent_type.clone()) {
                    let did_touch = req.disc_id.clone();
                    if let Err(e) = state
                        .db
                        .with_conn(move |conn| {
                            crate::db::discussion_sessions::touch_session_by_agent(
                                conn,
                                &did_touch,
                                &agent_type,
                            )?;
                            // 0.8.12 PR B — the agent just replied: the
                            // listening/reading placeholder vanishes the
                            // instant its message lands.
                            // No session id on this path — broad clear is
                            // the safe direction (a sibling's label returns
                            // at its next wait).
                            crate::db::discussion_sessions::clear_session_activity(
                                conn,
                                &did_touch,
                                &agent_type,
                                None,
                            )
                        })
                        .await
                    {
                        tracing::warn!("disc_append: failed to bump heartbeat / clear activity: {e}");
                    }
                }
            }
        }
    }

    Json(ApiResponse::ok(DiscAppendResponse {
        appended,
        skipped_as_duplicates: skipped,
        last_sort_order,
        diverged,
        lint: lint_summary,
    }))
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscLinkRequest {
    pub disc_id: String,
    pub source_agent: String,
    pub source_session_id: String,
}

/// `POST /api/disc/link`
pub async fn disc_link(
    State(state): State<AppState>,
    Json(req): Json<DiscLinkRequest>,
) -> Json<ApiResponse<bool>> {
    let result = state.db.with_conn(move |conn| {
        crate::db::disc_source::bind_to_source(conn, &req.disc_id, &req.source_agent, &req.source_session_id)
    }).await;
    match result {
        Ok(_) => Json(ApiResponse::ok(true)),
        Err(e) => Json(ApiResponse::err(format!("DB error linking: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscUnlinkRequest {
    pub disc_id: String,
}

/// `POST /api/disc/unlink`
pub async fn disc_unlink(
    State(state): State<AppState>,
    Json(req): Json<DiscUnlinkRequest>,
) -> Json<ApiResponse<bool>> {
    let result = state.db.with_conn(move |conn| {
        crate::db::disc_source::unbind_from_source(conn, &req.disc_id)
    }).await;
    match result {
        Ok(closed) => Json(ApiResponse::ok(closed)),
        Err(e) => Json(ApiResponse::err(format!("DB error unlinking: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscFindBySessionQuery {
    pub source_agent: String,
    pub source_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscFindBySessionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disc_id: Option<String>,
}

/// `GET /api/disc/find_by_session?source_agent=…&source_session_id=…`
pub async fn disc_find_by_session(
    State(state): State<AppState>,
    Query(q): Query<DiscFindBySessionQuery>,
) -> Json<ApiResponse<DiscFindBySessionResponse>> {
    let result = state.db.with_conn(move |conn| {
        crate::db::disc_source::find_disc_by_source_session(conn, &q.source_agent, &q.source_session_id)
    }).await;
    match result {
        Ok(disc_id) => Json(ApiResponse::ok(DiscFindBySessionResponse { disc_id })),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscSearchQuery {
    pub q: String,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `GET /api/disc/search?q=…&limit=…`
pub async fn disc_search(
    State(state): State<AppState>,
    Query(q): Query<DiscSearchQuery>,
) -> Json<ApiResponse<Vec<crate::db::disc_source::DiscSearchHit>>> {
    if q.q.trim().is_empty() {
        return Json(ApiResponse::err("query string `q` must not be empty"));
    }
    let limit = q.limit.unwrap_or(20);
    let result = state.db.with_conn(move |conn| {
        crate::db::disc_source::search_discussions(conn, &q.q, limit)
    }).await;
    match result {
        Ok(hits) => Json(ApiResponse::ok(hits)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct DiscLoadOtherQuery {
    pub disc_id: String,
    #[serde(default)]
    pub from: Option<u32>,
    #[serde(default)]
    pub to: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscLoadOtherMessage {
    pub idx: u32,
    pub role: MessageRole,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<AgentType>,
    pub timestamp: String,
    /// Files attached to this message (0.8.8). Mirrors `disc_get_message` so a
    /// cross-disc reader can discover an image's `disk_path` and open it with
    /// its file tools — without this, an agent browsing ANOTHER disc only sees
    /// the text and is blind to the attached images. Empty for most messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<crate::api::disc_introspection::MessageAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscLoadOtherResponse {
    pub disc_id: String,
    pub title: String,
    pub total_messages: u32,
    pub from_idx: u32,
    pub to_idx: u32,
    pub messages: Vec<DiscLoadOtherMessage>,
}

/// `GET /api/disc/sources`
///
/// 0.8.4 (#294) — batch endpoint that returns every currently-bound
/// disc with its source binding. The frontend sidebar calls this
/// once per mount to decorate disc rows with an "imported from X"
/// badge + drive the source-filter dropdown. Returns `[]` when no
/// disc has a binding (the common case on fresh installs).
pub async fn list_source_bindings(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<crate::db::disc_source::DiscSourceBinding>>> {
    let result = state.db.with_conn(|conn| {
        crate::db::disc_source::list_all_source_bindings(conn)
    }).await;
    match result {
        Ok(bindings) => Json(ApiResponse::ok(bindings)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscSourceDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<crate::db::disc_source::DiscSourceBinding>,
    pub history: Vec<crate::db::disc_source::DiscSourceHistoryEntry>,
}

/// `GET /api/discussions/{id}/source`
///
/// Returns the current binding (if any) + the full append-only
/// history chain for tooltip rendering ("first owned by ClaudeCode
/// sess A, then Cursor sess B"). Empty `history: []` for discs that
/// have never been imported.
pub async fn disc_source_detail(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Json<ApiResponse<DiscSourceDetail>> {
    let id_for_bindings = id.clone();
    let bindings = state.db.with_conn(move |conn| {
        crate::db::disc_source::list_all_source_bindings(conn)
    }).await.unwrap_or_default();
    let current = bindings.into_iter().find(|b| b.disc_id == id_for_bindings);

    let id_for_hist = id.clone();
    let history = state.db.with_conn(move |conn| {
        crate::db::disc_source::list_source_history(conn, &id_for_hist)
    }).await.unwrap_or_default();
    Json(ApiResponse::ok(DiscSourceDetail { current, history }))
}

/// `GET /api/disc/load_other?disc_id=…&from=…&to=…`
///
/// Defaults: `from=0`, `to=total` (full disc). Clamped to the actual
/// length so a curious caller can't OOM us with a huge range.
pub async fn disc_load_other(
    State(state): State<AppState>,
    Query(q): Query<DiscLoadOtherQuery>,
) -> Json<ApiResponse<DiscLoadOtherResponse>> {
    let did = q.disc_id.clone();
    let result = state.db.with_conn(move |conn| {
        crate::db::discussions::get_discussion(conn, &did)
    }).await;
    let disc = match result {
        Ok(Some(d)) => d,
        Ok(None) => return Json(ApiResponse::err("Discussion not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let non_system: Vec<&DiscussionMessage> = disc.messages.iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .collect();
    let total = non_system.len() as u32;
    let from = q.from.unwrap_or(0).min(total);
    let to = q.to.unwrap_or(total).min(total);
    let (from, to) = if from > to { (to, from) } else { (from, to) };

    // Group the disc's attachments by message id in one read, so each returned
    // message can carry the files pinned to it (0.8.8). list_context_files is
    // a single indexed query — cheaper than one query per message.
    let did_files = q.disc_id.clone();
    let files = state.db.with_conn(move |conn| {
        crate::db::discussions::list_context_files(conn, &did_files).map_err(|e| anyhow::anyhow!(e))
    }).await.unwrap_or_default();
    let mut by_msg: std::collections::HashMap<String, Vec<crate::api::disc_introspection::MessageAttachment>> =
        std::collections::HashMap::new();
    for f in files {
        if let Some(mid) = f.message_id.clone() {
            by_msg.entry(mid).or_default().push(crate::api::disc_introspection::MessageAttachment {
                id: f.id,
                filename: f.filename,
                mime_type: f.mime_type,
                disk_path: f.disk_path,
            });
        }
    }

    let msgs = non_system[(from as usize)..(to as usize)].iter().enumerate().map(|(rel, m)| {
        DiscLoadOtherMessage {
            idx: from + rel as u32,
            role: m.role.clone(),
            content: m.content.clone(),
            agent_type: m.agent_type.clone(),
            timestamp: m.timestamp.to_rfc3339(),
            attachments: by_msg.get(&m.id).cloned().unwrap_or_default(),
        }
    }).collect();

    Json(ApiResponse::ok(DiscLoadOtherResponse {
        disc_id: q.disc_id,
        title: disc.title,
        total_messages: total,
        from_idx: from,
        to_idx: to,
        messages: msgs,
    }))
}

#[cfg(test)]
mod tests {
    //! Route-level unit tests live in `backend/tests/api_tests.rs` —
    //! the in-memory DB integration there is what exercises the full
    //! HTTP→DB→response loop. This block only pins shape-level
    //! invariants (no I/O).

    use super::*;
    use serial_test::serial;

    /// In-memory state with a project rooted at a real tempdir + one disc.
    async fn lint_state(bind_project: bool) -> (crate::AppState, tempfile::TempDir) {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/real.rs"), "fn real() {}\n").unwrap();
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let path = tmp.path().to_string_lossy().to_string();
        let bind = bind_project;
        db.with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p-lint', 'LintProj', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language, participants_json,
                 created_at, updated_at, message_count, workspace_mode)
                 VALUES ('d-lint', ?1, 'T', 'ClaudeCode', 'fr', '[]', datetime('now'), datetime('now'), 0, 'Direct')",
                rusqlite::params![if bind { Some("p-lint") } else { None }],
            )?;
            Ok(())
        }).await.unwrap();
        let cfg = Arc::new(RwLock::new(crate::core::config::default_config()));
        (crate::AppState::new_defaults(cfg, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS), tmp)
    }

    fn agent_msg(id: &str, content: &str) -> DiscAppendMessage {
        DiscAppendMessage {
            source_msg_id: id.into(),
            role: MessageRole::Agent,
            content: content.into(),
            agent_type: Some(AgentType::Codex),
        }
    }

    async fn append(state: &crate::AppState, msgs: Vec<DiscAppendMessage>) -> DiscAppendResponse {
        let resp = disc_append(
            axum::extract::State(state.clone()),
            Json(DiscAppendRequest { disc_id: "d-lint".into(), messages: msgs }),
        ).await;
        resp.0.data.expect("append succeeds")
    }

    #[tokio::test]
    #[serial] // global anti-halluc mode cell
    async fn append_clears_the_activity_placeholder() {
        // 0.8.12 PR B (Copilot review): the REAL disc_append path must
        // clear the presence placeholder of the posting agent — the
        // "prépare une réponse" label vanishes the instant the reply lands.
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::discussion_sessions::join_disc_session(conn, "d-lint", "Codex", "s-x")
                    .map(|_| ())?;
                crate::db::discussion_sessions::set_session_activity(
                    conn, "d-lint", "Codex", None, "reading", 300,
                )
            })
            .await
            .unwrap();

        append(&state, vec![agent_msg("s-act", "voilà ma réponse")]).await;

        let activity = state
            .db
            .with_conn(|conn| crate::db::discussion_sessions::list_sessions(conn, "d-lint", false))
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.agent_type == "Codex")
            .and_then(|s| s.activity);
        assert!(activity.is_none(), "disc_append must clear the poster's activity");
    }

    #[tokio::test]
    #[serial] // global anti-halluc mode cell
    async fn append_returns_the_real_sort_order_of_the_last_message() {
        // stab-1 — agents estimated their position (+1 per post) because the
        // response carried no sort_order; concurrent posters made the
        // estimate drift and long-polls silently skipped messages.
        crate::core::anti_halluc::set_mode("off");
        let (state, _tmp) = lint_state(false).await;

        let first = append(&state, vec![agent_msg("s1", "un")]).await;
        let a = first.last_sort_order.expect("appended → position present");

        let second = append(&state, vec![agent_msg("s2", "deux"), agent_msg("s3", "trois")]).await;
        let b = second.last_sort_order.expect("batch → position of the LAST message");
        assert_eq!(b, a + 2, "two more rows after the first");

        // Pure duplicate: nothing appended → no position (the caller keeps
        // its previous marker).
        let dup = append(&state, vec![agent_msg("s3", "trois")]).await;
        assert_eq!(dup.skipped_as_duplicates, 1);
        assert!(dup.last_sort_order.is_none());
    }

    #[tokio::test]
    #[serial] // global anti-halluc mode cell
    async fn live_agent_append_with_fabricated_source_carries_lint() {
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(true).await;
        let out = append(&state, vec![agent_msg("m1",
            "Confirmed the bug. [src: file: src/does-not-exist.rs:42]")]).await;
        assert_eq!(out.appended, 1, "insert is NEVER blocked");
        let lint = out.lint.expect("fabricated citation must produce a summary");
        assert!(lint.fabricated_count >= 1, "{lint:?}");
        // The stored message carries the full report (UI badge).
        let msg = state.db.with_conn(|conn| {
            crate::db::discussions::list_messages(conn, "d-lint")
        }).await.unwrap().pop().unwrap();
        assert!(msg.lint_report.is_some());
        crate::core::anti_halluc::set_mode("off");
    }

    #[tokio::test]
    #[serial]
    async fn live_agent_append_with_valid_source_has_no_fabricated() {
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(true).await;
        let out = append(&state, vec![agent_msg("m1",
            "Verified. [src: file: src/real.rs:1]")]).await;
        assert_eq!(out.appended, 1);
        if let Some(l) = out.lint {
            assert_eq!(l.fabricated_count, 0, "valid citation must not read as fabricated: {l:?}");
        }
        crate::core::anti_halluc::set_mode("off");
    }

    #[tokio::test]
    #[serial]
    async fn bulk_import_and_user_messages_are_never_linted() {
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(true).await;
        // Bulk (2 messages) with a fabricated citation → no lint.
        let out = append(&state, vec![
            agent_msg("b1", "one [src: file: src/ghost.rs:1]"),
            agent_msg("b2", "two"),
        ]).await;
        assert!(out.lint.is_none(), "bulk import must not lint");
        // Single USER message with a fabricated citation → no lint.
        let user = DiscAppendMessage {
            source_msg_id: "u1".into(),
            role: MessageRole::User,
            content: "look at [src: file: src/ghost.rs:1]".into(),
            agent_type: None,
        };
        let out = append(&state, vec![user]).await;
        assert!(out.lint.is_none(), "user messages must not lint");
        crate::core::anti_halluc::set_mode("off");
    }

    #[tokio::test]
    #[serial]
    async fn projectless_disc_and_off_mode_skip_lint() {
        // Project-less disc: no roots → no lint, no false fabricated.
        crate::core::anti_halluc::set_mode("warn");
        let (state, _tmp) = lint_state(false).await;
        let out = append(&state, vec![agent_msg("m1", "x [src: file: src/ghost.rs:1]")]).await;
        assert!(out.lint.is_none(), "no project → no lint");
        // Mode off: bound project but lint disabled.
        crate::core::anti_halluc::set_mode("off");
        let (state2, _tmp2) = lint_state(true).await;
        let out = append(&state2, vec![agent_msg("m1", "x [src: file: src/ghost.rs:1]")]).await;
        assert!(out.lint.is_none(), "mode off → no lint");
    }


    #[test]
    fn disc_create_request_deserializes_with_optional_source_binding() {
        // Without source binding — pure local create.
        let minimal: DiscCreateRequest = serde_json::from_str(r#"{
            "title": "test",
            "agent": "ClaudeCode"
        }"#).expect("minimal create body must parse");
        assert_eq!(minimal.title, "test");
        assert!(minimal.source_agent.is_none());

        // With source binding — CLI-initiated import.
        let bound: DiscCreateRequest = serde_json::from_str(r#"{
            "title": "imported",
            "agent": "ClaudeCode",
            "source_agent": "ClaudeCode",
            "source_session_id": "abc-123"
        }"#).expect("bound create body must parse");
        assert_eq!(bound.source_agent.as_deref(), Some("ClaudeCode"));
        assert_eq!(bound.source_session_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn disc_append_requires_source_msg_id_per_entry() {
        // The dedup pass depends on `source_msg_id` being present —
        // missing it is a programmer error, not a runtime fallback.
        // serde_json refuses to deserialize without it.
        let bad = serde_json::from_str::<DiscAppendMessage>(r#"{
            "role": "User",
            "content": "no id"
        }"#);
        assert!(bad.is_err(), "missing source_msg_id must fail deser (dedup invariant)");
    }
}
