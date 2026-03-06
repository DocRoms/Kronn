use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::models::{AgentType, TokensConfig};

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

    let (binary, npx_pkg, args, env_key, stderr_mode) =
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

    Ok(AgentProcess { child, rx, stderr_capture })
}

/// Get the command configuration for an agent type.
/// MCP context is injected via --append-system-prompt for Claude Code,
/// or prepended to the prompt for other agents.
/// Returns: (binary, npx_package, args, env_key, stderr_mode)
fn agent_command(agent_type: &AgentType, prompt: &str, full_access: bool, mcp_context: &str) -> (&'static str, Option<&'static str>, Vec<String>, &'static str, StderrMode) {
    match agent_type {
        AgentType::ClaudeCode => {
            let mut args = vec!["--print".into()];
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
                StderrMode::Merge,
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
            )
        },
        AgentType::Custom => (
            "echo",
            None,
            vec!["Custom agent not configured".into()],
            "NONE",
            StderrMode::Merge,
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
        let bin_path = super::find_binary(binary)
            .ok_or_else(|| format!("Binary '{}' not found", binary))?;
        (bin_path, args.to_vec())
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

fn get_api_key(env_key: &str, tokens: &TokensConfig) -> Option<String> {
    let from_config = match env_key {
        "ANTHROPIC_API_KEY" => tokens.anthropic.clone(),
        "OPENAI_API_KEY" => tokens.openai.clone(),
        _ => None,
    };
    from_config.or_else(|| std::env::var(env_key).ok())
}
