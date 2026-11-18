//! Tests of `crate::core::audit_validation` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::audit_validation::*;
use crate::core::td_sheet::{
    append_history_entry, replace_current_status, STATUS_ACCEPTED_DECISION, STATUS_CONFIRMED,
    STATUS_DEFERRED, STATUS_REJECTED,
};
use crate::core::td_sheet::{index_status_decision, td_decision, TdDecision};
use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::{Path, PathBuf};
#[path = "../core/audit_validation_tests_up.rs"]
pub(crate) mod tests;
