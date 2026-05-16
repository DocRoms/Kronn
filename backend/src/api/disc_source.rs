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
        message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".to_string(),
        workspace_path: None,
        worktree_branch: None,
        tier: ModelTier::Reasoning,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscAppendResponse {
    pub appended: u32,
    pub skipped_as_duplicates: u32,
    /// When true, the disc has been edited inside Kronn since the
    /// last import — the caller should warn the user before pushing
    /// MORE messages (they might be applying stale state on top).
    pub diverged: bool,
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
    if !matches!(exists, Ok(Some(_))) {
        return match exists {
            Ok(None) => Json(ApiResponse::err("Discussion not found")),
            Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
            _ => unreachable!(),
        };
    }

    // 0.8.4 (#294) — `diverged_at` lives on the table but NOT on the
    // `Discussion` struct (see migration 054 + the model comment).
    // Read the column directly so we can warn the caller their import
    // is landing on a user-edited disc.
    let did_div = req.disc_id.clone();
    let diverged = state.db.with_conn(move |conn| {
        crate::db::disc_source::get_diverged_at(conn, &did_div)
    }).await.ok().flatten().is_some();
    let mut appended = 0u32;
    let mut skipped = 0u32;

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
        };
        let did_insert = did_for_loop.clone();
        let msg_clone = msg.clone();
        let insert_result = state.db.with_conn(move |conn| {
            crate::db::discussions::insert_message(conn, &did_insert, &msg_clone)
        }).await;
        if let Err(e) = insert_result {
            return Json(ApiResponse::err(format!("DB error appending message: {}", e)));
        }
        appended += 1;
    }

    Json(ApiResponse::ok(DiscAppendResponse {
        appended,
        skipped_as_duplicates: skipped,
        diverged,
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

    let msgs = non_system[(from as usize)..(to as usize)].iter().enumerate().map(|(rel, m)| {
        DiscLoadOtherMessage {
            idx: from + rel as u32,
            role: m.role.clone(),
            content: m.content.clone(),
            agent_type: m.agent_type.clone(),
            timestamp: m.timestamp.to_rfc3339(),
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
