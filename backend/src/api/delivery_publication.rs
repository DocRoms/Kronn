//! Turning an accepted delivery into the one report published in the discussion
//! (KT-544).
//!
//! The worker never authors this text. It submitted a `DeliveryManifestV1` for
//! review; once the orchestrator accepts, Kronn derives the semantic fields
//! from that manifest, stamps the identity facts only the control plane knows,
//! validates the payload and renders the Markdown in a fixed order. A model can
//! therefore influence *what* is reported, never the layout, the identity, nor
//! whether a claim counts as evidenced.

use anyhow::Context;
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;

use crate::delivery::{
    DeliveryChange, DeliveryCommit, DeliveryDocumentation, DeliveryMetrics,
    DeliveryPrincipalVerification, DeliverySummaryError, DeliverySummaryInput, DeliverySummaryV1,
    DeliveryValidation,
};
use crate::models::{
    DeliveryManifestV1, FileChangeKind, MessageTargetKind, ReviewDecisionV1, ReviewDodVerification,
    TaskExecution, TestStatus,
};

/// Facts about the execution that the manifest cannot be trusted for: who
/// actually ran, on what runtime, which tier and model, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionFacts {
    pub agent: String,
    pub runtime: String,
    pub tier: String,
    pub model: String,
    pub branch: String,
    pub duration_ms: u64,
    pub tokens: Option<u64>,
    pub cost_usd_micros: Option<u64>,
}

impl ExecutionFacts {
    /// Read identity from the persisted execution rather than from the payload.
    ///
    /// A missing field becomes `unknown` instead of an empty string: the
    /// contract refuses empty identity, and failing the whole publication
    /// because a runtime never reported its tier would hide an accepted
    /// delivery behind an unrelated defect.
    pub fn from_execution(execution: &TaskExecution, duration_ms: u64) -> Self {
        const UNKNOWN: &str = "unknown";
        Self {
            agent: non_empty(execution.worker_agent_type.as_deref(), UNKNOWN),
            runtime: execution
                .worker_target_kind
                .map(runtime_label)
                .unwrap_or_else(|| UNKNOWN.to_owned()),
            tier: non_empty(execution.worker_model_tier.as_deref(), UNKNOWN),
            model: non_empty(execution.worker_model.as_deref(), UNKNOWN),
            branch: non_empty(execution.child_branch.as_deref(), UNKNOWN),
            duration_ms,
            tokens: None,
            cost_usd_micros: None,
        }
    }
}

/// How the worker was actually reached. Named for a human reading a report,
/// not after the internal enum: `Cli` alone would not tell a reader whether the
/// work went through a joined CLI or a server-side agent.
fn runtime_label(kind: MessageTargetKind) -> String {
    match kind {
        MessageTargetKind::DiscussionAgent => "discussion-agent",
        MessageTargetKind::Agent => "server-agent",
        MessageTargetKind::Cli => "joined-cli",
    }
    .to_owned()
}

fn non_empty(value: Option<&str>, fallback: &str) -> String {
    match value.map(str::trim) {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => fallback.to_owned(),
    }
}

fn change_nature(kind: FileChangeKind) -> &'static str {
    match kind {
        FileChangeKind::Added => "added",
        FileChangeKind::Modified => "modified",
        FileChangeKind::Deleted => "removed",
    }
}

fn test_result(status: TestStatus) -> &'static str {
    match status {
        TestStatus::Pass => "pass",
        TestStatus::Fail => "fail",
        TestStatus::Skipped => "skipped",
    }
}

/// Marker used when a manifest reports a verdict without anything to back it.
///
/// Explicit on purpose: a reader must see that the claim is unbacked, and a
/// grep must be able to find every report that carries one.
pub const EVIDENCE_NOT_PROVIDED: &str = "evidence not provided by the worker";

/// Build the accepted summary from the reviewed manifest and the execution.
///
/// Total by design once a delivery is accepted: the review gate is where an
/// unevidenced claim gets refused, so failing here would leave an accepted
/// delivery with no report at all. A missing proof therefore degrades the
/// report — the validation is published with an explicit "not provided" marker
/// and an attention point naming it — rather than suppressing it.
pub fn summary_from_manifest(
    task_reference: &str,
    execution_id: &str,
    manifest: &DeliveryManifestV1,
    facts: &ExecutionFacts,
    // KT-613 — what the principal verified itself during the review that
    // accepted this delivery. Empty when the review added none; the report then
    // says nothing about principal evidence rather than implying its absence
    // means the work is unverified.
    principal_verifications: &[ReviewDodVerification],
    now: DateTime<Utc>,
) -> Result<DeliverySummaryV1, DeliverySummaryError> {
    let changes = manifest
        .files_touched
        .iter()
        .map(|file| DeliveryChange {
            path: file.path.clone(),
            nature: change_nature(file.kind).to_owned(),
        })
        .collect();

    // A manifest carries the head SHA it was reviewed against. An empty one is
    // reported as an explicit, justified absence rather than a blank commit.
    let commit = if manifest.head_sha.trim().is_empty() {
        DeliveryCommit::None {
            justification: "the delivery reported no commit for this attempt".to_owned(),
        }
    } else {
        DeliveryCommit::Made {
            sha: manifest.head_sha.trim().to_owned(),
            branch: facts.branch.clone(),
        }
    };

    let mut unevidenced: Vec<String> = Vec::new();
    let validations = manifest
        .tests
        .iter()
        .map(|test| {
            let evidence = match test.evidence.as_deref().map(str::trim) {
                Some(text) if !text.is_empty() => text.to_owned(),
                _ => {
                    unevidenced.push(test.name.clone());
                    EVIDENCE_NOT_PROVIDED.to_owned()
                }
            };
            DeliveryValidation {
                command: test.name.clone(),
                result: test_result(test.status).to_owned(),
                // The manifest reports verdicts, not timings. Left absent so
                // the report says "not measured" instead of showing a zero
                // that reads like a measurement.
                duration_ms: None,
                evidence,
            }
        })
        .collect::<Vec<_>>();

    let documentation = if manifest.docs.is_empty() {
        DeliveryDocumentation::None
    } else {
        DeliveryDocumentation::Updated {
            files: manifest.docs.clone(),
        }
    };

    // Risks and limitations are both attention points, but losing which is
    // which would make the report less useful than the manifest it came from.
    let attention_points =
        manifest
            .risks
            .iter()
            .map(|risk| format!("risk: {risk}"))
            .chain(
                manifest
                    .limitations
                    .iter()
                    .map(|limitation| format!("limitation: {limitation}")),
            )
            // An unbacked validation is an attention point in its own right: the
            // degraded report must be readable as degraded, not merely quieter.
            .chain(unevidenced.iter().map(|name| {
                format!("unverified: validation `{name}` was reported without evidence")
            }))
            .collect();

    DeliverySummaryV1::build(
        task_reference,
        execution_id,
        now.to_rfc3339(),
        DeliverySummaryInput {
            agent: facts.agent.clone(),
            runtime: facts.runtime.clone(),
            tier: facts.tier.clone(),
            model: facts.model.clone(),
            summary: manifest.summary.clone(),
            changes,
            commit,
            validations,
            documentation,
            principal_verifications: principal_verifications
                .iter()
                .map(|verification| DeliveryPrincipalVerification {
                    dod_id: verification.dod_id.clone(),
                    met: verification.met,
                    evidence: verification.evidence.clone(),
                })
                .collect(),
            attention_points,
            metrics: DeliveryMetrics {
                duration_ms: facts.duration_ms,
                tokens: facts.tokens,
                cost_usd_micros: facts.cost_usd_micros,
            },
        },
    )
}

/// Publish the accepted delivery's one report into the parent discussion.
///
/// Called on the approve path, and again on a replayed approve: the record is
/// the idempotency guard, so a retry republishes nothing and a crash between
/// the record and its message is repaired rather than left reportless.
///
/// Returns `None` when the execution carries no persisted manifest — an
/// approval that never went through delivery has nothing to report, and that
/// is not an error here (the approve guard is where a missing manifest is
/// refused).
pub async fn publish_accepted_delivery(
    db: &crate::db::Database,
    execution: &TaskExecution,
) -> anyhow::Result<Option<crate::db::delivery_summaries::Published>> {
    let execution_id = execution.id.clone();
    let attempt_no = execution.attempt_no;
    let task_id = execution.task_id.clone();

    let (delivery, task, assignment_started_at, review) = db
        .with_conn(move |conn| {
            let delivery =
                crate::db::worker_deliveries::get_delivery(conn, &execution_id, attempt_no)?;
            let task = crate::db::planning::get_task(conn, &task_id)?;
            let assignment_started_at: Option<String> = conn
                .query_row(
                    "SELECT created_at FROM task_execution_assignment_events \
                     WHERE task_execution_id = ?1 ORDER BY generation DESC LIMIT 1",
                    [execution_id.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            let review = crate::db::worker_reviews::get_review(conn, &execution_id, attempt_no)?
                .map(|row| row.decision_json);
            Ok((delivery, task, assignment_started_at, review))
        })
        .await?;

    let Some(delivery) = delivery else {
        return Ok(None);
    };
    let task = task.context("the execution's task vanished before publication")?;
    let manifest: DeliveryManifestV1 = serde_json::from_str(&delivery.manifest_json)
        .context("the stored delivery manifest is not a valid v1 payload")?;

    let delivered_at = parse_rfc3339(&delivery.created_at).unwrap_or_else(Utc::now);
    // Measured from the current assignment: the worker's own attempt, not the
    // whole execution with its earlier review rounds folded in.
    let started_at = assignment_started_at
        .as_deref()
        .and_then(parse_rfc3339)
        .unwrap_or(execution.created_at);
    let duration_ms = (delivered_at - started_at).num_milliseconds().max(0) as u64;

    let facts = ExecutionFacts::from_execution(execution, duration_ms);
    // KT-613 — the review that accepted THIS attempt, and only it. A decision
    // that failed to parse leaves the report without principal evidence rather
    // than without a report: publication is total once a delivery is accepted.
    let principal_verifications = review
        .as_deref()
        .and_then(|json| serde_json::from_str::<ReviewDecisionV1>(json).ok())
        .map(|decision| decision.dod_verifications)
        .unwrap_or_default();
    let summary = summary_from_manifest(
        &task.summary.reference,
        &execution.id,
        &manifest,
        &facts,
        &principal_verifications,
        delivered_at,
    )?;

    let stored = crate::db::delivery_summaries::StoredDeliverySummary {
        execution_id: execution.id.clone(),
        attempt_no,
        canonical_json: summary.canonical_json(),
        message_id: crate::db::delivery_summaries::message_id_for(&execution.id, attempt_no),
        discussion_id: execution.parent_discussion_id.clone(),
        correlation_id: format!("orch-delivery:{}:{}", execution.id, attempt_no),
    };
    let now = Utc::now();
    let outcome = db
        .with_conn(move |conn| {
            crate::db::delivery_summaries::publish(conn, &stored, now, |stored| {
                crate::api::orchestration::orchestrator_message(
                    stored.message_id.clone(),
                    render_stored(stored),
                )
            })
        })
        .await?;
    Ok(Some(outcome))
}

/// Render the STORED payload, never the caller's.
///
/// A payload that no longer parses is reported as such instead of silently
/// publishing nothing: an accepted delivery must leave a trace a human can
/// follow back to the record.
fn render_stored(stored: &crate::db::delivery_summaries::StoredDeliverySummary) -> String {
    match serde_json::from_str::<DeliverySummaryV1>(&stored.canonical_json) {
        Ok(summary) => summary.render_markdown(),
        Err(error) => format!(
            "## ✅ Delivery accepted — execution `{}` (attempt {})\n\n             The stored report could not be rendered: {error}. The canonical              payload is intact in `delivery_summaries` under correlation              `{}` — it is the record, this message is only its projection.\n",
            stored.execution_id, stored.attempt_no, stored.correlation_id
        ),
    }
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::Database,
        models::{
            ManifestDodStatus, ManifestFile, ManifestTest, ReviewVerdict, TaskExecution,
            TaskExecutionStatus,
        },
    };

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn facts() -> ExecutionFacts {
        ExecutionFacts {
            agent: "OpenCode".into(),
            runtime: "cli".into(),
            tier: "reasoning".into(),
            model: "claude-opus-5".into(),
            branch: "kronn/task/kt-544".into(),
            duration_ms: 91_000,
            tokens: Some(42_000),
            cost_usd_micros: None,
        }
    }

    fn manifest() -> DeliveryManifestV1 {
        DeliveryManifestV1 {
            version: "delivery_manifest/v1".into(),
            task_ref: "KT-544".into(),
            head_sha: "abc1234".into(),
            files_touched: vec![
                ManifestFile {
                    path: "backend/src/delivery.rs".into(),
                    kind: FileChangeKind::Modified,
                },
                ManifestFile {
                    path: "backend/src/old.rs".into(),
                    kind: FileChangeKind::Deleted,
                },
            ],
            tests: vec![ManifestTest {
                name: "cargo test --lib delivery".into(),
                status: TestStatus::Pass,
                evidence: Some("12 passed; 0 failed".into()),
            }],
            dod_status: vec![ManifestDodStatus {
                dod_id: "dod-1".into(),
                met: true,
                evidence: Some("see test".into()),
            }],
            docs: vec!["docs/architecture/delegation.md".into()],
            migrations: vec!["163_delivery_summaries".into()],
            risks: vec!["touches a hot file".into()],
            limitations: vec!["no UI yet".into()],
            summary: "Wired the delivery contract to the approval path.".into(),
        }
    }

    fn verification(dod_id: &str, met: bool, evidence: &str) -> ReviewDodVerification {
        ReviewDodVerification {
            dod_id: dod_id.to_owned(),
            met,
            evidence: evidence.to_owned(),
        }
    }

    /// KT-613 — the defect, reproduced on KT-611 and then on KT-612: the
    /// principal ran the browser checks, they passed, and the durable report
    /// still said the worker had skipped them and that validation remained to
    /// be done. Six months later that report is all anyone has.
    #[test]
    fn the_report_carries_what_the_principal_verified_itself() {
        let summary = summary_from_manifest(
            "KT-612",
            "exec-1",
            &manifest(),
            &facts(),
            &[
                verification("dod-1", true, "4 Chromium tests, 0 retry, 1.3 min"),
                verification("dod-2", true, "36 tour unit tests pass"),
            ],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("a complete manifest must produce a summary");

        assert_eq!(summary.principal_verifications.len(), 2);
        assert_eq!(summary.principal_verifications[0].dod_id, "dod-1");
        assert!(summary.principal_verifications[0].met);
        assert_eq!(
            summary.principal_verifications[0].evidence,
            "4 Chromium tests, 0 retry, 1.3 min"
        );

        let report = summary.render_markdown();
        assert!(
            report.contains("### Verified by the principal at review"),
            "{report}"
        );
        assert!(
            report.contains("4 Chromium tests, 0 retry, 1.3 min"),
            "{report}"
        );
    }

    /// And it must not rewrite the worker's own record while doing it. "The
    /// worker could not start Chromium" stays true after the principal ran it;
    /// turning that `skipped` into a `pass` would put a claim in the worker's
    /// mouth that it never made.
    #[test]
    fn principal_evidence_never_rewrites_what_the_worker_reported() {
        let mut with_skip = manifest();
        with_skip.tests = vec![crate::models::ManifestTest {
            name: "Chromium E2E".into(),
            status: TestStatus::Skipped,
            evidence: Some("no browser in this worktree".into()),
        }];

        let summary = summary_from_manifest(
            "KT-612",
            "exec-1",
            &with_skip,
            &facts(),
            &[verification("dod-1", true, "principal ran them: 4 PASS")],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("a complete manifest must produce a summary");

        // The worker's line is untouched, verdict and evidence both.
        assert_eq!(summary.validations.len(), 1);
        assert_eq!(summary.validations[0].result, "skipped");
        assert_eq!(
            summary.validations[0].evidence,
            "no browser in this worktree"
        );
        // And the principal's sits beside it, not on top of it.
        assert_eq!(summary.principal_verifications.len(), 1);

        let report = summary.render_markdown();
        assert!(report.contains("→ skipped"), "{report}");
        assert!(report.contains("principal ran them: 4 PASS"), "{report}");
    }

    /// A review that added no evidence of its own must produce no section at
    /// all. An empty heading reads as "the principal checked nothing", which is
    /// a claim; silence is the absence.
    #[test]
    fn a_review_without_its_own_evidence_says_nothing_about_it() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("a complete manifest must produce a summary");

        assert!(summary.principal_verifications.is_empty());
        let report = summary.render_markdown();
        assert!(
            !report.contains("Verified by the principal"),
            "an absent verification must not be announced: {report}"
        );
    }

    /// The renderer faithfully preserves the supplied verdict. The approval
    /// gate is responsible for refusing `met: false`; this narrow unit test
    /// keeps the rendering contract independent from that gate.
    #[test]
    fn the_renderer_reports_an_unmet_verdict_rather_than_assuming_it_met() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[verification("dod-3", false, "documentation still missing")],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("a complete manifest must produce a summary");

        assert!(!summary.principal_verifications[0].met);
        let report = summary.render_markdown();
        assert!(report.contains("`dod-3` → not met"), "{report}");
    }

    /// The publication path reads the decision persisted for the current
    /// attempt. It must preserve the worker's skipped test as a worker fact,
    /// add the principal's evidence in its own section, and retain the first
    /// accepted projection when a replay reaches it again.
    #[tokio::test]
    async fn publication_loads_persisted_principal_evidence_without_rewriting_worker_facts() {
        let db = Database::open_in_memory().expect("in-memory database");
        let manifest = DeliveryManifestV1 {
            tests: vec![ManifestTest {
                name: "cargo test --lib delivery".into(),
                status: TestStatus::Skipped,
                evidence: Some("no shell; principal must run".into()),
            }],
            ..manifest()
        };
        let decision = ReviewDecisionV1 {
            version: "1".into(),
            task_ref: "KT-613".into(),
            decision: ReviewVerdict::Approve,
            reviewed_head_sha: Some(manifest.head_sha.clone()),
            dod_verifications: vec![verification(
                "dod-1",
                true,
                "principal inspected the delivered SHA in this test",
            )],
            comment: None,
            findings: vec![],
        };
        let execution = TaskExecution {
            id: "exec-kt-613".into(),
            orchestration_run_id: "run-kt-613".into(),
            task_id: "task-kt-613".into(),
            parent_discussion_id: "parent-kt-613".into(),
            sub_discussion_id: None,
            workspace_id: None,
            dispatch_job_id: None,
            base_sha: None,
            child_branch: Some("kronn/task/kt-613".into()),
            worker_target_kind: None,
            worker_cli_session_id: None,
            worker_connection_id: None,
            worker_agent_type: Some("ClaudeCode".into()),
            worker_model: None,
            worker_model_tier: None,
            worker_profile_id: None,
            worker_scope: None,
            worker_dod_ids: None,
            attempt_no: 1,
            status: TaskExecutionStatus::Approved,
            blocked_from_status: None,
            interrupted_from_status: None,
            review_rounds: 0,
            max_review_rounds: 3,
            candidate_target_sha: None,
            candidate_merge_sha: None,
            integrated_sha: None,
            backup_ref: None,
            blocked_reason: None,
            blocked_reason_code: None,
            outcome_reason: None,
            idempotency_key: None,
            created_at: at("2026-09-01T09:00:00Z"),
            updated_at: at("2026-09-01T09:00:00Z"),
            finished_at: None,
        };
        let manifest_json = serde_json::to_string(&manifest).expect("manifest JSON");
        let decision_json = serde_json::to_string(&decision).expect("decision JSON");
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('parent-kt-613', 'Parent', ?1, ?1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            conn.execute(
                "INSERT INTO planning_tasks \
                 (id, task_number, title, created_at, updated_at) \
                 VALUES ('task-kt-613', 613, 'Delivery summary', ?1, ?1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            conn.execute(
                "INSERT INTO orchestration_runs \
                 (id, discussion_id, created_at, updated_at) \
                 VALUES ('run-kt-613', 'parent-kt-613', ?1, ?1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            conn.execute(
                "INSERT INTO task_executions \
                 (id, orchestration_run_id, task_id, parent_discussion_id, status, created_at, updated_at) \
                 VALUES ('exec-kt-613', 'run-kt-613', 'task-kt-613', 'parent-kt-613', 'Approved', ?1, ?1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            crate::db::worker_deliveries::upsert_delivery(
                conn,
                "exec-kt-613",
                0,
                "abc1234",
                &manifest_json,
            )?;
            crate::db::worker_reviews::upsert_review(
                conn,
                "exec-kt-613",
                0,
                "approve",
                &serde_json::json!({
                    "version": "1",
                    "task_ref": "KT-613",
                    "decision": "approve",
                    "reviewed_head_sha": "abc1234",
                    "dod_verifications": [{
                        "dod_id": "dod-1",
                        "met": true,
                        "evidence": "attempt zero evidence must not be published"
                    }]
                })
                .to_string(),
            )?;
            crate::db::worker_deliveries::upsert_delivery(
                conn,
                "exec-kt-613",
                1,
                "abc1234",
                &manifest_json,
            )?;
            crate::db::worker_reviews::upsert_review(
                conn,
                "exec-kt-613",
                1,
                "approve",
                &decision_json,
            )?;
            Ok(())
        })
        .await
        .expect("seed accepted attempt");

        assert!(matches!(
            publish_accepted_delivery(&db, &execution)
                .await
                .expect("publish"),
            Some(crate::db::delivery_summaries::Published::Created)
        ));
        let message_id = crate::db::delivery_summaries::message_id_for(&execution.id, 1);
        let report = {
            let message_id = message_id.clone();
            db.with_conn(move |conn| {
                conn.query_row(
                    "SELECT content FROM messages WHERE id = ?1",
                    [message_id],
                    |row| row.get::<_, String>(0),
                )
                .map_err(Into::into)
            })
            .await
            .expect("published report")
        };
        assert!(report.contains("→ skipped"), "{report}");
        assert!(report.contains("no shell; principal must run"), "{report}");
        assert!(
            report.contains("### Verified by the principal at review"),
            "{report}"
        );
        assert!(
            report.contains("principal inspected the delivered SHA in this test"),
            "{report}"
        );
        assert!(
            !report.contains("attempt zero evidence must not be published"),
            "{report}"
        );

        // An identical approval replay reaches the stored summary and writes
        // neither a second message nor a different report.
        assert!(matches!(
            publish_accepted_delivery(&db, &execution)
                .await
                .expect("replay"),
            Some(crate::db::delivery_summaries::Published::AlreadyComplete)
        ));

        // The publication record is authoritative even if a caller presents
        // divergent persisted evidence after the report already exists.
        let divergent = serde_json::json!({
            "version": "1",
            "task_ref": "KT-613",
            "decision": "approve",
            "reviewed_head_sha": "abc1234",
            "dod_verifications": [{
                "dod_id": "dod-1",
                "met": true,
                "evidence": "divergent evidence must not rewrite history"
            }]
        })
        .to_string();
        db.with_conn(move |conn| {
            crate::db::worker_reviews::upsert_review(
                conn,
                "exec-kt-613",
                1,
                "approve",
                &divergent,
            )?;
            Ok(())
        })
        .await
        .expect("seed divergent replay input");
        assert!(matches!(
            publish_accepted_delivery(&db, &execution)
                .await
                .expect("divergent replay"),
            Some(crate::db::delivery_summaries::Published::AlreadyComplete)
        ));
        let (messages, replayed_report, stored) = db
            .with_conn(move |conn| {
                let messages = conn.query_row(
                    "SELECT COUNT(*) FROM messages WHERE id = ?1",
                    [&message_id],
                    |row| row.get::<_, i64>(0),
                )?;
                let replayed_report = conn.query_row(
                    "SELECT content FROM messages WHERE id = ?1",
                    [&message_id],
                    |row| row.get::<_, String>(0),
                )?;
                let stored = crate::db::delivery_summaries::get(conn, "exec-kt-613", 1)?
                    .expect("stored report");
                Ok((messages, replayed_report, stored))
            })
            .await
            .expect("read replay result");
        assert_eq!(messages, 1);
        assert_eq!(replayed_report, report);
        assert!(stored
            .canonical_json
            .contains("principal inspected the delivered SHA in this test"));
        assert!(!stored.canonical_json.contains("divergent evidence"));
    }

    /// Publication is idempotent on the decision: the same review evidence
    /// replayed must yield byte-identical canonical JSON, or a retry after a
    /// lost response would look like a second, different report.
    #[test]
    fn replaying_the_same_review_produces_the_same_report() {
        let evidence = [verification("dod-1", true, "4 Chromium tests pass")];
        let build = || {
            summary_from_manifest(
                "KT-612",
                "exec-1",
                &manifest(),
                &facts(),
                &evidence,
                at("2026-09-01T10:00:00Z"),
            )
            .expect("a complete manifest must produce a summary")
        };

        assert_eq!(build().canonical_json(), build().canonical_json());
    }

    #[test]
    fn identity_comes_from_the_execution_not_from_the_payload() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("a complete manifest must produce a summary");

        assert_eq!(summary.agent, "OpenCode");
        assert_eq!(summary.model, "claude-opus-5");
        assert_eq!(summary.tier, "reasoning");
        // Stamped by Kronn, never by the worker.
        assert_eq!(summary.status, "accepted");
        assert_eq!(summary.schema_version, "delivery_summary/v1");
        assert_eq!(summary.timestamp, "2026-09-01T10:00:00+00:00");
        assert_eq!(summary.metrics.duration_ms, 91_000);
        assert_eq!(summary.metrics.tokens, Some(42_000));
    }

    #[test]
    fn every_manifest_fact_reaches_the_report_without_losing_its_nature() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .unwrap();

        assert_eq!(
            summary.changes,
            vec![
                DeliveryChange {
                    path: "backend/src/delivery.rs".into(),
                    nature: "modified".into()
                },
                DeliveryChange {
                    path: "backend/src/old.rs".into(),
                    nature: "removed".into()
                },
            ]
        );
        assert_eq!(
            summary.commit,
            DeliveryCommit::Made {
                sha: "abc1234".into(),
                branch: "kronn/task/kt-544".into()
            }
        );
        // A risk and a limitation are both attention points, but the reader
        // must still be able to tell them apart.
        assert_eq!(
            summary.attention_points,
            vec!["risk: touches a hot file", "limitation: no UI yet"]
        );
        assert_eq!(
            summary.documentation,
            DeliveryDocumentation::Updated {
                files: vec!["docs/architecture/delegation.md".into()]
            }
        );
    }

    #[test]
    fn a_validation_without_evidence_degrades_the_report_instead_of_losing_it() {
        let mut without = manifest();
        without.tests[0].evidence = None;
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &without,
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("an accepted delivery must always get its report");

        // The claim is published, but marked as unbacked in both places a
        // reader looks: the validation line and the attention points.
        assert_eq!(summary.validations[0].evidence, EVIDENCE_NOT_PROVIDED);
        assert!(
            summary
                .attention_points
                .iter()
                .any(|point| point.starts_with("unverified: validation")),
            "the degradation must be visible in the attention points: {:?}",
            summary.attention_points
        );
        let markdown = summary.render_markdown();
        assert!(markdown.contains(EVIDENCE_NOT_PROVIDED));
    }

    #[test]
    fn an_untimed_validation_says_so_rather_than_showing_zero_milliseconds() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(summary.validations[0].duration_ms, None);
        let markdown = summary.render_markdown();
        assert!(
            markdown.contains("duration not measured"),
            "an unmeasured duration must not render as a measurement: {markdown}"
        );
        assert!(!markdown.contains("(0 ms)"));
    }

    #[test]
    fn a_delivery_without_a_commit_says_so_instead_of_showing_a_blank_sha() {
        let mut no_commit = manifest();
        no_commit.head_sha = "   ".into();
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &no_commit,
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .expect("an absent commit is reportable, not fatal");
        match summary.commit {
            DeliveryCommit::None { justification } => assert!(!justification.trim().is_empty()),
            other => panic!("expected an explicit absence, got {other:?}"),
        }
    }

    #[test]
    fn the_rendered_markdown_keeps_a_fixed_order_whatever_the_payload() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .unwrap();
        let markdown = summary.render_markdown();
        let sections = [
            "### Summary",
            "### Changes",
            "### Commit",
            "### Validations",
        ];
        let mut cursor = 0usize;
        for section in sections {
            let at = markdown[cursor..]
                .find(section)
                .unwrap_or_else(|| panic!("{section} missing from the report"));
            cursor += at + section.len();
        }
        // Two identical inputs must render byte-identically: the layout is
        // Kronn's, so it cannot drift between two publications.
        let twin = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
            &[],
            at("2026-09-01T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(markdown, twin.render_markdown());
    }
}
