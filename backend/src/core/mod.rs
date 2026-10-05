pub mod agent_skill;
pub mod anti_halluc;
pub mod audit_detectors;
pub mod audit_mcp_filter;
pub mod audit_validation;
pub mod backup;
pub mod bridge_token;
pub mod checksums;
pub mod child_env;
pub mod cli_access_probe;
pub mod cmd;
pub mod config;
pub mod content_memo;
pub mod context_audit;
pub mod context_files;
pub mod crypto;
pub mod dependency_updates;
pub mod desktop_port;
pub mod directives;
pub mod docs_migration;
pub mod docs_sidecar;
pub mod docs_write_filter;
pub mod document_optimization;
pub mod env;
pub mod execution_variables;
pub mod export_secrets;
pub mod faithfulness;
pub mod fs_guard;
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
pub mod log_buffer;
pub mod mcp_scanner;
pub mod mcp_secret_refs;
pub mod media_probe;
pub mod message_file_links;
pub mod model_catalog;
pub mod native_files;
pub mod net_expose;
pub mod oauth2_cache;
pub mod ollama_registry;
pub mod operator_secret;
pub mod power_guard;
pub mod pricing;
pub mod profiles;
pub mod quick_exec;
pub mod quick_exec_templates;
pub mod recovery;
pub mod redact;
pub mod registry;
pub mod repository_resources;
pub mod resource_refs;
pub mod resource_snapshot;
pub mod resume_bundle;
pub mod review_payload;
pub mod root_agent_files;
pub mod rtk_detect;
pub mod rtk_state;
pub mod run_eta;
pub mod run_notify;
pub mod run_retention;
pub mod scanner;
pub mod session_budget;
pub mod skill_migration;
pub mod skills;
pub mod sse_limits;
pub mod static_context;
pub mod tailscale;
pub mod usage;
pub mod user_context;
pub mod versions;
pub mod vibe_trust;
pub mod worktree;
pub mod ws_client;

// crypto_test.rs removed 2026-07-01 — it was a strict subset of crypto.rs's
// richer inline `tests` module (roundtrip / wrong-key / base64 / parse / mask),
// adding duplicate tests with zero new coverage. The inline suite + proptest
// module in crypto.rs is the single source of truth.

#[cfg(test)]
#[path = "registry_test.rs"]
mod registry_test;

#[cfg(test)]
#[path = "scanner_test.rs"]
mod scanner_test;

#[cfg(test)]
#[path = "mcp_scanner_test.rs"]
mod mcp_scanner_test;

#[cfg(test)]
#[path = "template_homogeneity_test.rs"]
mod template_homogeneity_test;
