pub mod checksums;
pub mod kronn_state;
pub mod redact;
pub mod anti_halluc;
pub mod faithfulness;
pub mod learning_scope;
pub mod learning_gate;
pub mod learning_promote;
pub mod learning_sweep;
pub mod learning_doc;
pub mod cmd;
pub mod usage;
pub mod context_files;
pub mod env;
pub mod config;
pub mod crypto;
pub mod directives;
pub mod key_discovery;
pub mod mcp_scanner;
pub mod host_mcp_discovery;
pub mod pricing;
pub mod profiles;
pub mod registry;
pub mod scanner;
pub mod skills;
pub mod native_files;
pub mod oauth2_cache;
pub mod docs_sidecar;
pub mod docs_write_filter;
pub mod docs_migration;
pub mod legacy_docs;
pub mod root_agent_files;
pub mod audit_mcp_filter;
pub mod user_context;
pub mod sse_limits;
pub mod worktree;
pub mod tailscale;
pub mod ws_client;
pub mod log_buffer;
pub mod rtk_detect;
pub mod versions;
pub mod run_eta;
pub mod audit_detectors;

#[cfg(test)]
#[path = "crypto_test.rs"]
mod crypto_test;

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
