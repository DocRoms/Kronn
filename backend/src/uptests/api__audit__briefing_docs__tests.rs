//! Tests of `crate::api::audit::briefing_docs` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::audit::briefing_docs::*;
use crate::api::audit::project_doc::{cell, check_text};
use crate::core::briefing_docs as detect;
use crate::core::docs_location::DocsRoot;
use crate::models::{
    ApiResponse, BriefingDetectedDoc, BriefingDocKind, BriefingDocLocation, BriefingDocState,
    BriefingDocsAnswers, CheckBriefingDocRequest,
};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
#[path = "../api/audit/briefing_docs_tests.rs"]
pub(crate) mod tests;
