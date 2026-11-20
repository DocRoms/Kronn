//! Tests of `crate::core::checksums` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::checksums::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
#[path = "../core/checksums_test.rs"]
pub(crate) mod tests;
