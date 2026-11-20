//! Tests of `crate::core::lane_git` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::lane_git::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
#[path = "../core/lane_git_test.rs"]
pub(crate) mod lane_git_test;
