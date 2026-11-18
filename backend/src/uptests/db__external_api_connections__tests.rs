//! Tests of `crate::db::external_api_connections` that reach into the layers above it.
#![allow(unused_imports)]
use crate::db::external_api_connections::*;
use crate::db::parse_dt;
use crate::models::{
    AgentType, AppConfig, ExternalApiConnection, ExternalApiConnectionPreset, MessageTarget,
    ModelTierConfig,
};
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};
#[path = "../db/external_api_connections_tests_up.rs"]
pub(crate) mod tests;
