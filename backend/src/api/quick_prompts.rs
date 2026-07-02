use axum::{extract::{Path, State}, Json};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// GET /api/quick-prompts
pub async fn list(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<QuickPrompt>>> {
    match state.db.with_conn(crate::db::quick_prompts::list_quick_prompts).await {
        Ok(items) => Json(ApiResponse::ok(items)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/quick-prompts
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateQuickPromptRequest>,
) -> Json<ApiResponse<QuickPrompt>> {
    if req.name.is_empty() || req.name.len() > 200 {
        return Json(ApiResponse::err("Name must be 1-200 characters"));
    }
    if req.prompt_template.is_empty() {
        return Json(ApiResponse::err("Prompt template cannot be empty"));
    }

    let now = Utc::now();
    let qp = QuickPrompt {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        icon: req.icon.unwrap_or_else(|| "⚡".into()),
        prompt_template: req.prompt_template,
        variables: req.variables,
        agent: req.agent.unwrap_or(AgentType::ClaudeCode),
        project_id: req.project_id,
        skill_ids: req.skill_ids,
        profile_ids: req.profile_ids,
        directive_ids: req.directive_ids,
        tier: req.tier,
        agent_settings: req.agent_settings,
        description: req.description,
        created_at: now,
        updated_at: now,
    };

    let q = qp.clone();
    match state.db.with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(qp)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/quick-prompts/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CreateQuickPromptRequest>,
) -> Json<ApiResponse<QuickPrompt>> {
    let qp_id = id.clone();
    let existing = match state.db.with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &qp_id)).await {
        Ok(Some(qp)) => qp,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let updated = QuickPrompt {
        id: existing.id,
        name: if req.name.is_empty() { existing.name } else { req.name },
        icon: req.icon.unwrap_or(existing.icon),
        prompt_template: if req.prompt_template.is_empty() { existing.prompt_template } else { req.prompt_template },
        variables: req.variables,
        agent: req.agent.unwrap_or(existing.agent),
        project_id: req.project_id,
        skill_ids: req.skill_ids,
        profile_ids: req.profile_ids,
        directive_ids: req.directive_ids,
        tier: req.tier,
        agent_settings: req.agent_settings,
        // Description is always taken from the request, even if empty —
        // that's how the user clears it.
        description: req.description,
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let q = updated.clone();
    match state.db.with_conn(move |conn| crate::db::quick_prompts::update_quick_prompt(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(updated)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/quick-prompts/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::quick_prompts::delete_quick_prompt(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/quick-prompts/:id/history
///
/// 0.8.5 — returns the full version snapshot list for a QP, newest
/// first. Pre-0.8.5 QPs have no history (v1 is seeded by
/// `insert_quick_prompt` for new ones); the frontend handles the
/// empty case by showing "No version history yet".
pub async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::QuickPromptVersion>>> {
    match state.db.with_conn(move |conn| crate::db::quick_prompts::list_quick_prompt_versions(conn, &id)).await {
        Ok(v) => Json(ApiResponse::ok(v)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/quick-prompts/:id/versions/:version_index
///
/// 0.8.5 — remove an archived QP version from the history. The
/// CURRENT version (highest `version_index`) is refused — it's the
/// anchor for the live QP body. Discussions that referenced the
/// deleted version see their lineage cleared so the metrics aggregator
/// stops attributing those launches.
pub async fn delete_version(
    State(state): State<AppState>,
    Path((id, version_index)): Path<(String, u32)>,
) -> Json<ApiResponse<bool>> {
    match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::delete_quick_prompt_version(conn, &id, version_index)
    }).await {
        Ok(b) => Json(ApiResponse::ok(b)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// GET /api/quick-prompts/:id/metrics
///
/// 0.8.5 — aggregated launch metrics per QP version (avg tokens, avg
/// duration_ms, avg cost_usd, launch count). One row per version that
/// has ≥ 1 launch with `originating_qp_version` set. Versions with
/// zero launches are NOT returned — the frontend pairs the metrics
/// rows against the full version list from `/history` and renders
/// "no runs yet" where appropriate.
pub async fn metrics(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::QuickPromptVersionMetrics>>> {
    match state.db.with_conn(move |conn| crate::db::quick_prompts::list_quick_prompt_version_metrics(conn, &id)).await {
        Ok(v) => Json(ApiResponse::ok(v)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

const QP_EXPORT_KIND: &str = "kronn.quick_prompt";
const QP_EXPORT_VERSION: u32 = 1;

/// GET /api/quick-prompts/:id/export
///
/// Returns a self-contained `QuickPromptExportEnvelope` JSON download.
/// Mirror of [`crate::api::workflows::export_workflow`] for QPs — same
/// envelope discipline (`kind` + `version` + `exported_at`).
pub async fn export_qp(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let qp_id = id.clone();
    let qp = match state.db.with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &qp_id)).await {
        Ok(Some(qp)) => qp,
        Ok(None) => return (StatusCode::NOT_FOUND, "Quick prompt not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)).into_response(),
    };

    let envelope = QuickPromptExportEnvelope {
        kind: QP_EXPORT_KIND.to_string(),
        version: QP_EXPORT_VERSION,
        exported_at: Utc::now(),
        quick_prompt: qp.clone(),
    };

    let safe_name: String = qp.name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let filename = format!("{}.kronn-qp.json", safe_name);

    let body = match serde_json::to_string_pretty(&envelope) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("Serialization error: {}", e)).into_response(),
    };

    (
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename)),
        ],
        body,
    ).into_response()
}

/// POST /api/quick-prompts/import
///
/// Body: `ImportQuickPromptRequest { content, project_id }`. Mints a
/// fresh id + timestamps, attaches to `project_id` (or null), inserts.
pub async fn import_qp(
    State(state): State<AppState>,
    Json(req): Json<ImportQuickPromptRequest>,
) -> Json<ApiResponse<QuickPrompt>> {
    let envelope: QuickPromptExportEnvelope = match serde_json::from_str(&req.content) {
        Ok(env) => env,
        Err(e) => return Json(ApiResponse::err(format!("JSON invalide : {}", e))),
    };

    if envelope.kind != QP_EXPORT_KIND {
        return Json(ApiResponse::err(format!(
            "Type incorrect : attendu `{}`, reçu `{}`. Vérifie que tu importes bien un Quick Prompt exporté depuis Kronn.",
            QP_EXPORT_KIND, envelope.kind
        )));
    }
    if envelope.version > QP_EXPORT_VERSION {
        return Json(ApiResponse::err(format!(
            "Version d'export non supportée ({} > {} max). Mets à jour Kronn pour importer ce fichier.",
            envelope.version, QP_EXPORT_VERSION
        )));
    }

    let mut qp = envelope.quick_prompt;
    if qp.name.trim().is_empty() {
        return Json(ApiResponse::err("Le Quick Prompt importé n'a pas de nom — fichier corrompu ?"));
    }
    if qp.prompt_template.trim().is_empty() {
        return Json(ApiResponse::err("Le Quick Prompt importé n'a pas de prompt template — fichier corrompu ?"));
    }

    let now = Utc::now();
    qp.id = Uuid::new_v4().to_string();
    qp.project_id = req.project_id;
    qp.created_at = now;
    qp.updated_at = now;

    let q = qp.clone();
    match state.db.with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &q)).await {
        Ok(()) => Json(ApiResponse::ok(qp)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Batch execution — fan out a Quick Prompt to N discussions in parallel
// ═══════════════════════════════════════════════════════════════════════════════

/// One item in the batch list: the title for the discussion and the fully
/// rendered user prompt. The frontend does the template rendering (it already
/// has `renderTemplate` from the QP launch flow) so the backend just receives
/// a list of already-filled prompts.
#[derive(Debug, Deserialize)]
pub struct BatchItem {
    pub title: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
pub struct BatchRunRequest {
    pub items: Vec<BatchItem>,
    /// Display name for the batch group in the sidebar.
    /// Example: "Cadrage to-Frame — 10 avr 14:00"
    pub batch_name: String,
    /// Optional project ID to attach all child discussions to.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Workspace mode for each child discussion: `"Direct"` (default) or
    /// `"Isolated"` for per-disc git worktrees. Isolated is required when
    /// the agents will write code in parallel.
    #[serde(default)]
    pub workspace_mode: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct BatchRunResponse {
    pub run_id: String,
    pub discussion_ids: Vec<String>,
    pub batch_total: u32,
}

/// POST /api/quick-prompts/:id/batch
///
/// Create N child discussions from a Quick Prompt + list of pre-rendered
/// prompts. All discussions are linked to a single batch WorkflowRun so the
/// frontend can group them in the sidebar and track progress live.
///
/// The actual agent runs are NOT started here — the frontend walks the
/// returned `discussion_ids` list and hits `POST /api/discussions/:id/run`
/// on each, honoring the existing `agent_semaphore` for parallelism control.
/// This keeps the backend simple and reuses the per-disc streaming pipeline
/// unchanged.
pub async fn batch_run(
    State(state): State<AppState>,
    Path(qp_id): Path<String>,
    Json(req): Json<BatchRunRequest>,
) -> Json<ApiResponse<BatchRunResponse>> {
    // Hard cap to prevent accidental megabatches
    const MAX_BATCH_SIZE: usize = 50;
    if req.items.is_empty() {
        return Json(ApiResponse::err("Batch must contain at least 1 item"));
    }
    if req.items.len() > MAX_BATCH_SIZE {
        return Json(ApiResponse::err(format!(
            "Batch too large: {} items (max {})", req.items.len(), MAX_BATCH_SIZE
        )));
    }
    if req.batch_name.trim().is_empty() {
        return Json(ApiResponse::err("batch_name is required"));
    }

    // Load the QP to get agent + skill_ids + tier
    let qp_lookup = qp_id.clone();
    let qp = match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup)
    }).await {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // Read user identity for message attribution
    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };

    // Delegate to the shared pure fn — same logic as the workflow step executor.
    let batch_name_for_log = req.batch_name.clone();
    let qp_name_for_log = qp.name.clone();
    let items: Vec<crate::db::workflows::BatchItemInput> = req.items.into_iter()
        .map(|i| crate::db::workflows::BatchItemInput {
            title: i.title,
            prompt: i.prompt,
            agent_override: None, // classic batch mode = same agent for all items
        })
        .collect();
    let workspace_mode = req.workspace_mode.unwrap_or_else(|| "Direct".into());

    // Safety: Isolated mode needs a project (git repo) to worktree against.
    // Check the effective project_id (request override OR QP default).
    if workspace_mode == "Isolated"
        && req.project_id.is_none()
        && qp.project_id.is_none()
    {
        return Json(ApiResponse::err(
            "Isolated workspace mode requires a project_id (the Quick Prompt or the batch request must target a git-backed project)"
        ));
    }

    let outcome = match state.db.with_conn(move |conn| {
        crate::db::workflows::create_batch_run(conn, crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp,
            items,
            batch_name: Some(req.batch_name),
            project_id: req.project_id,
            parent_run_id: None,
            author_pseudo,
            author_avatar_email,
            language: "fr".into(),
            workspace_mode,
        })
    }).await {
        Ok(o) => o,
        Err(e) => return Json(ApiResponse::err(format!("Failed to create batch: {}", e))),
    };

    tracing::info!(
        "Created batch run {} with {} discussions (QP: {}, name: {})",
        outcome.run_id, outcome.batch_total, qp_name_for_log, batch_name_for_log
    );

    Json(ApiResponse::ok(BatchRunResponse {
        run_id: outcome.run_id,
        discussion_ids: outcome.discussion_ids,
        batch_total: outcome.batch_total,
    }))
}

// ─── Compare-agents mode (2026-05-10) ───────────────────────────────────
//
// Fan out the SAME prompt across N agents in parallel — one
// discussion per agent, all linked under a single batch group. Lets
// the user click between siblings and read responses side-by-side
// without rewriting the QP per agent.
//
// vs. the regular `batch_run`: regular = N inputs × 1 agent (vary
// input). Compare = 1 input × N agents (vary agent). Both reuse
// `create_batch_run` with the new `agent_override` per item.

#[derive(Debug, Deserialize)]
pub struct CompareAgentsRequest {
    /// Pre-rendered prompt — caller has already substituted any
    /// QP variables. We don't re-render here so the same prompt
    /// hits every agent verbatim.
    pub prompt: String,
    /// Display name for the batch group, e.g.
    /// "Compare · summarise PR #42 · 14:00".
    pub batch_name: String,
    /// Tier override for all child discussions. Optional — falls
    /// back to the QP's default tier when None.
    #[serde(default)]
    pub tier: Option<ModelTier>,
    /// Subset of agents to compare. Frontend usually sends every
    /// installed+enabled agent, but a power user could pick e.g.
    /// `["ClaudeCode", "Codex"]` to skip the heavier ones.
    pub agents: Vec<AgentType>,
    /// Optional project to attach all child discussions to.
    /// Overrides the QP's default project when set.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// `POST /api/quick-prompts/:id/compare-agents` — Compare-agents
/// batch fan-out (Phase 1 of `project_qp_compare_agents`).
pub async fn compare_agents(
    State(state): State<AppState>,
    Path(qp_id): Path<String>,
    Json(req): Json<CompareAgentsRequest>,
) -> Json<ApiResponse<BatchRunResponse>> {
    // Hard cap mirrors `batch_run`'s — keeps a runaway "compare 50
    // agents" from blowing up the agent semaphore.
    const MAX_BATCH_SIZE: usize = 50;
    if req.agents.is_empty() {
        return Json(ApiResponse::err("Compare-agents needs at least 1 agent in the list"));
    }
    if req.agents.len() > MAX_BATCH_SIZE {
        return Json(ApiResponse::err(format!(
            "Compare-agents too large: {} agents (max {})",
            req.agents.len(), MAX_BATCH_SIZE
        )));
    }
    if req.prompt.trim().is_empty() {
        return Json(ApiResponse::err("Prompt is required"));
    }
    if req.batch_name.trim().is_empty() {
        return Json(ApiResponse::err("batch_name is required"));
    }

    // De-dupe agents — the frontend usually filters but a paranoid
    // guard avoids 2 disc on the same agent.
    let mut seen = std::collections::HashSet::new();
    let agents: Vec<AgentType> = req.agents.into_iter()
        .filter(|a| seen.insert(format!("{:?}", a)))
        .collect();

    // Load the QP for skill_ids + tier defaults + (optional)
    // project_id fallback.
    let qp_lookup = qp_id.clone();
    let qp = match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup)
    }).await {
        Ok(Some(q)) => q,
        Ok(None) => return Json(ApiResponse::err("Quick prompt not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    // If the request specified a tier, write it onto a cloned QP
    // so each child discussion picks it up via `qp.tier`. The DB
    // QP isn't mutated.
    let mut qp_for_run = qp.clone();
    if let Some(t) = req.tier {
        qp_for_run.tier = t;
    }

    let (author_pseudo, author_avatar_email) = {
        let config = state.config.read().await;
        (config.server.pseudo.clone(), config.server.avatar_email.clone())
    };

    // Build one item per agent — same prompt, same title prefix,
    // agent suffix so the user can tell siblings apart in the
    // sidebar.
    let prompt = req.prompt.clone();
    let qp_display_name = qp.name.clone();
    let items: Vec<crate::db::workflows::BatchItemInput> = agents.into_iter()
        .map(|agent| {
            let agent_label = format!("{:?}", agent); // ClaudeCode, Codex, …
            crate::db::workflows::BatchItemInput {
                title: format!("{} · {}", qp_display_name, agent_label),
                prompt: prompt.clone(),
                agent_override: Some(agent),
            }
        })
        .collect();

    let batch_total = items.len() as u32;
    let outcome = match state.db.with_conn(move |conn| {
        crate::db::workflows::create_batch_run(conn, crate::db::workflows::CreateBatchRunInput {
            quick_prompt: &qp_for_run,
            items,
            batch_name: Some(req.batch_name.clone()),
            project_id: req.project_id,
            parent_run_id: None,
            author_pseudo,
            author_avatar_email,
            language: "fr".into(),
            workspace_mode: "Direct".into(),
        })
    }).await {
        Ok(o) => o,
        Err(e) => return Json(ApiResponse::err(format!("Failed to create compare-agents batch: {}", e))),
    };

    tracing::info!(
        "Created compare-agents batch {} with {} discussions (QP: {})",
        outcome.run_id, batch_total, qp.name,
    );

    Json(ApiResponse::ok(BatchRunResponse {
        run_id: outcome.run_id,
        discussion_ids: outcome.discussion_ids,
        batch_total: outcome.batch_total,
    }))
}
