//! Tests of `crate::db::resource_changes` that reach into the layers above it.
#![allow(unused_imports)]
use crate::db::resource_changes::*;
use rusqlite::Connection;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
#[path = "../db/resource_changes_tests_up.rs"]
pub(crate) mod tests;
