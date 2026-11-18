//! Tests of `crate::db::schema_check` that reach into the layers above it.
#![allow(unused_imports)]
use crate::db::schema_check::*;
use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
#[path = "../db/schema_check_tests_up.rs"]
pub(crate) mod tests;
