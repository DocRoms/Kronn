//! Filter the project's `.mcp.json` down to an audit-friendly subset.
//!
//! Background: an audit IA reads local files and fills `docs/` templates.
//! The 10-15 MCP servers a user has wired for everyday work (Fastly,
//! Docker, GitLab, Microsoft 365, …) are useless during an audit AND
//! costly: each server adds 50-300 tool descriptions to the agent's
//! system prompt (the `--append-system-prompt` payload baked from the
//! MCP context files). On a project with 15 MCPs, the system prompt
//! hits ~12-15K tokens BEFORE the agent starts thinking; with the
//! audit allowlist applied it drops to ~3-4K — a measurable speedup
//! on the first audit step (typically `docs/AGENTS.md`).
//!
//! Allowlist is the union of:
//!   - [`AUDIT_MCP_ALLOWLIST`]: hard-coded set useful for audits
//!     (introspection, reasoning, memory, lib-docs lookup, git).
//!   - `KRONN_AUDIT_MCP_EXTRA` env var (comma-separated): user
//!     override for power users who NEED a specific MCP during an
//!     audit (rare).
//!
//! Comparison is case-insensitive on the server name to be robust
//! against capitalization drift between `.mcp.json` writes.

use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::Path;

/// Hard-coded set of MCP server names useful during an AI audit.
/// Names must match the keys under `mcpServers` in `.mcp.json` (the
/// host CLIs use the same convention). Case-insensitive matching at
/// runtime so user-curated `.mcp.json` files with different
/// capitalization still resolve.
pub const AUDIT_MCP_ALLOWLIST: &[&str] = &[
    "kronn-internal",        // Kronn's own introspection — always
    "Sequential Thinking",   // Structured reasoning, useful on big audits
    "Memory",                // Cross-step state
    "context7",              // External lib docs lookup
    "Git",                   // Repo history (git log, blame) without Bash
];

/// Env var the user can set to extend the allowlist for a single
/// audit run. Comma-separated names, case-insensitive. Empty / unset
/// → only the hard-coded allowlist applies. Whitespace around each
/// name is trimmed.
pub const AUDIT_MCP_EXTRA_ENV: &str = "KRONN_AUDIT_MCP_EXTRA";

/// Result of filtering an `.mcp.json` payload. `kept` is the list
/// of MCP server names that survived the filter (audit-allowed);
/// `dropped` is everything else. Useful for surfacing "filtered N
/// out of M MCPs" in logs / SSE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditMcpFilterReport {
    pub kept: Vec<String>,
    pub dropped: Vec<String>,
}

/// Build the audit-mode allowlist (hard-coded + env-extra). Names
/// are lowercased to make the [`is_allowed`] lookup case-insensitive
/// regardless of how the user wrote them in `.mcp.json` or in the
/// env var.
fn build_allowlist() -> HashSet<String> {
    let mut set: HashSet<String> = AUDIT_MCP_ALLOWLIST
        .iter()
        .map(|s| s.to_lowercase())
        .collect();
    if let Ok(extra) = std::env::var(AUDIT_MCP_EXTRA_ENV) {
        for raw in extra.split(',') {
            let name = raw.trim();
            if !name.is_empty() {
                set.insert(name.to_lowercase());
            }
        }
    }
    set
}

/// Check whether a single MCP server name is allowed for the audit
/// run. Exposed for tests + the SSE reporter; not used directly by
/// the filter (which inlines the comparison for performance).
pub fn is_allowed(name: &str, allowlist: &HashSet<String>) -> bool {
    allowlist.contains(&name.to_lowercase())
}

/// Apply the audit allowlist to a raw `.mcp.json` payload. Returns
/// the filtered JSON value (same shape as input — `{mcpServers: {...}}`)
/// alongside a report listing which servers were kept vs dropped.
/// Servers without a `mcpServers` root key are passed through unchanged
/// (no servers to filter — the audit will run with 0 MCPs).
pub fn filter_mcp_json(raw: &str) -> Result<(Value, AuditMcpFilterReport), serde_json::Error> {
    let mut value: Value = serde_json::from_str(raw)?;
    let allowlist = build_allowlist();

    let mut report = AuditMcpFilterReport {
        kept: Vec::new(),
        dropped: Vec::new(),
    };

    // Defensive: only mutate when the root has the expected shape.
    // A `.mcp.json` without `mcpServers` is unusual but not an error;
    // we pass it through verbatim so the audit doesn't crash on a
    // half-written file.
    if let Some(servers) = value
        .get_mut("mcpServers")
        .and_then(|s| s.as_object_mut())
    {
        let original: Map<String, Value> = std::mem::take(servers);
        for (name, cfg) in original {
            if is_allowed(&name, &allowlist) {
                servers.insert(name.clone(), cfg);
                report.kept.push(name);
            } else {
                report.dropped.push(name);
            }
        }
        // Stable ordering for tests + reproducible logs.
        report.kept.sort();
        report.dropped.sort();
    }

    Ok((value, report))
}

/// RAII guard that filters the project's `.mcp.json` down to the
/// audit allowlist when constructed, and restores the original file
/// when dropped (including on panic). This is how the audit pipeline
/// gives the agent a small toolset without touching the user's
/// regular `.mcp.json` permanently.
///
/// Trade-off: a discussion / workflow that spawns DURING the audit
/// window sees the filtered `.mcp.json` instead of the full set —
/// acceptable because (a) running parallel agents during an audit is
/// rare, and (b) the `mcp_scanner::sync_project_mcps_to_disk` call
/// in the discussion path re-writes the full `.mcp.json` from DB
/// before spawning anyway, so the window is sub-second.
///
/// Drop strategy: rename `.mcp.json.kronn-audit-bak` → `.mcp.json`
/// on drop. If the bak file is missing (manual cleanup, race), we
/// log and skip — never silently leave a half-state.
pub struct AuditMcpSwap {
    /// Path to the `.mcp.json` that's currently filtered. Set to
    /// `None` after a successful drop so a manual `.restore()` call
    /// becomes a no-op.
    mcp_path: Option<std::path::PathBuf>,
    /// Backup path (`<mcp_path>.kronn-audit-bak`).
    bak_path: std::path::PathBuf,
    /// Report from the filter run — exposed via `.report()` so the
    /// audit caller can surface "filtered N/M MCPs" in logs / SSE.
    report: AuditMcpFilterReport,
}

impl AuditMcpSwap {
    /// Install the filter on `project_path/.mcp.json`. If the file
    /// doesn't exist OR is malformed, returns `Ok(None)` — the audit
    /// proceeds with whatever default behavior the runner has (zero
    /// MCPs, in practice). The boolean returned by `report.kept.len()`
    /// lets the caller decide whether the swap actually shrank
    /// anything.
    ///
    /// Safe to call from sync context.
    pub fn install(project_path: &Path) -> std::io::Result<Option<Self>> {
        let mcp_path = project_path.join(".mcp.json");
        if !mcp_path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&mcp_path)?;
        let (filtered, report) = match filter_mcp_json(&raw) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "Audit MCP filter: malformed `.mcp.json` — proceeding without filter ({})",
                    e
                );
                return Ok(None);
            }
        };
        if report.dropped.is_empty() {
            // Nothing to filter (all allowed, or empty list). Skip
            // the swap entirely so the file's mtime doesn't change
            // and we don't risk a race with parallel agents.
            return Ok(None);
        }
        let bak_path = mcp_path.with_extension("json.kronn-audit-bak");
        // Atomic rename of original → bak, then write filtered.
        std::fs::rename(&mcp_path, &bak_path)?;
        let body = serde_json::to_string_pretty(&filtered)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if let Err(e) = std::fs::write(&mcp_path, body) {
            // Roll back the rename so the user's file isn't lost.
            let _ = std::fs::rename(&bak_path, &mcp_path);
            return Err(e);
        }
        tracing::info!(
            "Audit MCP filter active: kept {} / dropped {} servers",
            report.kept.len(), report.dropped.len()
        );
        Ok(Some(Self {
            mcp_path: Some(mcp_path),
            bak_path,
            report,
        }))
    }

    /// Report from the filter call. Useful for SSE / logs.
    pub fn report(&self) -> &AuditMcpFilterReport {
        &self.report
    }

    /// Manual restore — usually not needed, the Drop impl handles it.
    /// Idempotent: a second call after a successful first is a no-op.
    pub fn restore(&mut self) {
        let Some(mcp_path) = self.mcp_path.take() else { return; };
        if !self.bak_path.exists() {
            tracing::warn!(
                "Audit MCP swap restore: backup file {} missing — original `.mcp.json` may have been replaced by another writer",
                self.bak_path.display()
            );
            return;
        }
        if let Err(e) = std::fs::rename(&self.bak_path, &mcp_path) {
            tracing::error!(
                "Audit MCP swap restore failed: {} (filtered `.mcp.json` may still be in place)",
                e
            );
        }
    }
}

impl Drop for AuditMcpSwap {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    /// Lock the allowlist contents so a future "let's add foo" PR
    /// doesn't silently drop one of the 5 servers — these are the
    /// only MCPs Kronn defaults to ON during an audit, and dropping
    /// one (e.g. `kronn-internal`) would break introspection on
    /// every audit run.
    #[test]
    fn allowlist_covers_the_5_audit_friendly_servers() {
        assert!(AUDIT_MCP_ALLOWLIST.contains(&"kronn-internal"));
        assert!(AUDIT_MCP_ALLOWLIST.contains(&"Sequential Thinking"));
        assert!(AUDIT_MCP_ALLOWLIST.contains(&"Memory"));
        assert!(AUDIT_MCP_ALLOWLIST.contains(&"context7"));
        assert!(AUDIT_MCP_ALLOWLIST.contains(&"Git"));
    }

    #[test]
    fn filter_drops_non_allowlisted_servers() {
        // Real-world shape: user has 5 MCPs configured. The 3
        // not in the allowlist (Fastly, Docker, atlassian) drop;
        // the 2 in it (kronn-internal, context7) survive.
        let raw = json!({
            "mcpServers": {
                "kronn-internal": {"command": "python3", "args": ["x.py"]},
                "Fastly":         {"command": "fastly-mcp"},
                "Docker":         {"command": "docker-mcp"},
                "context7":       {"command": "node", "args": ["c7.js"]},
                "atlassian":      {"command": "npx", "args": ["jira-mcp"]},
            }
        }).to_string();
        let (filtered, report) = filter_mcp_json(&raw).unwrap();
        let servers = filtered.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.contains_key("kronn-internal"));
        assert!(servers.contains_key("context7"));
        // Report mirrors the split for SSE / logs.
        assert_eq!(report.kept, vec!["context7".to_string(), "kronn-internal".to_string()]);
        assert_eq!(report.dropped, vec!["Docker".to_string(), "Fastly".to_string(), "atlassian".to_string()]);
    }

    #[test]
    fn allowlist_matching_is_case_insensitive() {
        // A user-curated `.mcp.json` may capitalize names differently
        // ("SEQUENTIAL THINKING", "memory", "GIT"). The lookup must
        // not care so we don't silently drop them.
        let raw = json!({
            "mcpServers": {
                "SEQUENTIAL THINKING": {"command": "x"},
                "memory":              {"command": "y"},
                "GIT":                 {"command": "z"},
            }
        }).to_string();
        let (filtered, report) = filter_mcp_json(&raw).unwrap();
        let servers = filtered.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers.len(), 3, "all three should survive despite capitalization drift");
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn env_var_extra_extends_the_allowlist() {
        // Power-user override: `KRONN_AUDIT_MCP_EXTRA=Fastly,Docker`
        // → Fastly + Docker survive alongside the hard-coded set.
        // Whitespace around names tolerated.
        // SAFETY: tests in this module run sequentially via mutex
        // (vitest-style) so the env var doesn't leak between tests.
        let _lock = ENV_LOCK.lock().unwrap();
        // SAFETY: lock guarantees no concurrent env writers; OK in test ctx.
        unsafe { std::env::set_var(AUDIT_MCP_EXTRA_ENV, " Fastly , Docker "); }

        let raw = json!({
            "mcpServers": {
                "kronn-internal": {"command": "a"},
                "Fastly":         {"command": "b"},
                "Docker":         {"command": "c"},
                "atlassian":      {"command": "d"},
            }
        }).to_string();
        let (_filtered, report) = filter_mcp_json(&raw).unwrap();
        assert!(report.kept.contains(&"kronn-internal".to_string()));
        assert!(report.kept.contains(&"Fastly".to_string()));
        assert!(report.kept.contains(&"Docker".to_string()));
        assert!(report.dropped.contains(&"atlassian".to_string()));

        unsafe { std::env::remove_var(AUDIT_MCP_EXTRA_ENV); }
    }

    #[test]
    fn empty_or_missing_env_var_uses_hardcoded_allowlist_only() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var(AUDIT_MCP_EXTRA_ENV); }
        let allowlist = build_allowlist();
        assert_eq!(
            allowlist.len(),
            AUDIT_MCP_ALLOWLIST.len(),
            "no env → exactly the hard-coded count"
        );
    }

    #[test]
    fn env_var_with_empty_string_does_not_add_phantom_entries() {
        // `KRONN_AUDIT_MCP_EXTRA=" , , "` → all-empty tokens are
        // silently skipped. Without the .is_empty() filter, the set
        // would gain a `""` entry that matches an MCP server named
        // "" (impossible but defensive).
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var(AUDIT_MCP_EXTRA_ENV, " , , "); }
        let allowlist = build_allowlist();
        assert_eq!(allowlist.len(), AUDIT_MCP_ALLOWLIST.len(),
            "empty / whitespace-only entries in the env var must NOT extend the allowlist");
        unsafe { std::env::remove_var(AUDIT_MCP_EXTRA_ENV); }
    }

    #[test]
    fn payload_without_mcpservers_root_passes_through_unchanged() {
        // Edge: malformed/half-written `.mcp.json` without the
        // expected `mcpServers` root. Filter must not crash; the
        // audit proceeds with 0 MCPs configured.
        let raw = json!({"foo": "bar"}).to_string();
        let (filtered, report) = filter_mcp_json(&raw).unwrap();
        assert_eq!(filtered, json!({"foo": "bar"}));
        assert!(report.kept.is_empty());
        assert!(report.dropped.is_empty());
    }

    #[test]
    fn malformed_json_returns_error_not_panic() {
        let raw = "this is not json";
        let result = filter_mcp_json(raw);
        assert!(result.is_err(),
            "malformed JSON must surface as Err so caller can fallback to no MCP rather than panic");
    }

    // ── AuditMcpSwap RAII guard ─────────────────────────────────────────

    #[test]
    fn swap_install_filters_then_drop_restores() {
        // The whole contract: install replaces .mcp.json with the
        // filtered subset; drop restores the original. This is what
        // the audit pipeline relies on to NEVER permanently mutate
        // the user's MCP setup.
        let tmp = TempDir::new().unwrap();
        let mcp = tmp.path().join(".mcp.json");
        let original = json!({
            "mcpServers": {
                "kronn-internal": {"command": "x"},
                "Fastly":         {"command": "y"},
                "Docker":         {"command": "z"},
            }
        });
        std::fs::write(&mcp, original.to_string()).unwrap();

        {
            let swap = AuditMcpSwap::install(tmp.path()).unwrap()
                .expect("swap should install when there's something to filter");
            assert_eq!(swap.report().kept, vec!["kronn-internal".to_string()]);
            assert_eq!(swap.report().dropped, vec!["Docker".to_string(), "Fastly".to_string()]);
            // During the swap, .mcp.json contains only the allowlist.
            let live: Value = serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
            let servers = live.get("mcpServers").unwrap().as_object().unwrap();
            assert_eq!(servers.len(), 1);
            assert!(servers.contains_key("kronn-internal"));
        } // swap drops here → restore must run

        let restored: Value = serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        let servers = restored.get("mcpServers").unwrap().as_object().unwrap();
        assert_eq!(servers.len(), 3, "drop must restore the original 3-server config");
        assert!(servers.contains_key("Fastly"));
        assert!(servers.contains_key("Docker"));
        // Bak file is gone after restore (it was renamed back to .mcp.json).
        assert!(!tmp.path().join(".mcp.json.kronn-audit-bak").exists());
    }

    #[test]
    fn swap_returns_none_when_nothing_to_filter() {
        // All servers are already in the allowlist → no swap installed
        // (avoid touching mtime + race window). The file mtime stays
        // unchanged, no bak file is created.
        let tmp = TempDir::new().unwrap();
        let mcp = tmp.path().join(".mcp.json");
        std::fs::write(&mcp, json!({
            "mcpServers": {
                "kronn-internal": {"command": "x"},
                "Memory":         {"command": "y"},
            }
        }).to_string()).unwrap();
        let result = AuditMcpSwap::install(tmp.path()).unwrap();
        assert!(result.is_none(),
            "no servers to drop → no swap, no bak file");
        assert!(!tmp.path().join(".mcp.json.kronn-audit-bak").exists());
    }

    #[test]
    fn swap_returns_none_when_mcp_json_missing() {
        // Fresh project with no `.mcp.json` → no swap to install,
        // audit runs with zero MCPs (the default for projects without
        // MCP setup).
        let tmp = TempDir::new().unwrap();
        let result = AuditMcpSwap::install(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn swap_returns_none_when_mcp_json_malformed() {
        // A half-written or invalid `.mcp.json` must not crash the
        // audit. We log a warning and proceed without the filter
        // (worst case: agent gets all servers, slower audit but works).
        let tmp = TempDir::new().unwrap();
        let mcp = tmp.path().join(".mcp.json");
        std::fs::write(&mcp, "this is not json").unwrap();
        let result = AuditMcpSwap::install(tmp.path()).unwrap();
        assert!(result.is_none(),
            "malformed JSON falls through to no-swap (defensive)");
        // The user's malformed file MUST stay in place — we don't
        // rewrite or back it up.
        assert_eq!(std::fs::read_to_string(&mcp).unwrap(), "this is not json");
    }

    #[test]
    fn manual_restore_is_idempotent() {
        // Calling .restore() twice (manual + Drop) must not error.
        // The Option::take in restore() guards against the second
        // call doing anything.
        let tmp = TempDir::new().unwrap();
        let mcp = tmp.path().join(".mcp.json");
        std::fs::write(&mcp, json!({
            "mcpServers": {
                "kronn-internal": {"command": "x"},
                "Fastly":         {"command": "y"},
            }
        }).to_string()).unwrap();
        let mut swap = AuditMcpSwap::install(tmp.path()).unwrap().unwrap();
        swap.restore();
        // Second call (and the Drop later) should both be no-ops.
        swap.restore();
        let restored: Value = serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        assert!(restored.get("mcpServers").unwrap().get("Fastly").is_some());
    }

    #[test]
    fn swap_survives_panic_via_drop() {
        // The classic RAII contract: even if the audit panics mid-run,
        // the Drop impl must put the user's `.mcp.json` back. We
        // simulate panic via std::panic::catch_unwind. The TempDir
        // must outlive the closure (otherwise it gets dropped during
        // unwinding and the post-panic assertion can't read the file).
        let tmp = TempDir::new().unwrap();
        let mcp = tmp.path().join(".mcp.json");
        std::fs::write(&mcp, json!({
            "mcpServers": {
                "kronn-internal": {"command": "x"},
                "Fastly":         {"command": "y"},
            }
        }).to_string()).unwrap();
        let project_path = tmp.path().to_path_buf();
        let mcp_path_for_closure = mcp.clone();
        let result = std::panic::catch_unwind(move || {
            let _swap = AuditMcpSwap::install(&project_path).unwrap().unwrap();
            // Confirm filter is active mid-panic.
            let live: Value = serde_json::from_str(
                &std::fs::read_to_string(&mcp_path_for_closure).unwrap()
            ).unwrap();
            assert_eq!(live.get("mcpServers").unwrap().as_object().unwrap().len(), 1);
            panic!("simulated audit crash");
        });
        assert!(result.is_err());
        // After the panic unwound through Drop, original must be back.
        // `tmp` is still alive (lives in the outer scope), so `mcp` is
        // readable.
        let restored: Value = serde_json::from_str(&std::fs::read_to_string(&mcp).unwrap()).unwrap();
        assert_eq!(restored.get("mcpServers").unwrap().as_object().unwrap().len(), 2,
            "Drop on panic must restore the original `.mcp.json`");
    }

    // Single mutex guard for tests that mutate the process-wide env
    // var. Rust runs unit tests in parallel by default; without this
    // lock the env-var tests race and randomly fail when one removes
    // the var while another is reading it.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
