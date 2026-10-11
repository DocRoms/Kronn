// Pre-audit briefing: six product/team questions plus closed project cards
// (KT-1173), written to `docs/briefing.md`.
// The audit later reads `docs/briefing.md` (via
// `projects::resolve_briefing_notes`) and injects the user's answers
// into every step prompt.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

/// GET /api/projects/:id/briefing
/// Returns the briefing notes for a project.
pub async fn get_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<String>>> {
    match state
        .db
        .with_conn(move |conn| crate::db::projects::get_project_briefing_notes(conn, &id))
        .await
    {
        Ok(notes) => Json(ApiResponse::ok(notes)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/projects/:id/briefing
/// Sets or clears the briefing notes for a project. The audit reads
/// `docs/briefing.md` before the DB column, so the file is written (or
/// removed) first: a PUT that only touched the DB was silently ignored once
/// the file existed (AUD-14).
pub async fn set_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetBriefingRequest>,
) -> Json<ApiResponse<bool>> {
    if audit_running(&state, &id) {
        return Json(ApiResponse::err(BRIEFING_LOCKED));
    }
    let notes = req.notes.filter(|n| !n.trim().is_empty());
    let pid = id.clone();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .ok()
        .flatten();
    let Some(project) = project else {
        return Json(ApiResponse::err("Project not found"));
    };
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    if let Err(e) = sync_briefing_file(&project_path, notes.as_deref()) {
        return Json(ApiResponse::err(e));
    }
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, &id, notes.as_deref())
        })
        .await
    {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

pub(crate) const BRIEFING_LOCKED: &str =
    "The briefing is locked while an audit runs on this project: save it again once the audit ends.";

/// An audit holds the project's lease: it read the briefing at launch and
/// keeps syncing the coding-rules doc from it, so a save now would be undone.
pub(crate) fn audit_running(state: &AppState, project_id: &str) -> bool {
    state
        .audit_tracker
        .lock()
        .map(|t| t.leased.contains(project_id))
        .unwrap_or(true)
}

/// Mirror the notes into `<docs>/briefing.md`: written when set, removed when
/// cleared. No-op before the project has a docs directory (the DB column is
/// then the only source, which `resolve_briefing_notes` falls back to).
pub(crate) fn sync_briefing_file(
    project_path: &std::path::Path,
    notes: Option<&str>,
) -> Result<(), String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    if !docs_dir.is_dir() {
        return Ok(());
    }
    let target = docs_dir.join("briefing.md");
    crate::core::fs_guard::assert_contained_no_symlink(project_path, &target)?;
    match notes {
        Some(body) => std::fs::write(&target, body)
            .map_err(|e| format!("Failed to write {}: {}", target.display(), e)),
        None => match std::fs::remove_file(&target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to remove {}: {}", target.display(), e)),
        },
    }
}

/// Longest briefing, in characters, injected into each audit step prompt.
pub(crate) const BRIEFING_PROMPT_MAX_CHARS: usize = 6_000;

/// The `## Project briefing` block appended to an audit step prompt, capped
/// at [`BRIEFING_PROMPT_MAX_CHARS`] with an explicit truncation marker. The
/// machine copy of the project answers is dropped: the readable facts above
/// it say the same thing. The documentation section stays out (KT-999).
pub(crate) fn briefing_prompt_section(
    notes: &str,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> String {
    let notes = super::briefing_docs::strip_section(notes);
    let notes = super::briefing_rules::strip_section(&notes);
    let notes = super::project_doc::strip_answers_line(&notes);
    let notes = notes.trim_end();
    let total = notes.chars().count();
    let body = if total <= BRIEFING_PROMPT_MAX_CHARS {
        notes.to_string()
    } else {
        let mut cut: String = notes.chars().take(BRIEFING_PROMPT_MAX_CHARS).collect();
        // End on a line break when one is close, so no line is cut mid-way.
        if let Some(i) = cut.rfind('\n') {
            if i >= cut.len() * 4 / 5 {
                cut.truncate(i);
            }
        }
        let shown = cut.chars().count();
        format!(
            "{cut}\n\n[Briefing truncated by Kronn: {shown} of {total} characters shown. Read {} for the rest.]",
            docs_root.path("briefing.md")
        )
    };
    format!("\n\n## Project briefing (from the user)\n{body}\n")
}

/// After `KRONN:BRIEFING_COMPLETE`: store the reviewed `docs/briefing.md`,
/// with the form's project facts put back as answered.
pub(crate) async fn complete_reviewed_briefing(
    state: &AppState,
    project_id: String,
    briefing_file: std::path::PathBuf,
) {
    let file = briefing_file.clone();
    let reviewed = tokio::task::spawn_blocking(move || std::fs::read_to_string(&file).ok())
        .await
        .unwrap_or(None);
    let Some(reviewed) = reviewed else {
        tracing::warn!(
            "BRIEFING_COMPLETE detected but docs/briefing.md not found for project {}",
            project_id
        );
        return;
    };
    let pid = project_id.clone();
    let previous = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project_briefing_notes(conn, &pid))
        .await
        .ok()
        .flatten();
    let content = super::project_doc::reattach_facts(previous.as_deref(), &reviewed);
    if content != reviewed {
        let file = briefing_file.clone();
        let body = content.clone();
        if let Ok(Err(e)) = tokio::task::spawn_blocking(move || std::fs::write(&file, body)).await {
            tracing::warn!("Failed to restore the project facts in docs/briefing.md: {e}");
        }
    }
    let pid = project_id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, &pid, Some(&content))
        })
        .await
    {
        Ok(_) => tracing::info!("Briefing notes saved for project {}", project_id),
        Err(e) => tracing::error!(
            "Failed to save briefing notes for project {}: {e}",
            project_id
        ),
    }
}

/// POST /api/projects/:id/start-briefing
/// Creates a conversational briefing discussion for a project.
pub async fn start_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Json<ApiResponse<StartBriefingResponse>> {
    // 1. Look up the project
    let pid = id.clone();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .ok()
        .flatten();

    let Some(project) = project else {
        return Json(ApiResponse::err("Project not found"));
    };

    // 2. Get language
    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    // 3. Build briefing prompt. If the user already submitted the form
    //    (`docs/briefing.md` exists with content OR DB notes are set),
    //    switch to the SHORT review prompt — the agent reads back the
    //    answers, asks at most 2-3 targeted clarifications, then
    //    finalizes. Without this guard the agent re-asks the 6 questions
    //    the user just answered (the original UX confusion).
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    let prefilled =
        crate::api::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);
    // The documentation section is put back by `reattach_facts` after the review.
    let prefilled = prefilled.map(|notes| super::briefing_docs::strip_section(&notes));
    let briefing_prompt = super::helpers::build_briefing_prompt_in(
        &language,
        prefilled.as_deref(),
        &crate::core::docs_location::DocsRoot::of(&project_path),
    );

    // 4. Create discussion
    let now = Utc::now();
    let discussion_id = Uuid::new_v4().to_string();
    let briefing_tier = req.tier.unwrap_or_default();
    let agent_type = req.agent;

    let initial_message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content: briefing_prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };

    let title = match language.as_str() {
        "en" => "Project Briefing".to_string(),
        "es" => "Briefing del proyecto".to_string(),
        _ => "Briefing projet".to_string(),
    };

    let discussion = Discussion {
        connection_id: None,
        awaiting_agent: false,
        agent_running: false,
        id: discussion_id.clone(),
        project_id: Some(project.id.clone()),
        title,
        agent: agent_type.clone(),
        language: language.clone(),
        participants: vec![agent_type],
        messages: vec![initial_message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: briefing_tier,
        model: None,
        pin_first_message: true,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: crate::models::SummaryStrategy::OnDemand,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    let disc = discussion.clone();
    let msg = initial_message;
    if let Err(e) = state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
            Ok(())
        })
        .await
    {
        return Json(ApiResponse::err(format!(
            "Failed to create discussion: {}",
            e
        )));
    }

    Json(ApiResponse::ok(StartBriefingResponse { discussion_id }))
}

/// 0.8.4 (#285) — désagentified briefing : 6 réponses utilisateur →
/// `docs/briefing.md` + `projects.briefing_notes` directement, sans
/// passer par une discussion LLM. Token cost = 0, instantané, et le
/// user peut éditer son briefing sans avoir à relancer une conv.
///
/// Le format markdown produit ici est BYTE-FOR-BYTE compatible avec
/// celui généré par l'ancien flow conversationnel — l'audit consomme
/// le même fichier via `projects::resolve_briefing_notes` (audit/full.rs
/// Phase 1 + chaque step prompt). Aucun changement nécessaire en aval.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct BriefingFormRequest {
    pub purpose: String,
    pub team: String,
    pub maturity: String,
    pub dependencies: String,
    pub traps: String,
    pub additional: String,
    /// KT-1173 — closed project cards; absent from older clients.
    #[serde(default)]
    pub project: Option<BriefingProjectAnswers>,
    /// KT-1167 — after a successful Full audit, turn the TDs into Kronn tasks.
    #[serde(default)]
    pub td_to_tasks: bool,
    /// KT-1183 — experimental cross-agent audit: 2 or 3 agents, empty = off.
    #[serde(default)]
    pub cross_audit_agents: Vec<AgentType>,
    /// KT-999 — documentation locations and their state; absent from older clients.
    #[serde(default)]
    pub existing_docs: Option<BriefingDocsAnswers>,
    /// KT-1193 — rules and bans the team states about the code; optional.
    #[serde(default)]
    pub project_rules: Option<BriefingProjectRules>,
    /// KT-1192 — « Dossier de la documentation IA », relative to the project
    /// root. Absent from older clients: the folder is left as it is.
    #[serde(default)]
    pub docs_dir: Option<String>,
}

/// Why a briefing was not saved.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BriefingSaveError {
    /// An answer is missing or malformed.
    Invalid(String),
    /// The docs folder leaves the project, or cannot be checked (HTTP 400).
    DocsDir(String),
    /// A listed documentation location is refused (HTTP 400, KT-999).
    ExistingDocs(String),
    NotFound,
    Internal(String),
}

impl BriefingSaveError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Invalid(m) | Self::DocsDir(m) | Self::ExistingDocs(m) | Self::Internal(m) => {
                m.clone()
            }
            Self::NotFound => "Project not found".into(),
        }
    }
}

/// The folder the briefing proposes, and where the proposal comes from.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BriefingDefaults {
    /// Relative to the project root, without a trailing slash.
    pub docs_dir: String,
    pub docs_dir_source: BriefingDocsDirSource,
    /// The folder already exists in the project.
    pub folder_exists: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BriefingDocsDirSource {
    /// The project's own override.
    ProjectOverride,
    /// A folder the project already documents itself in.
    Existing,
    /// The global default from the settings.
    GlobalDefault,
    /// `docs`.
    Default,
}

/// The pre-fill of « Dossier de la documentation IA »: the project's
/// override, else the folder it already documents itself in, else the global
/// default, else `docs` (KT-1192). An existing folder wins over the global
/// default so a documented project is never proposed a move.
pub(crate) fn briefing_defaults(
    project_root: &std::path::Path,
    stored_override: Option<&str>,
    global_default: Option<&str>,
) -> BriefingDefaults {
    use crate::core::docs_location;
    let (docs_dir, docs_dir_source) = if let Some(rel) =
        stored_override.and_then(|d| docs_location::normalize_relative(d).ok())
    {
        (rel, BriefingDocsDirSource::ProjectOverride)
    } else {
        let detected = docs_location::resolve(project_root, None);
        match detected
            .relative(project_root)
            .filter(|_| detected.dir.is_dir())
        {
            Some(rel) => (rel, BriefingDocsDirSource::Existing),
            None => match global_default.and_then(|d| docs_location::normalize_relative(d).ok()) {
                Some(rel) => (rel, BriefingDocsDirSource::GlobalDefault),
                None => (
                    docs_location::DocsRoot::DEFAULT.to_string(),
                    BriefingDocsDirSource::Default,
                ),
            },
        }
    };
    BriefingDefaults {
        folder_exists: project_root.join(&docs_dir).is_dir(),
        docs_dir,
        docs_dir_source,
    }
}

/// GET /api/projects/:id/briefing-defaults
pub async fn get_briefing_defaults(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<BriefingDefaults>> {
    let lookup = id.clone();
    let found = state
        .db
        .with_read_conn(move |conn| {
            let Some(project) = crate::db::projects::get_project(conn, &lookup)? else {
                return Ok(None);
            };
            let dir = crate::db::projects::project_docs_dir(conn, &lookup)?;
            Ok(Some((project.path, dir)))
        })
        .await;
    let (path, stored) = match found {
        Ok(Some(found)) => found,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Project not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("DB error: {e}"))),
    };
    let global = state.config.read().await.scan.default_docs_dir.clone();
    let root = crate::core::scanner::resolve_host_path(&path);
    Json(ApiResponse::ok(briefing_defaults(
        &root,
        stored.as_deref(),
        global.as_deref(),
    )))
}

/// Record the folder the briefing chose. Stored as the project's override
/// only when it differs from what the project resolves to without one, so a
/// briefing that keeps the existing layout changes nothing.
async fn apply_briefing_docs_dir(
    state: &AppState,
    project: &Project,
    requested: &str,
) -> Result<(), BriefingSaveError> {
    use crate::core::docs_location;
    let root = crate::core::scanner::resolve_host_path(&project.path);
    let requested = requested.to_string();
    let checked = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || {
            let rel = docs_location::validate_for_project(&root, &requested)?;
            let unconfigured = docs_location::resolve(&root, None).relative(&root);
            Ok::<_, String>((unconfigured.as_deref() != Some(rel.as_str())).then_some(rel))
        })
        .await
        .unwrap_or_else(|e| Err(format!("check failed: {e}")))
    };
    let stored = checked.map_err(BriefingSaveError::DocsDir)?;
    let id = project.id.clone();
    let value = stored.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::projects::set_project_docs_dir(conn, &id, value.as_deref())
        })
        .await
        .map_err(|e| BriefingSaveError::Internal(format!("DB error: {e}")))?;
    docs_location::set_project_override(&project.path, stored.as_deref());
    Ok(())
}

/// Save the briefing answers: validation, docs folder, `briefing.md` and the
/// DB notes. Shared by the form and the briefing cards (KT-1192).
pub(crate) async fn save_briefing_answers(
    state: &AppState,
    id: &str,
    form: BriefingFormRequest,
) -> Result<(), BriefingSaveError> {
    // 1. Validate: Q1-Q5 are mandatory (matches the conversational flow).
    if form.purpose.trim().is_empty()
        || form.team.trim().is_empty()
        || form.maturity.trim().is_empty()
        || form.dependencies.trim().is_empty()
        || form.traps.trim().is_empty()
    {
        return Err(BriefingSaveError::Invalid(
            "Q1-Q5 are mandatory (purpose, team, maturity, dependencies, traps). \
             Q6 (additional) is optional. Use \"Not provided\" or \"None\" if you \
             want to skip a mandatory question explicitly."
                .into(),
        ));
    }

    if !form.cross_audit_agents.is_empty() {
        let names: Vec<String> = form
            .cross_audit_agents
            .iter()
            .map(crate::core::cross_audit::agent_label)
            .collect();
        if crate::core::cross_audit::parse_agent_list(&names.join(",")).is_none() {
            return Err(BriefingSaveError::Invalid(
                "The cross-agent audit needs 2 or 3 distinct audit-capable agents (a Custom connection cannot be one).".into(),
            ));
        }
    }

    let project_facts = match form
        .project
        .as_ref()
        .map(super::project_doc::normalize_answers)
    {
        None => None,
        Some(Ok(answers)) => super::project_doc::render_facts_section(&answers),
        Some(Err(e)) => return Err(BriefingSaveError::Invalid(e)),
    };
    let project_rules = match form
        .project_rules
        .as_ref()
        .map(super::briefing_rules::normalize)
    {
        None => None,
        Some(Ok(rules)) => Some(rules),
        Some(Err(e)) => {
            return (StatusCode::BAD_REQUEST, Json(ApiResponse::<bool>::err(e))).into_response()
        }
    };
    let rules_section = project_rules
        .as_ref()
        .and_then(super::briefing_rules::render_section);

    // 2. Look up project for the on-disk path.
    let pid = id.to_string();
    let project = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &pid))
        .await
        .ok()
        .flatten()
        .ok_or(BriefingSaveError::NotFound)?;

    // A location outside the repository, missing or contradicting another
    // source of truth is refused before anything is written.
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    let docs_section = match form.existing_docs.as_ref() {
        None => None,
        Some(docs) => super::briefing_docs::normalize_with_dates(&project_path, docs)
            .await
            .map_err(BriefingSaveError::ExistingDocs)
            .map(|docs| super::briefing_docs::render_section(&docs))?,
    };

    // 3. The docs folder first: `briefing.md` is written into it.
    if let Some(dir) = form
        .docs_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        apply_briefing_docs_dir(state, &project, dir).await?;
    }

    // 4. Format the briefing.md body — same shape as
    // `build_briefing_prompt`'s STEP 2 instructions (EN spelling
    // because that's the canonical format the audit reads).
    let trap_lines = form.traps.trim();
    let traps_rendered = if trap_lines.is_empty() {
        "Not provided".to_string()
    } else {
        trap_lines.to_string()
    };
    let additional_rendered = if form.additional.trim().is_empty() {
        "None.".to_string()
    } else {
        form.additional.trim().to_string()
    };

    let mut body = format!(
        "# Project Briefing\n\
         > Auto-generated by Kronn briefing form (no AI call). Source: user answers.\n\n\
         ## Purpose\n{}\n\n\
         ## Team\n{}\n\n\
         ## Maturity\n{}\n\n\
         ## External Dependencies\n{}\n\n\
         ## Traps & Fragile Areas\n{}\n\n\
         ## Additional Context\n{}\n",
        form.purpose.trim(),
        form.team.trim(),
        form.maturity.trim(),
        form.dependencies.trim(),
        traps_rendered,
        additional_rendered,
    );
    for section in [project_facts, docs_section].into_iter().flatten() {
        body.push('\n');
        body.push_str(&section);
    }
    let body = with_all_kronn_options(body, form.td_to_tasks, &form.cross_audit_agents);

    // 5. Write `briefing.md` on disk + persist briefing_notes in DB.
    // The on-disk file is what the audit reads at Phase 1; the DB
    // column is what the ProjectCard / API surfaces back to the UI.
    // Pre-bootstrap projects have no docs folder yet: the DB only, and the
    // audit's Phase 1 install picks it up via `resolve_briefing_notes`.
    let docs_dir = crate::core::scanner::detect_docs_dir(&project_path);
    if docs_dir.is_dir() {
        let target = docs_dir.join("briefing.md");
        std::fs::write(&target, &body).map_err(|e| {
            BriefingSaveError::Internal(format!("Failed to write {}: {}", target.display(), e))
        })?;
    }
    let id = id.to_string();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, &id, Some(&body))
        })
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(BriefingSaveError::NotFound),
        Err(e) => Err(BriefingSaveError::Internal(format!("DB error: {}", e))),
    }
}

pub async fn save_briefing_form(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(form): Json<BriefingFormRequest>,
) -> Response {
    match save_briefing_answers(&state, &id, form).await {
        Ok(()) => Json(ApiResponse::ok(true)).into_response(),
        Err(BriefingSaveError::DocsDir(reason)) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<bool>::err_coded(
                ApiErrorCode::Validation,
                reason,
            )),
        )
            .into_response(),
        Err(BriefingSaveError::ExistingDocs(reason)) => {
            (StatusCode::BAD_REQUEST, Json(ApiResponse::<bool>::err(reason))).into_response()
        }
        Err(error) => Json(ApiResponse::<bool>::err(error.message())).into_response(),
    }
}

/// `docs/briefing.md` from the form: the six questions, then the rendered
/// form sections in their order, then the Kronn options.
pub(crate) fn compose_briefing(
    form: &BriefingFormRequest,
    project_facts: Option<String>,
    docs_section: Option<String>,
    rules_section: Option<String>,
) -> String {
    let traps = form.traps.trim();
    let traps_rendered = if traps.is_empty() {
        "Not provided"
    } else {
        traps
    };
    let additional = form.additional.trim();
    let additional_rendered = if additional.is_empty() {
        super::briefing_load::ADDITIONAL_NONE
    } else {
        additional
    };
    let mut body = super::briefing_load::render_questions([
        form.purpose.trim(),
        form.team.trim(),
        form.maturity.trim(),
        form.dependencies.trim(),
        traps_rendered,
        additional_rendered,
    ]);
    for section in [project_facts, docs_section, rules_section]
        .into_iter()
        .flatten()
    {
        body.push('\n');
        body.push_str(&section);
    }
    with_all_kronn_options(body, form.td_to_tasks, &form.cross_audit_agents)
}

/// Appends the Kronn options section, only when an option is set, so a
/// briefing without one keeps its historical shape.
#[cfg(test)]
pub(crate) fn with_kronn_options(body: String, td_to_tasks: bool) -> String {
    with_all_kronn_options(body, td_to_tasks, &[])
}

/// Every Kronn option under one heading; no heading when none is set.
pub(crate) fn with_all_kronn_options(
    mut body: String,
    td_to_tasks: bool,
    cross_audit_agents: &[AgentType],
) -> String {
    let mut lines: Vec<String> = Vec::new();
    if td_to_tasks {
        lines.push(crate::core::td_tasks::BRIEFING_OPTION_LINE.to_string());
    }
    if !cross_audit_agents.is_empty() {
        lines.push(crate::core::cross_audit::briefing_option_line(
            cross_audit_agents,
        ));
    }
    if !lines.is_empty() {
        body.push_str(&format!(
            "\n{}\n{}\n",
            crate::core::td_tasks::BRIEFING_OPTIONS_HEADING,
            lines.join("\n")
        ));
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults(
        root: &std::path::Path,
        stored: Option<&str>,
        global: Option<&str>,
    ) -> (String, BriefingDocsDirSource) {
        let d = briefing_defaults(root, stored, global);
        (d.docs_dir, d.docs_dir_source)
    }

    /// KT-1192: override, then an existing folder, then the global default,
    /// then `docs`.
    #[test]
    fn the_docs_folder_prefill_follows_its_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(
            defaults(root, None, None),
            ("docs".into(), BriefingDocsDirSource::Default)
        );
        assert_eq!(
            defaults(root, None, Some("./.ai/")),
            (".ai".into(), BriefingDocsDirSource::GlobalDefault)
        );
        std::fs::create_dir_all(root.join("doc")).unwrap();
        assert_eq!(
            defaults(root, None, Some(".ai")),
            ("doc".into(), BriefingDocsDirSource::Existing)
        );
        assert_eq!(
            defaults(root, Some("context"), Some(".ai")),
            ("context".into(), BriefingDocsDirSource::ProjectOverride)
        );
        assert!(!briefing_defaults(root, Some("context"), None).folder_exists);
        assert!(briefing_defaults(root, None, None).folder_exists);
    }

    #[test]
    fn an_invalid_global_default_falls_back_to_docs() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            defaults(tmp.path(), None, Some("../escape")),
            ("docs".into(), BriefingDocsDirSource::Default)
        );
    }
}
