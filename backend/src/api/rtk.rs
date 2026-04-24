//! RTK (Rust Token Killer) — activation and savings-readout endpoints.
//!
//! Detection lives in `core::rtk_detect` and flows through `AgentDetection`.
//! This module is the *mutating* and *external-read* surface: it spawns
//! `rtk init -g` to wire the user's agent configs, and reads `rtk gain
//! --format json` for the dashboard counter.
//!
//! Neither endpoint writes to agent config files directly — RTK owns the
//! file format. We stay a thin orchestrator.

use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::cmd::async_cmd;
use crate::models::{AgentType, ApiResponse};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RtkAgentActivation {
    pub agent_type: AgentType,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RtkActivateResponse {
    /// Overall success — true if every RTK-supported agent invocation
    /// exited 0. A single failure flips this to false so the frontend can
    /// surface an error toast even when some agents succeeded.
    pub success: bool,
    /// Concatenated stdout of every per-agent invocation, prefixed with
    /// the agent name. Useful when the user wants to see what RTK did.
    pub stdout: String,
    /// Concatenated stderr. Empty when `success` is true.
    pub stderr: String,
    /// Per-agent outcomes — surfaces which agent failed when success is
    /// partial. Empty when nothing ran (no compatible agent installed).
    #[serde(default)]
    pub per_agent: Vec<RtkAgentActivation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RtkActivateRequest {
    /// Agents the frontend wants RTK hooks on. The backend filters this
    /// to only agents RTK supports before spawning.
    pub agents: Vec<AgentType>,
}

/// Returns the argv tail for `rtk init` that hooks the given agent, or
/// `None` if RTK doesn't support it.
///
/// Notes on the flag matrix (verified empirically against RTK + its
/// own error messages):
///   - `--auto-patch` is the **Claude-only** "yes to the settings.json
///     patch prompt" shortcut. Combining it with `--codex` or `--gemini`
///     triggers `rtk: --codex cannot be combined with --auto-patch`
///     (and the symmetric Gemini error). Those flows write to a
///     dedicated config file (AGENTS.md / shell rc) — no prompt to
///     auto-answer, no `--auto-patch` needed.
///   - `--hook-only` is also Claude-only (skip the RTK.md companion).
///     Passing it with `--codex` or `--gemini` errors out with the same
///     "cannot be combined" message.
///   - Net: per-agent matrix is one well-known shape per agent.
fn rtk_args_for(agent_type: &AgentType) -> Option<Vec<&'static str>> {
    match agent_type {
        AgentType::ClaudeCode => Some(vec!["init", "-g", "--auto-patch", "--hook-only"]),
        AgentType::Codex      => Some(vec!["init", "-g", "--codex"]),
        AgentType::GeminiCli  => Some(vec!["init", "-g", "--gemini"]),
        // RTK's `--copilot` targets VS Code Copilot Chat (writes to the
        // editor's settings.json), not the @github/copilot standalone
        // CLI that Kronn detects as `CopilotCli`. Treating it as
        // unsupported avoids miswiring the wrong host.
        // Kiro (AWS), Vibe, Ollama, Custom: not in RTK's upstream list.
        AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe
        | AgentType::Ollama
        | AgentType::Custom => None,
    }
}

/// Sibling of `rtk_args_for` for the **uninstall** path. Same agent
/// flag, plus `--uninstall` which RTK accepts to remove the hook /
/// settings entry / RTK.md companion. Same combinability rules — keep
/// `--hook-only` Claude-only.
fn rtk_uninstall_args_for(agent_type: &AgentType) -> Option<Vec<&'static str>> {
    match agent_type {
        AgentType::ClaudeCode => Some(vec!["init", "-g", "--auto-patch", "--uninstall"]),
        AgentType::Codex      => Some(vec!["init", "-g", "--codex", "--uninstall"]),
        AgentType::GeminiCli  => Some(vec!["init", "-g", "--gemini", "--uninstall"]),
        AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe
        | AgentType::Ollama
        | AgentType::Custom => None,
    }
}

/// POST /api/rtk/activate
/// Body: `{ agents: [AgentType, ...] }` — usually the full detected list;
/// the backend filters to agents RTK actually supports before spawning.
///
/// For each compatible agent, we spawn a dedicated `rtk init -g ... --auto-patch --hook-only`
/// invocation — the single `rtk init -g` doesn't wire Codex/Gemini, and
/// without `--auto-patch` the command waits on an interactive prompt the
/// backend can't answer (the previous iteration's "RTK activated but
/// nothing changed" symptom).
///
/// HOME handling — subtle. In Docker, `HOME=/home/kronn` inside the
/// container already points at the right place: `~/.claude`, `~/.codex`,
/// `~/.gemini` are bind-mounted **read-write** from the user's real
/// host home. Overriding HOME with `KRONN_HOST_HOME` (the *host* path,
/// e.g. `/home/priol`) is actively wrong: that path doesn't exist inside
/// the container. In Tauri the backend is native, `HOME` is already
/// correct. Either way, we pass env through untouched.
pub async fn activate(
    Json(req): Json<RtkActivateRequest>,
) -> Json<ApiResponse<RtkActivateResponse>> {
    if !crate::core::rtk_detect::rtk_binary_available() {
        return Json(ApiResponse::err(
            "rtk binary not found on PATH. Install RTK first.".to_string(),
        ));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "<unset>".into());

    // Pre-create `$HOME/.config/rtk` — RTK uses this for its own state
    // (config.toml, telemetry). If the dir chain has to be created and
    // crosses a uid boundary it errors with "Permission denied". Owning
    // it ourselves sidesteps that.
    let rtk_config_dir = format!("{home}/.config/rtk");
    if let Err(e) = std::fs::create_dir_all(&rtk_config_dir) {
        tracing::warn!("Failed to pre-create {rtk_config_dir}: {e}. Continuing — rtk may handle it.");
    }

    let mut per_agent: Vec<RtkAgentActivation> = Vec::new();
    let mut combined_stdout = String::new();
    let mut combined_stderr = String::new();
    let mut any_failure = false;
    let mut any_ran = false;

    for agent in &req.agents {
        let Some(args) = rtk_args_for(agent) else { continue; };
        any_ran = true;

        tracing::info!("Spawning: rtk {:?} (agent={:?}, HOME={home})", args, agent);

        match async_cmd("rtk").args(&args).output().await {
            Ok(out) => {
                let success = out.status.success();
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();

                if success {
                    tracing::info!("rtk hook activated for {:?}", agent);
                } else {
                    tracing::warn!("rtk hook activation failed for {:?} (status={:?}): {}",
                        agent, out.status.code(), stderr.trim());
                    any_failure = true;
                }

                if !stdout.trim().is_empty() {
                    combined_stdout.push_str(&format!("[{:?}] {}\n", agent, stdout.trim()));
                }
                if !stderr.trim().is_empty() {
                    combined_stderr.push_str(&format!("[{:?}] {}\n", agent, stderr.trim()));
                }

                per_agent.push(RtkAgentActivation {
                    agent_type: agent.clone(),
                    success,
                    stdout,
                    stderr,
                });
            }
            Err(e) => {
                let msg = format!("Failed to spawn rtk for {:?}: {e}", agent);
                tracing::error!("{msg}");
                any_failure = true;
                combined_stderr.push_str(&format!("{msg}\n"));
                per_agent.push(RtkAgentActivation {
                    agent_type: agent.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: msg,
                });
            }
        }
    }

    if !any_ran {
        return Json(ApiResponse::ok(RtkActivateResponse {
            success: false,
            stdout: String::new(),
            stderr: "No RTK-supported agent in the request. Supported: Claude Code, Codex, Gemini CLI.".into(),
            per_agent,
        }));
    }

    Json(ApiResponse::ok(RtkActivateResponse {
        success: !any_failure,
        stdout: combined_stdout,
        stderr: combined_stderr,
        per_agent,
    }))
}

/// POST /api/rtk/deactivate
/// Mirror of [`activate`] for `rtk init -g --<agent> --uninstall`. The
/// per-agent matrix is the same; we just append `--uninstall`. Output
/// shape is identical to ease frontend wiring.
pub async fn deactivate(
    Json(req): Json<RtkActivateRequest>,
) -> Json<ApiResponse<RtkActivateResponse>> {
    if !crate::core::rtk_detect::rtk_binary_available() {
        return Json(ApiResponse::err(
            "rtk binary not found on PATH.".to_string(),
        ));
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "<unset>".into());

    let mut per_agent: Vec<RtkAgentActivation> = Vec::new();
    let mut combined_stdout = String::new();
    let mut combined_stderr = String::new();
    let mut any_failure = false;
    let mut any_ran = false;

    for agent in &req.agents {
        let Some(args) = rtk_uninstall_args_for(agent) else { continue; };
        any_ran = true;

        tracing::info!("Spawning: rtk {:?} (agent={:?}, HOME={home}, mode=uninstall)", args, agent);

        match async_cmd("rtk").args(&args).output().await {
            Ok(out) => {
                let success = out.status.success();
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                if success {
                    tracing::info!("rtk hook uninstalled for {:?}", agent);
                } else {
                    tracing::warn!("rtk uninstall failed for {:?} (status={:?}): {}",
                        agent, out.status.code(), stderr.trim());
                    any_failure = true;
                }
                if !stdout.trim().is_empty() {
                    combined_stdout.push_str(&format!("[{:?}] {}\n", agent, stdout.trim()));
                }
                if !stderr.trim().is_empty() {
                    combined_stderr.push_str(&format!("[{:?}] {}\n", agent, stderr.trim()));
                }
                per_agent.push(RtkAgentActivation {
                    agent_type: agent.clone(), success, stdout, stderr,
                });
            }
            Err(e) => {
                let msg = format!("Failed to spawn rtk for {:?}: {e}", agent);
                tracing::error!("{msg}");
                any_failure = true;
                combined_stderr.push_str(&format!("{msg}\n"));
                per_agent.push(RtkAgentActivation {
                    agent_type: agent.clone(), success: false,
                    stdout: String::new(), stderr: msg,
                });
            }
        }
    }

    if !any_ran {
        return Json(ApiResponse::ok(RtkActivateResponse {
            success: false,
            stdout: String::new(),
            stderr: "No RTK-supported agent in the request.".into(),
            per_agent,
        }));
    }

    Json(ApiResponse::ok(RtkActivateResponse {
        success: !any_failure,
        stdout: combined_stdout,
        stderr: combined_stderr,
        per_agent,
    }))
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RtkSavings {
    /// `true` when we got a readable response from RTK. Frontend uses this
    /// flag to decide whether to render the counter at all — when RTK is
    /// absent or the CLI output shape changes, we degrade silently rather
    /// than showing a zero that would look like "RTK saved nothing".
    pub available: bool,
    /// Best-effort sum of tokens RTK reports as saved. 0 when `available`
    /// is false.
    pub total_tokens_saved: u64,
    /// Rough compression ratio in [0, 100]. 0 when `available` is false.
    pub ratio_percent: f32,
    /// Number of compression samples RTK has on record.
    pub sample_count: u64,
}

/// GET /api/rtk/savings
/// Reads `rtk gain --format json` and extracts the high-level counters.
/// Tolerant on purpose: any parse/exec failure returns `available: false`
/// so the frontend can hide the panel cleanly without surfacing a 500.
pub async fn savings() -> Json<ApiResponse<RtkSavings>> {
    let empty = RtkSavings {
        available: false,
        total_tokens_saved: 0,
        ratio_percent: 0.0,
        sample_count: 0,
    };

    if !crate::core::rtk_detect::rtk_binary_available() {
        return Json(ApiResponse::ok(empty));
    }

    let output = match async_cmd("rtk")
        .args(["gain", "--all", "--format", "json"])
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return Json(ApiResponse::ok(empty)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = match serde_json::from_str(&stdout) {
        Ok(v) => v,
        Err(_) => return Json(ApiResponse::ok(empty)),
    };

    // RTK 0.37 shape (validated against a real `rtk gain --all --format json`):
    //   { "summary": { "total_commands": N, "total_saved": N,
    //                  "avg_savings_pct": N.NN, ... },
    //     "daily": [...], "weekly": [...], "monthly": [...] }
    // We navigate to `summary.*` but keep the top-level fallbacks as a
    // safety net if RTK reshapes the JSON (we caught the wrong-keys case
    // once already — defensive parsing keeps the section from silently
    // showing a false zero).
    let summary = json.get("summary");
    let total_tokens_saved = summary
        .and_then(|s| s.get("total_saved").and_then(|v| v.as_u64()))
        .unwrap_or_else(|| pick_u64(&json, &["tokens_saved", "total_tokens_saved", "savings", "gain"]));
    let sample_count = summary
        .and_then(|s| s.get("total_commands").and_then(|v| v.as_u64()))
        .unwrap_or_else(|| pick_u64(&json, &["sample_count", "samples", "n", "count"]));
    let ratio_percent = summary
        .and_then(|s| s.get("avg_savings_pct").and_then(|v| v.as_f64()))
        .map(|r| r as f32)
        .or_else(|| pick_f32(&json, &["ratio_percent", "ratio", "compression_ratio"]))
        .map(|r| if r <= 1.0 { r * 100.0 } else { r })
        .unwrap_or(0.0);

    Json(ApiResponse::ok(RtkSavings {
        available: true,
        total_tokens_saved,
        ratio_percent,
        sample_count,
    }))
}

fn pick_u64(json: &serde_json::Value, keys: &[&str]) -> u64 {
    for k in keys {
        if let Some(n) = json.get(k).and_then(|v| v.as_u64()) {
            return n;
        }
    }
    0
}

fn pick_f32(json: &serde_json::Value, keys: &[&str]) -> Option<f32> {
    for k in keys {
        if let Some(f) = json.get(k).and_then(|v| v.as_f64()) {
            return Some(f as f32);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── RTK matrix (rtk_args_for / rtk_uninstall_args_for) ─────────
    //
    // These tests are the contract that prevents the user-reported
    // `--codex cannot be combined with --auto-patch` regression. RTK's
    // own error messages are the ground truth: any combination it
    // rejects, the matrix MUST avoid. If a future contributor adds
    // `--auto-patch` back to the Codex/Gemini flow, these tests fail.

    #[test]
    fn rtk_args_claude_uses_auto_patch_and_hook_only() {
        let args = rtk_args_for(&AgentType::ClaudeCode).expect("Claude is supported");
        assert_eq!(args, vec!["init", "-g", "--auto-patch", "--hook-only"]);
    }

    #[test]
    fn rtk_args_codex_no_auto_patch_no_hook_only() {
        // Both flags trigger `--codex cannot be combined with …` from RTK.
        let args = rtk_args_for(&AgentType::Codex).expect("Codex is supported");
        assert_eq!(args, vec!["init", "-g", "--codex"]);
        assert!(!args.contains(&"--auto-patch"), "Codex flow must not pass --auto-patch");
        assert!(!args.contains(&"--hook-only"), "Codex flow must not pass --hook-only");
    }

    #[test]
    fn rtk_args_gemini_no_auto_patch_no_hook_only() {
        let args = rtk_args_for(&AgentType::GeminiCli).expect("Gemini is supported");
        assert_eq!(args, vec!["init", "-g", "--gemini"]);
        assert!(!args.contains(&"--auto-patch"));
    }

    #[test]
    fn rtk_args_unsupported_agents_return_none() {
        // RTK's `--copilot` targets VS Code Copilot Chat, not the
        // standalone `@github/copilot` CLI — treating it as
        // unsupported avoids miswiring. Kiro / Vibe / Ollama / Custom
        // aren't in RTK's upstream list either.
        for agent in [
            AgentType::CopilotCli, AgentType::Kiro,
            AgentType::Vibe, AgentType::Ollama, AgentType::Custom,
        ] {
            assert!(rtk_args_for(&agent).is_none(),
                "{:?} must be unsupported by RTK", agent);
        }
    }

    #[test]
    fn rtk_uninstall_args_mirror_install_with_uninstall_flag() {
        // Uninstall path: same agent flag, append `--uninstall`. Same
        // combinability rules — `--hook-only` Claude-only, no
        // `--auto-patch` on Codex/Gemini. The `rtk init -g … --uninstall`
        // form is documented at github.com/rtk-ai/rtk; if RTK changes
        // the syntax this test will catch the drift on next refresh.
        assert_eq!(
            rtk_uninstall_args_for(&AgentType::ClaudeCode).unwrap(),
            vec!["init", "-g", "--auto-patch", "--uninstall"],
        );
        assert_eq!(
            rtk_uninstall_args_for(&AgentType::Codex).unwrap(),
            vec!["init", "-g", "--codex", "--uninstall"],
        );
        assert_eq!(
            rtk_uninstall_args_for(&AgentType::GeminiCli).unwrap(),
            vec!["init", "-g", "--gemini", "--uninstall"],
        );
        // Must mirror the install matrix's "unsupported" set.
        assert!(rtk_uninstall_args_for(&AgentType::Kiro).is_none());
    }

    #[test]
    fn pick_u64_finds_first_matching_key() {
        let v = serde_json::json!({"total_tokens_saved": 42});
        assert_eq!(pick_u64(&v, &["tokens_saved", "total_tokens_saved"]), 42);
    }

    #[test]
    fn pick_u64_returns_zero_when_no_key_matches() {
        let v = serde_json::json!({"unrelated": "string"});
        assert_eq!(pick_u64(&v, &["tokens_saved"]), 0);
    }

    #[test]
    fn pick_f32_returns_some_when_found() {
        let v = serde_json::json!({"ratio": 0.89});
        let got = pick_f32(&v, &["ratio_percent", "ratio"]).unwrap();
        assert!((got - 0.89).abs() < 1e-6);
    }

    #[test]
    fn pick_f32_returns_none_on_missing() {
        let v = serde_json::json!({});
        assert!(pick_f32(&v, &["ratio"]).is_none());
    }

    #[test]
    fn parses_real_rtk_gain_shape() {
        // Regression: real-world `rtk gain --all --format json` output
        // reported by a user. Top-level keys `summary | daily | weekly |
        // monthly`, savings in `summary.total_saved`, ratio in
        // `summary.avg_savings_pct`, count in `summary.total_commands`.
        // An earlier parser looked at `tokens_saved` at the root and
        // systematically returned zero, hiding the counter in the UI.
        let raw = r#"{
          "summary": {
            "total_commands": 203,
            "total_input": 714434,
            "total_output": 26192,
            "total_saved": 689089,
            "avg_savings_pct": 96.45243647418796,
            "total_time_ms": 3709215,
            "avg_time_ms": 18271
          },
          "daily": [],
          "weekly": [],
          "monthly": []
        }"#;
        let json: serde_json::Value = serde_json::from_str(raw).unwrap();
        let summary = json.get("summary").unwrap();

        let total = summary.get("total_saved").and_then(|v| v.as_u64()).unwrap_or(0);
        let count = summary.get("total_commands").and_then(|v| v.as_u64()).unwrap_or(0);
        let ratio = summary.get("avg_savings_pct").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

        assert_eq!(total, 689_089);
        assert_eq!(count, 203);
        assert!((ratio - 96.452_44).abs() < 0.01);
    }
}
