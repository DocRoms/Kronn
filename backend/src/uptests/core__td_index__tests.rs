//! Tests of `crate::core::td_index` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::td_index::*;
use crate::core::td_sheet::is_td_id;
use crate::core::td_sheet::{index_status_decision, td_decision, TdDecision};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
#[path = "../core/td_index_test.rs"]
pub(crate) mod tests;
