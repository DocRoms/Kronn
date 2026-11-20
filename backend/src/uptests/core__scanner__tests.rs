//! Tests of `crate::core::scanner` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::scanner::*;
use crate::models::{AiConfigType, DetectedRepo, ProjectWriteAccess, ProjectWriteAccessStatus};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
#[path = "../core/scanner_tests_up.rs"]
pub(crate) mod tests;
