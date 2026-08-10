//! 0.8.7 — Agent CLI usage / cost reporting via `ccusage`.
//!
//! Kronn's own `core::pricing` estimates cost from a static table + a guessed
//! 60/40 input/output split, ignoring prompt caching — which massively
//! over-estimates cost on cache-heavy sessions. `ccusage`
//! (https://github.com/ccusage/ccusage) reads the CLIs' OWN local JSONL logs
//! and reports the REAL token breakdown (input / output / cache-create /
//! cache-read) with up-to-date per-model pricing, across Claude / Codex /
//! Gemini and more.
//!
//! This module shells out to the `ccusage` binary (installed globally in the
//! Docker image, RTK-style) and parses its `--json` output into Kronn types.
//!
//! ### Scope (0.8.7 MVP)
//! This surfaces the **global** usage views (daily / weekly / monthly) — the
//! aggregate spend across ALL of the user's CLI sessions, not attributed to a
//! specific Kronn discussion/workflow. Per-Kronn-project attribution would
//! require correlating ccusage session ids to Kronn discs and is deliberately
//! deferred (ccusage's session JSON exposes a session id, not a project path).
//!
//! ### How it reads host logs from inside the container
//! The backend runs in Docker; the CLI logs live on the host. The host home is
//! mounted read-only at `/host-home`, so we invoke ccusage with
//! `HOME=/host-home` (overridable via `KRONN_USAGE_HOME`) so its auto-discovery
//! finds `/host-home/.claude`, `/host-home/.codex`, `/host-home/.gemini`. npm's
//! cache is redirected to a writable `/tmp` path (the host mount is read-only).

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Per-model cost within a row — lets the frontend roll up by agent
/// (model name prefix → Claude / Codex / Gemini …) for the breakdown chart.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageModelBreakdown {
    pub model_name: String,
    pub cost: f64,
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

/// One row of a usage report (a date / week / month, possibly per-agent).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageRow {
    /// The period label — a date (`2026-05-27`), week, month, or session id.
    pub period: String,
    /// Agent slug as ccusage reports it (`all`, `claude`, `codex`, `gemini`, …).
    pub agent: String,
    pub models_used: Vec<String>,
    /// Per-model cost split, for agent-level rollup on the frontend.
    pub model_breakdowns: Vec<UsageModelBreakdown>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
}

/// Aggregate totals across all rows.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
    pub total_cost: f64,
}

/// A full usage report for one period kind.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageReport {
    /// `daily` | `weekly` | `monthly`.
    pub period_kind: String,
    pub rows: Vec<UsageRow>,
    pub totals: UsageTotals,
    /// Distinct agents that appear across the rows (for header chips).
    pub agents_detected: Vec<String>,
}

// ─── ccusage raw JSON (camelCase) ─────────────────────────────────────────

#[derive(Deserialize, Default)]
struct RawMetadata {
    /// ccusage stamps the underlying agents here on aggregate (`agent: "all"`)
    /// rows — the top-level `agent` field is just "all" in that case.
    #[serde(default)]
    agents: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawModelBreakdown {
    #[serde(default)]
    model_name: String,
    #[serde(default)]
    cost: f64,
    // ccusage's per-model breakdown ships the four token components but no
    // aggregate `totalTokens`; sum them ourselves (fall back to an explicit
    // `totalTokens` if a future ccusage version starts emitting one).
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
}

impl RawModelBreakdown {
    fn resolved_total_tokens(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.input_tokens
                + self.output_tokens
                + self.cache_creation_tokens
                + self.cache_read_tokens
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRow {
    #[serde(default)]
    period: String,
    #[serde(default = "default_agent")]
    agent: String,
    #[serde(default)]
    metadata: RawMetadata,
    #[serde(default)]
    models_used: Vec<String>,
    #[serde(default)]
    model_breakdowns: Vec<RawModelBreakdown>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    total_cost: f64,
}

fn default_agent() -> String {
    "all".to_string()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawTotals {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    total_cost: f64,
}

/// Validate the requested period and return the ccusage subcommand name.
/// Defaults to `daily` for anything unexpected (never trusts caller input
/// blindly into the shell).
pub fn normalize_period(period: &str) -> &'static str {
    match period {
        "weekly" => "weekly",
        "monthly" => "monthly",
        _ => "daily",
    }
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

/// Extra package-manager bin directories used by native desktop launches.
/// Finder / Explorer do not necessarily inherit the interactive shell PATH,
/// even though the same global install works from a terminal.
fn conventional_bin_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(value) = std::env::var_os("PNPM_HOME").filter(|value| !value.is_empty()) {
        push_unique(&mut dirs, PathBuf::from(value));
    }
    if let Some(value) = std::env::var_os("NPM_CONFIG_PREFIX")
        .or_else(|| std::env::var_os("npm_config_prefix"))
        .filter(|value| !value.is_empty())
    {
        let prefix = PathBuf::from(value);
        push_unique(&mut dirs, prefix.join("bin"));
        push_unique(&mut dirs, prefix);
    }

    if let Some(home) = home {
        for relative in [
            ".local/bin",
            ".local/share/pnpm",
            ".local/share/pnpm/bin",
            ".bun/bin",
            ".volta/bin",
            "Library/pnpm",
            "Library/pnpm/bin",
        ] {
            push_unique(&mut dirs, home.join(relative));
        }
    }

    #[cfg(target_os = "macos")]
    for path in ["/opt/homebrew/bin", "/usr/local/bin"] {
        push_unique(&mut dirs, PathBuf::from(path));
    }

    #[cfg(target_os = "linux")]
    for path in ["/usr/local/bin", "/usr/bin", "/snap/bin"] {
        push_unique(&mut dirs, PathBuf::from(path));
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(value) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
            push_unique(&mut dirs, PathBuf::from(value).join("npm"));
        }
        if let Some(value) = std::env::var_os("ProgramFiles").filter(|value| !value.is_empty()) {
            push_unique(&mut dirs, PathBuf::from(value).join("nodejs"));
        }
    }

    dirs
}

fn tool_names(name: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec![
            format!("{name}.cmd"),
            format!("{name}.exe"),
            name.to_string(),
        ]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![name.to_string()]
    }
}

fn find_in_dirs(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        for file_name in tool_names(name) {
            let candidate = dir.join(file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn collect_node_modules_dirs(root: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if entry.file_name() == "node_modules" {
            found.push(path);
        } else {
            collect_node_modules_dirs(&path, depth - 1, found);
        }
    }
}

/// Locate a ccusage package already installed by an official package runner
/// (`npx`, `pnpm dlx`, `bunx`). Package runners intentionally do not create a
/// global binary; their cached package is nevertheless a valid local install.
fn find_cached_ccusage(root: &Path) -> Option<PathBuf> {
    let mut node_modules_dirs = Vec::new();
    collect_node_modules_dirs(root, 5, &mut node_modules_dirs);

    let mut candidates: Vec<(bool, SystemTime, PathBuf)> = Vec::new();
    for node_modules in node_modules_dirs {
        // ccusage 20.x ships a platform-native executable. Prefer it to the JS
        // shim so a GUI-launched app does not also need `node` in its PATH.
        let native_scope = node_modules.join("@ccusage");
        if let Ok(packages) = std::fs::read_dir(native_scope) {
            for package in packages.flatten() {
                if !package
                    .file_name()
                    .to_string_lossy()
                    .starts_with("ccusage-")
                {
                    continue;
                }
                for file_name in ["ccusage", "ccusage.exe"] {
                    let path = package.path().join("bin").join(file_name);
                    if path.is_file() {
                        let modified = path
                            .metadata()
                            .and_then(|metadata| metadata.modified())
                            .unwrap_or(UNIX_EPOCH);
                        candidates.push((true, modified, path));
                    }
                }
            }
        }

        for file_name in tool_names("ccusage") {
            let path = node_modules.join(".bin").join(file_name);
            if path.is_file() {
                let modified = path
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(UNIX_EPOCH);
                candidates.push((false, modified, path));
            }
        }
    }

    candidates
        .into_iter()
        .max_by_key(|(native, modified, _)| (*native, *modified))
        .map(|(_, _, path)| path)
}

fn package_runner_cache_roots(home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(cache) = std::env::var_os("NPM_CONFIG_CACHE")
        .or_else(|| std::env::var_os("npm_config_cache"))
        .filter(|value| !value.is_empty())
    {
        push_unique(&mut roots, PathBuf::from(cache).join("_npx"));
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        push_unique(&mut roots, PathBuf::from(cache).join("pnpm/dlx"));
    }
    if let Some(home) = home {
        for relative in [
            ".npm/_npx",
            ".cache/pnpm/dlx",
            "Library/Caches/pnpm/dlx",
            ".bun/install/cache",
        ] {
            push_unique(&mut roots, home.join(relative));
        }
    }
    roots
}

fn resolve_ccusage_program() -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os("KRONN_CCUSAGE_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return explicit.is_file().then_some(explicit).ok_or_else(|| {
            "KRONN_CCUSAGE_BIN does not point to a readable ccusage executable".to_string()
        });
    }

    if let Ok(path) = which::which("ccusage") {
        return Ok(path);
    }

    let home = user_home_dir();
    if let Some(path) = find_in_dirs("ccusage", &conventional_bin_dirs(home.as_deref())) {
        return Ok(path);
    }
    for root in package_runner_cache_roots(home.as_deref()) {
        if let Some(path) = find_cached_ccusage(&root) {
            return Ok(path);
        }
    }

    Err("ccusage not available. Install it globally, or run `npx ccusage@latest --version`, `pnpm dlx ccusage --version`, or `bunx ccusage --version` once so Kronn can use the local package-runner cache.".to_string())
}

fn usage_home_for_command(explicit: Option<OsString>, in_docker: bool) -> Option<OsString> {
    explicit
        .filter(|value| !value.is_empty())
        .or_else(|| in_docker.then(|| OsString::from("/host-home")))
}

/// Parse ccusage `--json` stdout (for the given period kind) into a UsageReport.
/// Pure — unit-testable without invoking the binary.
pub fn parse_report(period_kind: &str, json: &[u8]) -> Result<UsageReport, String> {
    let v: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| format!("invalid ccusage JSON: {e}"))?;

    // The rows live under the key matching the period kind (`daily` / `weekly`
    // / `monthly`). Fall back to the first array value if the key shape ever
    // changes, so a ccusage bump doesn't silently zero the report.
    let rows_val = v
        .get(period_kind)
        .or_else(|| {
            v.as_object()
                .and_then(|o| o.values().find(|x| x.is_array()))
        })
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(vec![]));

    let raw_rows: Vec<RawRow> =
        serde_json::from_value(rows_val).map_err(|e| format!("parse rows: {e}"))?;

    let raw_totals: RawTotals = v
        .get("totals")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("parse totals: {e}"))?
        .unwrap_or_default();

    let mut agents: Vec<String> = Vec::new();
    let rows: Vec<UsageRow> = raw_rows
        .into_iter()
        .map(|r| {
            // Collect agents from the top-level field (per-agent rows) AND
            // from metadata.agents (aggregate `all` rows stamp them there).
            if r.agent != "all" && !agents.contains(&r.agent) {
                agents.push(r.agent.clone());
            }
            for a in &r.metadata.agents {
                if !agents.contains(a) {
                    agents.push(a.clone());
                }
            }
            UsageRow {
                period: r.period,
                agent: r.agent,
                models_used: r.models_used,
                model_breakdowns: r
                    .model_breakdowns
                    .into_iter()
                    .map(|m| UsageModelBreakdown {
                        total_tokens: m.resolved_total_tokens(),
                        model_name: m.model_name,
                        cost: m.cost,
                        input_tokens: m.input_tokens,
                        output_tokens: m.output_tokens,
                        cache_creation_tokens: m.cache_creation_tokens,
                        cache_read_tokens: m.cache_read_tokens,
                    })
                    .collect(),
                input_tokens: r.input_tokens,
                output_tokens: r.output_tokens,
                cache_creation_tokens: r.cache_creation_tokens,
                cache_read_tokens: r.cache_read_tokens,
                total_tokens: r.total_tokens,
                total_cost: r.total_cost,
            }
        })
        .collect();
    agents.sort();

    Ok(UsageReport {
        period_kind: period_kind.to_string(),
        rows,
        totals: UsageTotals {
            input_tokens: raw_totals.input_tokens,
            output_tokens: raw_totals.output_tokens,
            cache_creation_tokens: raw_totals.cache_creation_tokens,
            cache_read_tokens: raw_totals.cache_read_tokens,
            total_tokens: raw_totals.total_tokens,
            total_cost: raw_totals.total_cost,
        },
        agents_detected: agents,
    })
}

/// Run `ccusage <period> --json` and parse the result.
///
/// Returns a clean `Err(String)` if the binary is missing or errors — the
/// caller surfaces it as a friendly "usage reporting unavailable" message
/// (e.g. in local dev where ccusage isn't installed; it ships in the Docker
/// image).
pub async fn fetch_usage(period: &str) -> Result<UsageReport, String> {
    let period_kind = normalize_period(period);
    let program = resolve_ccusage_program()?;
    let mut command = crate::core::cmd::async_cmd(program);
    command.arg(period_kind).arg("--json");
    if let Some(home) = usage_home_for_command(
        std::env::var_os("KRONN_USAGE_HOME"),
        crate::core::env::is_docker(),
    ) {
        command.env("HOME", home);
    }
    if crate::core::env::is_docker() {
        command.env("npm_config_cache", "/tmp/.npm-cache");
    }

    let output = command
        .output()
        .await
        .map_err(|e| format!("ccusage could not be started ({e})"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ccusage failed: {}", stderr.trim()));
    }

    parse_report(period_kind, &output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn normalize_period_whitelists() {
        assert_eq!(normalize_period("daily"), "daily");
        assert_eq!(normalize_period("weekly"), "weekly");
        assert_eq!(normalize_period("monthly"), "monthly");
        // Anything else (incl. injection attempts) → daily.
        assert_eq!(normalize_period("daily; rm -rf /"), "daily");
        assert_eq!(normalize_period(""), "daily");
        assert_eq!(normalize_period("session"), "daily");
    }

    #[test]
    fn parse_real_ccusage_daily_shape() {
        // Trimmed real ccusage `daily --json` payload.
        let json = br#"{
          "daily": [
            {
              "agent": "all",
              "period": "2026-02-23",
              "inputTokens": 7209,
              "outputTokens": 563,
              "cacheCreationTokens": 478207,
              "cacheReadTokens": 3507122,
              "totalTokens": 3993101,
              "totalCost": 5.26,
              "modelsUsed": ["claude-opus-4-6"],
              "modelBreakdowns": [{"modelName": "claude-opus-4-6", "cost": 5.26, "inputTokens": 7209, "outputTokens": 563, "cacheCreationTokens": 478207, "cacheReadTokens": 3507122}],
              "metadata": {"agents": ["claude"]}
            },
            {
              "agent": "codex",
              "period": "2026-02-24",
              "inputTokens": 410524,
              "outputTokens": 23057,
              "cacheCreationTokens": 0,
              "cacheReadTokens": 2162432,
              "totalTokens": 2596013,
              "totalCost": 1.42,
              "modelsUsed": ["gpt-5.2-codex"]
            }
          ],
          "totals": {
            "inputTokens": 417733,
            "outputTokens": 23620,
            "cacheCreationTokens": 478207,
            "cacheReadTokens": 5669554,
            "totalTokens": 6589114,
            "totalCost": 6.68
          }
        }"#;
        let report = parse_report("daily", json).unwrap();
        assert_eq!(report.period_kind, "daily");
        assert_eq!(report.rows.len(), 2);
        assert_eq!(report.rows[0].period, "2026-02-23");
        assert_eq!(report.rows[0].cache_read_tokens, 3507122);
        assert!((report.rows[0].total_cost - 5.26).abs() < 1e-9);
        // modelBreakdowns parsed for agent rollup on the frontend.
        assert_eq!(report.rows[0].model_breakdowns.len(), 1);
        assert_eq!(
            report.rows[0].model_breakdowns[0].model_name,
            "claude-opus-4-6"
        );
        // ccusage ships no per-model `totalTokens`; we sum the 4 components.
        assert_eq!(
            report.rows[0].model_breakdowns[0].total_tokens,
            7209 + 563 + 478207 + 3507122
        );
        assert_eq!(report.rows[1].agent, "codex");
        assert!((report.totals.total_cost - 6.68).abs() < 1e-9);
        // `claude` comes from row 0's metadata.agents (the `all` row),
        // `codex` from row 1's top-level agent. Sorted.
        assert_eq!(
            report.agents_detected,
            vec!["claude".to_string(), "codex".to_string()]
        );
    }

    #[test]
    fn agents_collected_from_metadata_on_all_rows() {
        // ccusage daily rows are `agent: "all"` with the real agents in
        // metadata.agents — agents_detected must surface them so the UI
        // chips aren't empty. Regression guard for the live-smoke finding.
        let json = br#"{
          "daily": [
            {"agent":"all","period":"2026-02-23","metadata":{"agents":["claude"]},"totalCost":5.0,"totalTokens":100},
            {"agent":"all","period":"2026-02-24","metadata":{"agents":["claude","codex"]},"totalCost":2.0,"totalTokens":50}
          ],
          "totals": {"totalCost": 7.0, "totalTokens": 150}
        }"#;
        let report = parse_report("daily", json).unwrap();
        assert_eq!(
            report.agents_detected,
            vec!["claude".to_string(), "codex".to_string()]
        );
    }

    #[test]
    fn parse_handles_empty_and_missing_totals() {
        let report = parse_report("daily", br#"{"daily": []}"#).unwrap();
        assert!(report.rows.is_empty());
        assert_eq!(report.totals.total_cost, 0.0);
        assert!(report.agents_detected.is_empty());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_report("daily", b"not json").is_err());
    }

    #[test]
    fn usage_home_preserves_native_and_container_paths() {
        assert_eq!(
            usage_home_for_command(Some(OsString::from("/custom/log-home")), true),
            Some(OsString::from("/custom/log-home"))
        );
        assert_eq!(
            usage_home_for_command(None, true),
            Some(OsString::from("/host-home"))
        );
        assert_eq!(usage_home_for_command(None, false), None);
    }

    #[test]
    fn package_runner_cache_prefers_the_native_ccusage_binary() {
        let temp = tempfile::tempdir().unwrap();
        let node_modules = temp.path().join("run/node_modules");
        let shim = node_modules.join(".bin/ccusage");
        let native = node_modules.join("@ccusage/ccusage-darwin-arm64/bin/ccusage");
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::write(&shim, "#!/usr/bin/env node\n").unwrap();
        fs::write(&native, "native").unwrap();

        assert_eq!(find_cached_ccusage(temp.path()), Some(native));
    }

    #[test]
    fn package_runner_cache_supports_legacy_js_shims() {
        let temp = tempfile::tempdir().unwrap();
        let shim = temp.path().join("hash/node_modules/.bin/ccusage");
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, "#!/usr/bin/env node\n").unwrap();

        assert_eq!(find_cached_ccusage(temp.path()), Some(shim));
    }
}
