//! Tests of `crate::api::projects::docker` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::projects::docker::*;
use crate::{
    core::{cmd::async_cmd, scanner},
    models::{
        ApiResponse, ProjectDockerAction, ProjectDockerActionRequest, ProjectDockerEndpoint,
        ProjectDockerHostStatus, ProjectDockerLogs, ProjectDockerRunningSummary,
        ProjectDockerService, ProjectDockerStatus,
    },
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
    path::{Path as FsPath, PathBuf},
    process::Output,
    time::Duration,
};
#[path = "../api/projects/docker_tests_up.rs"]
pub(crate) mod tests;
