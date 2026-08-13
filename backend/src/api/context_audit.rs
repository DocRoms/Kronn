//! `GET /api/projects/{id}/context-audit` — KT-194.
//!
//! Read-only by construction: the module behind it has no write path, and the tier
//! split it returns is a proposal for a human. An audit that rewrote instruction
//! files could delete the one rule holding something together.

use axum::{
    extract::{Path, State},
    Json,
};

use crate::core::context_audit::{audit_repo, drift, render, ContextAudit, Drift};
use crate::models::{AiAuditStatus, ApiResponse};
use crate::AppState;

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ContextAuditResponse {
    pub project_id: String,
    pub audit: ContextAudit,
    /// The report as text, already bounded.
    pub rendered: String,
    /// `None` on the first inspection, which establishes the baseline.
    pub drift: Option<Drift>,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AuditEvidenceKind {
    NoDocumentation,
    IncompleteTemplate,
    MissingEvidence,
    CorruptState,
    KronnAudit,
    HumanAttestation,
    LegacyEvidence,
    BootstrapOnly,
}

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AuditEvidenceResponse {
    pub project_id: String,
    pub status: AiAuditStatus,
    pub kind: AuditEvidenceKind,
    /// Repository-relative persisted evidence file. Kept distinct from
    /// `.kronn/`, which is the ignored runtime/worktree workspace.
    pub state_file: String,
    pub runtime_workspace: String,
    pub audit_runs: u32,
    pub interrupted_runs: u32,
    pub interruption_rate_percent: f64,
    /// Present only when the authoritative newest run passes the exact resume
    /// gate. The next usable step is this checkpoint + 1.
    pub resumable_after_step: Option<u32>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AttestDocumentationRequest {
    pub confirmed: bool,
}

async fn lookup_project(
    state: &AppState,
    project_id: &str,
) -> Result<crate::models::Project, String> {
    let lookup = project_id.to_string();
    state
        .db
        .with_conn(move |conn| crate::db::projects::get_project(conn, &lookup))
        .await
        .map_err(|error| format!("project lookup failed: {error}"))?
        .ok_or_else(|| format!("no project {project_id}"))
}

fn evidence_kind(root: &std::path::Path, status: &AiAuditStatus) -> AuditEvidenceKind {
    let docs_entry = crate::core::scanner::detect_docs_entry(root);
    if !crate::core::scanner::detect_docs_dir(root).is_dir() {
        return AuditEvidenceKind::NoDocumentation;
    }
    if !docs_entry.is_file() {
        return AuditEvidenceKind::IncompleteTemplate;
    }
    let state_path = crate::core::kronn_state::state_path(root);
    let state = crate::core::kronn_state::read(root);
    if state_path.exists() && state.is_none() {
        return AuditEvidenceKind::CorruptState;
    }
    if let Some(state) = state {
        if let Some(entry) = state.audits.last() {
            return match entry.provenance {
                crate::core::kronn_state::AuditProvenance::KronnAudit => {
                    AuditEvidenceKind::KronnAudit
                }
                crate::core::kronn_state::AuditProvenance::HumanAttestation => {
                    AuditEvidenceKind::HumanAttestation
                }
                crate::core::kronn_state::AuditProvenance::LegacyEvidence => {
                    AuditEvidenceKind::LegacyEvidence
                }
            };
        }
        if state.bootstrapped_at.is_some() {
            return AuditEvidenceKind::BootstrapOnly;
        }
    }
    if matches!(status, AiAuditStatus::TemplateInstalled) {
        AuditEvidenceKind::MissingEvidence
    } else {
        AuditEvidenceKind::IncompleteTemplate
    }
}

async fn build_evidence(
    state: &AppState,
    project: &crate::models::Project,
) -> Result<AuditEvidenceResponse, String> {
    let project_id = project.id.clone();
    let stats_id = project.id.clone();
    let (audit_runs, interrupted_runs, resumable_after_step) = state
        .db
        .with_conn(move |conn| {
            let (total, interrupted): (i64, i64) = conn.query_row(
                "SELECT COUNT(*), COALESCE(SUM(status = 'Interrupted'), 0)
                   FROM audit_runs WHERE project_id = ?1",
                [&stats_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let resumable = crate::db::audit_runs::latest_resumable(conn, &stats_id)?
                .map(|run| run.last_completed_step);
            Ok((total.max(0) as u32, interrupted.max(0) as u32, resumable))
        })
        .await
        .map_err(|error| format!("audit reliability lookup failed: {error}"))?;

    let root = crate::core::scanner::resolve_host_path(&project.path);
    let status = crate::core::scanner::detect_audit_status(&project.path);
    let kind = evidence_kind(&root, &status);
    let interruption_rate_percent = if audit_runs == 0 {
        0.0
    } else {
        (interrupted_runs as f64 / audit_runs as f64) * 100.0
    };
    let docs_dir = crate::core::scanner::detect_docs_dir(&root);
    let state_file = docs_dir
        .strip_prefix(&root)
        .unwrap_or(&docs_dir)
        .join(crate::core::kronn_state::KRONN_STATE_FILENAME)
        .to_string_lossy()
        .to_string();
    Ok(AuditEvidenceResponse {
        project_id,
        status,
        kind,
        state_file,
        runtime_workspace: ".kronn/".to_string(),
        audit_runs,
        interrupted_runs,
        interruption_rate_percent,
        resumable_after_step,
    })
}

/// Explain exactly why the project owns its current audit badge.
pub async fn project_audit_evidence(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<AuditEvidenceResponse>> {
    let project = match lookup_project(&state, &project_id).await {
        Ok(project) => project,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    match build_evidence(&state, &project).await {
        Ok(evidence) => Json(ApiResponse::ok(evidence)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// Human attestation for already-useful documentation. It writes explicit
/// human provenance and never creates an audit_run or claims Kronn inspected it.
pub async fn attest_project_documentation(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
    Json(request): Json<AttestDocumentationRequest>,
) -> Json<ApiResponse<AuditEvidenceResponse>> {
    if !request.confirmed {
        return Json(ApiResponse::err(
            "explicit confirmation is required before recording an attestation",
        ));
    }
    let project = match lookup_project(&state, &project_id).await {
        Ok(project) => project,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let project_path = project.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let root = crate::core::scanner::resolve_host_path(&project_path);
        if !crate::core::scanner::detect_docs_entry(&root).is_file() {
            return Err("documentation entry is missing — complete the template first".to_string());
        }
        crate::core::kronn_state::attest_documentation(&root)
    })
    .await
    .unwrap_or_else(|error| Err(format!("attestation task failed: {error}")));
    if let Err(error) = result {
        return Json(ApiResponse::err(error));
    }
    match build_evidence(&state, &project).await {
        Ok(evidence) => Json(ApiResponse::ok(evidence)),
        Err(error) => Json(ApiResponse::err(error)),
    }
}

/// `GET /api/projects/{id}/context-audit`
pub async fn project_context_audit(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<ContextAuditResponse>> {
    let path = match lookup_project(&state, &project_id).await {
        Ok(project) => project.path,
        Err(error) => return Json(ApiResponse::err(error)),
    };

    let root = crate::core::scanner::resolve_host_path(&path);
    if !root.is_dir() {
        // Said rather than audited as empty: a missing checkout would otherwise
        // produce a clean report about a repository nobody looked at.
        return Json(ApiResponse::err(format!(
            "{path} is not a directory — the project checkout is missing, so its \
             context cannot be audited"
        )));
    }

    // Blocking filesystem work off the async runtime: a large tree would otherwise
    // stall every other request on this thread.
    let audit = match tokio::task::spawn_blocking(move || audit_repo(&root)).await {
        Ok(audit) => audit,
        Err(error) => return Json(ApiResponse::err(format!("audit failed: {error}"))),
    };
    let rendered = render(&audit);
    let snapshot = audit.clone();
    let snapshot_project_id = project_id.clone();
    let previous = match state
        .db
        .with_conn(move |conn| {
            crate::db::context_audits::load_or_create_snapshot(
                conn,
                &snapshot_project_id,
                &snapshot,
            )
        })
        .await
    {
        Ok(previous) => previous,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "context audit snapshot failed: {error}"
            )))
        }
    };
    let audit_drift = previous.as_ref().map(|baseline| drift(baseline, &audit));

    Json(ApiResponse::ok(ContextAuditResponse {
        project_id,
        audit,
        rendered,
        drift: audit_drift,
    }))
}

/// Accept the current instruction surface as the new baseline. This is an
/// explicit mutation: refreshes and React development double-mounts must never
/// acknowledge real drift merely by reading it twice.
pub async fn accept_project_context_baseline(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<ContextAuditResponse>> {
    let path = match lookup_project(&state, &project_id).await {
        Ok(project) => project.path,
        Err(error) => return Json(ApiResponse::err(error)),
    };
    let root = crate::core::scanner::resolve_host_path(&path);
    if !root.is_dir() {
        return Json(ApiResponse::err(format!(
            "{path} is not a directory — baseline unchanged"
        )));
    }
    let audit = match tokio::task::spawn_blocking(move || audit_repo(&root)).await {
        Ok(audit) => audit,
        Err(error) => return Json(ApiResponse::err(format!("audit failed: {error}"))),
    };
    let rendered = render(&audit);
    let snapshot = audit.clone();
    let snapshot_project_id = project_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::context_audits::replace_snapshot(conn, &snapshot_project_id, &snapshot)
        })
        .await
    {
        return Json(ApiResponse::err(format!(
            "context baseline update failed: {error}"
        )));
    }
    Json(ApiResponse::ok(ContextAuditResponse {
        project_id,
        audit,
        rendered,
        drift: Some(Drift {
            grown: Vec::new(),
            new_files: Vec::new(),
            newly_broken_routes: Vec::new(),
            unused_files: Vec::new(),
            paid_agent_growth: Vec::new(),
        }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path as AxPath, State};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    fn state() -> AppState {
        let db = Arc::new(crate::db::Database::open_in_memory().unwrap());
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn seed_project(state: &AppState, root: &std::path::Path) {
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(
            root.join("docs/AGENTS.md"),
            "# Existing docs\nHuman rules.\n",
        )
        .unwrap();
        std::fs::write(root.join("AGENTS.md"), "# Agent rules\nStay concise.\n").unwrap();
        let path = root.to_string_lossy().to_string();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO projects (id, name, path, created_at, updated_at)
                     VALUES ('p1', 'P', ?1, datetime('now'), datetime('now'))",
                    [path],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn on_demand_audit_persists_a_baseline_then_reports_paid_growth() {
        let temp = tempfile::TempDir::new().unwrap();
        let state = state();
        seed_project(&state, temp.path()).await;

        let first = project_context_audit(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0;
        assert!(first.success);
        assert!(
            first.data.unwrap().drift.is_none(),
            "first call is baseline"
        );

        std::fs::write(
            temp.path().join("AGENTS.md"),
            "# Agent rules\nStay concise.\nThis added rule is paid on every task.\n",
        )
        .unwrap();
        let second = project_context_audit(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
            .data
            .unwrap();
        let drift = second.drift.expect("second call compares baseline");
        assert!(drift
            .grown
            .iter()
            .any(|(path, delta)| { path == "AGENTS.md" && *delta > 0 }));
        assert!(drift
            .paid_agent_growth
            .iter()
            .any(|growth| { growth.agent == "Codex" && growth.delta_bytes > 0 }));

        let accepted =
            accept_project_context_baseline(State(state.clone()), AxPath("p1".to_string()))
                .await
                .0
                .data
                .unwrap();
        assert!(accepted.drift.unwrap().grown.is_empty());
        let after = project_context_audit(State(state), AxPath("p1".to_string()))
            .await
            .0
            .data
            .unwrap();
        assert!(after.drift.unwrap().grown.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attestation_is_explicit_human_evidence_and_reliability_is_measured() {
        let temp = tempfile::TempDir::new().unwrap();
        let state = state();
        seed_project(&state, temp.path()).await;

        let before = project_audit_evidence(State(state.clone()), AxPath("p1".to_string()))
            .await
            .0
            .data
            .unwrap();
        assert!(matches!(before.kind, AuditEvidenceKind::MissingEvidence));

        let refused = attest_project_documentation(
            State(state.clone()),
            AxPath("p1".to_string()),
            Json(AttestDocumentationRequest { confirmed: false }),
        )
        .await
        .0;
        assert!(!refused.success);

        let attested = attest_project_documentation(
            State(state.clone()),
            AxPath("p1".to_string()),
            Json(AttestDocumentationRequest { confirmed: true }),
        )
        .await
        .0
        .data
        .unwrap();
        assert!(matches!(attested.kind, AuditEvidenceKind::HumanAttestation));
        assert_eq!(attested.status, AiAuditStatus::Audited);
        let persisted = crate::core::kronn_state::read(temp.path()).unwrap();
        assert_eq!(
            persisted.audits.last().unwrap().provenance,
            crate::core::kronn_state::AuditProvenance::HumanAttestation
        );

        state
            .db
            .with_conn(|conn| {
                let old = chrono::Utc::now() - chrono::Duration::minutes(5);
                crate::db::audit_runs::insert_running(conn, "done", "p1", "Full", "Codex", old)?;
                crate::db::audit_runs::complete(
                    conn,
                    "done",
                    old + chrono::Duration::minutes(1),
                    "Completed",
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    100,
                    None,
                    None,
                )?;
                crate::db::audit_runs::insert_running(
                    conn,
                    "cut",
                    "p1",
                    "Full",
                    "Codex",
                    chrono::Utc::now(),
                )?;
                crate::db::audit_runs::update_last_completed_step(conn, "cut", 4)?;
                crate::db::audit_runs::mark_interrupted(conn, "cut", "network")?;
                Ok(())
            })
            .await
            .unwrap();
        let measured = project_audit_evidence(State(state), AxPath("p1".to_string()))
            .await
            .0
            .data
            .unwrap();
        assert_eq!(measured.audit_runs, 2);
        assert_eq!(measured.interrupted_runs, 1);
        assert_eq!(measured.interruption_rate_percent, 50.0);
        assert_eq!(measured.resumable_after_step, Some(4));
    }
}
