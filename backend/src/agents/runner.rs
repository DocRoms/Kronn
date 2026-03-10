use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::models::{AgentType, TokensConfig};

/// Output mode — how to interpret stdout from the agent
#[derive(Clone, Copy, PartialEq)]
pub enum OutputMode {
    /// Each line is plain text (default for most agents)
    Text,
    /// Each line is a JSON event (Claude Code --output-format stream-json)
    StreamJson,
}

/// Result of parsing a single stream-json line
pub enum StreamJsonEvent {
    /// A text chunk to stream to the user
    Text(String),
    /// Token usage from a message_delta event (input_tokens, output_tokens)
    Usage { input_tokens: u64, output_tokens: u64 },
    /// Nothing useful (metadata, start/stop events, etc.)
    Skip,
}

/// How to handle stderr from an agent process
#[derive(Clone, Copy)]
enum StderrMode {
    /// Merge stderr into output stream (default — agent puts useful output on both)
    Merge,
    /// Only use stdout; log stderr but don't stream it (agent puts noise on stderr)
    /// Stderr is still captured so it can be shown on failure.
    StdoutOnly,
}

/// Running agent process with streaming output
pub struct AgentProcess {
    pub child: tokio::process::Child,
    pub output_mode: OutputMode,
    rx: mpsc::Receiver<String>,
    stderr_capture: Arc<Mutex<Vec<String>>>,
}

impl AgentProcess {
    pub async fn next_line(&mut self) -> Option<String> {
        self.rx.recv().await
    }

    /// Return captured stderr lines (only populated in StdoutOnly mode)
    pub fn captured_stderr(&self) -> Vec<String> {
        self.stderr_capture.lock().unwrap().clone()
    }
}

/// Start an agent process for the given agent type and prompt.
/// Uses the local CLI agent's own authentication by default.
/// API keys from config are passed as optional overrides only.
/// Reads MCP context files from the project and injects them into the agent prompt.
pub async fn start_agent(
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
    tokens: &TokensConfig,
    full_access: bool,
) -> Result<AgentProcess, String> {
    // Read MCP context files for this project (if any)
    let mcp_context = if !project_path.is_empty() {
        crate::core::mcp_scanner::read_all_mcp_contexts(project_path)
    } else {
        String::new()
    };

    let (binary, npx_pkg, args, env_key, stderr_mode, output_mode) =
        agent_command(agent_type, prompt, full_access, &mcp_context);

    let work_dir = if project_path.is_empty() {
        // Global discussion: use a temp working directory
        std::env::temp_dir()
    } else {
        let container_path = crate::core::scanner::resolve_host_path(project_path);
        if container_path.exists() {
            container_path
        } else {
            let p = PathBuf::from(project_path);
            if !p.exists() {
                return Err(format!("Project path not found: {}", p.display()));
            }
            p
        }
    };

    // API key is optional — agents use their own local auth by default
    let api_key = get_api_key(env_key, tokens);

    // Try direct binary first, then npx fallback
    let mut child = match try_spawn(binary, None, &args, &work_dir, env_key, api_key.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            tracing::info!("Direct binary '{}' failed ({}), trying npx...", binary, e);
            if let Some(pkg) = npx_pkg {
                try_spawn("npx", Some(pkg), &args, &work_dir, env_key, api_key.as_deref())?
            } else {
                return Err(e);
            }
        }
    };

    let (tx, rx) = mpsc::channel::<String>(256);
    let stderr_capture = Arc::new(Mutex::new(Vec::new()));

    // Always stream stdout
    if let Some(stdout) = child.stdout.take() {
        let tx_out = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if tx_out.send(line).await.is_err() { break; }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        match stderr_mode {
            StderrMode::Merge => {
                let tx_err = tx;
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if tx_err.send(line).await.is_err() { break; }
                    }
                });
            }
            StderrMode::StdoutOnly => {
                // Log stderr for debugging but don't stream it to the user.
                // Capture it so we can show it on failure.
                let capture = stderr_capture.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!("agent stderr: {}", line);
                        if let Ok(mut buf) = capture.lock() {
                            buf.push(line);
                        }
                    }
                });
            }
        }
    }

    Ok(AgentProcess { child, output_mode, rx, stderr_capture })
}

/// Get the command configuration for an agent type.
/// MCP context is injected via --append-system-prompt for Claude Code,
/// or prepended to the prompt for other agents.
/// Returns: (binary, npx_package, args, env_key, stderr_mode, output_mode)
fn agent_command(agent_type: &AgentType, prompt: &str, full_access: bool, mcp_context: &str) -> (&'static str, Option<&'static str>, Vec<String>, &'static str, StderrMode, OutputMode) {
    match agent_type {
        AgentType::ClaudeCode => {
            let mut args = vec![
                "--print".into(),
                "--output-format".into(), "stream-json".into(),
                "--verbose".into(),
                "--include-partial-messages".into(),
            ];
            if full_access {
                args.push("--dangerously-skip-permissions".into());
            }
            // Inject MCP context via --append-system-prompt (separate from user prompt)
            if !mcp_context.is_empty() {
                args.push("--append-system-prompt".into());
                args.push(mcp_context.into());
            }
            args.push(prompt.into());
            (
                "claude",
                Some("@anthropic-ai/claude-code"),
                args,
                "ANTHROPIC_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::StreamJson,
            )
        },
        AgentType::Codex => {
            let mut args: Vec<String> = vec!["exec".into()];
            if full_access {
                args.push("--full-auto".into());
            }
            // Codex requires a trusted git directory by default.
            // Inside Docker the paths are mapped, so skip the check.
            args.push("--skip-git-repo-check".into());
            // Codex has no system prompt flag — prepend context to the prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push(full_prompt);
            (
                "codex",
                Some("@openai/codex"),
                args,
                "OPENAI_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        },
        AgentType::Vibe => {
            // Vibe has no system prompt flag — prepend context to the prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            (
                "uvx",
                None,
                vec!["--from".into(), "mistral-vibe".into(), "vibe".into(),
                     "-p".into(), full_prompt, "--output".into(), "text".into()],
                "MISTRAL_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        },
        AgentType::GeminiCli => {
            let mut args: Vec<String> = vec!["-p".into()];
            if full_access {
                args.push("--yolo".into());
            }
            // Gemini CLI has no system prompt flag — prepend context to prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push(full_prompt);
            (
                "gemini",
                Some("@google/gemini-cli"),
                args,
                "GEMINI_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        },
        AgentType::Custom => (
            "echo",
            None,
            vec!["Custom agent not configured".into()],
            "NONE",
            StderrMode::Merge,
            OutputMode::Text,
        ),
    }
}

/// Spawn an agent process. If npx_package is Some, uses npx to run.
fn try_spawn(
    binary: &str,
    npx_package: Option<&str>,
    args: &[String],
    work_dir: &Path,
    env_key: &str,
    api_key: Option<&str>,
) -> Result<tokio::process::Child, String> {
    let (cmd_name, cmd_args) = if let Some(pkg) = npx_package {
        let mut npx_args = vec!["--yes".to_string(), pkg.to_string()];
        npx_args.extend_from_slice(args);
        ("npx".to_string(), npx_args)
    } else {
        let bin_loc = super::find_binary(binary)
            .ok_or_else(|| format!("Binary '{}' not found", binary))?;
        (bin_loc.path, args.to_vec())
    };

    tracing::info!("Spawning agent: {} {:?} in {} (key: {})",
        cmd_name, cmd_args, work_dir.display(),
        if api_key.is_some() { "override" } else { "local auth" }
    );

    let mut cmd = Command::new(&cmd_name);
    cmd.args(&cmd_args)
        .current_dir(work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Tell Claude Code we're in a containerized environment.
    // This bypasses the root/sudo check for --dangerously-skip-permissions.
    // Note: use CLAUDE_CODE_BUBBLEWRAP, not IS_SANDBOX — IS_SANDBOX also
    // suppresses 529 overloaded errors causing infinite silent retries.
    cmd.env("CLAUDE_CODE_BUBBLEWRAP", "1");

    // Only set API key env var if explicitly configured (override)
    // Otherwise let the agent use its own local auth
    if let Some(key) = api_key {
        cmd.env(env_key, key);
    }

    cmd.spawn()
        .map_err(|e| format!("Spawn failed for {}: {}", cmd_name, e))
}

/// Parse a single line from Claude Code's --output-format stream-json output.
///
/// With `--verbose --include-partial-messages`, stream-json emits wrapped Anthropic API events:
/// ```json
/// {"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}}
/// ```
///
/// The final result line contains cost/token info:
/// ```json
/// {"type":"result","subtype":"success","cost_usd":0.01,"duration_ms":1234,"session_id":"...","usage":{"input_tokens":100,"output_tokens":50}}
/// ```
pub fn parse_claude_stream_line(line: &str) -> StreamJsonEvent {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return StreamJsonEvent::Skip;
    }

    let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        // Not valid JSON — pass through as plain text
        return StreamJsonEvent::Text(line.to_string());
    };

    let event_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        // Wrapped Anthropic streaming events
        "stream_event" => {
            let Some(event) = json.get("event") else { return StreamJsonEvent::Skip };

            // Text delta: event.delta.type == "text_delta"
            if let Some(delta) = event.get("delta") {
                if delta.get("type").and_then(|v| v.as_str()) == Some("text_delta") {
                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                        return StreamJsonEvent::Text(text.to_string());
                    }
                }
            }

            // message_delta may carry usage
            if event.get("type").and_then(|v| v.as_str()) == Some("message_delta") {
                if let Some(usage) = event.get("usage") {
                    let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                    if input > 0 || output > 0 {
                        return StreamJsonEvent::Usage { input_tokens: input, output_tokens: output };
                    }
                }
            }

            StreamJsonEvent::Skip
        }

        // Final result line — contains token usage and cost
        "result" => {
            // Try usage field first
            if let Some(usage) = json.get("usage") {
                let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                if input > 0 || output > 0 {
                    return StreamJsonEvent::Usage { input_tokens: input, output_tokens: output };
                }
            }
            StreamJsonEvent::Skip
        }

        // "assistant" messages with --include-partial-messages are cumulative snapshots
        // (they contain the full text so far, not a delta). We skip them to avoid
        // duplicating text already received via stream_event deltas.
        "assistant" => StreamJsonEvent::Skip,

        // Everything else (system, init, etc.)
        _ => StreamJsonEvent::Skip,
    }
}

/// Parse token usage from agent output.
/// Codex outputs "tokens used\nN,NNN" on stderr (StdoutOnly mode captures it).
/// Returns (cleaned_response, tokens_used) — token lines are stripped if found in response.
pub fn parse_token_usage(agent_type: &AgentType, response: &str, stderr_lines: &[String]) -> (String, u64) {
    match agent_type {
        AgentType::Codex => {
            // Codex outputs "tokens used" then the count on stderr
            // Check stderr first (primary source)
            if stderr_lines.len() >= 2 {
                let last = stderr_lines[stderr_lines.len() - 1].trim();
                let second_last = stderr_lines[stderr_lines.len() - 2].trim();
                if second_last == "tokens used" {
                    let count_str = last.replace(',', "").replace('.', "");
                    if let Ok(count) = count_str.parse::<u64>() {
                        return (response.to_string(), count);
                    }
                }
            }
            // Fallback: check stdout (some versions may put it there)
            let lines: Vec<&str> = response.lines().collect();
            if lines.len() >= 2 {
                let last = lines[lines.len() - 1].trim();
                let second_last = lines[lines.len() - 2].trim();
                if second_last == "tokens used" {
                    let count_str = last.replace(',', "").replace('.', "");
                    if let Ok(count) = count_str.parse::<u64>() {
                        let clean = lines[..lines.len() - 2].join("\n");
                        return (clean, count);
                    }
                }
            }
            (response.to_string(), 0)
        }
        // Claude Code: tokens parsed inline via parse_claude_stream_line() in discussions.rs
        // TODO: Gemini CLI, Vibe — not yet supported
        _ => (response.to_string(), 0),
    }
}

fn get_api_key(env_key: &str, tokens: &TokensConfig) -> Option<String> {
    let provider = match env_key {
        "ANTHROPIC_API_KEY" => "anthropic",
        "OPENAI_API_KEY" => "openai",
        "GEMINI_API_KEY" => "google",
        _ => return None,
    };

    // If override is disabled for this provider, fall back to env var
    if tokens.disabled_overrides.iter().any(|d| d == provider) {
        return std::env::var(env_key).ok();
    }

    // Use active key from multi-key system
    tokens.active_key_for(provider)
        .map(|s| s.to_string())
        .or_else(|| std::env::var(env_key).ok())
}
