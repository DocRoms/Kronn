//! Layer 1 of the Kronn backend: a crate of its own so that no single
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

pub mod acp;
pub mod agents {
    pub use kronn_l0::agents::*;
    pub mod acp_start;
    pub mod chat_codec;
    pub mod generation_settings;
    pub mod ollama_memory;
    pub mod read_only_repos;
    pub mod runner;
    pub mod tool_trace;
    pub mod tools;
}
pub mod api {
    pub use kronn_l0::api::*;
    pub mod agent_workspace_tools;
    pub mod comfyui;
    pub mod discover;
    pub mod discussion_important;
    pub mod git_ops;
    pub mod lite_llm;
    pub mod nvidia;
    pub mod ollama_health;
    #[cfg(any(test, feature = "test-support"))]
    pub mod other_projects_fixture;
}
pub mod core {
    pub use kronn_l0::core::*;
    pub mod api_access;
    pub mod argv_roles;
    pub mod artifacts_reach;
    pub mod audit_baseline;
    pub mod audit_detectors;
    pub mod audit_ecosystems;
    pub mod audit_state;
    pub mod bridge_token;
    pub mod ci_facts;
    pub mod comfyui_references;
    pub mod commit_hooks;
    pub mod commit_policy;
    pub mod context_files;
    pub mod contribution;
    pub mod directives;
    pub mod doc_provenance;
    pub mod docs_citations;
    pub mod docs_folder;
    pub mod docs_merge;
    pub mod docs_sidecar;
    pub mod git_guard;
    pub mod home_credentials;
    pub mod inline_code;
    pub mod legacy_docs;
    pub mod mcp_scanner;
    pub mod model_catalog;
    pub mod native_files;
    pub mod net_expose;
    pub mod ollama_registry;
    pub mod operator_secret;
    pub mod profiles;
    pub mod project_rules;
    pub mod prompt_limit;
    pub mod quick_exec;
    pub mod repository_resources;
    pub mod repository_skills;
    pub mod resolution_proof;
    pub mod resource_refs;
    pub mod resource_snapshot;
    pub mod run_provenance;
    pub mod secret_facts;
    pub mod skill_bundle;
    pub mod skill_guide;
    pub mod skill_migration;
    pub mod skill_versions;
    pub mod skills;
    pub mod task_worker_env;
    pub mod task_worker_lane_settings;
    pub mod task_worker_lane_warm;
    pub mod task_worker_sandbox;
    pub mod task_worker_seed_align;
    pub mod task_worker_warm_skip;
    pub mod td_index;
    pub mod trust;
    pub mod trust_sandbox;
    pub mod uplink_state;
    pub mod vault_guard;
    pub mod worktree;
}
pub mod db {
    pub use kronn_l0::db::*;
    pub mod agent_decisions;
    pub mod api_call_logs;
    pub mod default_contents;
    pub mod discussion_effort;
    pub mod discussion_important;
    pub mod discussion_launch_settings;
    pub mod discussion_scope_grants;
    pub mod human_credentials;
    pub mod lite_llm_model_failures;
    pub mod live_pages;
    pub mod model_catalog;
    pub mod project_skill_references;
    pub mod quick_apis;
    pub mod quick_execs;
    pub mod quick_prompts;
    pub mod resource_identities;
    pub mod run_retention;
    pub mod shared_runs;
    pub mod step_artifacts;
    pub mod task_boards;
    pub mod trust;
    pub mod workflow_step_rooms;
    pub mod workflows;
}
pub use kronn_l0::delivery;
pub mod http_transport;
pub use kronn_l0::models;
pub mod state;
pub mod workflows {
    pub use kronn_l0::workflows::*;
    pub mod api_auth_failure;
    pub mod api_call_binary;
    pub mod api_call_executor;
    pub mod big_ticket_template;
    pub mod gate_step;
    pub mod json_data_step;
    pub mod refs;
    pub mod run_artifacts;
    pub mod run_scope;
    pub mod step_output_format;
    pub mod step_room;
    pub mod steps;
    pub mod template;
    pub mod triage;
}

pub use crate::db::Database;
pub use crate::models::AppConfig;
pub use crate::state::*;

// Tests of lower modules that need this layer.
#[cfg(test)]
#[allow(non_snake_case)] // <module path>__<test module>
mod uptests {
    #[cfg(all(test, feature = "full-tests"))]
    mod agents__run_progress__tests;
    #[cfg(test)]
    mod agents__wsl__tests;
    #[cfg(test)]
    mod core__checksums__tests;
    #[cfg(all(test, feature = "full-tests"))]
    mod core__child_env__child_env_test;
    #[cfg(test)]
    mod core__disk_usage__disk_usage_test;
    #[cfg(all(test, feature = "full-tests"))]
    mod db__resource_changes__tests;
}

/// The `backend/` folder: this crate's manifest sits two folders below it.
pub const BACKEND_DIR: &str = {
    let manifest = env!("CARGO_MANIFEST_DIR").as_bytes();
    let (backend, _) = manifest.split_at(manifest.len() - "/crates/l1".len());
    match std::str::from_utf8(backend) {
        Ok(path) => path,
        Err(_) => panic!("the manifest folder is not UTF-8"),
    }
};

// This crate's test build is a unit-test binary for the crates below it.
#[cfg(test)]
inventory::submit! { crate::core::test_mode::UnitTestBinary }
