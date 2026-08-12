//! Ingest a joined CLI's own token counters — KT-190.
//!
//! The collector runs where the CLI runs: only that machine has the vendor's
//! transcript, and only that process knows which session it is. So the bridge
//! measures and reports; the backend stores and never estimates.
//!
//! Absence travels intact. A counter the vendor does not publish arrives as
//! `null` and is stored as NULL, because a 0 would let a reader conclude
//! something about a field nobody measured.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::cli_telemetry::{CliSessionTelemetry, TelemetryCoverage};
use crate::models::ApiResponse;
use crate::AppState;

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ReportTelemetryRequest {
    /// The CLI's own durable session id, as sent at join. Resolved to the Kronn
    /// session row; an unknown one is refused rather than stored orphaned.
    pub session_id: String,
    pub vendor: String,
    /// Where the numbers came from, e.g. `claude-code-transcript`. Required:
    /// a figure whose origin is unstated cannot be audited later.
    pub provenance: String,
    /// `None` means the vendor does not publish this counter. It is NOT zero,
    /// and the distinction survives all the way to the column.
    #[serde(default)]
    pub input_tokens: Option<i64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<i64>,
    #[serde(default)]
    pub cache_read_tokens: Option<i64>,
    #[serde(default)]
    pub output_tokens: Option<i64>,
    #[serde(default)]
    pub measured_responses: Option<i64>,
    #[serde(default)]
    pub models_json: Option<String>,
    #[serde(default)]
    pub window_start: Option<String>,
    #[serde(default)]
    pub window_end: Option<String>,
    /// The vendor's own cost figure when it publishes one. Kept separate from
    /// any Kronn estimate.
    #[serde(default)]
    pub vendor_cost_usd: Option<f64>,
    /// Byte cursor the collector reached. Stored monotonically, so a replayed
    /// report cannot rewind it and cause a span to be counted twice.
    #[serde(default)]
    pub read_offset: i64,
    /// Timestamped responses covered by THIS report. Used to stamp each of the
    /// session's messages with the running session total at that instant — not
    /// with a per-message cost, which a CLI's spend cannot be cut into.
    #[serde(default)]
    pub timeline: Vec<crate::db::cli_telemetry::ResponseUsage>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReportTelemetryResponse {
    pub cli_session_pk: i64,
    /// Where the NEXT collection should resume. Echoed back because the stored
    /// value can be ahead of what this caller sent (another report landed, or
    /// this one was stale).
    pub read_offset: i64,
    /// Counters this report left unmeasured, echoed so a caller can see that
    /// its absences were understood as absences.
    pub unmeasured: Vec<String>,
    /// How many of this session's messages were stamped with a running total.
    pub messages_stamped: usize,
}

/// `POST /api/discussions/{id}/telemetry`
pub async fn report_telemetry(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
    Json(request): Json<ReportTelemetryRequest>,
) -> Json<ApiResponse<ReportTelemetryResponse>> {
    if request.provenance.trim().is_empty() {
        return Json(ApiResponse::err(
            "provenance is required: a counter with no stated origin cannot be audited".to_string(),
        ));
    }
    if let Some(negative) = [
        request.input_tokens,
        request.cache_creation_tokens,
        request.cache_read_tokens,
        request.output_tokens,
    ]
    .into_iter()
    .flatten()
    .find(|value| *value < 0)
    {
        // A negative counter is a bug upstream, and storing it would poison
        // every aggregate silently.
        return Json(ApiResponse::err(format!(
            "counter cannot be negative (got {negative})"
        )));
    }

    let unmeasured: Vec<String> = [
        ("input", request.input_tokens),
        ("cache_creation", request.cache_creation_tokens),
        ("cache_read", request.cache_read_tokens),
        ("output", request.output_tokens),
    ]
    .into_iter()
    .filter(|(_, value)| value.is_none())
    .map(|(name, _)| name.to_string())
    .collect();

    let result = state
        .db
        .with_conn(move |conn| {
            // Agent-agnostic lookup on purpose: a bridge whose provider is
            // still "Unknown" must be able to report its own cost. The disc is
            // checked so a stale binding cannot attach numbers to another room.
            let found = crate::db::discussion_sessions::find_active_session_by_id(
                conn,
                &request.session_id,
            )?;
            let Some((session_pk, _, _)) =
                found.filter(|(_, found_disc, _)| *found_disc == disc_id)
            else {
                return Ok(None);
            };
            let row = CliSessionTelemetry {
                cli_session_pk: session_pk,
                vendor: request.vendor.clone(),
                provenance: request.provenance.clone(),
                input_tokens: request.input_tokens,
                cache_creation_tokens: request.cache_creation_tokens,
                cache_read_tokens: request.cache_read_tokens,
                output_tokens: request.output_tokens,
                measured_responses: request.measured_responses,
                models_json: request.models_json.clone(),
                window_start: request.window_start.clone(),
                window_end: request.window_end.clone(),
                vendor_cost_usd: request.vendor_cost_usd,
                read_offset: request.read_offset.max(0),
                updated_at: chrono::Utc::now().to_rfc3339(),
            };
            // The baseline is what earlier reports already accounted for, so a
            // report covering only the newest slice still yields true absolute
            // figures. Read BEFORE the upsert overwrites it.
            let baseline = crate::db::cli_telemetry::get(conn, session_pk)?
                .and_then(|previous| previous.traffic_tokens())
                .unwrap_or(0);
            crate::db::cli_telemetry::upsert(conn, &row)?;
            let stamped = crate::db::cli_telemetry::attribute_to_messages(
                conn,
                session_pk,
                &request.timeline,
                baseline,
            )?;
            let stored = crate::db::cli_telemetry::read_offset(conn, session_pk)?;
            Ok(Some((session_pk, stored, stamped)))
        })
        .await;

    match result {
        Ok(Some((cli_session_pk, read_offset, messages_stamped))) => {
            Json(ApiResponse::ok(ReportTelemetryResponse {
                cli_session_pk,
                read_offset,
                unmeasured,
                messages_stamped,
            }))
        }
        // Refused rather than stored against a guess: telemetry attached to the
        // wrong session is worse than telemetry that is missing.
        Ok(None) => Json(ApiResponse::err(
            "unknown session for this discussion — join first".to_string(),
        )),
        Err(error) => Json(ApiResponse::err(format!(
            "telemetry report failed: {error}"
        ))),
    }
}

/// `GET /api/telemetry/coverage`
///
/// What share of CLI sessions Kronn can actually account for. Reported per agent
/// type because coverage is a per-vendor fact: a collector exists or it does not.
pub async fn telemetry_coverage(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<TelemetryCoverage>>> {
    match state.db.with_conn(crate::db::cli_telemetry::coverage).await {
        Ok(rows) => Json(ApiResponse::ok(rows)),
        Err(error) => Json(ApiResponse::err(format!("coverage query failed: {error}"))),
    }
}

/// `GET /api/discussions/{id}/token-cost`
///
/// The two figures a discussion header shows — KT-254. Two, never one: see
/// `DiscussionTokenCost` for why adding a per-reply cost to a whole-session
/// running total produces a number with no meaning.
pub async fn discussion_token_cost(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<crate::db::cli_telemetry::DiscussionTokenCost>> {
    match state
        .db
        .with_conn(move |conn| crate::db::cli_telemetry::cost_for_discussion(conn, &disc_id))
        .await
    {
        Ok(cost) => Json(ApiResponse::ok(cost)),
        Err(error) => Json(ApiResponse::err(format!(
            "token cost query failed: {error}"
        ))),
    }
}

#[cfg(test)]
#[path = "cli_telemetry_test.rs"]
mod cli_telemetry_test;

/// `GET /api/discussions/{id}/resume-bundle`
///
/// KT-193 DoD 3 — what a FRESH session needs to carry on, without the transcript.
///
/// Assembled from the plan, bounded, and deliberately free of message history: a
/// session that rotates should re-read the record that was written on purpose,
/// not the conversation that produced it. When the bundle is not enough, the
/// caller retrieves the specific messages it needs (`disc_load_other` with an
/// explicit range) — a retrieval, not a re-read.
pub async fn resume_bundle(
    State(state): State<AppState>,
    Path(disc_id): Path<String>,
) -> Json<ApiResponse<crate::core::resume_bundle::ResumeBundle>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            let plan = crate::db::planning::get_discussion_plan(conn, &disc_id)?;

            let objective = plan.primary_objective.as_ref();
            // The objective's own description and open checklist, fetched only
            // for THAT task: pulling every task's full body would rebuild the
            // bulk this bundle exists to avoid.
            let (objective_description, open_dod) = match objective {
                None => (None, Vec::new()),
                Some(summary) => match crate::db::planning::get_task(conn, &summary.id)? {
                    None => (None, Vec::<String>::new()),
                    Some(task) => {
                        let open: Vec<String> = task
                            .definition_of_done
                            .iter()
                            .filter(|item| !item.completed)
                            .map(|item| item.sentence.clone())
                            .collect();
                        // Empty means "no description written", which is not the
                        // same as a description that is blank — the bundle
                        // renders nothing rather than an empty heading.
                        let description =
                            (!task.description.trim().is_empty()).then(|| task.description.clone());
                        (description, open)
                    }
                },
            };

            let active: Vec<crate::core::resume_bundle::BundleTask> = plan
                .active
                .iter()
                .filter(|relation| {
                    !matches!(
                        relation.task.status,
                        crate::models::PlanningTaskStatus::Done
                            | crate::models::PlanningTaskStatus::Archived
                    )
                })
                .map(|relation| crate::core::resume_bundle::BundleTask {
                    reference: relation.task.reference.clone(),
                    title: relation.task.title.clone(),
                    status: format!("{:?}", relation.task.status).to_lowercase(),
                    dod_progress: (relation.task.total_subtasks > 0).then(|| {
                        format!(
                            "{}/{}",
                            relation.task.completed_subtasks, relation.task.total_subtasks
                        )
                    }),
                })
                .collect();

            // One line per blocked task, naming what blocks it — the fact a
            // fresh session most needs, since re-attempting a blocked task
            // wastes far more than this costs.
            let blockers: Vec<String> = plan
                .active
                .iter()
                .filter(|relation| !relation.active_blockers.is_empty())
                .map(|relation| {
                    let names: Vec<&str> = relation
                        .active_blockers
                        .iter()
                        .map(|blocker| blocker.reference.as_str())
                        .collect();
                    format!(
                        "{} blocked by {}",
                        relation.task.reference,
                        names.join(", ")
                    )
                })
                .collect();

            Ok(crate::core::resume_bundle::build(
                objective.map(|summary| summary.title.as_str()),
                objective_description.as_deref(),
                &open_dod,
                &active,
                &blockers,
            ))
        })
        .await;

    match result {
        Ok(bundle) => Json(ApiResponse::ok(bundle)),
        Err(error) => Json(ApiResponse::err(format!("resume bundle failed: {error}"))),
    }
}
