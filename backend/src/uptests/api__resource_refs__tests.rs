//! Tests of `crate::api::resource_refs` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::resource_refs::*;
use crate::{
    models::{ApiErrorCode, ApiResponse},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};
#[path = "../api/resource_refs_tests_up.rs"]
pub(crate) mod tests;
