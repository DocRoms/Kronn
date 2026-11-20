//! Tests of `crate::api::git_ops` that reach into the layers above it.
#![allow(unused_imports)]
use crate::api::git_ops::*;
use crate::core::cmd::sync_cmd;
use crate::core::commit_policy::CommitPolicy;
use crate::models::*;
use std::path::Path;
#[path = "../api/git_ops_tests_up.rs"]
pub(crate) mod tests;
