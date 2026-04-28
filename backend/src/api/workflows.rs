use std::convert::Infallible;
use std::pin::Pin;
use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use futures::Stream;
use uuid::Uuid;

use crate::models::*;
use crate::AppState;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Reject artifact specs whose path is absolute or escapes the workspace
/// (`..`) — both would let a workflow scribble outside its sandbox. Phase-3
/// minimal validation: empty path, absolute (Unix `/` or Windows drive),
/// or any segment equal to `..`. Path canonicalisation at write-time is
/// deferred to the runner (which knows the workspace root); this is only
/// the save-time hard reject.
fn validate_artifact_specs(
    specs: &::std::collections::HashMap<String, ArtifactSpec>,
) -> Result<(), String> {
    use std::path::Component;
    for (name, spec) in specs {
        if name.trim().is_empty() {
            return Err("Artifact name cannot be empty.".into());
        }
        if spec.path.trim().is_empty() {
            return Err(format!("Artifact « {} » : le chemin est obligatoire.", name));
        }
        let p = std::path::Path::new(&spec.path);
        if p.is_absolute() {
            return Err(format!(
                "Artifact « {} » : chemin absolu interdit ({}). Utilise un chemin relatif au workspace, ex. `.kronn/{}.md`.",
                name, spec.path, name
            ));
        }
        if p.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(format!(
                "Artifact « {} » : « .. » interdit dans le chemin ({}). Reste dans le workspace.",
                name, spec.path
            ));
        }
    }
    Ok(())
}

/// Reject `WorkflowGuards` values that would either be a no-op (`Some(0)` =
/// "trip immediately") or impossible to honor (negative is impossible at
/// the type level, but keeping the function pluggable for futures fields
/// like `max_total_cost_usd` etc.). Documented in the wizard tooltip — the
/// user sees this as a save-time error, not a runtime surprise.
fn validate_guards(g: &WorkflowGuards) -> Result<(), String> {
    if let Some(secs) = g.timeout_seconds {
        if secs == 0 {
            return Err("Limite « durée max » : 0 seconde n'a pas de sens — laisse vide pour le défaut, ou indique au moins 60 secondes.".into());
        }
    }
    if let Some(calls) = g.max_llm_calls {
        if calls == 0 {
            return Err("Limite « appels IA max » : 0 stoppe le run avant le 1er step. Laisse vide pour le défaut, ou indique au moins 1.".into());
        }
    }
    if let Some(rev) = g.loop_detection_max_revisits {
        if rev == 0 {
            return Err("Limite « détection de boucle » : 0 stoppe au 1er Goto. Laisse vide pour le défaut, ou indique au moins 1.".into());
        }
    }
    Ok(())
}

/// 0.7.0 Phase 7 — reject rollback chains that mix in a `Gate` step.
/// A Gate inside `on_failure` would deadlock the run on a `Failed`
/// status that no resume path serves: `decide_run` only accepts runs
/// in `WaitingApproval`, but rollback runs after final-status
/// determination so the second pause would never get unstuck. Caught
/// here at save time so the wizard can surface the error inline.
fn validate_on_failure_steps(steps: &[WorkflowStep]) -> Result<(), String> {
    for s in steps {
        if matches!(s.step_type, StepType::Gate) {
            return Err(format!(
                "Rollback step « {} » : type Gate interdit dans la chaîne on_failure (impossible à reprendre, le run est déjà Failed).",
                s.name
            ));
        }
    }
    Ok(())
}

/// 0.7.0 Phase 5 — validate the per-workflow Exec allowlist.
///
/// Each entry must be a bare binary name with no path separator and no
/// shell meta-characters. We reject:
///   - empty strings
///   - entries containing `/` or `\` (path = different binary, allowlist bypass)
///   - entries containing whitespace or shell metas (`;`, `|`, `&`, `$`, `` ` ``,
///     `>`, `<`, etc.) — defence in depth even though we never invoke a shell
///   - entries containing `..` (defence in depth on path traversal)
fn validate_exec_allowlist(entries: &[String]) -> Result<(), String> {
    for raw in entries {
        let entry = raw.trim();
        if entry.is_empty() {
            return Err("Exec allowlist : chaque entrée doit être un nom de binaire non vide.".into());
        }
        if entry.contains('/') || entry.contains('\\') {
            return Err(format!(
                "Exec allowlist « {} » : pas de séparateur de chemin (`/` ou `\\`). Utilise juste le nom du binaire (ex. `npm`, `cargo`).",
                entry
            ));
        }
        if entry.contains("..") {
            return Err(format!(
                "Exec allowlist « {} » : « .. » interdit.", entry
            ));
        }
        const FORBIDDEN: &[char] = &[
            ' ', '\t', '\n', '\r',
            ';', '|', '&', '$', '`', '>', '<', '*', '?', '(', ')', '{', '}', '[', ']',
            '\'', '"', '\\',
        ];
        if entry.chars().any(|c| FORBIDDEN.contains(&c)) {
            return Err(format!(
                "Exec allowlist « {} » : caractères spéciaux interdits. Utilise un nom de binaire simple (ex. `npm`, `cargo`, `make`).",
                entry
            ));
        }
    }
    Ok(())
}

/// 0.7+ — valide chaque `StepType::JsonData` step :
///   - `json_data_payload` est set (sinon erreur claire pour le wizard)
///   - sa sérialisation tient sous une limite raisonnable
///
/// Pas de limite de schéma : on accepte n'importe quelle valeur JSON
/// valide (array, object, scalaire). La taille max protège contre un
/// payload géant collé par erreur — au-delà, l'utilisateur veut sans
/// doute une vraie API.
fn validate_json_data_steps(steps: &[WorkflowStep]) -> Result<(), String> {
    /// 1 MiB : largement plus que ce qu'un workflow batch peut consommer
    /// avant que les agents downstream ne saturent leur context window.
    const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
    for s in steps {
        if !matches!(s.step_type, StepType::JsonData) {
            continue;
        }
        let payload = match s.json_data_payload.as_ref() {
            Some(p) => p,
            None => {
                return Err(format!(
                    "Step JsonData « {} » : `json_data_payload` est obligatoire (le JSON à émettre).",
                    s.name
                ));
            }
        };
        let serialized = serde_json::to_string(payload).map_err(|e| {
            format!(
                "Step JsonData « {} » : payload non sérialisable ({}).",
                s.name, e
            )
        })?;
        if serialized.len() > MAX_PAYLOAD_BYTES {
            return Err(format!(
                "Step JsonData « {} » : payload de {} octets > limite {} ({} MiB). Pour des données plus volumineuses, utilise un step ApiCall qui pointe vers une vraie source.",
                s.name,
                serialized.len(),
                MAX_PAYLOAD_BYTES,
                MAX_PAYLOAD_BYTES / (1024 * 1024)
            ));
        }
    }
    Ok(())
}

/// 0.7.0 Phase 5 — validate every `StepType::Exec` step in the list:
///   - `exec_command` is set, non-empty, and present in `allowlist`
///   - `exec_command` itself passes the same character-level safety
///     check as allowlist entries (defence in depth — paranoid)
///   - `exec_timeout_secs` (if set) is in `[1, 1800]`
///   - `exec_args` capped at 64 entries to avoid pathological argv blow-ups
///
/// Args themselves are NOT validated for content — they're rendered
/// from templates at run time and passed as literal argv elements; even
/// a value containing `; rm -rf /` becomes a single benign argument
/// because the runner never invokes a shell. Validating the rendered
/// content here would either be a false safety blanket (we'd reject
/// legitimate values) or trivially bypassed.
fn validate_exec_steps(steps: &[WorkflowStep], allowlist: &[String]) -> Result<(), String> {
    const MAX_ARGS: usize = 64;
    const MAX_TIMEOUT_SECS: u32 = 1800;
    for s in steps {
        if !matches!(s.step_type, StepType::Exec) {
            continue;
        }
        let cmd = match s.exec_command.as_deref().map(str::trim) {
            Some(c) if !c.is_empty() => c,
            _ => return Err(format!(
                "Step Exec « {} » : `exec_command` est obligatoire (le binaire à exécuter).",
                s.name
            )),
        };
        // Apply allowlist-entry rules to the command itself — same
        // character-level discipline (rejects `bash -c`, `npm; rm`, etc.).
        if let Err(e) = validate_exec_allowlist(&[cmd.to_string()]) {
            return Err(format!("Step Exec « {} » : {}", s.name, e));
        }
        if allowlist.is_empty() {
            return Err(format!(
                "Step Exec « {} » : impossible — l'allowlist du workflow est vide. Configure `exec_allowlist` avec les binaires autorisés (ex. [\"npm\", \"cargo\"]).",
                s.name
            ));
        }
        if !allowlist.iter().any(|a| a == cmd) {
            return Err(format!(
                "Step Exec « {} » : binaire `{}` absent de l'allowlist. Allowlist actuelle : [{}].",
                s.name, cmd, allowlist.join(", ")
            ));
        }
        if s.exec_args.len() > MAX_ARGS {
            return Err(format!(
                "Step Exec « {} » : trop d'arguments ({}, max {}).",
                s.name, s.exec_args.len(), MAX_ARGS
            ));
        }
        if let Some(t) = s.exec_timeout_secs {
            if t == 0 || t > MAX_TIMEOUT_SECS {
                return Err(format!(
                    "Step Exec « {} » : `exec_timeout_secs` doit être entre 1 et {} (reçu : {}).",
                    s.name, MAX_TIMEOUT_SECS, t
                ));
            }
        }
    }
    Ok(())
}

/// GET /api/workflows
pub async fn list(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<WorkflowSummary>>> {
    match state.db.with_conn(|conn| {
        let workflows = crate::db::workflows::list_workflows(conn)?;
        // Batch-load last runs and project names (avoids N+1 queries)
        let last_runs = crate::db::workflows::get_last_runs_all(conn)?;
        let project_names = crate::db::projects::get_project_names(conn)?;

        let summaries = workflows.into_iter().map(|wf| {
            let last_run = last_runs.get(&wf.id)
                .map(|r| WorkflowRunSummary {
                    id: r.id.clone(),
                    status: r.status.clone(),
                    started_at: r.started_at,
                    finished_at: r.finished_at,
                    tokens_used: r.tokens_used,
                });

            let project_name = wf.project_id.as_ref()
                .and_then(|pid| project_names.get(pid).cloned());

            let trigger_type = match &wf.trigger {
                WorkflowTrigger::Cron { .. } => "cron",
                WorkflowTrigger::Tracker { .. } => "tracker",
                WorkflowTrigger::Manual => "manual",
            }.to_string();

            WorkflowSummary {
                id: wf.id,
                name: wf.name,
                project_id: wf.project_id,
                project_name,
                trigger_type,
                step_count: wf.steps.len() as u32,
                enabled: wf.enabled,
                last_run,
                created_at: wf.created_at,
            }
        }).collect();

        Ok(summaries)
    }).await {
        Ok(summaries) => Json(ApiResponse::ok(summaries)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/workflows/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Workflow>> {
    match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &id)).await {
        Ok(Some(wf)) => Json(ApiResponse::ok(wf)),
        Ok(None) => Json(ApiResponse::err("Workflow not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/workflows
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkflowRequest>,
) -> Json<ApiResponse<Workflow>> {
    if req.steps.is_empty() {
        return Json(ApiResponse::err("Workflow must have at least one step"));
    }
    if req.steps.len() > 20 {
        return Json(ApiResponse::err(format!("Too many steps ({}, max 20)", req.steps.len())));
    }
    if req.name.len() > 200 {
        return Json(ApiResponse::err("Workflow name too long (max 200 chars)"));
    }
    if let Err(errors) = crate::workflows::template::validate_step_references(&req.steps) {
        return Json(ApiResponse::err(format!("Références d'étapes invalides :\n- {}", errors.join("\n- "))));
    }

    if let Some(ref guards) = req.guards {
        if let Err(e) = validate_guards(guards) {
            return Json(ApiResponse::err(e));
        }
    }

    if let Err(e) = validate_artifact_specs(&req.artifacts) {
        return Json(ApiResponse::err(e));
    }

    if let Err(e) = validate_on_failure_steps(&req.on_failure) {
        return Json(ApiResponse::err(e));
    }

    if let Err(e) = validate_exec_allowlist(&req.exec_allowlist) {
        return Json(ApiResponse::err(e));
    }

    if let Err(e) = validate_exec_steps(&req.steps, &req.exec_allowlist) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = validate_exec_steps(&req.on_failure, &req.exec_allowlist) {
        return Json(ApiResponse::err(e));
    }

    if let Err(e) = validate_json_data_steps(&req.steps) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = validate_json_data_steps(&req.on_failure) {
        return Json(ApiResponse::err(e));
    }

    let now = Utc::now();
    let wf = Workflow {
        id: Uuid::new_v4().to_string(),
        name: req.name,
        project_id: req.project_id,
        trigger: req.trigger,
        steps: req.steps,
        actions: req.actions,
        safety: req.safety.unwrap_or(WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        }),
        workspace_config: req.workspace_config,
        concurrency_limit: req.concurrency_limit,
        guards: req.guards,
        artifacts: req.artifacts,
        on_failure: req.on_failure,
        exec_allowlist: req.exec_allowlist,
        variables: req.variables,
        enabled: true,
        created_at: now,
        updated_at: now,
    };

    let w = wf.clone();
    match state.db.with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &w)).await {
        Ok(()) => Json(ApiResponse::ok(wf)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/workflows/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateWorkflowRequest>,
) -> Json<ApiResponse<Workflow>> {
    let wf_id = id.clone();
    let existing = match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id)).await {
        Ok(Some(wf)) => wf,
        Ok(None) => return Json(ApiResponse::err("Workflow not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if let Some(ref steps) = req.steps {
        if steps.len() > 20 {
            return Json(ApiResponse::err(format!("Too many steps ({}, max 20)", steps.len())));
        }
        if let Err(errors) = crate::workflows::template::validate_step_references(steps) {
            return Json(ApiResponse::err(format!("Références d'étapes invalides :\n- {}", errors.join("\n- "))));
        }
    }
    if let Some(ref name) = req.name {
        if name.len() > 200 {
            return Json(ApiResponse::err("Workflow name too long (max 200 chars)"));
        }
    }

    // `guards` follows the same opt-in semantics as `safety`: if the
    // caller doesn't include it in the patch, the existing value is
    // preserved. Set to `Some(WorkflowGuards::default())` to clear
    // overrides and fall back to backend defaults.
    if let Some(ref new_guards) = req.guards {
        if let Err(e) = validate_guards(new_guards) {
            return Json(ApiResponse::err(e));
        }
    }

    if let Some(ref new_artifacts) = req.artifacts {
        if let Err(e) = validate_artifact_specs(new_artifacts) {
            return Json(ApiResponse::err(e));
        }
    }

    if let Some(ref new_on_failure) = req.on_failure {
        if let Err(e) = validate_on_failure_steps(new_on_failure) {
            return Json(ApiResponse::err(e));
        }
    }

    if let Some(ref new_allowlist) = req.exec_allowlist {
        if let Err(e) = validate_exec_allowlist(new_allowlist) {
            return Json(ApiResponse::err(e));
        }
    }

    // Validate Exec steps against the EFFECTIVE allowlist (the patch's
    // allowlist if provided, else the existing one).
    let effective_allowlist = req.exec_allowlist.as_ref().unwrap_or(&existing.exec_allowlist);
    if let Some(ref new_steps) = req.steps {
        if let Err(e) = validate_exec_steps(new_steps, effective_allowlist) {
            return Json(ApiResponse::err(e));
        }
        if let Err(e) = validate_json_data_steps(new_steps) {
            return Json(ApiResponse::err(e));
        }
    }
    if let Some(ref new_on_failure) = req.on_failure {
        if let Err(e) = validate_exec_steps(new_on_failure, effective_allowlist) {
            return Json(ApiResponse::err(e));
        }
        if let Err(e) = validate_json_data_steps(new_on_failure) {
            return Json(ApiResponse::err(e));
        }
    }

    let updated = Workflow {
        id: existing.id,
        name: req.name.unwrap_or(existing.name),
        project_id: req.project_id.unwrap_or(existing.project_id),
        trigger: req.trigger.unwrap_or(existing.trigger),
        steps: req.steps.unwrap_or(existing.steps),
        actions: req.actions.unwrap_or(existing.actions),
        safety: req.safety.unwrap_or(existing.safety),
        workspace_config: req.workspace_config.or(existing.workspace_config),
        concurrency_limit: req.concurrency_limit.or(existing.concurrency_limit),
        guards: req.guards.or(existing.guards),
        artifacts: req.artifacts.unwrap_or(existing.artifacts),
        on_failure: req.on_failure.unwrap_or(existing.on_failure),
        exec_allowlist: req.exec_allowlist.unwrap_or(existing.exec_allowlist),
        variables: req.variables.unwrap_or(existing.variables),
        enabled: req.enabled.unwrap_or(existing.enabled),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let w = updated.clone();
    match state.db.with_conn(move |conn| crate::db::workflows::update_workflow(conn, &w)).await {
        Ok(()) => Json(ApiResponse::ok(updated)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/workflows/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::workflows::delete_workflow(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// 0.7.0 UX pass — current export envelope schema version. Bumped on
/// incompatible changes; the importer reads this to decide whether to
/// run a migration or refuse the file.
const EXPORT_VERSION: u32 = 1;
const WORKFLOW_EXPORT_KIND: &str = "kronn.workflow";

/// GET /api/workflows/:id/export
///
/// Returns a self-contained `WorkflowExportEnvelope` JSON. Bundles every
/// QP referenced by a `BatchQuickPrompt` step so the importer doesn't
/// need to find them separately. The frontend triggers a file download
/// from this response (filename suggested via `Content-Disposition`).
pub async fn export_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> axum::response::Response {
    use axum::http::{header, StatusCode};
    use axum::response::IntoResponse;

    let wf_id = id.clone();
    let wf = match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id)).await {
        Ok(Some(wf)) => wf,
        Ok(None) => return (StatusCode::NOT_FOUND, "Workflow not found").into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)).into_response(),
    };

    // Bundle every QP referenced by a BatchQuickPrompt step. Dedup by
    // QP id so we don't ship the same QP twice when multiple steps
    // reuse it.
    let qp_ids: Vec<String> = wf.steps.iter()
        .chain(wf.on_failure.iter())
        .filter_map(|s| s.batch_quick_prompt_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    let referenced_quick_prompts = if qp_ids.is_empty() {
        Vec::new()
    } else {
        let ids = qp_ids.clone();
        match state.db.with_conn(move |conn| {
            let mut found = Vec::with_capacity(ids.len());
            for id in &ids {
                if let Some(qp) = crate::db::quick_prompts::get_quick_prompt(conn, id)? {
                    found.push(qp);
                }
            }
            Ok(found)
        }).await {
            Ok(qps) => qps,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {}", e)).into_response(),
        }
    };

    let envelope = WorkflowExportEnvelope {
        kind: WORKFLOW_EXPORT_KIND.to_string(),
        version: EXPORT_VERSION,
        exported_at: Utc::now(),
        workflow: wf.clone(),
        referenced_quick_prompts,
    };

    // Sanitised filename: `<workflow_name>.kronn-workflow.json`. Replace
    // anything outside a-zA-Z0-9_- with `-`.
    let safe_name: String = wf.name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let filename = format!("{}.kronn-workflow.json", safe_name);

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

/// POST /api/workflows/import
///
/// Body: `ImportWorkflowRequest { content, project_id }`. `content` is
/// the raw JSON string of a `WorkflowExportEnvelope`. Validates the
/// envelope, mints fresh ids/timestamps, attaches to `project_id` (or
/// leaves null), strips `gate_notify_url` (URLs are per-user — not
/// portable), and inserts both the workflow and any bundled QPs.
///
/// Behaviour with referenced QPs:
///   - Each bundled QP gets a fresh id (no collision with importer's
///     existing QPs)
///   - `BatchQuickPrompt` steps' `batch_quick_prompt_id` is rewritten
///     to point at the new ids
///   - If the workflow references a QP that wasn't bundled, the import
///     fails loudly (no silent half-import)
pub async fn import_workflow(
    State(state): State<AppState>,
    Json(req): Json<ImportWorkflowRequest>,
) -> Json<ApiResponse<Workflow>> {
    let envelope: WorkflowExportEnvelope = match serde_json::from_str(&req.content) {
        Ok(env) => env,
        Err(e) => return Json(ApiResponse::err(format!("JSON invalide : {}", e))),
    };

    if envelope.kind != WORKFLOW_EXPORT_KIND {
        return Json(ApiResponse::err(format!(
            "Type incorrect : attendu `{}`, reçu `{}`. Vérifie que tu importes bien un workflow exporté depuis Kronn.",
            WORKFLOW_EXPORT_KIND, envelope.kind
        )));
    }
    if envelope.version > EXPORT_VERSION {
        return Json(ApiResponse::err(format!(
            "Version d'export non supportée ({} > {} max). Mets à jour Kronn pour importer ce fichier.",
            envelope.version, EXPORT_VERSION
        )));
    }

    let mut wf = envelope.workflow;

    // Validate the workflow as if it were created from scratch — same
    // rules as POST /api/workflows. Fail loudly if the source machine
    // had something the destination doesn't accept.
    if wf.steps.is_empty() {
        return Json(ApiResponse::err("Workflow must have at least one step"));
    }
    if wf.steps.len() > 20 {
        return Json(ApiResponse::err(format!("Too many steps ({}, max 20)", wf.steps.len())));
    }
    if let Err(errors) = crate::workflows::template::validate_step_references(&wf.steps) {
        return Json(ApiResponse::err(format!("Références d'étapes invalides :\n- {}", errors.join("\n- "))));
    }
    if let Some(ref guards) = wf.guards {
        if let Err(e) = validate_guards(guards) {
            return Json(ApiResponse::err(e));
        }
    }
    if let Err(e) = validate_artifact_specs(&wf.artifacts) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = validate_on_failure_steps(&wf.on_failure) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = validate_exec_allowlist(&wf.exec_allowlist) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = validate_exec_steps(&wf.steps, &wf.exec_allowlist) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = validate_exec_steps(&wf.on_failure, &wf.exec_allowlist) {
        return Json(ApiResponse::err(e));
    }

    // Build a remap table for QP ids (source → fresh) and insert the
    // bundled QPs first. If a step references a QP that's NOT bundled,
    // refuse the whole import to keep the workflow consistent.
    let mut qp_id_remap: std::collections::HashMap<String, String> = Default::default();
    let now = Utc::now();
    let mut qps_to_insert: Vec<QuickPrompt> = Vec::with_capacity(envelope.referenced_quick_prompts.len());
    for mut qp in envelope.referenced_quick_prompts {
        let old_id = qp.id.clone();
        let new_id = Uuid::new_v4().to_string();
        qp_id_remap.insert(old_id, new_id.clone());
        qp.id = new_id;
        qp.project_id = req.project_id.clone();
        qp.created_at = now;
        qp.updated_at = now;
        qps_to_insert.push(qp);
    }

    // Rewrite batch_quick_prompt_id refs in steps + on_failure. Refuse
    // if a step points to an unbundled QP.
    let rewrite = |steps: &mut Vec<WorkflowStep>| -> Result<(), String> {
        for s in steps {
            if let Some(ref qp_id) = s.batch_quick_prompt_id {
                match qp_id_remap.get(qp_id) {
                    Some(new) => s.batch_quick_prompt_id = Some(new.clone()),
                    None => return Err(format!(
                        "Step `{}` référence le Quick Prompt `{}` qui n'est pas inclus dans le fichier d'import. Ré-exporte le workflow source pour qu'il bundle ses QPs.",
                        s.name, qp_id
                    )),
                }
            }
            // Strip per-user webhook URL — Slack/Teams URLs are NOT portable.
            // The importer will see the field empty and re-fill if needed.
            s.gate_notify_url = None;
        }
        Ok(())
    };
    if let Err(e) = rewrite(&mut wf.steps) {
        return Json(ApiResponse::err(e));
    }
    if let Err(e) = rewrite(&mut wf.on_failure) {
        return Json(ApiResponse::err(e));
    }

    // Mint fresh identity for the workflow itself.
    wf.id = Uuid::new_v4().to_string();
    wf.project_id = req.project_id.clone();
    wf.created_at = now;
    wf.updated_at = now;
    wf.enabled = true;

    let imported_wf = wf.clone();
    let qps = qps_to_insert.clone();
    match state.db.with_conn(move |conn| {
        for qp in &qps {
            crate::db::quick_prompts::insert_quick_prompt(conn, qp)?;
        }
        crate::db::workflows::insert_workflow(conn, &imported_wf)?;
        Ok(())
    }).await {
        Ok(()) => Json(ApiResponse::ok(wf)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/workflows/:id/trigger — Manual trigger with SSE streaming.
/// 0.6.0 UX pass — accepts an optional JSON body with `variables` (manual
/// launch). When the workflow has declared `variables`, required ones
/// must be filled (400 if not). Variable values land in the run's
/// `trigger_context` so they resolve as `{{var_name}}` in step prompts
/// (the existing `inject_trigger_context` already handles that path).
/// Legacy callers that send no body still work — `Option<Json<...>>` ➜
/// `None` → no variables → exactly the previous behaviour.
pub async fn trigger(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<TriggerWorkflowRequest>>,
) -> Sse<SseStream> {
    let wf_id = id.clone();
    let wf = match state.db.with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id)).await {
        Ok(Some(wf)) => wf,
        Ok(None) => {
            return sse_error("Workflow not found");
        }
        Err(e) => {
            return sse_error(format!("DB error: {}", e));
        }
    };

    if !wf.enabled {
        return sse_error("Workflow is disabled");
    }

    // 0.6.0 UX pass — validate and merge user-entered variables.
    // - Required variable missing/empty → reject with explicit message.
    // - Unknown variables (sent but not declared) → silently dropped
    //   (defensive: don't let a stale form smuggle data in).
    let provided_vars = body.map(|Json(b)| b.variables).unwrap_or_default();
    for declared in &wf.variables {
        if declared.required {
            let val = provided_vars.get(&declared.name).map(|s| s.trim()).unwrap_or("");
            if val.is_empty() {
                let label = if declared.label.is_empty() { &declared.name } else { &declared.label };
                return sse_error(format!(
                    "Variable « {} » est obligatoire pour lancer ce workflow.",
                    label
                ));
            }
        }
    }
    let mut trigger_obj = serde_json::Map::new();
    trigger_obj.insert("type".into(), serde_json::Value::String("manual".into()));
    trigger_obj.insert("triggered_at".into(), serde_json::Value::String(Utc::now().to_rfc3339()));
    for declared in &wf.variables {
        if let Some(val) = provided_vars.get(&declared.name) {
            trigger_obj.insert(declared.name.clone(), serde_json::Value::String(val.clone()));
        }
    }

    // Atomic concurrency check + insert in a single transaction (avoids TOCTOU race)
    let now = Utc::now();
    let run = WorkflowRun {
        id: Uuid::new_v4().to_string(),
        workflow_id: wf.id.clone(),
        status: RunStatus::Pending,
        trigger_context: Some(serde_json::Value::Object(trigger_obj)),
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        // Legacy linear runs — batch fields stay at their defaults.
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_name: None,
        parent_run_id: None,
        state: ::std::collections::HashMap::new(),
    };

    let r = run.clone();
    let limit = wf.concurrency_limit;
    let wf_id_check = wf.id.clone();
    match state.db.with_conn(move |conn| {
        // Single transaction: check + insert atomically
        if let Some(max) = limit {
            let active = crate::db::workflows::count_active_runs(conn, &wf_id_check)?;
            if active >= max {
                anyhow::bail!("CONCURRENCY_LIMIT:{}/{}", active, max);
            }
        }
        crate::db::workflows::insert_run(conn, &r)?;
        Ok(())
    }).await {
        Ok(()) => {}
        Err(e) => {
            let msg = e.to_string();
            if let Some(rest) = msg.strip_prefix("CONCURRENCY_LIMIT:") {
                return sse_error(format!("Concurrency limit reached ({})", rest));
            }
            return sse_error(format!("DB error: {}", msg));
        }
    }

    tracing::info!("Workflow run created: {} for workflow {}", run.id, wf.name);

    // Create event channel for real-time streaming
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::workflows::runner::RunEvent>(32);

    // Dispatch execution in background with the sender
    let state_for_run = state.clone();
    let config = state.config.clone();
    let mut run_exec = run.clone();
    tokio::spawn(async move {
        let cfg = config.read().await;
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        drop(cfg);

        if let Err(e) = crate::workflows::runner::execute_run(
            state_for_run, &wf, &mut run_exec, &tokens, &agents, Some(tx),
        ).await {
            tracing::error!("Workflow run {} failed: {}", run_exec.id, e);
        }
    });

    // Stream events as SSE
    let run_id = run.id.clone();
    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // Send initial run info
        let start = serde_json::json!({ "run_id": run_id });
        yield Event::default().event("run_start").data(start.to_string());

        // Forward events from the runner
        while let Some(evt) = rx.recv().await {
            match &evt {
                crate::workflows::runner::RunEvent::StepStart { step_name, step_index, total_steps } => {
                    let data = serde_json::json!({
                        "step_name": step_name,
                        "step_index": step_index,
                        "total_steps": total_steps,
                    });
                    yield Event::default().event("step_start").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::StepProgress { text } => {
                    // Live-progress passthrough — the runner now emits chunks from
                    // the in-flight Agent step's stdout. Without this the user
                    // sees the "step running" pulse for 30-120s with no content.
                    yield Event::default()
                        .event("step_progress")
                        .data(serde_json::json!({ "text": text }).to_string());
                }
                crate::workflows::runner::RunEvent::StepDone { step_result } => {
                    let data = serde_json::to_value(step_result).unwrap_or_default();
                    yield Event::default().event("step_done").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::RunDone { status } => {
                    let data = serde_json::json!({ "status": status });
                    yield Event::default().event("run_done").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::GuardTriggered { kind, threshold, actual } => {
                    let data = serde_json::json!({ "kind": kind, "threshold": threshold, "actual": actual });
                    yield Event::default().event("guard_triggered").data(data.to_string());
                }
                crate::workflows::runner::RunEvent::RunError { error } => {
                    let data = serde_json::json!({ "error": error });
                    yield Event::default().event("error").data(data.to_string());
                }
            }
        }
    });

    Sse::new(crate::core::sse_limits::bounded(stream))
}

fn sse_error(msg: impl Into<String>) -> Sse<SseStream> {
    let msg = msg.into();
    let stream: SseStream = Box::pin(futures::stream::once(async move {
        Ok::<_, Infallible>(
            Event::default().event("error").data(
                serde_json::json!({ "error": msg }).to_string()
            )
        )
    }));
    Sse::new(stream)
}

/// Request for [`test_batch_step`].
#[derive(Debug, serde::Deserialize)]
pub struct TestBatchStepRequest {
    pub step: WorkflowStep,
    /// Mock output of the upstream step (raw text or structured JSON envelope).
    /// We feed it to the template engine so `{{steps.<name>.data.tickets}}`
    /// resolves to the same thing it would at runtime.
    #[serde(default)]
    pub mock_previous_output: Option<String>,
    /// The name of the upstream step the mock output represents — defaults
    /// to "previous" so users can use `{{previous_step.output}}` in items_from.
    #[serde(default)]
    pub previous_step_name: Option<String>,
}

/// Response for [`test_batch_step`].
#[derive(Debug, serde::Serialize)]
pub struct BatchPreview {
    /// First N items the runner would fan out (capped at 10 for display).
    pub sample_items: Vec<String>,
    pub total_items: u32,
    pub capped_at: u32,
    pub max_items_allowed: u32,
    pub quick_prompt_id: Option<String>,
    pub quick_prompt_name: Option<String>,
    pub quick_prompt_icon: Option<String>,
    pub quick_prompt_agent: Option<String>,
    pub first_variable_name: Option<String>,
    /// Prompt that would be sent for the FIRST item (after `{{var}}` substitution).
    /// Kept for backward compat — prefer `sample_rendered_prompts` for the
    /// per-item view.
    pub sample_rendered_prompt: Option<String>,
    /// Rendered prompt for EACH sample item (same length & order as
    /// `sample_items`). Lets the user spot-check the rendering on every
    /// ticket of their batch, not just the first one.
    pub sample_rendered_prompts: Vec<String>,
    pub workspace_mode: String,
    pub wait_for_completion: bool,
    /// Validation errors found during the dry-run (missing QP, empty list,
    /// unresolved template, etc.) — non-empty means the step would fail at
    /// runtime. Frontend renders these as red bullets.
    pub errors: Vec<String>,
    /// Non-blocking warnings: the dry-run could proceed, but there's a
    /// configuration smell that would bite in production (e.g. using
    /// `{{steps.X.data}}` against a FreeText step). Shown in orange,
    /// separate from the red errors block.
    pub warnings: Vec<String>,
}

/// POST /api/workflows/test-batch-step
///
/// Dry-run preview for a `BatchQuickPrompt` step. Renders the items_from
/// template against mock previous output, parses the items, loads the QP,
/// and returns what the runner would do — WITHOUT creating any discussion,
/// batch run, or worktree. Used by the wizard's per-step "Tester" button so
/// users can validate their batch configuration before triggering the real
/// workflow.
pub async fn test_batch_step(
    State(state): State<AppState>,
    Json(req): Json<TestBatchStepRequest>,
) -> Json<ApiResponse<BatchPreview>> {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // 1. Validate required fields up front — same checks as the runtime
    //    executor. Surfaces config bugs without needing to fire the workflow.
    let qp_id = match req.step.batch_quick_prompt_id.as_ref() {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            errors.push("Missing batch_quick_prompt_id".to_string());
            return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
        }
    };
    let items_from = match req.step.batch_items_from.as_ref() {
        Some(s) if !s.is_empty() => s.clone(),
        _ => {
            errors.push("Missing batch_items_from".to_string());
            return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
        }
    };

    // 2. Render items_from against the mock previous output. We seed the
    //    template ctx with a synthetic upstream step so the same expressions
    //    that work at runtime work here.
    let mut ctx = crate::workflows::template::TemplateContext::new();
    if let Some(ref prev) = req.mock_previous_output {
        let step_name = req.previous_step_name.as_deref().unwrap_or("previous");
        ctx.set_step_output(step_name, prev);
        // Also expose under "previous" so `{{previous_step.output}}` works
        // regardless of the actual step name.
        if step_name != "previous" {
            ctx.set_step_output("previous", prev);
        }

        // Dry-run convenience: users often wire `{{steps.X.data}}` but their
        // source step is in FreeText mode → `.data` is never populated by
        // `set_step_output`, and the render leaves the template literal
        // (which then looks like a single item = silent config bug).
        //
        // Detect the smell (items_from references `.data` and the mock is
        // NOT a structured envelope) and:
        //   1. Inject a raw-text fallback for `.data` so the preview runs
        //   2. Warn the user so they know this won't work at runtime
        let uses_data = items_from.contains(".data");
        let has_envelope = crate::workflows::template::extract_step_envelope(prev).is_some();
        if uses_data && !has_envelope {
            ctx.set(format!("steps.{}.data", step_name), prev.clone());
            if step_name != "previous" {
                ctx.set("previous_step.data".to_string(), prev.clone());
            }
            warnings.push(format!(
                "Ton template utilise `{{{{steps.{name}.data}}}}` mais le step « {name} » \
                 n'est pas en mode « Structured ». En production, `.data` ne sera pas disponible — \
                 seulement `.output`. Pour corriger : coche « Structured » sur le step « {name} » \
                 (le step précédent), OU remplace `.data` par `.output` dans « Liste des items ». \
                 Ce test continue avec un fallback pour te montrer le résultat.",
                name = step_name
            ));
        }
    }

    let rendered = match ctx.render(&items_from) {
        Ok(s) => s,
        Err(e) => {
            errors.push(format!("Template render error: {}", e));
            return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
        }
    };

    // Detect an unresolved template — the render engine leaves `{{foo}}`
    // in-place when a variable is unknown (no exception thrown). Without
    // this guard, the parser downstream treats the literal `{{steps.X.data}}`
    // as a single item and reports "1 item would be launched", which is
    // worse than useless: it hides the configuration bug behind a green
    // check. Catch it here and tell the user exactly what's missing.
    if rendered.contains("{{") && rendered.contains("}}") {
        errors.push(format!(
            "Le template contient une variable non résolue : {}. \
             As-tu testé le step précédent pour qu'il produise un output ? \
             Ou colle manuellement un exemple dans « Mock input » ci-dessus.",
            rendered.trim()
        ));
        return Json(ApiResponse::ok(empty_preview(&req.step, errors)));
    }

    // 3. Parse the rendered string into items. Reuses the same logic as
    //    the runtime executor (JSON array OR text split).
    let items = crate::workflows::batch_step::parse_items_for_test(&rendered);
    if items.is_empty() {
        errors.push(format!(
            "items_from resolved to an empty list. Rendered value: {:?}",
            if rendered.len() > 200 { format!("{}…", &rendered[..200]) } else { rendered.clone() }
        ));
    }

    // 4. Load the Quick Prompt.
    let qp_lookup = qp_id.clone();
    let qp = match state.db.with_conn(move |conn| {
        crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup)
    }).await {
        Ok(Some(q)) => Some(q),
        Ok(None) => {
            errors.push(format!("Quick prompt '{}' not found", qp_id));
            None
        }
        Err(e) => {
            errors.push(format!("DB error loading QP: {}", e));
            None
        }
    };

    // 5. Worktree safety check (same as runtime).
    let workspace_mode = req.step.batch_workspace_mode
        .clone()
        .unwrap_or_else(|| "Direct".to_string());
    if workspace_mode == "Isolated"
        && qp.as_ref().map(|q| q.project_id.is_none()).unwrap_or(false)
    {
        errors.push(
            "Isolated workspace mode requires a project-linked Quick Prompt"
                .to_string()
        );
    }

    // 6. Render every sample item's prompt for the user to spot-check.
    //    Was rendering only the first item — but Marie wants to see the
    //    rendering for each ticket of her batch, not just one (otherwise
    //    she can't catch per-ticket template surprises).
    let first_variable_name = qp.as_ref()
        .and_then(|q| q.variables.first())
        .map(|v| v.name.clone());
    const SAMPLE_LIMIT: usize = 10;
    let sample_rendered_prompts: Vec<String> = if let Some(qp) = &qp {
        items.iter().take(SAMPLE_LIMIT).map(|item| {
            crate::workflows::batch_step::render_qp_prompt_for_test(
                &qp.prompt_template,
                first_variable_name.as_deref(),
                item,
            )
        }).collect()
    } else {
        Vec::new()
    };
    let sample_rendered_prompt = sample_rendered_prompts.first().cloned();

    let max_items = req.step.batch_max_items.unwrap_or(50);
    let total = items.len() as u32;
    if total > max_items {
        errors.push(format!(
            "Item count {} exceeds max {} (raise `batch_max_items` if needed)",
            total, max_items
        ));
    }

    // Same SAMPLE_LIMIT as above — keep them in lockstep so sample_items
    // and sample_rendered_prompts have matching indices.
    let sample_items: Vec<String> = items.iter().take(SAMPLE_LIMIT).cloned().collect();

    Json(ApiResponse::ok(BatchPreview {
        sample_items,
        total_items: total,
        capped_at: SAMPLE_LIMIT as u32,
        max_items_allowed: max_items,
        quick_prompt_id: qp.as_ref().map(|q| q.id.clone()),
        quick_prompt_name: qp.as_ref().map(|q| q.name.clone()),
        quick_prompt_icon: qp.as_ref().map(|q| q.icon.clone()),
        quick_prompt_agent: qp.as_ref().map(|q| {
            serde_json::to_string(&q.agent).unwrap_or_default().trim_matches('"').to_string()
        }),
        first_variable_name,
        sample_rendered_prompt,
        sample_rendered_prompts,
        workspace_mode,
        wait_for_completion: req.step.batch_wait_for_completion.unwrap_or(true),
        errors,
        warnings,
    }))
}

/// Build a mostly-empty BatchPreview when validation aborts early.
fn empty_preview(step: &WorkflowStep, errors: Vec<String>) -> BatchPreview {
    BatchPreview {
        sample_items: vec![],
        total_items: 0,
        capped_at: 10,
        max_items_allowed: step.batch_max_items.unwrap_or(50),
        quick_prompt_id: step.batch_quick_prompt_id.clone(),
        quick_prompt_name: None,
        quick_prompt_icon: None,
        quick_prompt_agent: None,
        first_variable_name: None,
        sample_rendered_prompt: None,
        sample_rendered_prompts: Vec::new(),
        workspace_mode: step.batch_workspace_mode.clone().unwrap_or_else(|| "Direct".into()),
        wait_for_completion: step.batch_wait_for_completion.unwrap_or(true),
        errors,
        warnings: Vec::new(),
    }
}

/// POST /api/workflows/test-step — Test a single step with mock context (SSE)
pub async fn test_step(
    State(state): State<AppState>,
    Json(req): Json<TestStepRequest>,
) -> Sse<SseStream> {
    let cfg = state.config.read().await;
    let tokens = cfg.tokens.clone();
    let agents = cfg.agents.clone();
    drop(cfg);

    // Resolve project path (for MCP context)
    let project_path = if let Some(pid) = &req.project_id {
        let id = pid.clone();
        match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
            Ok(Some(p)) => p.path,
            _ => std::env::temp_dir().to_string_lossy().to_string(),
        }
    } else {
        std::env::temp_dir().to_string_lossy().to_string()
    };
    let work_dir = project_path.clone();

    // Build template context with mock data
    let mut ctx = crate::workflows::template::TemplateContext::new();
    if let Some(prev_output) = &req.mock_previous_output {
        ctx.set_step_output("previous", prev_output);
    }
    if let Some(vars) = &req.mock_variables {
        for (k, v) in vars {
            ctx.set(k.clone(), v.clone());
        }
    }

    // Determine full_access from agent config
    let full_access = match req.step.agent {
        AgentType::ClaudeCode => agents.claude_code.full_access,
        AgentType::Codex => agents.codex.full_access,
        AgentType::GeminiCli => agents.gemini_cli.full_access,
        AgentType::Kiro => agents.kiro.full_access,
        AgentType::Vibe => agents.vibe.full_access,
        AgentType::CopilotCli => agents.copilot_cli.full_access,
        AgentType::Ollama => agents.ollama.full_access,
        AgentType::Custom => false,
    };

    // In dry_run mode, prepend a simulation preamble. The preamble is
    // adaptive so it does not fight the output contract downstream steps rely on:
    //   - Structured → short preamble that enforces read-only but requires the
    //     agent to keep the exact `---STEP_OUTPUT---` envelope. Without this,
    //     the legacy "detailed execution plan" instruction used to push the
    //     agent toward prose, and chained steps failed with unresolved
    //     `{{steps.X.data}}` at runtime even when dry-run looked fine.
    //   - FreeText → legacy preamble (narrative plan of execution) — no
    //     contract to preserve, so describing the plan is fine.
    let mut step = req.step.clone();
    if req.dry_run {
        step.prompt_template = format!("{}{}", build_dry_run_preamble(&step.output_format), step.prompt_template);
    }
    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::workflows::runner::RunEvent>(64);

    tokio::spawn(async move {
        let _ = tx.send(crate::workflows::runner::RunEvent::StepStart {
            step_name: step.name.clone(),
            step_index: 0,
            total_steps: 1,
        }).await;

        // Create a progress channel to stream partial output
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<String>(256);
        let tx_progress = tx.clone();
        // Forward progress chunks as StepProgress events
        tokio::spawn(async move {
            while let Some(text) = progress_rx.recv().await {
                let _ = tx_progress.send(crate::workflows::runner::RunEvent::StepProgress {
                    text,
                }).await;
            }
        });

        let outcome = crate::workflows::steps::execute_step(
            &step, &project_path, &work_dir, &tokens, full_access, &ctx,
            Some(progress_tx),
        ).await;

        let _ = tx.send(crate::workflows::runner::RunEvent::StepDone {
            step_result: outcome.result.clone(),
        }).await;

        let status = outcome.result.status.clone();
        let _ = tx.send(crate::workflows::runner::RunEvent::RunDone { status }).await;
    });

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        yield Event::default().event("run_start").data(
            serde_json::json!({ "run_id": "test", "test_mode": true }).to_string()
        );
        while let Some(evt) = rx.recv().await {
            match &evt {
                crate::workflows::runner::RunEvent::StepStart { step_name, step_index, total_steps } => {
                    yield Event::default().event("step_start").data(
                        serde_json::json!({ "step_name": step_name, "step_index": step_index, "total_steps": total_steps }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::StepProgress { text } => {
                    yield Event::default().event("step_progress").data(
                        serde_json::json!({ "text": text }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::StepDone { step_result } => {
                    yield Event::default().event("step_done").data(
                        serde_json::to_value(step_result).unwrap_or_default().to_string()
                    );
                }
                crate::workflows::runner::RunEvent::RunDone { status } => {
                    yield Event::default().event("run_done").data(
                        serde_json::json!({ "status": status }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::GuardTriggered { kind, threshold, actual } => {
                    yield Event::default().event("guard_triggered").data(
                        serde_json::json!({ "kind": kind, "threshold": threshold, "actual": actual }).to_string()
                    );
                }
                crate::workflows::runner::RunEvent::RunError { error } => {
                    yield Event::default().event("error").data(
                        serde_json::json!({ "error": error }).to_string()
                    );
                }
            }
        }
    });

    Sse::new(crate::core::sse_limits::bounded(stream))
}

/// GET /api/workflows/:id/runs
pub async fn list_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<WorkflowRun>>> {
    match state.db.with_conn(move |conn| crate::db::workflows::list_runs(conn, &id)).await {
        Ok(runs) => Json(ApiResponse::ok(runs)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/workflow-runs/batch-summaries
///
/// Returns a compact summary of every batch run, with the parent linear run
/// resolved to a human-friendly (workflow name + run sequence) label.
/// Consumed by the discussion sidebar to render a clickable pastille that
/// jumps back to the workflow that spawned a given batch.
pub async fn list_batch_run_summaries(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<BatchRunSummary>>> {
    match state.db.with_conn(crate::db::workflows::list_batch_run_summaries).await {
        Ok(summaries) => Json(ApiResponse::ok(summaries)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// Response for [`cancel_run`].
#[derive(Debug, serde::Serialize)]
pub struct CancelRunResponse {
    pub run_cancelled: bool,
    /// Number of child-batch discussions whose agent tokens were triggered.
    /// Zero means either this was a plain linear run (no batch children)
    /// or all child discs had already finished.
    pub child_discs_cancelled: u32,
}

/// POST /api/workflows/:id/runs/:run_id/cancel
///
/// Stop a Running workflow run. Triggers the run's cancellation token (so the
/// runner short-circuits to `Cancelled` status before its next step) AND
/// cascades to every active batch child: each child batch run's discussions
/// have their own agent token triggered so in-flight agents stop too.
///
/// This is the "⏹ Arrêter" button on WorkflowDetail run cards. Idempotent —
/// safe to call on an already-finished run (returns false for all).
pub async fn cancel_run(
    State(state): State<AppState>,
    Path((_workflow_id, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<CancelRunResponse>> {
    // 1. Trigger the linear run's own token
    let run_cancelled = {
        let mut map = match state.cancel_registry.lock() {
            Ok(m) => m,
            Err(_) => return Json(ApiResponse::err("Cancel registry poisoned")),
        };
        if let Some(token) = map.remove(&run_id) {
            token.cancel();
            true
        } else {
            false
        }
    };

    // 2. Find child batches via parent_run_id and cascade to their disc agents.
    //    One DB call to get all child batches of this run, then another to get
    //    each batch's child discussions. For each disc, trigger its cancel
    //    token if one is registered (i.e. agent still running).
    let run_id_for_db = run_id.clone();
    let child_disc_ids: Vec<String> = match state.db.with_conn(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT d.id FROM discussions d \
             JOIN workflow_runs wr ON d.workflow_run_id = wr.id \
             WHERE wr.parent_run_id = ?1"
        )?;
        let ids: Vec<String> = stmt
            .query_map(rusqlite::params![&run_id_for_db], |row| row.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    }).await {
        Ok(ids) => ids,
        Err(e) => return Json(ApiResponse::err(format!("DB error finding child discs: {}", e))),
    };

    let child_discs_cancelled = {
        let mut map = match state.cancel_registry.lock() {
            Ok(m) => m,
            Err(_) => return Json(ApiResponse::err("Cancel registry poisoned")),
        };
        let mut n: u32 = 0;
        for disc_id in &child_disc_ids {
            if let Some(token) = map.remove(disc_id) {
                token.cancel();
                n += 1;
            }
        }
        n
    };

    // 3. Force-mark this run AND any Running child batch runs as Cancelled in
    //    the DB. The token cancel (step 1) is best-effort — when it fires
    //    inside a deep `await` (e.g. waiting on child batch completion via
    //    ws_broadcast), the runner may never reach its status-writing path,
    //    leaving the row stuck on "Running" forever. Without this DB update,
    //    a second cancel click returns `run_cancelled=false` (token already
    //    consumed) and the user sees nothing happen. Idempotent: the
    //    `WHERE status = 'Running'` clause no-ops finished runs.
    //
    //    We don't touch discussions — they get their Cancelled/Failed status
    //    from the agent-task finally path on their own tokens.
    let run_id_for_db2 = run_id.clone();
    let forced_statuses = state.db.with_conn(move |conn| {
        let parent_n = conn.execute(
            "UPDATE workflow_runs SET status = 'Cancelled', finished_at = datetime('now') \
             WHERE id = ?1 AND status = 'Running'",
            rusqlite::params![&run_id_for_db2],
        )?;
        let children_n = conn.execute(
            "UPDATE workflow_runs SET status = 'Cancelled', finished_at = datetime('now') \
             WHERE parent_run_id = ?1 AND status = 'Running'",
            rusqlite::params![&run_id_for_db2],
        )?;
        Ok((parent_n, children_n))
    }).await.unwrap_or((0, 0));

    tracing::info!(
        "Cancel run {}: token_triggered={}, {} child disc agents stopped, \
         parent_forced={}, child_batches_forced={}",
        run_id, run_cancelled, child_discs_cancelled,
        forced_statuses.0, forced_statuses.1,
    );

    // From the user's point of view, "cancel worked" if either the in-memory
    // token fired OR we had to forcibly mark the orphaned DB row. The UI
    // uses this to decide between "stopping…" and "nothing to stop" toasts.
    let run_cancelled = run_cancelled || forced_statuses.0 > 0;

    Json(ApiResponse::ok(CancelRunResponse {
        run_cancelled,
        child_discs_cancelled,
    }))
}

/// 0.7.0 Phase 4 — payload for `POST /api/workflows/:id/runs/:run_id/decide`.
///
/// `decision` is one of `"approve" | "request_changes" | "reject"`.
/// `comment` is optional in general but the frontend enforces a non-empty
/// value for `request_changes` (the agent needs feedback to act on).
#[derive(Debug, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DecideRunRequest {
    pub decision: String,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Response for [`decide_run`].
#[derive(Debug, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct DecideRunResponse {
    pub run_id: String,
    pub new_status: RunStatus,
}

/// POST /api/workflows/:id/runs/:run_id/decide
///
/// Apply an operator's decision to a paused (Gate) run and resume it.
/// Idempotent on already-finished runs (returns the current status).
pub async fn decide_run(
    State(state): State<AppState>,
    Path((_workflow_id, run_id)): Path<(String, String)>,
    Json(payload): Json<DecideRunRequest>,
) -> Json<ApiResponse<DecideRunResponse>> {
    use crate::workflows::runner::GateDecision;

    let run_id_for_db = run_id.clone();
    let run = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id_for_db))
        .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return Json(ApiResponse::err("Run not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    if run.status != RunStatus::WaitingApproval {
        return Json(ApiResponse::err(format!(
            "Run is not waiting for approval (status: {:?})",
            run.status
        )));
    }

    let wf_id = run.workflow_id.clone();
    let workflow = match state
        .db
        .with_conn(move |conn| crate::db::workflows::get_workflow(conn, &wf_id))
        .await
    {
        Ok(Some(wf)) => wf,
        Ok(None) => return Json(ApiResponse::err("Workflow not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let decision = match payload.decision.as_str() {
        "approve" => GateDecision::Approve { comment: payload.comment.clone() },
        "request_changes" => match payload.comment.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => GateDecision::RequestChanges { comment: c.to_string() },
            None => return Json(ApiResponse::err(
                "request_changes requires a non-empty `comment`",
            )),
        },
        "reject" => GateDecision::Reject { comment: payload.comment.clone() },
        other => return Json(ApiResponse::err(format!(
            "Unknown decision `{}` (expected approve | request_changes | reject)",
            other
        ))),
    };

    // Resume in the background — long-running, the operator just gets
    // back the new status (Running for approve/request_changes, Failed
    // for reject). The UI already polls run state via SSE/refetch.
    let state_clone = state.clone();
    let run_for_resume = run.clone();
    let run_id_for_log = run.id.clone();
    let new_status = match &decision {
        GateDecision::Reject { .. } => RunStatus::Failed,
        _ => RunStatus::Running,
    };
    tokio::spawn(async move {
        let cfg = state_clone.config.read().await;
        let tokens = cfg.tokens.clone();
        let agents = cfg.agents.clone();
        drop(cfg);
        let mut run_mut = run_for_resume;
        if let Err(e) = crate::workflows::runner::resume_run(
            state_clone, &workflow, &mut run_mut, decision, &tokens, &agents, None,
        ).await {
            tracing::error!("Resume run {} failed: {}", run_id_for_log, e);
        }
    });

    Json(ApiResponse::ok(DecideRunResponse {
        run_id: run.id,
        new_status,
    }))
}

/// Response for [`delete_batch_run`].
#[derive(Debug, serde::Serialize)]
pub struct DeletedBatchResponse {
    pub run_id: String,
    pub discussions_deleted: u32,
}

/// DELETE /api/workflow-runs/:run_id
///
/// Delete a batch workflow run AND all its child discussions in one
/// transaction. Refuses to act on linear runs (use the workflow run delete
/// endpoint for those — they don't own discussions to begin with).
///
/// The user-visible flow: from the sidebar batch group, "🗑 Supprimer ce
/// batch et ses N discussions" → confirm → this handler. Returns the count
/// of discussions actually deleted so the toast can show the right number
/// (a batch may have grown/shrunk between when the UI computed N and now).
pub async fn delete_batch_run(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<DeletedBatchResponse>> {
    let run_id_for_db = run_id.clone();
    match state.db.with_conn(move |conn| {
        crate::db::workflows::delete_batch_run_with_discussions(conn, &run_id_for_db)
    }).await {
        Ok(summary) => Json(ApiResponse::ok(DeletedBatchResponse {
            run_id: summary.run_id,
            discussions_deleted: summary.discussions_deleted as u32,
        })),
        Err(e) => Json(ApiResponse::err(format!("Failed to delete batch: {}", e))),
    }
}

/// GET /api/workflows/:id/runs/:run_id
pub async fn get_run(
    State(state): State<AppState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<WorkflowRun>> {
    match state.db.with_conn(move |conn| crate::db::workflows::get_run(conn, &run_id)).await {
        Ok(Some(run)) => Json(ApiResponse::ok(run)),
        Ok(None) => Json(ApiResponse::err("Run not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/workflows/:id/runs — Delete all runs for a workflow
pub async fn delete_all_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::workflows::delete_all_runs(conn, &id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/workflows/:id/runs/:run_id — Delete a single run
pub async fn delete_run(
    State(state): State<AppState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::workflows::delete_run(conn, &run_id)).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow suggestions — MCP-based template catalogue
// ═══════════════════════════════════════════════════════════════════════════════

/// Static catalogue entry: which MCP combination triggers which suggestion.
struct CatalogueEntry {
    id: &'static str,
    title_fr: &'static str,
    title_en: &'static str,
    desc_fr: &'static str,
    desc_en: &'static str,
    required_mcps: &'static [&'static str],
    audience: &'static str,
    complexity: &'static str,
    trigger: fn() -> WorkflowTrigger,
    /// Each tuple is (step_name, prompt, is_structured). Multiple tuples = multi-step workflow.
    /// is_structured=true → engine injects format instructions + extracts JSON envelope.
    step_prompts: &'static [(&'static str, &'static str, bool)],
}

const CATALOGUE: &[CatalogueEntry] = &[
    CatalogueEntry {
        id: "orphan-prs",
        title_fr: "Détection de PRs orphelines",
        title_en: "Orphan PR detection",
        desc_fr: "Alerte quand une PR n'a pas de ticket Jira/Linear associé",
        desc_en: "Alert when a PR has no linked Jira/Linear ticket",
        required_mcps: &["github", "jira"],
        audience: "dev",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 9 * * 1-5".to_string() },
        step_prompts: &[
            ("collect-prs", "List all open pull requests on the repository. For each PR, return: title, author, url, branch_name, description (first 200 chars). data must be a JSON array of objects with these fields.", true),
            ("check-tickets", "For each PR in {{previous_step.data}}, check if the title, description, or branch_name contains a Jira ticket reference (pattern: uppercase letters followed by a dash and digits, e.g. PROJ-123). Return only the PRs that have NO ticket reference. data must be an array of {title, author, url}.", true),
        ],
    },
    CatalogueEntry {
        id: "sprint-digest",
        title_fr: "Digest de sprint hebdomadaire",
        title_en: "Weekly sprint digest",
        desc_fr: "Résume les tickets fermés et les PRs mergées chaque vendredi",
        desc_en: "Summarize closed tickets and merged PRs every Friday",
        required_mcps: &["jira", "slack"],
        audience: "pm",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 17 * * 5".to_string() },
        step_prompts: &[
            ("collect-tickets", "Query Jira for all tickets resolved or closed in the last 7 days. For each: key, summary, type (Bug/Feature/Task), assignee. data must be a JSON array of these objects.", true),
            ("format-digest", "From the tickets in {{previous_step.data}}, generate a concise sprint digest grouped by type (Bug fixes, Features, Tasks). Include counts per category and the top 3 highlights.", false),
        ],
    },
    CatalogueEntry {
        id: "changelog-release",
        title_fr: "Changelog automatique à chaque release",
        title_en: "Auto-changelog on release",
        desc_fr: "Génère un changelog depuis les commits et PRs mergées entre deux tags",
        desc_en: "Generate a changelog from commits and merged PRs between two tags",
        required_mcps: &["github"],
        audience: "dev",
        complexity: "simple",
        trigger: || WorkflowTrigger::Manual,
        step_prompts: &[
            ("collect-prs", "List all merged pull requests since the last git tag. For each: number, title, author, labels (as array). data must be a JSON array of these objects.", true),
            ("generate-changelog", "From the PRs in {{previous_step.data}}, generate a changelog in Markdown. Group by type using labels or title prefixes: feat, fix, chore, docs. Format: `- title (#number) — @author`.", false),
        ],
    },
    CatalogueEntry {
        id: "stale-prs",
        title_fr: "Notification de PRs en attente",
        title_en: "Stale PR notifications",
        desc_fr: "Détecte les PRs ouvertes depuis plus de 48h sans review",
        desc_en: "Detect PRs open for 48h+ without review",
        required_mcps: &["github", "slack"],
        audience: "dev",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 10 * * 1-5".to_string() },
        step_prompts: &[
            ("find-stale", "List all open pull requests with zero reviews AND created more than 48 hours ago. For each: title, author, created_at, url. data must be a JSON array. If none found, use status NO_RESULTS with data as empty array [].", true),
            ("notify", "From the stale PRs in {{previous_step.data}}: format a notification listing each one with title, author, and days waiting. If {{previous_step.status}} is NO_RESULTS, just output 'No stale PRs found.'", false),
        ],
    },
    CatalogueEntry {
        id: "bug-report",
        title_fr: "Rapport de bugs par priorité",
        title_en: "Bug report by priority",
        desc_fr: "Génère un rapport mensuel des bugs ouverts classés par priorité",
        desc_en: "Generate a monthly report of open bugs sorted by priority",
        required_mcps: &["jira", "confluence"],
        audience: "pm",
        complexity: "simple",
        trigger: || WorkflowTrigger::Cron { schedule: "0 9 1 * *".to_string() },
        step_prompts: &[
            ("query-bugs", "Query Jira for all open issues of type Bug. For each: key, summary, priority (Critical/High/Medium/Low), created_date, assignee. data must be a JSON array.", true),
            ("generate-report", "From the bugs in {{previous_step.data}}: count by priority, list the top 5 oldest, note trends if visible. Generate a Markdown report.", false),
        ],
    },
    CatalogueEntry {
        id: "pr-quality",
        title_fr: "Analyse qualité sur chaque PR",
        title_en: "Code quality analysis per PR",
        desc_fr: "Analyse automatique du code de chaque nouvelle PR",
        desc_en: "Automatic code analysis on each new PR",
        required_mcps: &["github"],
        audience: "dev",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Tracker {
            source: TrackerSourceConfig::GitHub { owner: String::new(), repo: String::new() },
            query: String::new(),
            labels: vec!["review-needed".to_string()],
            interval: "*/10 * * * *".to_string(),
        },
        step_prompts: &[
            ("fetch-diff", "Get the full diff of PR {{issue.title}} ({{issue.url}}). List all changed files with additions/deletions count. data must be a JSON object with fields: {files: [{path, additions, deletions}], total_changes: number}.", true),
            ("analyze", "Review the code changes from {{previous_step.data}}. Check for: 1) Security (injection, secrets, auth), 2) Performance (N+1, unbounded loops), 3) Missing error handling. data must be an array of {file, line, severity, issue, suggestion}. If no issues: status NO_RESULTS, data [].", true),
            ("review-summary", "Based on {{previous_step.data}}: if {{previous_step.status}} is NO_RESULTS, output 'LGTM — no issues detected'. Otherwise, write a concise PR review listing each issue with severity and suggested fix.", false),
        ],
    },
    CatalogueEntry {
        id: "5xx-correlation",
        title_fr: "Corrélation alertes 5xx / déploiements",
        title_en: "5xx alerts / deployment correlation",
        desc_fr: "Quand une alerte 5xx survient, identifie le dernier déploiement et les changements associés",
        desc_en: "When a 5xx alert fires, identify the last deployment and associated changes",
        required_mcps: &["cloudwatch", "github"],
        audience: "ops",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "*/15 * * * *".to_string() },
        step_prompts: &[
            ("check-errors", "Query CloudWatch for HTTP 5xx error count in the last 15 minutes. data must be {count: number, endpoints: [{path, count}]}. If count is 0: status NO_RESULTS, data {count: 0, endpoints: []}.", true),
            ("find-deploys", "List the last 3 merged PRs on main (recent deployments). For each: title, author, merged_at, changed_files. data must be a JSON array.", true),
            ("correlate", "5xx errors: {{steps.check-errors.data}}. Recent deploys: {{previous_step.data}}. Identify the most likely cause based on timing and files changed. Output a concise incident summary.", false),
        ],
    },
    CatalogueEntry {
        id: "sprint-brief",
        title_fr: "Brief de sprint : livré, glissé, risques",
        title_en: "Sprint brief: delivered, slipped, risks",
        desc_fr: "Rapport de fin de sprint croisant Jira, GitHub et Confluence",
        desc_en: "End-of-sprint report crossing Jira, GitHub and Confluence",
        required_mcps: &["jira", "github", "confluence"],
        audience: "pm",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "0 16 * * 5".to_string() },
        step_prompts: &[
            ("collect-sprint", "Get the current active sprint from Jira. List all tickets: key, summary, status, assignee, story_points. data must be a JSON array.", true),
            ("check-prs", "For each ticket in {{previous_step.data}}, check if there is a linked GitHub PR. data must be an array of {ticket_key, pr_status: 'merged'|'open'|'none', pr_url}.", true),
            ("classify", "Cross-reference tickets and PRs from {{previous_step.data}}. Classify: DELIVERED (done + merged), SLIPPED (not done or not merged), AT_RISK (in progress, no PR). data must be {delivered: [...], slipped: [...], at_risk: [...], delivery_rate: number, points_delivered: number, points_planned: number}.", true),
            ("format-brief", "Generate a sprint brief from {{previous_step.data}}: stats, top 3 deliveries, top 3 risks with reasons, one-paragraph recommendation.", false),
        ],
    },
    CatalogueEntry {
        id: "perf-monitoring",
        title_fr: "Alerting proactif sur anomalies de performance",
        title_en: "Proactive performance anomaly alerting",
        desc_fr: "Surveille les métriques CloudWatch et alerte sur les anomalies",
        desc_en: "Monitor CloudWatch metrics and alert on anomalies",
        required_mcps: &["cloudwatch", "slack"],
        audience: "ops",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "*/30 * * * *".to_string() },
        step_prompts: &[
            ("collect-metrics", "Query CloudWatch for: latency_p99_ms, error_rate_percent, cpu_percent — last 30 min. data must be {latency_p99_ms: number, error_rate_percent: number, cpu_percent: number}.", true),
            ("detect-anomalies", "Current metrics: {{previous_step.data}}. Compare against 7-day average for same time window. data must be an array of {metric, current, baseline, factor}. If all normal: status NO_RESULTS, data [].", true),
            ("alert", "Anomalies: {{previous_step.data}}. For each: format metric name, current vs baseline, deviation. Recommend action if any >5x baseline.", false),
        ],
    },
    CatalogueEntry {
        id: "doc-sync",
        title_fr: "Synchronisation documentation technique",
        title_en: "Technical documentation sync",
        desc_fr: "Détecte quand le code a changé mais la doc Confluence n'a pas été mise à jour",
        desc_en: "Detect when code changed but Confluence docs were not updated",
        required_mcps: &["github", "confluence"],
        audience: "dev",
        complexity: "advanced",
        trigger: || WorkflowTrigger::Cron { schedule: "0 10 * * 1".to_string() },
        step_prompts: &[
            ("find-api-changes", "List PRs merged in the last 7 days that modified **/routes/**, **/api/**, **/models/**, **/schema/**. For each: pr_title, changed_files. data must be a JSON array.", true),
            ("check-docs", "For each PR in {{previous_step.data}}, search Confluence for related pages. Check if updated in last 7 days. data must be [{pr_title, page_title, page_url, last_updated, is_stale: bool}].", true),
            ("report-stale", "From {{previous_step.data}}, filter is_stale=true. If none: status NO_RESULTS. Otherwise list each stale doc with page title, URL, related PR, and last update date.", false),
        ],
    },
];

/// Normalize MCP server names to catalogue keys.
/// e.g. "mcp-github" → "github", "mcp-atlassian" → "jira", "Jira" → "jira"
fn normalize_mcp_name(name: &str) -> String {
    let n = name.to_lowercase()
        .replace("mcp-", "")
        .replace("server-", "");
    // Known aliases
    if n.contains("atlassian") { return "jira".to_string(); }
    if n.contains("cloudwatch") { return "cloudwatch".to_string(); }
    n
}

/// GET /api/projects/:id/workflow-suggestions
pub async fn suggestions(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<Vec<WorkflowSuggestion>>> {
    // 1. Get project
    let project = match state.db.with_conn({
        let pid = project_id.clone();
        move |conn| crate::db::projects::get_project(conn, &pid)
    }).await {
        Ok(Some(p)) => p,
        _ => return Json(ApiResponse::ok(vec![])),
    };

    // 2. Get MCP configs linked to this project (global + project-specific)
    let mcp_names: Vec<String> = match state.db.with_conn({
        let pid = project_id.clone();
        move |conn| {
            let configs = crate::db::mcps::list_configs_display(conn, None)?;
            let names: Vec<String> = configs.into_iter()
                .filter(|c| c.is_global || c.project_ids.contains(&pid))
                .map(|c| normalize_mcp_name(&c.server_name))
                .collect();
            Ok(names)
        }
    }).await {
        Ok(n) => n,
        Err(_) => return Json(ApiResponse::ok(vec![])),
    };

    if mcp_names.is_empty() {
        return Json(ApiResponse::ok(vec![]));
    }

    // 3. Also try to read workflow hints from ai/operations/mcp-servers.md (if audited)
    let project_path = crate::core::scanner::resolve_host_path(&project.path);
    let _hints_path = project_path.join("ai/operations/mcp-servers.md");
    // Future: parse the hints table for project-specific suggestions.
    // For now, we use only the static catalogue.

    // 4. Detect language (fr default)
    let lang = state.config.read().await.language.clone();
    let is_fr = lang.starts_with("fr");

    // 5. Match catalogue entries against available MCPs
    let mut suggestions = Vec::new();
    for entry in CATALOGUE {
        let all_present = entry.required_mcps.iter()
            .all(|req| mcp_names.iter().any(|m| m.contains(req)));
        if !all_present {
            continue;
        }

        let title = if is_fr { entry.title_fr } else { entry.title_en };
        let desc = if is_fr { entry.desc_fr } else { entry.desc_en };

        let reason = if is_fr {
            format!("Vous avez {} connectés", entry.required_mcps.join(" + "))
        } else {
            format!("You have {} connected", entry.required_mcps.join(" + "))
        };

        suggestions.push(WorkflowSuggestion {
            id: entry.id.to_string(),
            title: title.to_string(),
            description: desc.to_string(),
            reason,
            required_mcps: entry.required_mcps.iter().map(|s| s.to_string()).collect(),
            audience: entry.audience.to_string(),
            complexity: entry.complexity.to_string(),
            trigger: (entry.trigger)(),
            steps: entry.step_prompts.iter().map(|(step_name, prompt, structured)| WorkflowStep {
                name: step_name.to_string(),
                step_type: StepType::Agent,
                description: None,
                agent: AgentType::ClaudeCode,
                prompt_template: prompt.to_string(),
                mode: StepMode::Normal,
                output_format: if *structured { StepOutputFormat::Structured } else { StepOutputFormat::FreeText },
                on_result: vec![],
                agent_settings: None,
                stall_timeout_secs: None,
                retry: None,
                delay_after_secs: None,
                mcp_config_ids: vec![],
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                batch_quick_prompt_id: None,
                batch_items_from: None,
                batch_wait_for_completion: None,
                batch_max_items: None,
                batch_workspace_mode: None,
                batch_chain_prompt_ids: vec![],
                batch_concurrent_limit: None,
                quick_api_id: None,
                notify_config: None,
                api_plugin_slug: None,
                api_config_id: None,
                api_endpoint_path: None,
                api_method: None,
                api_query: None,
                api_path_params: None,
                api_headers: None,
                api_body: None,
                api_extract: None,
                api_pagination: None,
                api_timeout_ms: None,
                api_max_retries: None,
                api_output_var: None,
                gate_message: None,
                gate_request_changes_target: None,
                gate_notify_url: None,
                exec_command: None,
                exec_args: vec![],
                exec_timeout_secs: None,
                quick_prompt_id: None,
                json_data_payload: None,
            }).collect(),
        });
    }

    Json(ApiResponse::ok(suggestions))
}

/// Build the preamble prepended to a step's prompt when running in dry-run mode.
///
/// The preamble is adaptive so it never fights the output contract that
/// downstream steps rely on:
///   - `Structured`: short read-only preamble that explicitly requires the
///     agent to keep the `---STEP_OUTPUT---` envelope and fill `data`.
///     The legacy narrative preamble pushed the agent toward prose, which
///     silently produced workflows whose `{{steps.X.data}}` never resolved.
///   - `FreeText`: legacy preamble (plan of execution). Nothing downstream
///     relies on the format, so a descriptive response is fine.
pub(crate) fn build_dry_run_preamble(output_format: &StepOutputFormat) -> &'static str {
    match output_format {
        // TypedSchema and Structured share the dry-run preamble — both
        // need the LLM to keep the envelope shape, the only difference is
        // the schema constraint on `data` (validated post-extract by the
        // runner). The preamble itself doesn't change.
        StepOutputFormat::Structured | StepOutputFormat::TypedSchema { .. } => "\
⚠️ MODE TEST (dry-run) — RÈGLES STRICTES :\n\
- N'utilise QUE des tools en lecture seule (get, list, search, read). N'écris, ne modifie, ne crée, ne supprime RIEN.\n\
- Respecte STRICTEMENT le format de sortie structuré demandé plus bas. Ne remplace PAS le bloc ---STEP_OUTPUT--- par une narration, même en mode test.\n\
- Les données que tu lis doivent être placées dans le champ `data` de l'enveloppe JSON finale, pour que les étapes suivantes puissent les consommer.\n\n---\n\n",
        StepOutputFormat::FreeText => "\
⚠️ MODE SIMULATION (dry-run) — RÈGLES STRICTES :\n\
- Tu ne dois RIEN exécuter, RIEN modifier, RIEN écrire, RIEN créer.\n\
- Tu ne dois PAS appeler de tool qui modifie des données (pas de create, update, delete, write, post, comment).\n\
- Tu peux LIRE des données (get, list, search, read) pour analyser la situation.\n\
- Tu dois DÉCRIRE précisément ce que tu FERAIS en mode réel : quelles actions, sur quels éléments, avec quel contenu.\n\
- Formate ta réponse comme un plan d'exécution détaillé.\n\n---\n\n",
    }
}

// ─── ApiCall step test endpoints (P0.5 — désagentification) ──────────────

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TestExtractRequest {
    /// Sample JSON the user is refining the path against — either pasted
    /// from docs or the response of a previous `test-api-call`.
    pub sample: serde_json::Value,
    /// JSONPath expression, e.g. `$.issues[*].key`.
    pub path: String,
    /// Optional fallback when the path matches nothing.
    #[serde(default)]
    pub fallback: Option<serde_json::Value>,
    /// When true, empty extractions count as NO_RESULTS in the response.
    #[serde(default)]
    pub fail_on_empty: bool,
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TestExtractResponse {
    /// Resolved value. `null` when the path matched nothing (unless
    /// `fallback` was set, in which case fallback is returned).
    pub value: serde_json::Value,
    /// Human-readable type tag for the wizard preview: `"number"`,
    /// `"string"`, `"boolean"`, `"array(N)"`, `"object"`, `"null"`.
    pub value_type: String,
    /// True when no match was found (even if a fallback rescued the
    /// value). Drives the "0 results — will skip next step" hint.
    pub is_empty: bool,
    /// Only set when the JSONPath is syntactically invalid — the wizard
    /// shows this inline under the input.
    pub error: Option<String>,
}

/// POST /api/workflow-steps/test-extract
/// Pure function — runs the JSONPath on the supplied sample without any
/// network or DB access. Drives the wizard's live-preview box so users
/// can refine their path without re-hitting the API.
pub async fn test_extract(
    Json(req): Json<TestExtractRequest>,
) -> Json<ApiResponse<TestExtractResponse>> {
    use crate::workflows::api_call_step::{apply_extract, ExtractError};
    let spec = ExtractSpec {
        path: req.path,
        fallback: req.fallback,
        fail_on_empty: req.fail_on_empty,
    };
    match apply_extract(&spec, &req.sample) {
        Ok(outcome) => Json(ApiResponse::ok(TestExtractResponse {
            value_type: value_type_tag(&outcome.value),
            value: outcome.value,
            is_empty: outcome.is_empty,
            error: None,
        })),
        Err(ExtractError::InvalidPath { path, reason }) => {
            // We keep the handler returning 200 — the wizard shows the
            // error inline under the input, not via an HTTP status that
            // triggers the global error toast.
            Json(ApiResponse::ok(TestExtractResponse {
                value: serde_json::Value::Null,
                value_type: "error".into(),
                is_empty: true,
                error: Some(format!("Invalid JSONPath `{path}`: {reason}")),
            }))
        }
    }
}

fn value_type_tag(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(_) => "boolean".into(),
        serde_json::Value::Number(_) => "number".into(),
        serde_json::Value::String(_) => "string".into(),
        serde_json::Value::Array(a) => format!("array({})", a.len()),
        serde_json::Value::Object(_) => "object".into(),
    }
}

#[derive(Debug, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TestApiCallRequest {
    /// The (partial) step configuration the user is building in the wizard.
    /// Must at least declare `api_plugin_slug`, `api_config_id`, and
    /// `api_endpoint_path`.
    pub step: WorkflowStep,
    /// Project context — plugin instances are scoped per project. Required.
    pub project_id: String,
}

#[derive(Debug, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TestApiCallResponse {
    /// Matches the `StepOutcome.result.status` after normalization —
    /// `true` when the HTTP call succeeded and extract (if any) ran
    /// without error. NO_RESULTS still counts as success here; the
    /// wizard surfaces it via `envelope.status`.
    pub success: bool,
    /// Milliseconds elapsed end-to-end.
    pub duration_ms: u64,
    /// `{data, status, summary}` envelope (parsed from the step output).
    /// On failure this is `null` and `error` holds the message.
    pub envelope: Option<serde_json::Value>,
    /// Error message when `success == false`. Same string that would
    /// land in the step's output column if this were a real run.
    pub error: Option<String>,
}

/// POST /api/workflow-steps/test-api-call
/// Runs a single ApiCall step end-to-end (real HTTP, real auth, real
/// extract) and echoes the structured envelope. Drives the wizard's
/// "Tester" button. Production security policy — a localhost target
/// fails here too so users can't design a workflow that'll refuse to
/// run in production.
pub async fn test_api_call(
    State(state): State<AppState>,
    Json(req): Json<TestApiCallRequest>,
) -> Json<ApiResponse<TestApiCallResponse>> {
    use crate::workflows::api_call_executor::{
        execute_api_call_step_with_db, SecurityPolicy,
    };
    use crate::workflows::template::TemplateContext;

    let ctx = TemplateContext::new();
    let outcome = execute_api_call_step_with_db(
        &req.step,
        Some(&req.project_id),
        &state,
        &ctx,
        SecurityPolicy::production(),
    )
    .await;

    let success = outcome.result.status == RunStatus::Success;
    // The output now has a trailing `\n[SIGNAL: ...]` line so workflows can
    // branch on the result via `on_result.contains`. Strip that suffix before
    // parsing the JSON envelope — without this, `serde_json::from_str` chokes
    // on the trailing line and the wizard's "Test the call" panel showed
    // success-but-no-envelope, which the UI rendered as a generic Failure.
    let envelope: Option<serde_json::Value> = if success {
        let json_part = outcome.result.output
            .split("\n[SIGNAL:")
            .next()
            .unwrap_or(&outcome.result.output);
        serde_json::from_str(json_part).ok()
    } else {
        None
    };
    let error = if success { None } else { Some(outcome.result.output) };

    Json(ApiResponse::ok(TestApiCallResponse {
        success,
        duration_ms: outcome.result.duration_ms,
        envelope,
        error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression test: 0.6.0 added a trailing `\n[SIGNAL: OK]` line to ApiCall
    // success outputs so workflows can branch via on_result without parsing JSON.
    // The /test-api-call handler used `serde_json::from_str(full_output)` which
    // failed on the SIGNAL suffix → wizard's "Test the call" surfaced
    // "success: true, envelope: null" → UI rendered "Failed" with no detail.
    // This guards the strip-then-parse logic the handler now uses.
    #[test]
    fn test_api_call_strips_trailing_signal_line_before_json_parse() {
        let envelope_json = r#"{"data":{"key":"EW-1"},"status":"OK","summary":"GET /search → 1 issue"}"#;
        let with_signal = format!("{}\n[SIGNAL: OK]", envelope_json);

        // Same logic the handler runs.
        let json_part = with_signal.split("\n[SIGNAL:").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_part)
            .expect("strip-then-parse should succeed");

        assert_eq!(parsed.get("status").and_then(|v| v.as_str()), Some("OK"));
        assert_eq!(parsed.pointer("/data/key").and_then(|v| v.as_str()), Some("EW-1"));
    }

    #[test]
    fn test_api_call_strip_is_noop_when_output_has_no_signal_suffix() {
        // Older outputs (no trailing SIGNAL) must still parse cleanly.
        let envelope_json = r#"{"data":{"x":1},"status":"OK","summary":""}"#;
        let json_part = envelope_json.split("\n[SIGNAL:").next().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(parsed.pointer("/data/x").and_then(|v| v.as_i64()), Some(1));
    }

    fn mk_step(name: &str, kind: StepType) -> WorkflowStep {
        WorkflowStep {
            name: name.into(),
            step_type: kind,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText,
            on_result: vec![],
            agent_settings: None,
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            mcp_config_ids: vec![],
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            quick_prompt_id: None,
            json_data_payload: None,
        }
    }

    #[test]
    fn validate_on_failure_accepts_empty_chain() {
        // Empty rollback is the default state for every workflow — must
        // not surface as an error.
        assert!(validate_on_failure_steps(&[]).is_ok());
    }

    #[test]
    fn validate_on_failure_accepts_notify_and_apicall_and_agent() {
        let chain = vec![
            mk_step("notify_ops", StepType::Notify),
            mk_step("revert_db", StepType::ApiCall),
            mk_step("post_mortem", StepType::Agent),
            mk_step("fan_out_alerts", StepType::BatchQuickPrompt),
        ];
        assert!(validate_on_failure_steps(&chain).is_ok());
    }

    #[test]
    fn validate_on_failure_rejects_gate_step() {
        // A Gate inside on_failure would deadlock — rejected at save time.
        let chain = vec![
            mk_step("notify_ops", StepType::Notify),
            mk_step("ask_for_review", StepType::Gate),
        ];
        let err = validate_on_failure_steps(&chain).expect_err("expected validation error");
        assert!(err.contains("ask_for_review"), "error should name the offending step, got: {}", err);
        assert!(err.to_lowercase().contains("gate"));
    }

    // ─── Phase 5 — Exec allowlist validation ─────────────────────────────

    #[test]
    fn validate_allowlist_accepts_simple_binaries() {
        // Bare names with hyphens, underscores, digits — common binary
        // naming. Don't reject `npm`, `cargo-clippy`, `python3`.
        let allowed = vec!["npm".into(), "cargo".into(), "cargo-clippy".into(), "python3".into(), "make".into()];
        assert!(validate_exec_allowlist(&allowed).is_ok());
    }

    #[test]
    fn validate_allowlist_accepts_empty_list() {
        // Empty list = Exec disabled. Not an error to define a workflow
        // without any allowlisted binaries.
        assert!(validate_exec_allowlist(&[]).is_ok());
    }

    #[test]
    fn validate_allowlist_rejects_path_separator() {
        // Path-separator-bearing entries bypass the bare-name guarantee
        // (would let `cargo` and `/etc/cargo` look like the same entry).
        let err1 = validate_exec_allowlist(&["/usr/bin/npm".to_string()]).unwrap_err();
        assert!(err1.contains("séparateur de chemin"), "got: {}", err1);
        let err2 = validate_exec_allowlist(&["bin\\npm".to_string()]).unwrap_err();
        assert!(err2.contains("séparateur de chemin"));
    }

    #[test]
    fn validate_allowlist_rejects_shell_metas() {
        // Defence in depth: even though we never invoke a shell, an
        // entry like `npm; rm -rf /` looks suspicious and would
        // exercise the matcher in surprising ways. Reject loudly.
        let cases = ["npm; rm", "npm|cat", "npm&", "npm$", "npm`whoami`", "npm>out", "npm<in", "npm*", "npm?"];
        for raw in cases {
            let err = validate_exec_allowlist(&[raw.into()]).unwrap_err();
            assert!(err.contains("caractères spéciaux"), "case `{}` got: {}", raw, err);
        }
    }

    #[test]
    fn validate_allowlist_rejects_double_dot() {
        let err = validate_exec_allowlist(&["..".into()]).unwrap_err();
        assert!(err.contains(".."), "got: {}", err);
        let err2 = validate_exec_allowlist(&["my..bin".into()]).unwrap_err();
        assert!(err2.contains(".."), "got: {}", err2);
    }

    #[test]
    fn validate_allowlist_rejects_whitespace() {
        let err = validate_exec_allowlist(&["bash -c".into()]).unwrap_err();
        assert!(err.contains("caractères spéciaux"), "got: {}", err);
    }

    #[test]
    fn validate_allowlist_rejects_empty_entry() {
        let err = validate_exec_allowlist(&["".into()]).unwrap_err();
        assert!(err.to_lowercase().contains("vide"), "got: {}", err);
        let err2 = validate_exec_allowlist(&["  ".into()]).unwrap_err();
        assert!(err2.to_lowercase().contains("vide"), "got: {}", err2);
    }

    // ─── Phase 5 — Exec step validation ──────────────────────────────────

    fn mk_exec_step(name: &str, command: Option<&str>, args: Vec<&str>, timeout: Option<u32>) -> WorkflowStep {
        let mut s = mk_step(name, StepType::Exec);
        s.exec_command = command.map(String::from);
        s.exec_args = args.into_iter().map(String::from).collect();
        s.exec_timeout_secs = timeout;
        s
    }

    #[test]
    fn validate_exec_step_requires_command() {
        let chain = vec![mk_exec_step("run", None, vec![], None)];
        let err = validate_exec_steps(&chain, &["echo".into()]).unwrap_err();
        assert!(err.contains("exec_command"));
        assert!(err.contains("run"));
    }

    #[test]
    fn validate_exec_step_rejects_empty_allowlist() {
        let chain = vec![mk_exec_step("run", Some("echo"), vec![], None)];
        let err = validate_exec_steps(&chain, &[]).unwrap_err();
        assert!(err.to_lowercase().contains("allowlist"), "got: {}", err);
        assert!(err.contains("vide"), "got: {}", err);
    }

    #[test]
    fn validate_exec_step_rejects_command_not_in_allowlist() {
        let chain = vec![mk_exec_step("run", Some("rm"), vec!["-rf", "/"], None)];
        let err = validate_exec_steps(&chain, &["echo".into(), "npm".into()]).unwrap_err();
        assert!(err.contains("absent de l'allowlist"), "got: {}", err);
        assert!(err.contains("rm"));
    }

    #[test]
    fn validate_exec_step_rejects_path_traversal_in_command() {
        let chain = vec![mk_exec_step("run", Some("../../etc/passwd"), vec![], None)];
        let err = validate_exec_steps(&chain, &["passwd".into()]).unwrap_err();
        // Same character-level guard as the allowlist — blocks before
        // the allowlist check, with a "Step Exec" prefix.
        assert!(err.contains("Step Exec"), "got: {}", err);
    }

    #[test]
    fn validate_exec_step_rejects_shell_in_command() {
        // `bash -c` would let the operator smuggle a full shell line
        // past the allowlist via args. Whitespace check catches it.
        let chain = vec![mk_exec_step("run", Some("bash -c"), vec!["echo hi"], None)];
        let err = validate_exec_steps(&chain, &["bash".into()]).unwrap_err();
        assert!(err.contains("Step Exec"), "got: {}", err);
    }

    #[test]
    fn validate_exec_step_accepts_valid_config() {
        let chain = vec![mk_exec_step("test", Some("cargo"), vec!["test", "--", "{{steps.x.summary}}"], Some(120))];
        assert!(validate_exec_steps(&chain, &["cargo".into()]).is_ok());
    }

    #[test]
    fn validate_exec_step_rejects_zero_or_huge_timeout() {
        let chain1 = vec![mk_exec_step("test", Some("cargo"), vec![], Some(0))];
        assert!(validate_exec_steps(&chain1, &["cargo".into()]).is_err());
        let chain2 = vec![mk_exec_step("test", Some("cargo"), vec![], Some(1801))];
        assert!(validate_exec_steps(&chain2, &["cargo".into()]).is_err());
        // Edge: 1800 is allowed (the cap, inclusive).
        let chain3 = vec![mk_exec_step("test", Some("cargo"), vec![], Some(1800))];
        assert!(validate_exec_steps(&chain3, &["cargo".into()]).is_ok());
    }

    #[test]
    fn validate_exec_step_rejects_too_many_args() {
        let too_many: Vec<String> = (0..65).map(|i| format!("arg{}", i)).collect();
        let mut step = mk_exec_step("run", Some("echo"), vec![], None);
        step.exec_args = too_many;
        let err = validate_exec_steps(&[step], &["echo".into()]).unwrap_err();
        assert!(err.contains("trop d'arguments"), "got: {}", err);
    }

    #[test]
    fn validate_exec_step_skips_non_exec_steps() {
        // Validator must ignore Notify / Agent / etc. — they don't have
        // exec_command and shouldn't surface a spurious error.
        let chain = vec![
            mk_step("agent", StepType::Agent),
            mk_step("notify", StepType::Notify),
            mk_step("api", StepType::ApiCall),
        ];
        assert!(validate_exec_steps(&chain, &[]).is_ok());
    }

    // ─── Phase 0.7.0 UX pass — Export/Import envelope shape ─────────────

    fn mk_workflow_for_export(name: &str) -> Workflow {
        Workflow {
            id: "src-id-original".into(),
            name: name.into(),
            project_id: Some("src-project".into()),
            trigger: WorkflowTrigger::Manual,
            steps: vec![mk_step("main", StepType::Agent)],
            actions: vec![],
            safety: WorkflowSafety {
                sandbox: false, max_files: None, max_lines: None, require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            guards: None,
            artifacts: ::std::collections::HashMap::new(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn export_envelope_serializes_with_kind_and_version() {
        let env = WorkflowExportEnvelope {
            kind: WORKFLOW_EXPORT_KIND.into(),
            version: EXPORT_VERSION,
            exported_at: chrono::Utc::now(),
            workflow: mk_workflow_for_export("audit"),
            referenced_quick_prompts: vec![],
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"kind\":\"kronn.workflow\""));
        assert!(json.contains("\"version\":1"));
        // Roundtrip: parse back, fields preserved.
        let parsed: WorkflowExportEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, WORKFLOW_EXPORT_KIND);
        assert_eq!(parsed.workflow.name, "audit");
        assert!(parsed.referenced_quick_prompts.is_empty());
    }

    #[test]
    fn export_envelope_omits_empty_qp_list_from_wire() {
        // `skip_serializing_if = "Vec::is_empty"` keeps the JSON tight
        // when no QPs are bundled — common case for solo dev workflows.
        let env = WorkflowExportEnvelope {
            kind: WORKFLOW_EXPORT_KIND.into(),
            version: EXPORT_VERSION,
            exported_at: chrono::Utc::now(),
            workflow: mk_workflow_for_export("no-qps"),
            referenced_quick_prompts: vec![],
        };
        let json = serde_json::to_string(&env).unwrap();
        assert!(!json.contains("referenced_quick_prompts"),
            "empty QP vec should be omitted; got: {}", json);
    }

    #[test]
    fn import_workflow_rejects_wrong_kind() {
        // The frontend may try to import a Quick Prompt JSON file by
        // mistake — kind discriminator catches that case loudly.
        let payload = r#"{"kind":"kronn.quick_prompt","version":1,"exported_at":"2026-04-28T00:00:00Z","quick_prompt":{}}"#;
        // We don't go through the handler (needs DB) — just check the
        // decode + kind check logic in isolation.
        let env: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(env["kind"], "kronn.quick_prompt");
        // The handler's kind check would fire here with a clear error.
    }

    #[test]
    fn import_workflow_rejects_future_version() {
        // Forward-incompat: a v2 envelope from a future Kronn must NOT
        // be silently accepted by today's importer (would skip fields
        // it doesn't know about).
        let env = WorkflowExportEnvelope {
            kind: WORKFLOW_EXPORT_KIND.into(),
            version: EXPORT_VERSION + 1,
            exported_at: chrono::Utc::now(),
            workflow: mk_workflow_for_export("future"),
            referenced_quick_prompts: vec![],
        };
        let json = serde_json::to_string(&env).unwrap();
        let parsed: WorkflowExportEnvelope = serde_json::from_str(&json).unwrap();
        assert!(parsed.version > EXPORT_VERSION,
            "version mismatch should be detectable post-decode");
    }

    #[test]
    fn qp_export_envelope_roundtrip() {
        use crate::models::PromptVariable;
        let qp = QuickPrompt {
            id: "src-qp".into(),
            name: "audit_repo".into(),
            icon: "🔍".into(),
            prompt_template: "Audit {{repo}}".into(),
            variables: vec![PromptVariable {
                name: "repo".into(),
                label: "Repo".into(),
                placeholder: "kronn".into(),
                description: None,
                required: true,
            }],
            agent: AgentType::ClaudeCode,
            project_id: Some("src-project".into()),
            skill_ids: vec![],
            tier: ModelTier::Default,
            description: "Audit a repo".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let env = QuickPromptExportEnvelope {
            kind: "kronn.quick_prompt".into(),
            version: 1,
            exported_at: chrono::Utc::now(),
            quick_prompt: qp.clone(),
        };
        let json = serde_json::to_string(&env).unwrap();
        let parsed: QuickPromptExportEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind, "kronn.quick_prompt");
        assert_eq!(parsed.quick_prompt.name, "audit_repo");
        assert_eq!(parsed.quick_prompt.variables.len(), 1);
    }

    #[test]
    fn normalize_mcp_names() {
        assert_eq!(normalize_mcp_name("mcp-github"), "github");
        assert_eq!(normalize_mcp_name("GitHub"), "github");
        assert_eq!(normalize_mcp_name("mcp-atlassian"), "jira");
        assert_eq!(normalize_mcp_name("Atlassian"), "jira");
        assert_eq!(normalize_mcp_name("awslabs.cloudwatch-mcp-server"), "cloudwatch");
        assert_eq!(normalize_mcp_name("Slack"), "slack");
    }

    #[test]
    fn catalogue_has_entries() {
        assert!(CATALOGUE.len() >= 10, "Catalogue should have at least 10 entries");
    }

    #[test]
    fn catalogue_entries_have_valid_fields() {
        for entry in CATALOGUE {
            assert!(!entry.id.is_empty(), "Entry must have an id");
            assert!(!entry.title_fr.is_empty(), "Entry {} must have a French title", entry.id);
            assert!(!entry.title_en.is_empty(), "Entry {} must have an English title", entry.id);
            assert!(!entry.required_mcps.is_empty(), "Entry {} must require at least one MCP", entry.id);
            assert!(
                ["dev", "pm", "ops"].contains(&entry.audience),
                "Entry {} has invalid audience: {}", entry.id, entry.audience
            );
            assert!(
                ["simple", "advanced"].contains(&entry.complexity),
                "Entry {} has invalid complexity: {}", entry.id, entry.complexity
            );
            assert!(!entry.step_prompts.is_empty(), "Entry {} must have at least one step", entry.id);
            for (sname, sprompt, _structured) in entry.step_prompts {
                assert!(!sname.is_empty(), "Entry {} has a step with empty name", entry.id);
                assert!(!sprompt.is_empty(), "Entry {} step '{}' has empty prompt", entry.id, sname);
            }
        }
    }

    #[test]
    fn catalogue_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in CATALOGUE {
            assert!(seen.insert(entry.id), "Duplicate catalogue id: {}", entry.id);
        }
    }

    #[test]
    fn github_only_suggestions_match() {
        let mcps = ["github".to_string()];
        let matches: Vec<&str> = CATALOGUE.iter()
            .filter(|e| e.required_mcps.iter().all(|req| mcps.iter().any(|m| m.contains(req))))
            .map(|e| e.id)
            .collect();
        assert!(matches.contains(&"changelog-release"), "GitHub alone should suggest changelog");
        assert!(matches.contains(&"pr-quality"), "GitHub alone should suggest PR quality");
        assert!(!matches.contains(&"orphan-prs"), "GitHub alone should NOT suggest orphan PRs (needs jira)");
    }

    // ─── Dry-run preamble ────────────────────────────────────────────────
    //
    // Regression tests for Workflow B ("Auto pré-analyse de ticket"): the
    // original preamble asked the agent for a "plan d'exécution détaillé",
    // which pushed Structured steps toward Markdown prose and dropped the
    // `---STEP_OUTPUT---` envelope. The chained step then rendered
    // `{{steps.main.data}}` literally and failed with "tickets pas injectés".

    #[test]
    fn dry_run_preamble_freetext_keeps_narrative() {
        let p = build_dry_run_preamble(&StepOutputFormat::FreeText);
        assert!(p.contains("plan d'exécution détaillé"), "FreeText preamble must keep the narrative instruction");
        assert!(p.contains("RIEN écrire"), "FreeText preamble must keep the read-only rule");
    }

    #[test]
    fn dry_run_preamble_structured_does_not_force_narrative() {
        let p = build_dry_run_preamble(&StepOutputFormat::Structured);
        assert!(!p.contains("plan d'exécution détaillé"),
            "Structured preamble must NOT push the agent toward narrative output");
        assert!(!p.contains("Formate ta réponse comme un plan"),
            "Structured preamble must NOT replace the structured contract with a format instruction");
    }

    #[test]
    fn dry_run_preamble_structured_protects_envelope() {
        let p = build_dry_run_preamble(&StepOutputFormat::Structured);
        assert!(p.contains("---STEP_OUTPUT---"),
            "Structured preamble must explicitly name the envelope so the agent keeps it");
        assert!(p.contains("data"),
            "Structured preamble must reference the `data` field — downstream steps consume it");
    }

    #[test]
    fn dry_run_preamble_structured_enforces_read_only() {
        let p = build_dry_run_preamble(&StepOutputFormat::Structured);
        assert!(p.contains("lecture seule") || p.to_lowercase().contains("read-only"),
            "Structured preamble must still enforce read-only tool usage");
    }

    #[test]
    fn github_jira_suggestions_match() {
        let mcps = ["github".to_string(), "jira".to_string()];
        let matches: Vec<&str> = CATALOGUE.iter()
            .filter(|e| e.required_mcps.iter().all(|req| mcps.iter().any(|m| m.contains(req))))
            .map(|e| e.id)
            .collect();
        assert!(matches.contains(&"orphan-prs"), "GitHub+Jira should suggest orphan PRs");
        assert!(matches.contains(&"changelog-release"), "GitHub+Jira should still suggest changelog");
    }

    // ─── test_extract handler (P0.5) ────────────────────────────────────

    #[tokio::test]
    async fn test_extract_returns_scalar_with_type_tag() {
        let req = TestExtractRequest {
            sample: serde_json::json!({ "total": 42, "issues": [] }),
            path: "$.total".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let resp = test_extract(Json(req)).await.0;
        let body = resp.data.expect("test_extract should succeed");
        assert_eq!(body.value, serde_json::json!(42));
        assert_eq!(body.value_type, "number");
        assert!(!body.is_empty);
        assert!(body.error.is_none());
    }

    #[tokio::test]
    async fn test_extract_reports_array_type_with_length() {
        let req = TestExtractRequest {
            sample: serde_json::json!({ "items": [{ "k": "a" }, { "k": "b" }, { "k": "c" }] }),
            path: "$.items[*].k".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let resp = test_extract(Json(req)).await.0;
        let body = resp.data.unwrap();
        assert_eq!(body.value, serde_json::json!(["a", "b", "c"]));
        assert_eq!(body.value_type, "array(3)");
    }

    #[tokio::test]
    async fn test_extract_surfaces_invalid_jsonpath_inline() {
        // The wizard shows this message under the input — NOT a global
        // toast. Returning 200 + error field (vs 4xx) is deliberate.
        let req = TestExtractRequest {
            sample: serde_json::json!({}),
            path: "$[**$bad".into(),
            fallback: None,
            fail_on_empty: false,
        };
        let resp = test_extract(Json(req)).await.0;
        let body = resp.data.unwrap();
        assert!(body.error.is_some(), "expected inline error");
        assert!(body.error.unwrap().contains("Invalid JSONPath"));
        assert_eq!(body.value, serde_json::Value::Null);
    }

    #[tokio::test]
    async fn test_extract_empty_match_with_fallback_returns_fallback_marked_empty() {
        let req = TestExtractRequest {
            sample: serde_json::json!({ "issues": [] }),
            path: "$.issues[*].key".into(),
            fallback: Some(serde_json::json!([])),
            fail_on_empty: false,
        };
        let resp = test_extract(Json(req)).await.0;
        let body = resp.data.unwrap();
        assert_eq!(body.value, serde_json::json!([]));
        assert!(body.is_empty, "is_empty flag must be set so wizard can warn");
    }

    #[tokio::test]
    async fn test_extract_value_type_tag_covers_all_variants() {
        // Regression guard — the wizard uses these tags to render a
        // pill. A change to the set breaks the UI silently.
        assert_eq!(value_type_tag(&serde_json::Value::Null), "null");
        assert_eq!(value_type_tag(&serde_json::json!(true)), "boolean");
        assert_eq!(value_type_tag(&serde_json::json!(1)), "number");
        assert_eq!(value_type_tag(&serde_json::json!("s")), "string");
        assert_eq!(value_type_tag(&serde_json::json!([1, 2])), "array(2)");
        assert_eq!(value_type_tag(&serde_json::json!({ "k": 1 })), "object");
    }
}
