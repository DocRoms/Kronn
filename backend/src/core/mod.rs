pub mod anti_halluc;
pub mod audit_detectors;
pub mod audit_mcp_filter;
pub mod backup;
pub mod checksums;
pub mod cmd;
pub mod config;
pub mod context_files;
pub mod crypto;
pub mod directives;
pub mod docs_migration;
pub mod docs_sidecar;
pub mod docs_write_filter;
pub mod env;
pub mod faithfulness;
pub mod fs_guard;
pub mod host_mcp_discovery;
pub mod key_discovery;
pub mod keystore;
pub mod keyvault;
pub mod kronn_state;
pub mod learning_doc;
pub mod learning_gate;
pub mod learning_promote;
pub mod learning_scope;
pub mod learning_sweep;
pub mod legacy_docs;
pub mod log_buffer;
pub mod mcp_scanner;
pub mod native_files;
pub mod net_expose;
pub mod oauth2_cache;
pub mod pricing;
pub mod profiles;
pub mod recovery;
pub mod redact;
pub mod registry;
pub mod root_agent_files;
pub mod rtk_detect;
pub mod run_eta;
pub mod run_notify;
pub mod scanner;
pub mod skills;
pub mod sse_limits;
pub mod tailscale;
pub mod usage;
pub mod user_context;
pub mod versions;
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
