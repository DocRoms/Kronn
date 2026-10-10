//! Every integration test, in one binary: each `tests/*.rs` file is a module
//! here, so the library and its dependencies are linked once instead of once
//! per file. Add a new file to this list; Cargo does not discover it.

/// Shared by the KT-619 router modules.
#[path = "support/publication_fixture.rs"]
mod fixture_env;

#[path = "adapter_worker_policy.rs"]
mod adapter_worker_policy;
#[path = "anti_hallu_citation_forms.rs"]
mod anti_hallu_citation_forms;
#[path = "anti_hallu_lang_claims.rs"]
mod anti_hallu_lang_claims;
#[path = "anti_hallu_malformed_markers.rs"]
mod anti_hallu_malformed_markers;
#[path = "anti_hallu_multiroot_finalize.rs"]
mod anti_hallu_multiroot_finalize;
#[path = "anti_hallu_perf_caps.rs"]
mod anti_hallu_perf_caps;
#[path = "anti_hallu_security_jail.rs"]
mod anti_hallu_security_jail;
#[path = "anti_hallu_tiers_and_counts.rs"]
mod anti_hallu_tiers_and_counts;
#[path = "anti_hallu_utf8_unicode.rs"]
mod anti_hallu_utf8_unicode;
#[path = "anti_hallu_verify_outcomes.rs"]
mod anti_hallu_verify_outcomes;
#[path = "api_tests.rs"]
mod api_tests;
#[path = "bridge_token_effect_log.rs"]
mod bridge_token_effect_log;
#[path = "bridge_token_launch.rs"]
mod bridge_token_launch;
#[path = "bridge_token_scope.rs"]
mod bridge_token_scope;
#[path = "config_write_guard.rs"]
mod config_write_guard;
#[path = "context_file_safety.rs"]
mod context_file_safety;
#[path = "cors_policy.rs"]
mod cors_policy;
#[path = "discussion_target_model.rs"]
mod discussion_target_model;
#[path = "http_model_resolution.rs"]
mod http_model_resolution;
#[path = "http_probe_catalog.rs"]
mod http_probe_catalog;
#[path = "kt619_authority_review.rs"]
mod kt619_authority_review;
#[path = "kt619_credential_routes.rs"]
mod kt619_credential_routes;
#[path = "kt619_worker_grant_boundary.rs"]
mod kt619_worker_grant_boundary;
#[path = "learnings_api.rs"]
mod learnings_api;
#[path = "model_catalog_api.rs"]
mod model_catalog_api;
#[path = "model_catalog_migration.rs"]
mod model_catalog_migration;
#[path = "ollama_model_catalog.rs"]
mod ollama_model_catalog;
#[path = "orchestration_handoff_e2e.rs"]
mod orchestration_handoff_e2e;
#[path = "real_agent_e2e.rs"]
mod real_agent_e2e;
#[path = "reasoning_effort_runtime.rs"]
mod reasoning_effort_runtime;
#[path = "room_reaches_the_agent_e2e.rs"]
mod room_reaches_the_agent_e2e;
#[path = "rooted_io_umask.rs"]
mod rooted_io_umask;
#[path = "workflow_agent_launch_policy.rs"]
mod workflow_agent_launch_policy;
#[path = "workflow_read_only_repos_probe.rs"]
mod workflow_read_only_repos_probe;
#[path = "workflow_step_room.rs"]
mod workflow_step_room;

/// A `tests/*.rs` file missing from the list above would never be compiled.
#[test]
fn every_test_file_is_a_module_of_this_binary() {
    let listed = include_str!("it.rs");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests");
    for entry in std::fs::read_dir(dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if name.ends_with(".rs") && name != "it.rs" {
            assert!(
                listed.contains(&format!("#[path = \"{name}\"]")),
                "tests/{name} is not declared in tests/it.rs"
            );
        }
    }
}
