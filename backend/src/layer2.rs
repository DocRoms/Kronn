//! Layer 2 of the Kronn backend: a crate of its own so that no single
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
    pub use kronn_l1::acp::*;
}
pub mod agents {
    pub use kronn_l1::agents::*;
    pub mod media_codec;
}
pub mod api {
    pub use kronn_l1::api::*;
    pub mod activity;
    pub mod agent_api;
    pub mod agent_jobs;
    pub mod ai_docs;
    pub mod assistant_conversations;
    pub mod contacts;
    pub mod delivery_publication;
    pub mod disc_git;
    pub mod disc_introspection;
    pub mod disc_invite;
    pub mod disk;
    pub mod federation;
    pub mod live_pages;
    pub mod mcp_remote;
    pub mod media;
    pub mod media_reference_settings;
    pub mod media_request_guard;
    pub mod orchestration;
    pub mod planning;
    pub mod quick_apis;
    pub mod quick_execs;
    pub mod quick_prompts;
    pub mod shared_runs;
    pub mod signal_catalog;
    pub mod step_artifacts;
    pub mod strong_confirm;
    pub mod worker_time_limit;
    pub mod workflows;
    pub mod ws;
}
pub mod core {
    pub use kronn_l1::core::*;
    pub mod agent_fallback;
    pub mod approved_scripts;
    pub mod audit_validation;
    pub mod binding_bundle;
    pub mod briefing_docs;
    pub mod cli_access_probe;
    pub mod connection_fallback;
    pub mod cross_audit;
    pub mod cross_audit_decision;
    pub mod export_report;
    pub mod project_profile;
    pub mod repo_facts;
    pub mod run_notify;
    pub mod run_retention;
    pub mod task_worker_dep_fetch;
    pub mod task_worker_lane_seeds;
    pub mod td_decisions;
    pub mod td_review;
    pub mod td_tasks;
    pub mod upstream_mirror;
    pub mod ws_client;
}
pub mod db {
    pub use kronn_l1::db::*;
    pub mod agent_jobs;
    pub mod audit_cross;
    pub mod audit_run_cost;
    pub mod cli_worker_bindings;
    pub mod compare;
    pub mod discussion_actions;
    pub mod discussion_ceiling_requests;
    pub mod discussion_scope;
    pub mod kronn_action_engine;
    pub mod orchestration;
    pub mod step_shadow_merges;
    pub mod step_shadows;
    pub mod worker_offers;
}
pub use kronn_l1::delivery;
pub use kronn_l1::http_transport;
pub use kronn_l1::models;
pub use kronn_l1::state;
pub mod workflows;

pub use crate::db::Database;
pub use crate::models::AppConfig;
pub use crate::state::*;
pub use crate::workflows::WorkflowEngine;

// Tests of lower modules that need this layer.
#[cfg(test)]
#[allow(non_snake_case)] // <module path>__<test module>
mod uptests {
    #[cfg(all(test, feature = "full-tests"))]
    mod api__discover__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod api__git_ops__tests;
    #[cfg(test)]
    mod core__lane_git__lane_git_test;
    #[cfg(all(test, feature = "full-tests"))]
    mod db__agent_dispatch__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod db__discussion_workspaces__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod db__planning__tests;
    #[cfg(test)]
    mod db__schema_check__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod models__workflows__step_deserialization_tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__api_call_binary__broker_tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__authoring_doctrine__tests;
    #[cfg(test)]
    mod workflows__autocode_issue_template__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__doctrine_warnings__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod workflows__steps__provenance_tests;
}

/// The `backend/` folder: this crate's manifest sits two folders below it.
pub const BACKEND_DIR: &str = {
    let manifest = env!("CARGO_MANIFEST_DIR").as_bytes();
    let (backend, _) = manifest.split_at(manifest.len() - "/crates/l2".len());
    match std::str::from_utf8(backend) {
        Ok(path) => path,
        Err(_) => panic!("the manifest folder is not UTF-8"),
    }
};

// This crate's test build is a unit-test binary for the crates below it.
#[cfg(test)]
inventory::submit! { crate::core::test_mode::UnitTestBinary }
