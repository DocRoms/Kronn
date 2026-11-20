//! Tests of `crate::core::credential_store` that reach into the layers above it.
#![allow(unused_imports)]
use crate::core::credential_store::*;
use crate::core::{config_copies, crypto, keystore::KeyOutcome};
use crate::db::stored_credentials::{
    self as rows, StoredCredential, AUTH_TOKEN_ID, KIND_AUTH_TOKEN, KIND_PROVIDER_KEY,
};
use crate::db::Database;
use crate::models::{ApiKey, AppConfig};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
#[path = "../core/credential_store_tests.rs"]
pub(crate) mod tests;
