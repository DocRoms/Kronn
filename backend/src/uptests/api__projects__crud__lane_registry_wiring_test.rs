//! Tests of `crate::api::projects::crud` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::projects::crud::*;
use crate::api::projects::{
    enrich_audit_status, enrich_audit_status_with_runs, find_common_parent,
};
use crate::core::scanner;
use crate::models::*;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;
#[path = "../api/projects/lane_registry_wiring_test.rs"]
pub(crate) mod lane_registry_wiring_test;
