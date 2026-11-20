//! Tests of `crate::core::disk_usage` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::disk_usage::*;
use crate::models::setup::{DEFAULT_DISK_STALLED_BUILD_DAYS, DEFAULT_DISK_SWEEP_IDLE_HOURS};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
#[path = "../core/disk_usage_test.rs"]
pub(crate) mod disk_usage_test;
