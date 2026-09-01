//! Deterministic accepted-delivery-summary contract.
//!
//! After the orchestrator has privately validated a worker's structured result,
//! the worker publishes exactly one concise delivery report in the discussion.
//! Its structure and layout are NOT decided by the model: the worker supplies
//! only the semantic fields, Kronn validates the payload (refusing missing
//! required fields), keeps the canonical JSON for audit, and renders the
//! Markdown itself in a fixed order.
//!
//! This is deliberately distinct from an `important` pilot message: a delivery
//! summary reports one worker's accepted delivery, whereas an important card is
//! an orchestrator/human steering signal. See KT-368 objective, section
//! "Contrat déterministe du compte rendu accepté".

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The only status a published delivery summary may carry: a worker publishes
/// its report exclusively after the orchestrator accepted the delivery.
pub const ACCEPTED_STATUS: &str = "accepted";

/// Schema version stamped by Kronn, never by the model. Bump on any breaking
/// change to the field contract so persisted audit records stay interpretable.
pub const DELIVERY_SUMMARY_SCHEMA_VERSION: &str = "delivery_summary/v1";

/// Upper bound on the factual summary so a delivery report stays concise and a
/// worker cannot smuggle a transcript into the discussion.
const SUMMARY_MAX_CHARS: usize = 4000;

/// One file/artifact change and the nature of that change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryChange {
    pub path: String,
    /// Human-facing nature of the change (added, modified, removed, …).
    pub nature: String,
}

/// The commit produced by the delivery, or an explicit, justified absence.
/// A worker can never leave this ambiguous: either a real SHA+branch, or the
/// literal `none` with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveryCommit {
    Made { sha: String, branch: String },
    None { justification: String },
}

/// One validation the worker ran, with the evidence Kronn requires to accept a
/// `pass`. A result without a command or evidence is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryValidation {
    pub command: String,
    pub result: String,
    pub duration_ms: u64,
    pub evidence: String,
}

/// Documentation touched by the delivery, or an explicit absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveryDocumentation {
    Updated { files: Vec<String> },
    None,
}

/// Best-effort execution metrics. Tokens/cost are optional because not every
/// runtime reports them; duration is always known to the control plane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryMetrics {
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd_micros: Option<u64>,
}

/// The complete accepted-delivery-summary payload. `schema_version` and
/// `status` are set by Kronn; the model supplies only the semantic fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySummaryV1 {
    pub schema_version: String,
    pub status: String,
    pub task_reference: String,
    pub execution_id: String,
    /// Agent identity actually used, e.g. `OpenCode`.
    pub agent: String,
    /// Runtime actually used, e.g. `native-acp` / `direct-cli-migration`.
    pub runtime: String,
    /// Tier actually used, e.g. `reasoning`.
    pub tier: String,
    /// Concrete model actually used.
    pub model: String,
    pub summary: String,
    pub changes: Vec<DeliveryChange>,
    pub commit: DeliveryCommit,
    pub validations: Vec<DeliveryValidation>,
    pub documentation: DeliveryDocumentation,
    /// Explicit attention points; may be empty but never omitted.
    pub attention_points: Vec<String>,
    pub metrics: DeliveryMetrics,
    /// RFC 3339 timestamp set by Kronn.
    pub timestamp: String,
}

/// The semantic fields a worker submits. Kronn stamps identity, status,
/// schema version and timestamp; the model never provides those.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverySummaryInput {
    pub agent: String,
    pub runtime: String,
    pub tier: String,
    pub model: String,
    pub summary: String,
    pub changes: Vec<DeliveryChange>,
    pub commit: DeliveryCommit,
    pub validations: Vec<DeliveryValidation>,
    pub documentation: DeliveryDocumentation,
    #[serde(default)]
    pub attention_points: Vec<String>,
    pub metrics: DeliveryMetrics,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeliverySummaryError {
    #[error("delivery summary field `{0}` is required and must not be empty")]
    MissingField(&'static str),
    #[error("delivery summary summary exceeds the {SUMMARY_MAX_CHARS}-char bound")]
    SummaryTooLong,
    #[error("delivery summary change #{index} is missing `{field}`")]
    InvalidChange { index: usize, field: &'static str },
    #[error("delivery summary validation #{index} is missing `{field}`")]
    InvalidValidation { index: usize, field: &'static str },
    #[error("delivery summary commit is missing `{0}`")]
    InvalidCommit(&'static str),
    #[error("delivery summary documentation lists an empty file path")]
    EmptyDocumentationPath,
    #[error("delivery summary timestamp `{0}` is not valid RFC 3339")]
    InvalidTimestamp(String),
}

fn require(field: &'static str, value: &str) -> Result<(), DeliverySummaryError> {
    if value.trim().is_empty() {
        return Err(DeliverySummaryError::MissingField(field));
    }
    Ok(())
}

impl DeliverySummaryV1 {
    /// Build a validated summary from the worker's semantic input. Kronn stamps
    /// the non-model-controlled fields (`status`, `schema_version`,
    /// `task_reference`, `execution_id`, `timestamp`) and refuses the payload if
    /// any required semantic field is missing.
    pub fn build(
        task_reference: impl Into<String>,
        execution_id: impl Into<String>,
        timestamp: impl Into<String>,
        input: DeliverySummaryInput,
    ) -> Result<Self, DeliverySummaryError> {
        let summary = Self {
            schema_version: DELIVERY_SUMMARY_SCHEMA_VERSION.to_owned(),
            status: ACCEPTED_STATUS.to_owned(),
            task_reference: task_reference.into(),
            execution_id: execution_id.into(),
            agent: input.agent,
            runtime: input.runtime,
            tier: input.tier,
            model: input.model,
            summary: input.summary,
            changes: input.changes,
            commit: input.commit,
            validations: input.validations,
            documentation: input.documentation,
            attention_points: input.attention_points,
            metrics: input.metrics,
            timestamp: timestamp.into(),
        };
        summary.validate()?;
        Ok(summary)
    }

    /// Reject a payload with any missing required field. Attention points may be
    /// empty; every other identity/summary/commit/validation field is required.
    pub fn validate(&self) -> Result<(), DeliverySummaryError> {
        require("task_reference", &self.task_reference)?;
        require("execution_id", &self.execution_id)?;
        require("agent", &self.agent)?;
        require("runtime", &self.runtime)?;
        require("tier", &self.tier)?;
        require("model", &self.model)?;
        require("summary", &self.summary)?;
        if self.summary.chars().count() > SUMMARY_MAX_CHARS {
            return Err(DeliverySummaryError::SummaryTooLong);
        }
        for (index, change) in self.changes.iter().enumerate() {
            if change.path.trim().is_empty() {
                return Err(DeliverySummaryError::InvalidChange {
                    index,
                    field: "path",
                });
            }
            if change.nature.trim().is_empty() {
                return Err(DeliverySummaryError::InvalidChange {
                    index,
                    field: "nature",
                });
            }
        }
        match &self.commit {
            DeliveryCommit::Made { sha, branch } => {
                if sha.trim().is_empty() {
                    return Err(DeliverySummaryError::InvalidCommit("sha"));
                }
                if branch.trim().is_empty() {
                    return Err(DeliverySummaryError::InvalidCommit("branch"));
                }
            }
            DeliveryCommit::None { justification } => {
                if justification.trim().is_empty() {
                    return Err(DeliverySummaryError::InvalidCommit("justification"));
                }
            }
        }
        for (index, validation) in self.validations.iter().enumerate() {
            if validation.command.trim().is_empty() {
                return Err(DeliverySummaryError::InvalidValidation {
                    index,
                    field: "command",
                });
            }
            if validation.result.trim().is_empty() {
                return Err(DeliverySummaryError::InvalidValidation {
                    index,
                    field: "result",
                });
            }
            if validation.evidence.trim().is_empty() {
                return Err(DeliverySummaryError::InvalidValidation {
                    index,
                    field: "evidence",
                });
            }
        }
        if let DeliveryDocumentation::Updated { files } = &self.documentation {
            if files.iter().any(|file| file.trim().is_empty()) {
                return Err(DeliverySummaryError::EmptyDocumentationPath);
            }
        }
        if !is_rfc3339(&self.timestamp) {
            return Err(DeliverySummaryError::InvalidTimestamp(
                self.timestamp.clone(),
            ));
        }
        Ok(())
    }

    /// The canonical JSON kept for audit. Field order is fixed by the struct so
    /// two equal deliveries serialize identically.
    pub fn canonical_json(&self) -> String {
        serde_json::to_string(self).expect("delivery summary is always serializable")
    }

    /// Render the fixed-order Markdown. The model never provides layout: this is
    /// the single source of the published presentation, in a stable section
    /// order, so every accepted delivery reads identically.
    pub fn render_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "## ✅ Delivery accepted — {}\n\n",
            self.task_reference
        ));
        out.push_str(&format!(
            "**Execution** `{}` · **Agent** {} · **Runtime** {} · **Tier** {} · **Model** {}\n\n",
            self.execution_id, self.agent, self.runtime, self.tier, self.model
        ));

        out.push_str("### Summary\n");
        out.push_str(&self.summary);
        out.push_str("\n\n");

        out.push_str("### Changes\n");
        if self.changes.is_empty() {
            out.push_str("_none_\n\n");
        } else {
            for change in &self.changes {
                out.push_str(&format!("- `{}` — {}\n", change.path, change.nature));
            }
            out.push('\n');
        }

        out.push_str("### Commit\n");
        match &self.commit {
            DeliveryCommit::Made { sha, branch } => {
                out.push_str(&format!("`{sha}` on `{branch}`\n\n"));
            }
            DeliveryCommit::None { justification } => {
                out.push_str(&format!("none — {justification}\n\n"));
            }
        }

        out.push_str("### Validations\n");
        if self.validations.is_empty() {
            out.push_str("_none_\n\n");
        } else {
            for validation in &self.validations {
                out.push_str(&format!(
                    "- `{}` → {} ({} ms) — {}\n",
                    validation.command,
                    validation.result,
                    validation.duration_ms,
                    validation.evidence
                ));
            }
            out.push('\n');
        }

        out.push_str("### Documentation\n");
        match &self.documentation {
            DeliveryDocumentation::Updated { files } => {
                for file in files {
                    out.push_str(&format!("- `{file}`\n"));
                }
                out.push('\n');
            }
            DeliveryDocumentation::None => out.push_str("_none_\n\n"),
        }

        out.push_str("### Attention points\n");
        if self.attention_points.is_empty() {
            out.push_str("_none_\n\n");
        } else {
            for point in &self.attention_points {
                out.push_str(&format!("- {point}\n"));
            }
            out.push('\n');
        }

        out.push_str("### Metrics\n");
        out.push_str(&format!("- duration: {} ms\n", self.metrics.duration_ms));
        if let Some(tokens) = self.metrics.tokens {
            out.push_str(&format!("- tokens: {tokens}\n"));
        }
        if let Some(cost) = self.metrics.cost_usd_micros {
            out.push_str(&format!("- cost: ${:.6}\n", cost as f64 / 1_000_000.0));
        }
        out.push('\n');

        out.push_str(&format!(
            "<sub>{} · {} · schema {}</sub>\n",
            self.status, self.timestamp, self.schema_version
        ));
        out
    }
}

/// Minimal RFC 3339 acceptance sufficient for a Kronn-stamped timestamp: a
/// `date-time` with a `T` separator and either `Z` or a numeric offset. Kept
/// dependency-free and deliberately strict about the shape, not the calendar.
fn is_rfc3339(value: &str) -> bool {
    let bytes = value.as_bytes();
    // YYYY-MM-DDTHH:MM:SS is 19 chars; anything shorter cannot be a date-time.
    if bytes.len() < 20 {
        return false;
    }
    let digits = |slice: &[u8]| slice.iter().all(u8::is_ascii_digit);
    if !digits(&bytes[0..4]) || bytes[4] != b'-' || !digits(&bytes[5..7]) || bytes[7] != b'-' {
        return false;
    }
    if !digits(&bytes[8..10]) || (bytes[10] != b'T' && bytes[10] != b't') {
        return false;
    }
    if !digits(&bytes[11..13]) || bytes[13] != b':' || !digits(&bytes[14..16]) || bytes[16] != b':'
    {
        return false;
    }
    if !digits(&bytes[17..19]) {
        return false;
    }
    let rest = &value[19..];
    let rest = rest.strip_prefix('.').map_or(rest, |frac| {
        let end = frac
            .bytes()
            .position(|c| !c.is_ascii_digit())
            .unwrap_or(frac.len());
        &frac[end..]
    });
    rest == "Z" || rest == "z" || offset_is_valid(rest)
}

fn offset_is_valid(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() == 6
        && (bytes[0] == b'+' || bytes[0] == b'-')
        && bytes[1..3].iter().all(u8::is_ascii_digit)
        && bytes[3] == b':'
        && bytes[4..6].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> DeliverySummaryInput {
        DeliverySummaryInput {
            agent: "OpenCode".into(),
            runtime: "native-acp".into(),
            tier: "reasoning".into(),
            model: "opencode/zen".into(),
            summary: "Implemented the deterministic delivery-summary contract.".into(),
            changes: vec![DeliveryChange {
                path: "backend/src/delivery.rs".into(),
                nature: "added".into(),
            }],
            commit: DeliveryCommit::Made {
                sha: "abc1234".into(),
                branch: "kronn/task/kt-368".into(),
            },
            validations: vec![DeliveryValidation {
                command: "cargo test --lib delivery::".into(),
                result: "pass".into(),
                duration_ms: 1200,
                evidence: "8 passed".into(),
            }],
            documentation: DeliveryDocumentation::Updated {
                files: vec!["docs/design/adr-003-acp-control-plane.md".into()],
            },
            attention_points: vec![],
            metrics: DeliveryMetrics {
                duration_ms: 42_000,
                tokens: Some(1500),
                cost_usd_micros: Some(3200),
            },
        }
    }

    fn built() -> DeliverySummaryV1 {
        DeliverySummaryV1::build("KT-368", "exec-1", "2026-08-31T10:00:00Z", valid_input()).unwrap()
    }

    #[test]
    fn kronn_stamps_status_schema_and_identity_the_model_cannot_set() {
        let summary = built();
        // The worker input carries no status/schema/identity; Kronn owns them.
        assert_eq!(summary.status, ACCEPTED_STATUS);
        assert_eq!(summary.schema_version, DELIVERY_SUMMARY_SCHEMA_VERSION);
        assert_eq!(summary.task_reference, "KT-368");
        assert_eq!(summary.execution_id, "exec-1");
        assert_eq!(summary.timestamp, "2026-08-31T10:00:00Z");
    }

    #[test]
    fn a_missing_required_field_is_refused_not_silently_accepted() {
        let mut input = valid_input();
        input.model = "  ".into();
        assert_eq!(
            DeliverySummaryV1::build("KT-368", "exec-1", "2026-08-31T10:00:00Z", input)
                .unwrap_err(),
            DeliverySummaryError::MissingField("model")
        );
    }

    #[test]
    fn a_commit_absence_requires_an_explicit_justification() {
        let mut input = valid_input();
        input.commit = DeliveryCommit::None {
            justification: String::new(),
        };
        assert_eq!(
            DeliverySummaryV1::build("KT-368", "exec-1", "2026-08-31T10:00:00Z", input)
                .unwrap_err(),
            DeliverySummaryError::InvalidCommit("justification")
        );
    }

    #[test]
    fn a_validation_without_evidence_is_refused() {
        let mut input = valid_input();
        input.validations[0].evidence = String::new();
        assert_eq!(
            DeliverySummaryV1::build("KT-368", "exec-1", "2026-08-31T10:00:00Z", input)
                .unwrap_err(),
            DeliverySummaryError::InvalidValidation {
                index: 0,
                field: "evidence"
            }
        );
    }

    #[test]
    fn an_oversized_summary_is_bounded() {
        let mut input = valid_input();
        input.summary = "x".repeat(SUMMARY_MAX_CHARS + 1);
        assert_eq!(
            DeliverySummaryV1::build("KT-368", "exec-1", "2026-08-31T10:00:00Z", input)
                .unwrap_err(),
            DeliverySummaryError::SummaryTooLong
        );
    }

    #[test]
    fn an_invalid_timestamp_is_refused() {
        assert!(is_rfc3339("2026-08-31T10:00:00Z"));
        assert!(is_rfc3339("2026-08-31T10:00:00.250+02:00"));
        assert!(!is_rfc3339("2026-08-31 10:00:00"));
        assert!(!is_rfc3339("not-a-date"));
        let err =
            DeliverySummaryV1::build("KT-368", "exec-1", "31/08/2026", valid_input()).unwrap_err();
        assert_eq!(
            err,
            DeliverySummaryError::InvalidTimestamp("31/08/2026".into())
        );
    }

    #[test]
    fn markdown_is_generated_in_a_fixed_deterministic_order() {
        let markdown = built().render_markdown();
        let sections: Vec<usize> = [
            "### Summary",
            "### Changes",
            "### Commit",
            "### Validations",
            "### Documentation",
            "### Attention points",
            "### Metrics",
        ]
        .iter()
        .map(|section| markdown.find(section).expect("section present"))
        .collect();
        // Every section appears exactly once and strictly in the fixed order.
        assert!(sections.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(markdown.starts_with("## ✅ Delivery accepted — KT-368"));
        assert!(markdown.contains("schema delivery_summary/v1"));
        // Two identical deliveries render byte-for-byte identically.
        assert_eq!(built().render_markdown(), built().render_markdown());
    }

    #[test]
    fn empty_attention_points_render_explicitly_rather_than_being_omitted() {
        let markdown = built().render_markdown();
        assert!(markdown.contains("### Attention points\n_none_"));
    }

    #[test]
    fn canonical_json_round_trips_and_is_stable() {
        let summary = built();
        let json = summary.canonical_json();
        let parsed: DeliverySummaryV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, summary);
        assert_eq!(parsed.canonical_json(), json);
    }
}
