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
        .with_conn(move |conn| crate::db::projects::get_project(conn, &id))
        .await
    {
        Ok(Some(project)) => {
            let root = crate::core::scanner::resolve_host_path(&project.path);
            Json(ApiResponse::ok(
                crate::api::projects::resolve_briefing_notes(&root, &project.briefing_notes),
            ))
        }
        Ok(None) => Json(ApiResponse::err("Project not found")),
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
    let lease =
        match crate::api::audit::docs_write::DocsWriteLease::acquire(&state, &project.id).await {
            Ok(lease) => lease,
            Err(error) => return Json(ApiResponse::err(error)),
        };
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    if let Err(e) = sync_briefing_file(&project_path, notes.as_deref()) {
        return Json(ApiResponse::err(e));
    }
    match state
        .db
        .with_conn(move |conn| {
            let _lease = lease;
            crate::db::projects::update_project_briefing_notes(conn, &id, notes.as_deref())
        })
        .await
    {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
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
#[cfg(test)]
pub(crate) fn briefing_prompt_section(
    notes: &str,
    docs_root: &crate::core::docs_location::DocsRoot,
) -> String {
    briefing_prompt_section_for(notes, docs_root, "", false)
}

/// [`briefing_prompt_section`] for one step: when `with_docs` is set and the
/// step uses it, the documentation block follows the briefing, and the
/// briefing text gives up the room it takes so the whole stays within
/// [`BRIEFING_PROMPT_MAX_CHARS`].
pub(crate) fn briefing_prompt_section_for(
    notes: &str,
    docs_root: &crate::core::docs_location::DocsRoot,
    target_file: &str,
    with_docs: bool,
) -> String {
    let docs_block = if with_docs {
        super::briefing_docs::prompt_block(target_file, notes, docs_root)
    } else {
        String::new()
    };
    let budget = BRIEFING_PROMPT_MAX_CHARS.saturating_sub(docs_block.chars().count());
    let notes = super::briefing_docs::strip_section(notes);
    let notes = super::briefing_rules::strip_section(&notes);
    let notes = super::project_doc::strip_answers_line(&notes);
    let notes = notes.trim_end();
    let total = notes.chars().count();
    let body = if total <= budget {
        notes.to_string()
    } else {
        let mut cut: String = notes.chars().take(budget).collect();
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
    format!("\n\n## Project briefing (from the user)\n{body}\n{docs_block}")
}

/// After `KRONN:BRIEFING_COMPLETE`: store the reviewed `docs/briefing.md`,
/// with the form's project facts put back as answered.
pub(crate) async fn complete_reviewed_briefing(
    state: &AppState,
    project_id: String,
    briefing_file: std::path::PathBuf,
) -> Result<(), String> {
    let lease = crate::api::audit::docs_write::DocsWriteLease::acquire(state, &project_id).await?;
    let file = briefing_file.clone();
    let reviewed = lease
        .blocking(move || std::fs::read_to_string(&file))
        .await
        .map_err(|e| format!("Briefing save interrupted: {e}"))?
        .map_err(|e| {
            format!(
                "No briefing was stored: cannot read {}: {e}",
                briefing_file.display()
            )
        })?;
    if reviewed.trim().is_empty() {
        return Err("No briefing was stored: the briefing file is empty.".into());
    }
    let pid = project_id.clone();
    let previous = state
        .db
        .with_conn(move |conn| crate::db::projects::get_project_briefing_notes(conn, &pid))
        .await
        .map_err(|e| format!("Cannot read the saved briefing: {e}"))?;
    let content = super::project_doc::reattach_facts(previous.as_deref(), &reviewed);
    if content != reviewed {
        let body = content.clone();
        lease
            .blocking(move || crate::core::mcp_scanner::atomic_write(&briefing_file, &body))
            .await
            .map_err(|e| format!("Briefing restore interrupted: {e}"))?
            .map_err(|e| format!("Cannot restore the saved briefing answers: {e}"))?;
    }
    let saved = state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, &project_id, Some(&content))
        })
        .await
        .map_err(|e| format!("Cannot save the reviewed briefing: {e}"))?;
    if !saved {
        return Err("Cannot save the reviewed briefing: the project no longer exists.".into());
    }
    Ok(())
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
    let (pid, original) = (id.clone(), prefilled.clone());
    match state
        .db
        .with_conn(move |conn| {
            crate::db::projects::update_project_briefing_notes(conn, &pid, original.as_deref())
        })
        .await
    {
        Ok(true) => {}
        Ok(false) => return Json(ApiResponse::err("Project no longer exists")),
        Err(e) => {
            return Json(ApiResponse::err(format!(
                "Cannot preserve the briefing before review: {e}"
            )))
        }
    }
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
        session_continuity: None,
        kind: Default::default(),
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
    /// KT-1192 — the folder holds docs Kronn did not write and the human chose
    /// to move them to `<folder>/legacy/`. Without it, such a folder is refused.
    #[serde(default)]
    pub move_existing_to_legacy: bool,
}

/// Why a briefing was not saved.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BriefingSaveError {
    /// An answer is missing or malformed.
    Invalid(String),
    Busy(String),
    /// The docs folder leaves the project, or cannot be checked (HTTP 400).
    DocsDir(String),
    /// A listed documentation location is refused (HTTP 400, KT-999).
    ExistingDocs(String),
    /// A stated project rule is malformed (HTTP 400, KT-1193).
    Rules(String),
    NotFound,
    Internal(String),
}

impl BriefingSaveError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Invalid(m)
            | Self::Busy(m)
            | Self::DocsDir(m)
            | Self::ExistingDocs(m)
            | Self::Rules(m)
            | Self::Internal(m) => m.clone(),
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
    /// What the proposed folder already holds.
    pub folder: crate::core::docs_folder::DocsFolderState,
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
    recorded_run: bool,
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
        folder: crate::core::docs_folder::folder_state_with_runs(
            project_root,
            &docs_dir,
            recorded_run,
        ),
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
            let ran = crate::db::audit_runs::has_any_full(conn, &lookup)?;
            Ok(Some((project.path, dir, ran)))
        })
        .await;
    let (path, stored, ran) = match found {
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
        ran,
    )))
}

/// Record the folder the briefing chose, in the caller's transaction. Stored
/// as the project's override only when it differs from what the project
/// resolves to without one, so a briefing that keeps the existing layout
/// changes nothing. The override and the legacy move go to `after`.
fn apply_briefing_docs_dir(
    conn: &rusqlite::Connection,
    project: &Project,
    requested: &str,
    move_existing_to_legacy: bool,
    after: &mut BriefingAfterCommit,
) -> Result<(), BriefingSaveError> {
    use crate::core::docs_folder::{folder_state_with_runs, DocsFolderState};
    use crate::core::docs_location;
    let root = crate::core::scanner::resolve_host_path(&project.path);
    let rel = docs_location::validate_for_project(&root, requested)
        .map_err(BriefingSaveError::DocsDir)?;
    // A human's docs never move, and are never written over, without the
    // explicit choice; old Kronn docs are updated in place, unasked.
    let ran = crate::db::audit_runs::has_any_full(conn, &project.id)
        .map_err(|e| BriefingSaveError::Internal(format!("DB error: {e}")))?;
    let state = folder_state_with_runs(&root, &rel, ran);
    if let DocsFolderState::HumanDocs {
        files,
        legacy_exists,
        ..
    } = &state
    {
        if !move_existing_to_legacy {
            return Err(BriefingSaveError::DocsDir(format!(
                "The folder {rel}/ used by Kronn already exists and holds {files} file(s) Kronn \
                 did not write: choose another folder, or move them to {rel}/legacy/."
            )));
        }
        if *legacy_exists {
            return Err(BriefingSaveError::DocsDir(format!(
                "{rel}/legacy/ already exists: Kronn never merges into it. Choose another folder."
            )));
        }
    }
    let unconfigured = docs_location::resolve(&root, None).relative(&root);
    let stored = (unconfigured.as_deref() != Some(rel.as_str())).then(|| rel.clone());
    crate::db::projects::set_project_docs_dir(conn, &project.id, stored.as_deref())
        .map_err(|e| BriefingSaveError::Internal(format!("DB error: {e}")))?;
    after.docs_override = Some(stored);
    if matches!(state, DocsFolderState::HumanDocs { .. }) {
        after.legacy_move =
            Some(docs_location::DocsRoot::from_rel(&rel).map_err(BriefingSaveError::DocsDir)?);
    }
    Ok(())
}

/// What a saved briefing still does outside the database. A rename cannot be
/// rolled back, so it runs only once the briefing's rows are committed.
#[must_use = "apply it once the transaction is committed"]
pub(crate) struct BriefingAfterCommit {
    project_path: String,
    root: std::path::PathBuf,
    docs_override: Option<Option<String>>,
    legacy_move: Option<crate::core::docs_location::DocsRoot>,
    body: String,
    project_rules: Option<BriefingProjectRules>,
}

impl BriefingAfterCommit {
    /// The override, the explicit legacy move, then `briefing.md` in the
    /// folder. Pre-bootstrap projects have no docs folder yet: the DB only,
    /// and the audit's Phase 1 install picks it up via `resolve_briefing_notes`.
    pub(crate) fn apply(self) -> Result<(), BriefingSaveError> {
        if let Some(stored) = &self.docs_override {
            crate::core::docs_location::set_project_override(&self.project_path, stored.as_deref());
        }
        if let Some(docs_root) = &self.legacy_move {
            crate::core::docs_folder::move_to_legacy(&self.root, docs_root)
                .map_err(BriefingSaveError::DocsDir)?;
        }
        let docs_dir = crate::core::scanner::detect_docs_dir(&self.root);
        if docs_dir.is_dir() {
            let target = docs_dir.join("briefing.md");
            crate::core::fs_guard::assert_contained_no_symlink(&self.root, &target)
                .map_err(BriefingSaveError::DocsDir)?;
            std::fs::write(&target, &self.body).map_err(|e| {
                BriefingSaveError::Internal(format!("Failed to write {}: {}", target.display(), e))
            })?;
            // KT-1193 — every agent of the project reads the coding-rules doc.
            if let Err(e) = super::briefing_rules::sync_coding_rules_doc(
                &self.root,
                self.project_rules.as_ref(),
            ) {
                tracing::warn!("project rules not written to the coding-rules doc: {e}");
            }
        }
        Ok(())
    }
}

/// The mandatory answers and the cross-audit agents, checked before anything
/// is read or written.
fn check_answers(form: &BriefingFormRequest) -> Result<(), BriefingSaveError> {
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
    Ok(())
}

/// Save the briefing answers: validation, docs folder and DB notes, in the
/// caller's transaction; the returned effects run once it is committed.
/// Shared by the form and the briefing cards (KT-1192).
pub(crate) fn save_briefing_in(
    conn: &rusqlite::Connection,
    project: &Project,
    form: &BriefingFormRequest,
    docs_section: Option<&str>,
) -> Result<BriefingAfterCommit, BriefingSaveError> {
    let mut after = BriefingAfterCommit {
        project_path: project.path.clone(),
        root: crate::core::scanner::resolve_host_path(&project.path),
        docs_override: None,
        legacy_move: None,
        body: String::new(),
        project_rules: None,
    };
    check_answers(form)?;

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
        Some(Err(e)) => return Err(BriefingSaveError::Rules(e)),
    };
    let rules_section = project_rules
        .as_ref()
        .and_then(super::briefing_rules::render_section);

    // 2. The docs folder first: `briefing.md` is written into it.
    if let Some(dir) = form
        .docs_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        apply_briefing_docs_dir(conn, project, dir, form.move_existing_to_legacy, &mut after)?;
    }

    // 3. The briefing.md body (EN headings: the canonical format the audit reads).
    let body = compose_briefing(
        form,
        project_facts,
        docs_section.map(str::to_string),
        rules_section,
    );
    // 4. DB notes; `briefing.md` is written after the commit.
    match crate::db::projects::update_project_briefing_notes(conn, &project.id, Some(&body)) {
        Ok(true) => {}
        Ok(false) => return Err(BriefingSaveError::NotFound),
        Err(e) => return Err(BriefingSaveError::Internal(format!("DB error: {e}"))),
    }
    after.body = body;
    after.project_rules = project_rules;
    Ok(after)
}

/// `save_briefing_in` for a project id, on the database's connection.
pub(crate) async fn save_briefing_answers(
    state: &AppState,
    id: &str,
    form: BriefingFormRequest,
) -> Result<(), BriefingSaveError> {
    let lease = crate::api::audit::docs_write::DocsWriteLease::acquire(state, id)
        .await
        .map_err(BriefingSaveError::Busy)?;
    check_answers(&form)?;
    // The documentation locations are dated with `git log` (KT-999): checked
    // here, off the connection, and refused before anything is written.
    let docs_section = match form.existing_docs.as_ref() {
        None => None,
        Some(docs) => {
            let lookup = id.to_string();
            let project = state
                .db
                .with_read_conn(move |conn| crate::db::projects::get_project(conn, &lookup))
                .await
                .map_err(|e| BriefingSaveError::Internal(format!("DB error: {e}")))?
                .ok_or(BriefingSaveError::NotFound)?;
            let project_path = crate::core::scanner::resolve_host_path(&project.path);
            let docs = super::briefing_docs::normalize_with_dates(&project_path, docs)
                .await
                .map_err(BriefingSaveError::ExistingDocs)?;
            super::briefing_docs::render_section(&docs)
        }
    };
    let id = id.to_string();
    state
        .db
        .with_conn(move |conn| {
            let _lease = lease;
            let Some(project) = crate::db::projects::get_project(conn, &id)? else {
                return Ok(Err(BriefingSaveError::NotFound));
            };
            let tx = conn.unchecked_transaction()?;
            let after = match save_briefing_in(&tx, &project, &form, docs_section.as_deref()) {
                Ok(after) => after,
                Err(e) => return Ok(Err(e)),
            };
            tx.commit()?;
            Ok(after.apply())
        })
        .await
        .unwrap_or_else(|e| Err(BriefingSaveError::Internal(format!("DB error: {e}"))))
}

pub async fn save_briefing_form(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(form): Json<BriefingFormRequest>,
) -> Response {
    // KT-1196 — a running audit reads the briefing: `save_briefing_answers`
    // refuses with the shared `DOCS_BUSY` while the audit holds the lease.
    match save_briefing_answers(&state, &id, form).await {
        Ok(()) => Json(ApiResponse::ok(true)).into_response(),
        Err(BriefingSaveError::Busy(reason)) => {
            (StatusCode::CONFLICT, Json(ApiResponse::<bool>::err(reason))).into_response()
        }
        Err(BriefingSaveError::DocsDir(reason)) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<bool>::err_coded(
                ApiErrorCode::Validation,
                reason,
            )),
        )
            .into_response(),
        Err(BriefingSaveError::ExistingDocs(reason) | BriefingSaveError::Rules(reason)) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<bool>::err(reason)),
        )
            .into_response(),
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
        let d = briefing_defaults(root, stored, global, false);
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
        assert!(!briefing_defaults(root, Some("context"), None, false).folder_exists);
        assert!(briefing_defaults(root, None, None, false).folder_exists);
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
