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
