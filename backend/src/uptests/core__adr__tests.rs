//! Tests of `crate::core::adr` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::adr::*;
use crate::core::td_sheet::{
    append_history_entry, replace_current_status, td_decision, TdDecision, STATUS_ACCEPTED_DECISION,
};
use std::collections::BTreeSet;
use std::path::Path;
#[path = "../core/adr_test.rs"]
pub(crate) mod tests;
