//! Tests of `crate::db::audit_runs` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::adr::StartOfRun;
use crate::db::audit_runs::*;
use crate::models::{AuditRun, AuditRunStep};
use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
#[path = "../db/audit_runs_tests_up.rs"]
pub(crate) mod tests;
