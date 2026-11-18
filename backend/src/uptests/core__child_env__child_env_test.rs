//! Tests of `crate::core::child_env` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::child_env::*;
use std::ffi::{OsStr, OsString};
use std::path::Path;
#[path = "../core/child_env_test.rs"]
pub(crate) mod child_env_test;
