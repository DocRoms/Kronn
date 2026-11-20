//! Tests of `crate::api::live_page_actions` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::live_page_actions::*;
use crate::core::launch_context::LaunchContext;
use crate::db::discussion_actions::{DiscussionActionKind, DiscussionActionState};
use crate::db::kronn_action_engine::ActionCompletion;
use crate::db::live_page_action_trusts::{
    LivePageActionTrust, LivePageActionTrustRefusal, LivePageActionTrustState,
};
use crate::db::live_page_actions::{
    LaunchLivePageActionRequest, LivePageAction, LivePageActionClaimOutcome,
};
use crate::models::{ApiErrorCode, ApiResponse, RunQuickApiRequest, RunQuickExecRequest};
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use std::collections::HashMap;
#[path = "../api/live_page_action_trust_api_tests.rs"]
pub(crate) mod trust_api_tests;
