//! 0.8.7 — Agent CLI usage / cost reporting via `ccusage`.
//!
//! Kronn's own `core::pricing` estimates cost from a static table + a guessed
//! 60/40 input/output split, ignoring prompt caching — which massively
//! over-estimates cost on cache-heavy sessions. `ccusage`
//! (https://github.com/ryoppippi/ccusage) reads the CLIs' OWN local JSONL logs
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
    let host_home = std::env::var("KRONN_USAGE_HOME").unwrap_or_else(|_| "/host-home".to_string());

    let output = crate::core::cmd::async_cmd("ccusage")
        .arg(period_kind)
        .arg("--json")
        .env("HOME", &host_home)
        .env("npm_config_cache", "/tmp/.npm-cache")
        .output()
        .await
        .map_err(|e| {
            format!("ccusage not available ({e}). It ships in the Kronn Docker image; in local dev install it with `npm i -g ccusage`.")
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ccusage failed: {}", stderr.trim()));
    }

    parse_report(period_kind, &output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
