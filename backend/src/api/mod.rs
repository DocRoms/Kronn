//! HTTP handlers, one module per domain.
//!
//! This file declares the `api` modules of the lowest crate. The crates above
//! declare theirs in their root file (`layer1.rs` to `layer3.rs`, `lib.rs`)
//! and re-export the ones below, so `crate::api::<module>` reads the same in
//! every crate: docs/architecture/backend-crates.md.
//!
//! `docs/conventions/agents-md-format-v1.md` is a spec installed in user
//! projects and its bytes are pinned (a change is a template update for every
//! project). It uses this file as its example of a citation: line 14, lines
//! 14-22, and the name `context_audit` within them. Keep these lines here.
//!
//! Where a module is declared says which crate compiles it:
//! - `context_audit`, the spec's example, is declared in `lib.rs`: it belongs
//!   to the top crate, which sees every layer;
//! - the modules listed below belong to the lowest crate;
//! - any other one belongs to the crate whose root file names it.
//!
//! A module moves between crates by moving its `pub mod` line; the size of
//! each crate is capped (`backend/scripts/ci/crate_size.py`).

pub mod agent_workspace_structure;
