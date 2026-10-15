//! Every integration test, in one binary: each `tests/*.rs` file is a module
//! here, so the library and its dependencies are linked once instead of once
//! per file. Add a new file to this list; Cargo does not discover it.

/// No test of this binary may start an agent CLI installed on the machine
/// (KT-1189). The library arms its guard from `KRONN_TEST_NO_REAL_AGENTS`, a
/// variable it reads only in a build with the `test-support` feature (this
/// package's own dev-dependency turns it on for this binary and no other
/// build). It is set here, before `main`, so every test of every module has it whatever
/// helper it builds its state with — a flag each module had to remember to set
/// would be missing the day a module forgets it.
#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    link_section = ".init_array"
)]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static ARM_THE_REAL_AGENT_GUARD: extern "C" fn() = {
    extern "C" fn arm() {
        kronn::core::child_env::set_var(kronn::core::cmd::NO_REAL_AGENTS_ENV, "1");
    }
    arm
};

/// Shared by the KT-619 router modules.
#[path = "support/publication_fixture.rs"]
mod fixture_env;

/// A test that owns process-global state runs in its own process (KT-1186).
#[path = "support/own_process.rs"]
mod own_process;

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
#[path = "api_access_policy_scope.rs"]
mod api_access_policy_scope;
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
#[path = "no_real_agent_cli.rs"]
mod no_real_agent_cli;
#[path = "ollama_model_catalog.rs"]
mod ollama_model_catalog;
#[path = "orchestration_handoff_e2e.rs"]
mod orchestration_handoff_e2e;
#[path = "power_guard_orphan.rs"]
mod power_guard_orphan;
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

/// The guard is armed for this binary: it is what keeps `tests/*.rs` from
/// starting the agent CLIs of the machine. A platform where the constructor
/// above does not run fails here, not silently.
#[test]
fn the_real_agent_guard_is_armed_for_every_integration_test() {
    assert!(
        kronn::core::cmd::real_agents_forbidden(),
        "KRONN_TEST_NO_REAL_AGENTS is not set in the integration test binary"
    );
    assert_eq!(
        kronn::core::child_env::var(kronn::core::cmd::NO_REAL_AGENTS_ENV).as_deref(),
        Ok("1")
    );
}
