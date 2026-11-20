//! Tests of `crate::agents::wsl` that reach into the layers above it.
#![allow(unused_imports)]
use crate::agents::wsl::*;
use std::path::{Path, PathBuf};
#[path = "../agents/wsl_tests_up.rs"]
pub(crate) mod tests;
