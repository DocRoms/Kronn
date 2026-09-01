//! Turning an accepted delivery into the one report published in the discussion
//! (KT-544).
//!
//! The worker never authors this text. It submitted a `DeliveryManifestV1` for
//! review; once the orchestrator accepts, Kronn derives the semantic fields
//! from that manifest, stamps the identity facts only the control plane knows,
//! validates the payload and renders the Markdown in a fixed order. A model can
//! therefore influence *what* is reported, never the layout, the identity, nor
//! whether a claim counts as evidenced.

use chrono::{DateTime, Utc};

use crate::delivery::{
    DeliveryChange, DeliveryCommit, DeliveryDocumentation, DeliveryMetrics, DeliverySummaryError,
    DeliverySummaryInput, DeliverySummaryV1, DeliveryValidation,
};
use crate::models::{
    DeliveryManifestV1, FileChangeKind, MessageTargetKind, TaskExecution, TestStatus,
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
            attention_points,
            metrics: DeliveryMetrics {
                duration_ms: facts.duration_ms,
                tokens: facts.tokens,
                cost_usd_micros: facts.cost_usd_micros,
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ManifestDodStatus, ManifestFile, ManifestTest};

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

    #[test]
    fn identity_comes_from_the_execution_not_from_the_payload() {
        let summary = summary_from_manifest(
            "KT-544",
            "exec-1",
            &manifest(),
            &facts(),
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
            at("2026-09-01T10:00:00Z"),
        )
        .unwrap();
        assert_eq!(markdown, twin.render_markdown());
    }
}
