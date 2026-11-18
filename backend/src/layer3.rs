//! Layer 3 of the Kronn backend: a crate of its own so that no single
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
    all(test, not(feature = "full-tests")),
    allow(dead_code, unused_imports)
)]

pub use kronn_l2::acp;
pub mod agents {
    pub use kronn_l2::agents::*;
    pub mod room_reach;
}
pub mod api {
    pub use kronn_l2::api::*;
    pub mod agent_tools;
    pub mod artifact_portability;
    pub mod audit;
    pub mod disc_helpers;
    pub mod disc_prompts;
    pub mod disc_source;
    pub mod discussion_questions;
    pub mod discussion_scope;
    pub mod discussions;
    pub mod mcps;
    pub mod ollama;
    pub mod plugin_portability;
    pub mod projects;
    pub mod skills;
    pub mod trust;
}
pub mod core {
    pub use kronn_l2::core::*;
    pub mod bridge_admit;
    pub mod default_todo;
    pub mod document_optimization;
    pub mod template_manifest;
    pub mod token_checks;
}
pub mod db {
    pub use kronn_l2::db::*;
    pub mod audit_validation;
    pub mod discussion_questions;
    pub mod live_page_action_trusts;
    pub mod live_page_actions;
    pub mod message_ingest;
}
pub use kronn_l2::delivery;
pub use kronn_l2::http_transport;
pub use kronn_l2::models;
pub use kronn_l2::state;
pub use kronn_l2::workflows;

pub use crate::db::Database;
pub use crate::models::AppConfig;
pub use crate::state::*;
pub use crate::workflows::WorkflowEngine;

// Tests of lower modules that need this layer.
#[cfg(test)]
#[allow(non_snake_case)] // <module path>__<test module>
mod uptests {
    #[cfg(all(test, feature = "full-tests"))]
    mod agents__runner__acp_resume_tests;
    #[cfg(test)]
    mod core__adr__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod core__anti_halluc__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod core__api_access__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod core__kronn_state__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod core__scanner__tests;
    #[cfg(test)]
    mod core__td_index__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod db__audit_runs__tests;
    #[cfg(test)]
    mod db__orchestration__tests__items;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__concurrency__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__readiness__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__runner__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__task_board_step__tests;
}

/// The `backend/` folder: this crate's manifest sits two folders below it.
pub const BACKEND_DIR: &str = {
    let manifest = env!("CARGO_MANIFEST_DIR").as_bytes();
    let (backend, _) = manifest.split_at(manifest.len() - "/crates/l3".len());
    match std::str::from_utf8(backend) {
        Ok(path) => path,
        Err(_) => panic!("the manifest folder is not UTF-8"),
    }
};

// This crate's test build is a unit-test binary for the crates below it.
#[cfg(test)]
inventory::submit! { crate::core::test_mode::UnitTestBinary }
