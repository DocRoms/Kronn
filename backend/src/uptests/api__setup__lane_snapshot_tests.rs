//! Tests of `crate::api::setup` that reach into the layers above it.
#![allow(unused_imports)]
use crate::agents;
use crate::api::setup::*;
use crate::core::{config, scanner};
use crate::models::*;
use crate::AppState;
use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
#[path = "../api/setup_lane_snapshot_tests.rs"]
pub(crate) mod lane_snapshot_tests;
