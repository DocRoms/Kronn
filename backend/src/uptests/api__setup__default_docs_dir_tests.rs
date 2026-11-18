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
#[path = "../api/setup_default_docs_dir_tests_up.rs"]
pub(crate) mod default_docs_dir_tests;
