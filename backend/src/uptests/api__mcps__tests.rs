//! Tests of `crate::api::mcps` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::mcps::*;
use crate::core::{cli_access_probe, mcp_scanner, registry};
use crate::db;
use crate::models::*;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::Semaphore,
    time::{timeout, Duration},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
#[path = "../api/mcps_tests_up.rs"]
pub(crate) mod tests;
