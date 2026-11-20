//! Layer 0 of the Kronn backend: a crate of its own so that no single
//! compilation holds the whole backend. Modules keep their `crate::<top>::<name>`
//! paths in every layer; a layer sees its own modules and the ones below.
// Test fixtures may start processes directly; production code cannot (clippy.toml).
#![cfg_attr(test, allow(clippy::disallowed_methods))]
// Without `full-tests` (the macOS portability job) most unit-test modules are
// configured out, so test-only helpers that remain `cfg(test)` go unused.
// Built with `test-support`, the helpers of this crate's tests are compiled
// without the tests that use them.
#![cfg_attr(
    feature = "test-support",
    allow(dead_code, unused_imports, clippy::new_without_default)
)]
#![cfg_attr(
    all(test, not(feature = "full-tests"), not(feature = "test-support")),
    allow(dead_code, unused_imports)
)]

pub mod acp {
    pub mod secret_files;
}
pub mod agents;
pub mod api;
pub mod core;
pub mod db;
pub mod delivery;
pub mod models;
pub mod workflows {
    pub mod api_call_ratelimit;
    pub mod api_call_security;
    pub mod api_call_step;
    pub mod authoring_doctrine;
    pub mod doctrine_warnings;
    pub mod gate_checkpoint;
    pub mod guard_counters;
    pub mod project_scope;
    pub mod safety;
    pub mod tracker;
    pub mod trigger;
}

pub use crate::db::Database;
pub use crate::models::AppConfig;

/// The `backend/` folder: this crate's manifest sits two folders below it.
pub const BACKEND_DIR: &str = {
    let manifest = env!("CARGO_MANIFEST_DIR").as_bytes();
    let (backend, _) = manifest.split_at(manifest.len() - "/crates/l0".len());
    match std::str::from_utf8(backend) {
        Ok(path) => path,
        Err(_) => panic!("the manifest folder is not UTF-8"),
    }
};
