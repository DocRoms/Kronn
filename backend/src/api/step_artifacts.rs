//! KT-1990 — Artifacts made from the executions of a step: the run view's
//! « Make an Artifact » per execution and « Open the renders in a mosaic » per
//! step, the run folders of the Artifact library, and the counts the delete
//! confirmations show.
//!
//! Kronn makes the Artifact from the stored output, the same way for the main
//! execution and every shadow, on a person's click: every route that writes is
//! human only (absent from the bridge-token routes, and refused here to a
//! caller marked as an agent). One Artifact per execution: a second click opens
//! the one already made.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::step_artifacts::{self, RunScope, StepArtifactFolder, StepArtifactLink};
use crate::models::{ApiErrorCode, ApiResponse, LivePage, LivePageRevision, Workflow, WorkflowRun};
use crate::workflows::shadow::{ShadowExecution, StepShadowComparison};
use crate::workflows::shadow_artifact::{self, ExecutionHtml, Found};
use crate::AppState;

/// The label the main execution carries, as in the step's shadow summary.
pub const MAIN_LABEL: &str = "main";
const MAX_TITLE_CHARS: usize = 200;
const SLUG_BASE_CHARS: usize = 60;

/// A refusal the person can act on: no HTML, a cut document, fewer than two
/// renders, an unknown run or step.
#[derive(Debug)]
pub struct Refusal {
    pub code: ApiErrorCode,
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refusal {}

fn refused(code: ApiErrorCode, message: impl Into<String>) -> anyhow::Error {
    Refusal {
        code,
        message: message.into(),
    }
    .into()
}

fn answer<T: Serialize>(result: anyhow::Result<T>, what: &str) -> Json<ApiResponse<T>> {
    match result {
        Ok(value) => Json(ApiResponse::ok(value)),
        Err(error) => match error.downcast::<Refusal>() {
            Ok(refusal) => Json(ApiResponse::err_coded(refusal.code, refusal.message)),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("Unable to {what}: {error}"),
            )),
        },
    }
}

const HUMAN_ONLY: &str = "Only a person makes or deletes the Artifacts of a run's \
     executions: ask the user to do it in Kronn (the run view or the Artifact library)";

/// The execution of a step a request names.
#[derive(Debug, Deserialize)]
pub struct MakeStepArtifactRequest {
    pub result_index: u32,
    /// Absent or empty for the main execution.
    #[serde(default)]
    pub shadow_id: Option<String>,
}

/// The Artifact of one execution.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct StepArtifactMade {
    pub page_id: String,
    /// `false` when the execution already had its Artifact: it was opened, not
    /// made again.
    pub created: bool,
}

/// The step a mosaic request names.
#[derive(Debug, Deserialize)]
pub struct StepArtifactMosaicRequest {
    pub result_index: u32,
}

/// The Artifacts of a step's executions, in mosaic order: the main execution
/// first, then the shadows in start order.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct StepArtifactMosaic {
    pub page_ids: Vec<String>,
    /// How many of them this request made.
    pub created: u32,
}

/// How many Artifacts a delete would take with the runs.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct StepArtifactCount {
    pub count: u32,
}

#[derive(Debug, Default, Deserialize)]
pub struct StepArtifactCountQuery {
    /// One run; absent for every run of the workflow.
    pub run_id: Option<String>,
}

/// What the label of an execution is: `main`, or the shadow's label.
pub fn execution_label(execution: &ShadowExecution) -> String {
    match &execution.shadow_id {
        None => MAIN_LABEL.to_string(),
        Some(id) if execution.label.trim().is_empty() => id.clone(),
        Some(_) => execution.label.trim().to_string(),
    }
}

/// `<workflow> · <step> · <execution label>`, at most 200 characters.
pub fn artifact_title(workflow: &str, step: &str, label: &str) -> String {
    let title = format!("{} · {} · {}", workflow.trim(), step.trim(), label);
    title
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn free_slug(conn: &Connection, title: &str) -> anyhow::Result<String> {
    let base: String = crate::api::live_pages::slugify(title)
        .chars()
        .take(SLUG_BASE_CHARS)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string();
    let mut slug = base.clone();
    while !crate::api::live_pages::valid_slug(&slug)
        || crate::db::live_pages::slug_is_taken(conn, &slug)?
    {
        let suffix = Uuid::new_v4().simple().to_string();
        slug = format!("{base}-{}", &suffix[..8]);
    }
    Ok(slug)
}

/// The run and its step, as the run view shows them.
fn load(
    conn: &Connection,
    workflow_id: &str,
    run_id: &str,
    result_index: u32,
) -> anyhow::Result<(Workflow, WorkflowRun, StepShadowComparison)> {
    let workflow = crate::db::workflows::get_workflow(conn, workflow_id)?
        .ok_or_else(|| refused(ApiErrorCode::NotFound, "Workflow not found"))?;
    let run = crate::db::workflows::get_run(conn, run_id)?
        .filter(|run| run.workflow_id == workflow.id)
        .ok_or_else(|| refused(ApiErrorCode::NotFound, "Run not found"))?;
    let step = crate::workflows::shadow::comparisons(conn, &run)?
        .into_iter()
        .find(|step| step.result_index == result_index)
        .ok_or_else(|| {
            refused(
                ApiErrorCode::NotFound,
                "This step of the run has no compared executions",
            )
        })?;
    Ok((workflow, run, step))
}

/// The Artifact of `execution`: the one already made, or a new one from the
/// last complete HTML block of its output. The caller commits.
pub fn ensure(
    tx: &rusqlite::Transaction<'_>,
    workflow: &Workflow,
    run: &WorkflowRun,
    step: &StepShadowComparison,
    execution: &ShadowExecution,
) -> anyhow::Result<StepArtifactMade> {
    let shadow_id = execution.shadow_id.clone().unwrap_or_default();
    if let Some(page_id) =
        step_artifacts::for_execution(tx, &run.id, step.result_index, &shadow_id)?
    {
        return Ok(StepArtifactMade {
            page_id,
            created: false,
        });
    }
    let html = match shadow_artifact::find(&execution.output) {
        // The pass an agent's message gets when a discussion stores it
        // (`db/discussions.rs`), not the audit pass: that one masks game code
        // naming a secret word (`ball.spin = 0.4`) and drops every URL query,
        // breaking a page the model wrote correctly.
        Found::Complete(document) => crate::core::redact::redact_agent_output(document),
        Found::None => return Err(no_artifact(ExecutionHtml::None)),
        Found::Cut(why) => return Err(no_artifact(why)),
    };
    crate::api::live_pages::validate_page_html(&html)
        .map_err(|message| refused(ApiErrorCode::Validation, message))?;
    let label = execution_label(execution);
    let title = artifact_title(&workflow.name, &step.step_name, &label);
    let now = Utc::now();
    let page_id = Uuid::new_v4().to_string();
    let revision_id = Uuid::new_v4().to_string();
    let page = LivePage {
        id: page_id.clone(),
        project_id: run
            .project_id
            .clone()
            .or_else(|| workflow.project_id.clone()),
        slug: free_slug(tx, &title)?,
        title,
        current_revision_id: revision_id.clone(),
        data_revision: 0,
        created_at: now,
        updated_at: now,
        last_published_at: None,
        pinned: false,
        archived: false,
        html_size_bytes: html.len() as u64,
    };
    let revision = LivePageRevision {
        id: revision_id,
        page_id: page_id.clone(),
        revision: 1,
        html,
        created_by_agent: None,
        created_at: now,
    };
    crate::db::live_pages::create_live_page_in_transaction(tx, &page, &revision, &[], None)?;
    step_artifacts::link(
        tx,
        &StepArtifactLink {
            page_id: page_id.clone(),
            run_id: run.id.clone(),
            result_index: step.result_index,
            step_name: step.step_name.clone(),
            shadow_id,
            label,
            model: execution.model.clone(),
            created_at: now.to_rfc3339(),
        },
    )?;
    Ok(StepArtifactMade {
        page_id,
        created: true,
    })
}

fn no_artifact(why: ExecutionHtml) -> anyhow::Error {
    refused(
        ApiErrorCode::Validation,
        why.refusal().unwrap_or("No Artifact was made"),
    )
}

/// The Artifact of one execution, made once.
pub fn make(
    conn: &Connection,
    workflow_id: &str,
    run_id: &str,
    request: &MakeStepArtifactRequest,
) -> anyhow::Result<StepArtifactMade> {
    let (workflow, run, step) = load(conn, workflow_id, run_id, request.result_index)?;
    let wanted = request.shadow_id.as_deref().filter(|id| !id.is_empty());
    let execution = match wanted {
        None => &step.main,
        Some(id) => step
            .shadows
            .iter()
            .find(|shadow| shadow.shadow_id.as_deref() == Some(id))
            .ok_or_else(|| refused(ApiErrorCode::NotFound, "This shadow did not run here"))?,
    };
    let tx = conn.unchecked_transaction()?;
    let made = ensure(&tx, &workflow, &run, &step, execution)?;
    tx.commit()?;
    Ok(made)
}

/// The Artifacts of every execution of a step that has a render, made where
/// missing, in mosaic order.
pub fn mosaic(
    conn: &Connection,
    workflow_id: &str,
    run_id: &str,
    result_index: u32,
) -> anyhow::Result<StepArtifactMosaic> {
    let (workflow, run, step) = load(conn, workflow_id, run_id, result_index)?;
    let rendered: Vec<&ShadowExecution> = std::iter::once(&step.main)
        .chain(step.shadows.iter())
        .filter(|execution| {
            execution.artifact_id.is_some() || execution.html == ExecutionHtml::Complete
        })
        .collect();
    if rendered.len() < 2 {
        return Err(refused(
            ApiErrorCode::Validation,
            "A mosaic needs at least two executions of this step with a complete HTML document",
        ));
    }
    let tx = conn.unchecked_transaction()?;
    let mut out = StepArtifactMosaic {
        page_ids: Vec::with_capacity(rendered.len()),
        created: 0,
    };
    for execution in rendered {
        let made = ensure(&tx, &workflow, &run, &step, execution)?;
        out.created += u32::from(made.created);
        out.page_ids.push(made.page_id);
    }
    tx.commit()?;
    Ok(out)
}

/// POST /api/workflows/{id}/runs/{run_id}/step-artifacts — « Make an Artifact »
/// from one execution, or open the one it already has. Human only.
pub async fn make_route(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Path((id, run_id)): Path<(String, String)>,
    Json(request): Json<MakeStepArtifactRequest>,
) -> Json<ApiResponse<StepArtifactMade>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, HUMAN_ONLY));
    }
    let result = state
        .db
        .with_conn(move |conn| make(conn, &id, &run_id, &request))
        .await;
    answer(result, "make the Artifact")
}

/// POST /api/workflows/{id}/runs/{run_id}/step-artifacts/mosaic — the
/// Artifacts of a step's renders, made where missing, in mosaic order. Human
/// only.
pub async fn mosaic_route(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Path((id, run_id)): Path<(String, String)>,
    Json(request): Json<StepArtifactMosaicRequest>,
) -> Json<ApiResponse<StepArtifactMosaic>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, HUMAN_ONLY));
    }
    let result = state
        .db
        .with_conn(move |conn| mosaic(conn, &id, &run_id, request.result_index))
        .await;
    answer(result, "open the renders in a mosaic")
}

/// GET /api/step-artifacts — the run folders of the Artifact library.
pub async fn folders(State(state): State<AppState>) -> Json<ApiResponse<Vec<StepArtifactFolder>>> {
    let result = state.db.with_read_conn(step_artifacts::list_folders).await;
    answer(result, "list the run folders")
}

/// DELETE /api/workflow-runs/{run_id}/step-artifacts — « Delete the folder »:
/// every Artifact made from the run's executions. The run stays. Human only.
pub async fn delete_folder(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    marker: crate::core::bridge_token::AgentMark,
    Path(run_id): Path<String>,
) -> Json<ApiResponse<StepArtifactCount>> {
    if crate::core::bridge_token::agent_authored(&bridge, &marker) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, HUMAN_ONLY));
    }
    let result = state
        .db
        .with_conn(move |conn| {
            let deleted = step_artifacts::delete(conn, RunScope::Run(&run_id))?;
            Ok(StepArtifactCount {
                count: u32::try_from(deleted).unwrap_or(u32::MAX),
            })
        })
        .await;
    answer(result, "delete the folder")
}

/// GET /api/workflows/{id}/step-artifacts/count?run_id= — how many Artifacts
/// deleting one run (`run_id`), or every run of the workflow, would delete. A
/// run of another workflow is not found, as « Make an Artifact » answers.
pub async fn count_route(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StepArtifactCountQuery>,
) -> Json<ApiResponse<StepArtifactCount>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            let count = match query.run_id.as_deref().filter(|run| !run.is_empty()) {
                Some(run_id) => {
                    crate::db::workflows::get_run(conn, run_id)?
                        .filter(|run| run.workflow_id == id)
                        .ok_or_else(|| refused(ApiErrorCode::NotFound, "Run not found"))?;
                    step_artifacts::count(conn, RunScope::Run(run_id))?
                }
                None => step_artifacts::count(conn, RunScope::Workflow(&id))?,
            };
            Ok(StepArtifactCount { count })
        })
        .await;
    answer(result, "count the Artifacts")
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn the_title_names_workflow_step_and_execution() {
        assert_eq!(
            artifact_title("Jeux ", " jeu", "Qwen local"),
            "Jeux · jeu · Qwen local"
        );
        let long = artifact_title(&"é".repeat(300), "s", "main");
        assert_eq!(long.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn a_slug_is_valid_and_free() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let slug = free_slug(&conn, &artifact_title(&"Jeu ".repeat(40), "s", "main")).unwrap();
        assert!(crate::api::live_pages::valid_slug(&slug), "{slug}");
        assert!(slug.len() <= SLUG_BASE_CHARS);
        let accents = free_slug(&conn, "··· é ···").unwrap();
        assert!(crate::api::live_pages::valid_slug(&accents), "{accents}");
    }
}
