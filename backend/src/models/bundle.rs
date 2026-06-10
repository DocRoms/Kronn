//! Workflow bundle — atomic creation of (Quick Prompts × N) +
//! (Quick APIs × N) + (Custom API plugins × N) + (1 Workflow) in a
//! single transaction. 0.8.3 — closes the loop on the AI Architect
//! killer flow: the agent can now emit a `KRONN:BUNDLE_READY` block
//! that materializes the whole ecosystem in one click, instead of
//! asking the user to navigate 3 surfaces (Quick Prompts tab → Quick
//! APIs tab → Workflows wizard).
//!
//! Cross-references between artifacts use a sentinel id syntax:
//!
//! ```text
//! "@bundle:<bundle_id>"
//! ```
//!
//! The server walks the workflow JSON, finds each `@bundle:X`
//! reference, and substitutes it with the real id of the artifact
//! declared in this bundle with `bundle_id: "X"`. Bundle ids are
//! unique across all three artifact categories in a single payload.
//!
//! On any failure (invalid ref, DB insert error, schema validation),
//! the entire transaction rolls back — no orphan rows.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{CreateQuickApiRequest, CreateQuickPromptRequest, CreateWorkflowRequest, CustomApiPayload};

/// One Quick Prompt declared inside a bundle. The `bundle_id` is the
/// placeholder used by `@bundle:<id>` references in the workflow
/// JSON; the wrapped `request` is the same payload `/api/quick-prompts`
/// expects.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BundleQuickPrompt {
    pub bundle_id: String,
    #[serde(flatten)]
    pub request: CreateQuickPromptRequest,
}

/// One Quick API declared inside a bundle.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BundleQuickApi {
    pub bundle_id: String,
    #[serde(flatten)]
    pub request: CreateQuickApiRequest,
}

/// One Custom API plugin declared inside a bundle. The wrapped
/// `payload` mirrors what `POST /api/mcps/configs` accepts for the
/// `Custom API` flow (`materialize_custom_server` consumes it).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BundleCustomApi {
    pub bundle_id: String,
    #[serde(flatten)]
    pub payload: CustomApiPayload,
}

/// One **child workflow** declared inside a bundle (2026-06-11). The
/// parent workflow's `SubWorkflow` step references it via
/// `sub_workflow_id: "@bundle:<bundle_id>"`; the server creates the
/// child FIRST (so its real id exists before the parent's step is
/// substituted) and the child inherits the parent's `project_id` when
/// it doesn't set its own (so linked_repos / project MCPs / the
/// `[TRIAGE]` addendum apply inside the child run — see
/// `docs/design/decomposed-autopilot-presets.md` INV-3).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BundleChildWorkflow {
    pub bundle_id: String,
    #[serde(flatten)]
    pub request: CreateWorkflowRequest,
}

/// Top-level bundle payload. Every section is optional except
/// `workflow` — the bundle is anchored on its workflow. An empty
/// bundle (no QP/QA/CustomAPI, just a workflow) is valid and
/// behaves like a regular `POST /api/workflows`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BundleRequest {
    #[serde(default)]
    pub quick_prompts: Vec<BundleQuickPrompt>,
    #[serde(default)]
    pub quick_apis: Vec<BundleQuickApi>,
    #[serde(default)]
    pub custom_apis: Vec<BundleCustomApi>,
    /// Child workflows created before the parent (2026-06-11). Referenced
    /// from the parent's `SubWorkflow` step via `@bundle:<bundle_id>` on
    /// `sub_workflow_id`. Cycle / depth / no-gate are validated against the
    /// in-memory bundle graph + existing DB workflows.
    #[serde(default)]
    pub child_workflows: Vec<BundleChildWorkflow>,
    pub workflow: CreateWorkflowRequest,
}

/// One artifact that was created by the bundle endpoint. The
/// `bundle_id` is the placeholder the caller used; the `id` is the
/// real DB id the artifact now lives at.
#[derive(Debug, Serialize, TS, PartialEq)]
#[ts(export)]
pub struct BundleCreated {
    pub bundle_id: String,
    pub id: String,
    pub name: String,
}

/// Response payload from `POST /api/workflows/bundle`. Each section
/// mirrors the request's section so the frontend can show "Created N
/// QPs / M QAs / K Custom APIs / 1 Workflow".
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct BundleResponse {
    pub quick_prompts: Vec<BundleCreated>,
    pub quick_apis: Vec<BundleCreated>,
    pub custom_apis: Vec<BundleCreated>,
    /// Child workflows created before the parent (2026-06-11).
    #[serde(default)]
    pub child_workflows: Vec<BundleCreated>,
    /// The workflow doesn't have a `bundle_id` (only one per bundle);
    /// the frontend uses `id` + `name` to navigate to it.
    pub workflow: BundleWorkflowCreated,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct BundleWorkflowCreated {
    pub id: String,
    pub name: String,
}

/// Sentinel-id prefix recognized in workflow step fields. Any string
/// value of the form `@bundle:<bundle_id>` is substituted with the
/// real id of the artifact declared with `bundle_id: "<bundle_id>"`
/// in the same bundle payload.
pub const BUNDLE_REF_PREFIX: &str = "@bundle:";
