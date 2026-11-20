//! Tests of `crate::api::audit::cross_audit` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::audit::cross_audit::*;
use crate::core::cross_audit::{
    self as engine, ChallengeResult, ChecklistLine, CrossFindingRecord, DetectionOutput,
    DroppedFinding, FindingGroup, IndexInput, Outcome, RepliesOutput, ReplyResult, VerdictsOutput,
};
use crate::db::audit_runs::{StepTokens, TokenAccounting};
use crate::models::*;
use crate::AppState;
use axum::extract::{Path as AxumPath, Query, State};
use axum::Json;
use chrono::Utc;
use futures::future::BoxFuture;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;
#[path = "../api/audit/cross_audit_tests.rs"]
pub(crate) mod tests;
