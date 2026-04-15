pub mod checksums;
pub mod cmd;
pub mod context_files;
pub mod env;
pub mod config;
pub mod crypto;
pub mod directives;
pub mod key_discovery;
pub mod mcp_scanner;
pub mod pricing;
pub mod profiles;
pub mod registry;
pub mod scanner;
pub mod skills;
pub mod native_files;
pub mod sse_limits;
pub mod worktree;
pub mod tailscale;
pub mod ws_client;
pub mod log_buffer;

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
