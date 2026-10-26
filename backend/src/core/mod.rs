pub mod adr;
pub mod agent_orphans;
pub mod agent_skill;
pub mod anti_halluc;
pub mod api_access;
pub mod approved_scripts;
pub mod argv_roles;
pub mod audit_baseline;
pub mod audit_detectors;
pub mod audit_documents;
pub mod audit_ecosystems;
pub mod audit_mcp_filter;
pub mod audit_run_state;
pub mod audit_state;
pub mod audit_validation;
pub mod backup;
pub mod bridge_admit;
pub mod bridge_token;
pub mod briefing_docs;
pub mod checksums;
pub mod child_env;
pub mod ci_facts;
pub mod citation_drift;
pub mod cli_access_probe;
pub mod cli_args;
pub mod cmd;
pub mod comfyui;
pub mod commit_facts;
pub mod config;
pub mod content_memo;
pub mod context_audit;
pub mod context_files;
pub mod credential_store;
pub mod cross_audit;
pub mod cross_audit_decision;
pub mod crypto;
pub mod default_todo;
pub mod dependency_lockfiles;
pub mod dependency_updates;
pub mod desktop_port;
pub mod directives;
pub mod disk_usage;
pub mod dispatch_reaper;
pub mod doc_commands;
pub mod doc_provenance;
pub mod doc_relevance;
pub mod docs_citations;
pub mod docs_folder;
pub mod docs_location;
pub mod docs_merge;
pub mod docs_migration;
pub mod docs_sidecar;
pub mod docs_write_filter;
pub mod document_optimization;
pub mod embed_origins;
pub mod endpoint_reachability;
pub mod env;
pub mod execution_variables;
pub mod export_secrets;
pub mod faithfulness;
pub mod frontend_spa;
pub mod fs_guard;
pub mod generated_paths;
pub mod github_connection;
pub mod host_mcp_discovery;
pub mod inline_code;
pub mod key_discovery;
pub mod keystore;
pub mod keyvault;
pub mod kronn_state;
pub mod launch_context;
pub mod learning_doc;
pub mod learning_gate;
pub mod learning_promote;
pub mod learning_scope;
pub mod learning_sweep;
pub mod legacy_docs;
pub mod linked_repo_docs;
pub mod listen_guard;
pub mod log_buffer;
pub mod mcp_scanner;
pub mod mcp_secret_refs;
pub mod media_probe;
pub mod message_file_links;
pub mod model_catalog;
pub mod model_dir;
pub mod native_files;
pub mod net_expose;
pub mod oauth2_cache;
pub mod ollama_registry;
pub mod operator_secret;
pub mod power_guard;
pub mod pricing;
pub mod profiles;
pub mod project_profile;
pub mod project_rules;
pub mod quick_exec;
pub mod quick_exec_templates;
pub mod recovery;
pub mod redact;
pub mod registry;
pub mod repo_facts;
pub mod repository_resources;
pub mod resolution_proof;
pub mod resource_refs;
pub mod resource_snapshot;
pub mod resume_bundle;
pub mod review_payload;
pub mod room_lang;
pub mod root_agent_files;
pub mod rooted_io;
pub mod rtk_detect;
pub mod rtk_state;
pub mod run_eta;
pub mod run_notify;
pub mod run_provenance;
pub mod run_retention;
pub mod safe_http;
pub mod scanner;
pub mod secret_facts;
pub mod secret_scrub;
pub mod session_budget;
pub mod shell_path;
pub mod skill_bundle;
pub mod skill_guide;
pub mod skill_migration;
pub mod skill_versions;
pub mod skills;
pub mod sse_limits;
pub mod stack_consistency;
pub mod static_context;
pub mod tailscale;
pub mod task_worker_env;
pub mod task_worker_lane_warm;
pub mod task_worker_sandbox;
pub mod td_decisions;
pub mod td_index;
pub mod td_review;
pub mod td_tasks;
pub mod template_manifest;
pub mod timezone;
pub mod usage;
pub mod user_context;
pub mod version_sources;
pub mod versions;
pub mod vibe_trust;
pub mod worktree;
pub mod ws_client;
pub mod yaml_lite;

// crypto_test.rs removed 2026-07-01 — it was a strict subset of crypto.rs's
// richer inline `tests` module (roundtrip / wrong-key / base64 / parse / mask),
// adding duplicate tests with zero new coverage. The inline suite + proptest
// module in crypto.rs is the single source of truth.

#[cfg(all(test, feature = "full-tests"))]
#[path = "registry_test.rs"]
mod registry_test;

#[cfg(all(test, feature = "full-tests"))]
#[path = "scanner_test.rs"]
mod scanner_test;

#[cfg(all(test, feature = "full-tests"))]
#[path = "mcp_scanner_test.rs"]
mod mcp_scanner_test;

#[cfg(all(test, feature = "full-tests"))]
#[path = "template_homogeneity_test.rs"]
mod template_homogeneity_test;
