//! Layer 3 of the Kronn backend: a crate of its own so that no single
//! compilation holds the whole backend. Modules keep their `crate::<top>::<name>`
//! paths in every layer; a layer sees its own modules and the ones below.
// Test fixtures may start processes directly; production code cannot (clippy.toml).
#![cfg_attr(test, allow(clippy::disallowed_methods))]
// Without `full-tests` (the macOS portability job) most unit-test modules are
// configured out, so test-only helpers that remain `cfg(test)` go unused.
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
    pub mod message_ingest;
    pub mod discussion_questions;
    pub mod live_page_action_trusts;
    pub mod live_page_actions;
    #[cfg(all(test, feature = "full-tests"))]
    #[path = "tests.rs"]
    pub mod tests;
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
