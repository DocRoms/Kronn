//! Tests of `crate::core::anti_halluc` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::anti_halluc::*;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use ts_rs::TS;
#[path = "../core/anti_halluc_tests_up.rs"]
pub(crate) mod tests;
