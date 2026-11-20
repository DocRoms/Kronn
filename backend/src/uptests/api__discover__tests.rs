//! Tests of `crate::api::discover` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::discover::*;
use crate::core::github_connection::normalize_repo_url;
use crate::models::*;
use crate::AppState;
use axum::{extract::State, Json};
#[path = "../api/discover_tests_up.rs"]
pub(crate) mod tests;
