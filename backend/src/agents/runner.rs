use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::core::cmd::{async_cmd, sync_cmd};
use crate::models::{AgentType, ModelTier, ModelTiersConfig, TokensConfig};

/// Detect if we're running inside WSL (vs Windows native).
/// In WSL, /proc/version contains "microsoft" or "WSL".
#[allow(dead_code)]
fn is_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        // WSL2 always sets WSL_DISTRO_NAME — most reliable check
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            return true;
        }
        std::fs::read_to_string("/proc/version")
            .map(|v| v.contains("microsoft") || v.contains("WSL"))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Convert a Windows path (C:\Users\...) to WSL path (/mnt/c/Users/...).
#[cfg(target_os = "windows")]
fn windows_to_wsl_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        // Extended-length path
        convert_drive_path(rest)
    } else if s.len() >= 3 && s.as_bytes()[1] == b':' {
        convert_drive_path(&s)
    } else {
        path.to_path_buf()
    }
}

#[cfg(target_os = "windows")]
fn convert_drive_path(s: &str) -> PathBuf {
    let drive = s.chars().next().unwrap().to_lowercase().next().unwrap();
    let rest = s[2..].replace('\\', "/");
    PathBuf::from(format!("/mnt/{}{}", drive, rest))
}

/// Output mode — how to interpret stdout from the agent
#[derive(Clone, Copy, PartialEq)]
pub enum OutputMode {
    /// Each line is plain text (default for most agents)
    Text,
    /// Each line is a JSON event (Claude Code --output-format stream-json)
    StreamJson,
}

/// Result of parsing a single stream-json line
#[derive(Debug)]
pub enum StreamJsonEvent {
    /// A text chunk to stream to the user
    Text(String),
    /// Token usage from a message_delta event (input_tokens, output_tokens, optional cost)
    Usage { input_tokens: u64, output_tokens: u64, cost_usd: Option<f64> },
    /// Tool use started — name of the tool
    ToolStart(String),
    /// Partial JSON input for the current tool (accumulated to build full input)
    ToolInputDelta(String),
    /// Content block finished (tool input complete)
    ToolEnd,
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
    pub work_dir: PathBuf,
    agent_type: AgentType,
    rx: mpsc::Receiver<String>,
    pub stderr_capture: Arc<Mutex<Vec<String>>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

impl AgentProcess {
    /// Get next output line. For Kiro, strips ANSI codes and filters noise.
    pub async fn next_line(&mut self) -> Option<String> {
        loop {
            let line = self.rx.recv().await?;
            if self.agent_type == AgentType::Kiro {
                if let Some(cleaned) = clean_kiro_line(&line) {
                    return Some(cleaned);
                }
                // Filtered noise line — try next
                continue;
            }
            return Some(line);
        }
    }

    /// Wait for stderr reader to finish, then return captured lines.
    /// Must be called after `child.wait()` to ensure all stderr is flushed.
    pub async fn captured_stderr_flushed(&mut self) -> Vec<String> {
        if let Some(handle) = self.stderr_task.take() {
            // Give stderr reader a brief window to finish after process exit
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                handle,
            ).await;
        }
        self.stderr_capture.lock().unwrap().clone()
    }

    /// Return captured stderr lines (only populated in StdoutOnly mode)
    /// Note: may be incomplete if called before process exit. Prefer `captured_stderr_flushed`.
    pub fn captured_stderr(&self) -> Vec<String> {
        self.stderr_capture.lock().unwrap().clone()
    }

    /// Fix file ownership after agent execution.
    /// Files created by agents may have wrong ownership if container UID differs from host UID.
    pub fn fix_ownership(&self) {
        fix_file_ownership(&self.work_dir);
    }
}

/// Fix file ownership after agent execution or file operations.
/// Files created in Docker may have wrong ownership if container UID differs from host UID.
/// On macOS with VirtioFS, chown is silently ignored by the filesystem driver.
pub fn fix_file_ownership(work_dir: &Path) {
    // Only relevant in Docker — native apps own their own files
    if !crate::core::env::is_docker() {
        return;
    }
    let uid = std::env::var("KRONN_HOST_UID").unwrap_or_default();
    let gid = std::env::var("KRONN_HOST_GID").unwrap_or_default();
    if uid.is_empty() || gid.is_empty() {
        return;
    }

    // Skip if container user already matches the desired UID (expected when
    // APP_UID build arg matches KRONN_HOST_UID — the normal case after the fix).
    if let Ok(output) = sync_cmd("id").arg("-u")
        .stdout(Stdio::piped()).stderr(Stdio::null()).output()
    {
        let current_uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if current_uid == uid {
            return; // Already correct UID, no chown needed
        }
    }

    let ownership = format!("{}:{}", uid, gid);
    // Only fix files in the work directory, not system files
    let status = sync_cmd("chown")
        .args(["-R", &ownership])
        .arg(work_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if let Ok(s) = status {
        if !s.success() {
            tracing::debug!(
                "chown failed (exit {}), likely non-root container or VirtioFS — skipping",
                s.code().unwrap_or(-1)
            );
        }
    }
}

/// Configuration for starting an agent process.
pub struct AgentStartConfig<'a> {
    pub agent_type: &'a AgentType,
    /// Used to read .mcp.json and resolve MCP context.
    pub project_path: &'a str,
    /// Working directory for the agent. If `None`, defaults to `project_path`.
    pub work_dir: Option<&'a str>,
    pub prompt: &'a str,
    pub tokens: &'a TokensConfig,
    pub full_access: bool,
    pub skill_ids: &'a [String],
    pub directive_ids: &'a [String],
    pub profile_ids: &'a [String],
    /// Override MCP context instead of reading from project filesystem.
    /// Used for general discussions to inject global MCP configs.
    pub mcp_context_override: Option<&'a str>,
    /// Model capability tier. Resolved to a --model flag per agent.
    /// Priority: explicit model string > tier > Default (no flag).
    pub tier: ModelTier,
    /// Per-agent model tier config (from global settings). Used to resolve tier to model name.
    pub model_tiers: Option<&'a ModelTiersConfig>,
    /// Pre-built context files prompt (uploaded file contents for this discussion).
    pub context_files_prompt: &'a str,
}

/// Resolve a ModelTier to a concrete --model flag value for a given agent.
/// Returns None for Default tier or agents without --model support.
pub(crate) fn resolve_model_flag(agent_type: &AgentType, tier: ModelTier, overrides: Option<&ModelTiersConfig>) -> Option<String> {
    // Check user overrides first (all tiers including Default)
    if let Some(cfg) = overrides {
        let agent_cfg = match agent_type {
            AgentType::ClaudeCode => &cfg.claude_code,
            AgentType::Codex => &cfg.codex,
            AgentType::GeminiCli => &cfg.gemini_cli,
            AgentType::Kiro => &cfg.kiro,
            AgentType::Vibe => &cfg.vibe,
            AgentType::CopilotCli => &cfg.copilot_cli,
            AgentType::Ollama => &cfg.ollama,
            AgentType::Custom => return None,
        };
        let override_val = match tier {
            ModelTier::Economy => &agent_cfg.economy,
            ModelTier::Reasoning => &agent_cfg.reasoning,
            ModelTier::Default => &None, // Default uses built-in below
        };
        if let Some(ref val) = override_val {
            if !val.is_empty() {
                return Some(val.clone());
            }
        }
    }

    // Built-in defaults — explicit model for each tier so tiers are always distinct.
    // Default maps to the "standard" model, not "no flag" (which depends on user subscription).
    match (agent_type, tier) {
        (AgentType::ClaudeCode, ModelTier::Economy)  => Some("haiku".into()),
        (AgentType::ClaudeCode, ModelTier::Default)   => Some("sonnet".into()),
        (AgentType::ClaudeCode, ModelTier::Reasoning) => Some("opus".into()),
        (AgentType::Codex, ModelTier::Economy)        => Some("gpt-5-codex-mini".into()),
        (AgentType::Codex, ModelTier::Default)        => None, // Codex default is fine
        (AgentType::Codex, ModelTier::Reasoning)      => Some("gpt-5.4".into()),
        (AgentType::GeminiCli, ModelTier::Economy)    => Some("gemini-2.5-flash".into()),
        (AgentType::GeminiCli, ModelTier::Default)    => None, // Gemini default is fine
        (AgentType::GeminiCli, ModelTier::Reasoning)  => Some("gemini-3.1-pro-preview".into()),
        (AgentType::CopilotCli, ModelTier::Economy)   => Some("gpt-4o-mini".into()),
        (AgentType::CopilotCli, ModelTier::Default)    => None, // Copilot default is fine
        (AgentType::CopilotCli, ModelTier::Reasoning)  => Some("o4-mini".into()),
        // Ollama: user picks model explicitly via model_flag; return as-is
        (AgentType::Ollama, ModelTier::Default)        => Some("llama3.2".into()),
        (AgentType::Ollama, ModelTier::Economy)        => Some("llama3.2".into()),
        (AgentType::Ollama, ModelTier::Reasoning)      => Some("qwen3".into()),
        // Kiro, Vibe: no --model flag support
        _ => None,
    }
}

/// Start an agent process with minimal config (no skills/directives/profiles).
pub async fn start_agent(
    agent_type: &AgentType,
    project_path: &str,
    prompt: &str,
    tokens: &TokensConfig,
    full_access: bool,
) -> Result<AgentProcess, String> {
    start_agent_with_config(AgentStartConfig {
        agent_type, project_path, work_dir: None, prompt, tokens, full_access,
        skill_ids: &[], directive_ids: &[], profile_ids: &[],
        mcp_context_override: None,
        tier: ModelTier::Default,
        model_tiers: None,
        context_files_prompt: "",
    }).await
}

/// Start an agent process with full configuration.
pub async fn start_agent_with_config(config: AgentStartConfig<'_>) -> Result<AgentProcess, String> {
    // Read MCP context: use override if provided (general discussions),
    // otherwise read from project filesystem.
    let mcp_context = if let Some(override_ctx) = config.mcp_context_override {
        override_ctx.to_string()
    } else if !config.project_path.is_empty() {
        crate::core::mcp_scanner::read_all_mcp_contexts(config.project_path)
    } else {
        String::new()
    };

    // Use compact format for agents with small context windows (eco-design)
    let compact = matches!(config.agent_type, AgentType::Codex | AgentType::Kiro | AgentType::Vibe);

    // Ensure this discussion's skills/profiles exist as native files on disk.
    // Skills are installed at the PROJECT level (shared by all discussions).
    // This is additive: it only creates missing files, never removes others.
    // Full cleanup only happens at startup / project config change.
    let native_sync_ok = if !config.project_path.is_empty() && (!config.skill_ids.is_empty() || !config.profile_ids.is_empty()) {
        let profile_ids_vec: Vec<String> = config.profile_ids.to_vec();
        crate::core::native_files::sync_project_native_files(
            config.project_path, config.skill_ids, &profile_ids_vec,
        ).is_ok()
    } else {
        false
    };

    // If native files exist AND the agent discovers them (not all do — Vibe/Kiro don't),
    // send a lightweight hint (~15 tokens) instead of full content (~500-800 tokens).
    let native_skills = native_sync_ok
        && crate::core::native_files::supports_native_skills(config.agent_type)
        && crate::core::native_files::has_native_skills(config.project_path, config.agent_type);
    let native_profiles = native_sync_ok
        && config.profile_ids.len() == 1 // Multi-profile always needs prompt injection
        && crate::core::native_files::supports_native_profiles(config.agent_type)
        && crate::core::native_files::has_native_profiles(config.project_path, config.agent_type);

    // Build skills prompt — native hint (~15 tokens) or full injection (~500-800 tokens)
    let skills_prompt = if native_skills {
        crate::core::native_files::build_skills_reference_prompt(config.skill_ids)
    } else if compact {
        crate::core::skills::build_skills_prompt_compact(config.skill_ids)
    } else {
        crate::core::skills::build_skills_prompt(config.skill_ids)
    };

    // Build directives prompt (always injected — no native format)
    let directives_prompt = crate::core::directives::build_directives_prompt(config.directive_ids);

    // Build profiles prompt — skip if native agent file loaded by the agent
    let profiles_prompt = if native_profiles {
        String::new() // Agent loads the .claude/agents/ file natively
    } else if compact {
        crate::core::profiles::build_profiles_prompt_compact(config.profile_ids)
    } else {
        crate::core::profiles::build_profiles_prompt(config.profile_ids)
    };

    // Combine all context parts with explicit section markers
    // (helps non-Claude agents distinguish instructions from task)
    let mut parts = Vec::new();
    if !profiles_prompt.is_empty() { parts.push(format!("=== YOUR ROLE ===\n\n{}", profiles_prompt)); }
    if !skills_prompt.is_empty() { parts.push(format!("=== YOUR EXPERTISE ===\n\n{}", skills_prompt)); }
    if !config.context_files_prompt.is_empty() { parts.push(format!("=== CONTEXT FILES ===\n\n{}", config.context_files_prompt)); }
    if !mcp_context.is_empty() { parts.push(format!("=== AVAILABLE TOOLS ===\n\n{}", mcp_context)); }
    if !directives_prompt.is_empty() { parts.push(format!("=== OUTPUT REQUIREMENTS ===\n\n{}", directives_prompt)); }
    let extra_context = parts.join("\n\n");

    // ── Ollama HTTP path ────────────────────────────────────────────────
    // Ollama uses the HTTP API (/api/chat) instead of a CLI process.
    // This gives us: (1) separate system/user message roles — the model
    // doesn't confuse MCP context with the user's question, (2) token
    // counts in the response, (3) works without the ollama binary (Docker).
    if *config.agent_type == AgentType::Ollama {
        let model_flag = resolve_model_flag(config.agent_type, config.tier, config.model_tiers);
        return start_ollama_http(
            config.prompt,
            &extra_context,
            model_flag.as_deref().unwrap_or("llama3.2"),
        ).await;
    }

    // Resolve model tier to a --model flag
    let model_flag = resolve_model_flag(config.agent_type, config.tier, config.model_tiers);

    let (binary, npx_pkg, mut args, env_key, stderr_mode, output_mode) =
        agent_command(config.agent_type, config.prompt, config.full_access, &extra_context, model_flag.as_deref());

    // Use work_dir (or project_path) for the agent's CWD
    let effective_work_dir = config.work_dir.unwrap_or(config.project_path);
    let work_dir = if effective_work_dir.is_empty() {
        // Global discussion: use a temp working directory
        std::env::temp_dir()
    } else {
        let container_path = crate::core::scanner::resolve_host_path(effective_work_dir);
        if container_path.exists() {
            container_path
        } else {
            let p = PathBuf::from(effective_work_dir);
            if !p.exists() {
                return Err(format!("Project path not found: {}", p.display()));
            }
            p
        }
    };

    // Claude Code in --print mode does NOT auto-load .mcp.json from CWD.
    // Explicitly pass it via --mcp-config so MCP tools are available.
    // IMPORTANT: --mcp-config must come BEFORE --append-system-prompt and the
    // prompt argument, because --append-system-prompt consumes the next
    // positional arg. If --mcp-config is inserted between them, Claude Code
    // mis-parses the arguments and fails with "MCP config file not found".
    if *config.agent_type == AgentType::ClaudeCode {
        let mcp_json = work_dir.join(".mcp.json");
        if mcp_json.exists() {
            // Pop prompt (last arg) and --append-system-prompt value + flag (if present)
            let prompt_arg = args.pop();
            let sys_prompt_val = if args.last().map(|a| !a.starts_with("--")).unwrap_or(false) {
                // The last arg is the system prompt value (not a flag)
                let val = args.pop();
                let flag = args.pop(); // --append-system-prompt
                Some((flag, val))
            } else {
                None
            };

            // Insert --mcp-config at current position (before system prompt & prompt)
            args.push("--mcp-config".into());
            args.push(mcp_json.to_string_lossy().to_string());

            // Re-push --append-system-prompt and its value
            if let Some((flag, val)) = sys_prompt_val {
                if let Some(f) = flag { args.push(f); }
                if let Some(v) = val { args.push(v); }
            }
            // Re-push prompt
            if let Some(p) = prompt_arg { args.push(p); }
        }
    }

    // API key is optional — agents use their own local auth by default
    let api_key = get_api_key(env_key, config.tokens);

    // On macOS hosts, host-mounted kiro-cli is not runnable in Linux containers.
    // Ensure a Linux kiro-cli exists locally before spawning Kiro.
    if matches!(config.agent_type, AgentType::Kiro) {
        ensure_kiro_cli_available().await?;
    }

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
    let mut stderr_handle: Option<tokio::task::JoinHandle<()>> = None;

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
                stderr_handle = Some(tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!("agent stderr: {}", line);
                        if let Ok(mut buf) = capture.lock() {
                            buf.push(line);
                        }
                    }
                }));
            }
        }
    }

    Ok(AgentProcess { child, output_mode, work_dir, agent_type: config.agent_type.clone(), rx, stderr_capture, stderr_task: stderr_handle })
}

/// Ensure kiro-cli is available inside the container.
/// Uses the official installer if missing.
pub(crate) async fn ensure_kiro_cli_available() -> Result<(), String> {
    if super::find_binary("kiro-cli").is_some() {
        return Ok(());
    }

    tracing::info!("kiro-cli not found, installing Linux kiro-cli...");
    let output = async_cmd("sh")
        .args([
            "-c",
            "command -v unzip >/dev/null 2>&1 || { echo 'Missing dependency: unzip' >&2; exit 127; }; \
             curl -fsSL https://cli.kiro.dev/install | bash",
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to launch Kiro installer: {e}"))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Kiro CLI install failed (exit {:?}): {}{}",
            output.status.code(),
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!("\n{stdout}")
            }
        ));
    }

    if super::find_binary("kiro-cli").is_none() {
        return Err(
            "Kiro CLI installed but not found in PATH. Ensure kiro-cli is accessible from your shell."
                .into(),
        );
    }

    Ok(())
}

/// Resolve the path to vibe-runner.py.
/// Searches: env override → Docker bundle → next to executable → Tauri resource → cargo manifest (dev).
fn vibe_runner_path() -> String {
    // 0. Explicit override (allows custom deployments)
    if let Ok(custom) = std::env::var("KRONN_VIBE_RUNNER") {
        if std::path::Path::new(&custom).exists() {
            return custom;
        }
    }
    // 1. Docker: scripts are copied into /app/scripts/
    let docker_path = "/app/scripts/vibe-runner.py";
    if std::path::Path::new(docker_path).exists() {
        return docker_path.to_string();
    }
    // 2. Native/Tauri: next to the running executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Candidate paths relative to the binary
            let candidates = [
                dir.join("scripts").join("vibe-runner.py"),
                dir.join("..").join("scripts").join("vibe-runner.py"),
                // macOS .app bundle: Contents/Resources/scripts/
                dir.join("..").join("Resources").join("scripts").join("vibe-runner.py"),
                // Windows: alongside the .exe
                dir.join("vibe-runner.py"),
            ];
            for candidate in &candidates {
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
    }
    // 3. Dev mode: relative to cargo manifest
    let dev_path = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/vibe-runner.py");
    dev_path.to_string()
}

/// Start Ollama via HTTP API streaming (/api/chat) instead of CLI.
/// Returns an AgentProcess with a dummy child process and an rx fed by
/// the HTTP response stream. System context and user prompt are sent as
/// separate messages (role: system, role: user) so the model doesn't
/// confuse MCP instructions with the user's question.
async fn start_ollama_http(
    user_prompt: &str,
    system_context: &str,
    model: &str,
) -> Result<AgentProcess, String> {
    let base = crate::api::ollama::ollama_base_url_pub();
    let url = format!("{}/api/chat", base);

    let mut messages = Vec::new();
    if !system_context.is_empty() {
        messages.push(serde_json::json!({
            "role": "system",
            "content": system_context,
        }));
    }
    messages.push(serde_json::json!({
        "role": "user",
        "content": user_prompt,
    }));

    let body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600)) // 10 min max for slow models
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let response = client.post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Ollama unreachable at {}: {}", base, e))?;

    if !response.status().is_success() {
        let status = response.status();
        let err_body = response.text().await.unwrap_or_default();
        return Err(format!("Ollama error {}: {}", status, err_body));
    }

    // Stream the response — each line is a JSON object with a `message.content` field.
    // The last chunk has `done: true` and includes token counts.
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);
    let stderr_capture: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_clone = stderr_capture.clone();

    // Spawn a background task to read the HTTP stream and forward text to the channel.
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = match chunk {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("Ollama stream error: {}", e);
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&bytes));

            // Process complete JSON lines (newline-delimited)
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.trim().is_empty() { continue; }

                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
                    // Extract text content
                    if let Some(text) = json["message"]["content"].as_str() {
                        if !text.is_empty() {
                            let _ = tx.send(text.to_string()).await;
                        }
                    }
                    // Last chunk: extract token counts into stderr for parse_token_usage
                    if json["done"].as_bool() == Some(true) {
                        let prompt_tokens = json["prompt_eval_count"].as_u64().unwrap_or(0);
                        let eval_tokens = json["eval_count"].as_u64().unwrap_or(0);
                        if let Ok(mut stderr) = stderr_clone.lock() {
                            stderr.push(format!("ollama_tokens:{}:{}", prompt_tokens, eval_tokens));
                        }
                    }
                }
            }
        }
    });

    // Create a dummy child process (sleep infinity) — AgentProcess requires it
    // but Ollama's execution is HTTP-based, not process-based. The child is
    // never read from; it's just a handle that we kill on cancellation.
    let dummy_child = crate::core::cmd::async_cmd("sleep")
        .arg("3600")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn dummy process: {}", e))?;

    Ok(AgentProcess {
        child: dummy_child,
        output_mode: OutputMode::Text,
        work_dir: std::env::temp_dir(),
        agent_type: AgentType::Ollama,
        rx,
        stderr_capture,
        stderr_task: None,
    })
}

/// MCP context is injected via --append-system-prompt for Claude Code,
/// or prepended to the prompt for other agents.
/// Returns: (binary, npx_package, args, env_key, stderr_mode, output_mode)
fn agent_command(agent_type: &AgentType, prompt: &str, full_access: bool, mcp_context: &str, model_flag: Option<&str>) -> (&'static str, Option<&'static str>, Vec<String>, &'static str, StderrMode, OutputMode) {
    match agent_type {
        AgentType::ClaudeCode => {
            let mut args = vec![
                "--print".into(),
                "--output-format".into(), "stream-json".into(),
                "--verbose".into(),
                "--include-partial-messages".into(),
            ];
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
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
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            // Codex requires a trusted git directory by default.
            // Inside Docker the paths are mapped, so skip the check.
            args.push("--skip-git-repo-check".into());
            // In Docker, container paths don't match host trusted paths,
            // causing "Permission denied" on CWD listing with default sandbox.
            // On macOS Docker, workspace-write can block shell/apply_patch writes
            // despite rw mounts; prefer danger-full-access there.
            if std::env::var("KRONN_HOST_HOME").is_ok() {
                let host_os = std::env::var("KRONN_HOST_OS").unwrap_or_default();
                let force_full_access = host_os.eq_ignore_ascii_case("macOS");
                if full_access || force_full_access {
                    args.push("--sandbox=danger-full-access".into());
                } else {
                    args.push("--sandbox=workspace-write".into());
                }
            }
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
            // Vibe CLI hangs: get_prompt_from_stdin() blocks on sys.stdin.read()
            // when stdin is not a tty, and 429 rate limits cause infinite hangs.
            // vibe-runner.py bypasses the CLI and calls run_programmatic() directly,
            // giving a real agent (bash, file I/O, grep, etc. + MCP if configured).
            // Falls back to direct Mistral API streaming if vibe is not installed.
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            let runner_script = vibe_runner_path();
            let mut args = vec![runner_script];
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            args.push("--max-turns".into());
            args.push("30".into());
            args.push(full_prompt);
            (
                "python3",
                None,
                args,
                "MISTRAL_API_KEY",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        },
        AgentType::GeminiCli => {
            // Gemini CLI requires -p <prompt> as the LAST args.
            // Options (--model, --yolo) must come BEFORE -p, otherwise
            // Gemini interprets them as the prompt value and fails.
            let mut args: Vec<String> = Vec::new();
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            if full_access {
                args.push("--yolo".into());
            }
            // Gemini CLI has no system prompt flag — prepend context to prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push("-p".into());
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
        AgentType::Kiro => {
            // --trust-all-tools is REQUIRED in --no-interactive mode,
            // otherwise Kiro blocks waiting for tool confirmation that never comes.
            let mut args: Vec<String> = vec![
                "chat".into(),
                "--no-interactive".into(),
                "--trust-all-tools".into(),
                "--wrap".into(), "never".into(),
            ];
            let _ = full_access; // Always trusted in non-interactive mode
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push(full_prompt);
            (
                "kiro-cli",
                None, // No npx package
                args,
                "AWS_BUILDER_ID", // Not really used, but placeholder
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        },
        AgentType::CopilotCli => {
            let mut args: Vec<String> = vec![
                "-p".into(),
            ];
            if let Some(model) = model_flag {
                args.push("--model".into());
                args.push(model.into());
            }
            if full_access {
                args.push("--allow-all-tools".into());
            }
            // Copilot has no system prompt flag — prepend context to prompt
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            args.push(full_prompt);
            (
                "copilot",
                Some("@github/copilot"),
                args,
                "GH_TOKEN",
                StderrMode::StdoutOnly,
                OutputMode::Text,
            )
        },
        AgentType::Ollama => {
            // Ollama: local LLM inference via `ollama run <model> <prompt>`
            let model = model_flag.unwrap_or("llama3.2");
            let full_prompt = if mcp_context.is_empty() {
                prompt.into()
            } else {
                format!("{}\n\n{}", mcp_context, prompt)
            };
            let args = vec![
                "run".into(),
                "--nowordwrap".into(),
                model.into(),
                full_prompt,
            ];
            (
                "ollama",
                None,
                args,
                "OLLAMA_HOST",
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
    // Resolve the final command. We also remember whether the resolved binary
    // lives inside WSL (`via_wsl`) so we can pick the right exec strategy
    // below — sending a Linux path to a Windows-native spawn would just fail.
    let (cmd_name, mut cmd_args, resolved_via_wsl) = if let Some(pkg) = npx_package {
        let mut npx_args = vec!["--yes".to_string(), pkg.to_string()];
        npx_args.extend_from_slice(args);
        // For the npx fallback we don't know up-front where npx itself lives;
        // probe it so we wrap with wsl.exe only when npx is a WSL binary.
        let via_wsl = super::find_binary("npx")
            .map(|loc| loc.via_wsl)
            .unwrap_or(false);
        ("npx".to_string(), npx_args, via_wsl)
    } else {
        let bin_loc = super::find_binary(binary)
            .ok_or_else(|| format!("Binary '{}' not found", binary))?;
        let via_wsl = bin_loc.via_wsl;
        (bin_loc.path, args.to_vec(), via_wsl)
    };

    // Force current workspace as trusted for Codex sessions inside Docker.
    // This avoids path-style mismatch issues (/Users/... vs /host-home/...).
    let is_codex = binary == "codex" || npx_package == Some("@openai/codex");
    if is_codex {
        if let Some(exec_idx) = cmd_args.iter().position(|a| a == "exec") {
            let workdir_s = work_dir.display().to_string();
            let mut overrides = vec![
                "-c".to_string(),
                format!("projects.\"{}\".trust_level=\"trusted\"", workdir_s),
            ];
            if let Ok(host_home) = std::env::var("KRONN_HOST_HOME") {
                if let Some(relative) = workdir_s.strip_prefix("/host-home") {
                    overrides.push("-c".to_string());
                    let host_path = format!("{}{}", host_home, relative);
                    overrides.push(format!(
                        "projects.\"{}\".trust_level=\"trusted\"",
                        host_path,
                    ));
                }
            }
            cmd_args.splice(exec_idx + 1..exec_idx + 1, overrides);
        }
    }
    tracing::info!("Spawning agent: {} {:?} in {} (key: {})",
        cmd_name, cmd_args, work_dir.display(),
        if api_key.is_some() { "override" } else { "local auth" }
    );

    // Decide whether to wrap the command with `wsl.exe -e`. On Windows native
    // (Tauri desktop) most agents live inside WSL, but a user may also have
    // installed them as Windows-native binaries (npm-global, scoop, winget).
    // We trust `find_binary` to flag WSL-resolved binaries via `via_wsl` so
    // native Windows binaries (paths like `C:\Users\...\claude.cmd`) are
    // executed directly without the WSL detour.
    //
    // Linux paths (starting with `/`) on Windows are an unambiguous WSL
    // signal — fall back to wrapping in case `via_wsl` was not set, e.g. for
    // a binary picked up via `which::which` inside the host PATH.
    #[cfg(target_os = "windows")]
    let use_wsl = !is_wsl() && (resolved_via_wsl || cmd_name.starts_with('/'));
    #[cfg(not(target_os = "windows"))]
    {
        let _ = resolved_via_wsl; // suppress unused on non-Windows
    }
    #[cfg(not(target_os = "windows"))]
    let use_wsl = false;

    let (final_cmd, final_args, effective_work_dir) = if use_wsl {
        // Convert Windows path to WSL path for --cd
        #[cfg(target_os = "windows")]
        let wsl_work_dir = windows_to_wsl_path(work_dir);
        #[cfg(not(target_os = "windows"))]
        let wsl_work_dir = work_dir.to_path_buf();

        let mut wsl_args = vec![
            "--cd".to_string(),
            wsl_work_dir.display().to_string(),
            "-e".to_string(),
            cmd_name.clone(),
        ];
        wsl_args.extend(cmd_args.iter().cloned());
        // wsl.exe runs from the Windows current dir, but --cd sets WSL's cwd
        ("wsl.exe".to_string(), wsl_args, work_dir.to_path_buf())
    } else {
        (cmd_name.clone(), cmd_args.clone(), work_dir.to_path_buf())
    };

    let mut cmd = async_cmd(&final_cmd);
    cmd.args(&final_args)
        .current_dir(&effective_work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Set TMPDIR to a directory on the same filesystem as work_dir.
    // Prevents EXDEV (cross-device link) errors when agents like Codex do
    // os.rename() from temp files to the work directory (macOS Docker + VirtioFS).
    let agent_tmpdir = work_dir.join(".kronn/tmp");
    let _ = std::fs::create_dir_all(&agent_tmpdir);
    // Ensure .kronn/tmp/ is gitignored in the project (once per project, idempotent)
    if let Some(project_path) = work_dir.to_str() {
        crate::core::mcp_scanner::ensure_gitignore_public(project_path, ".kronn/tmp/");
    }
    cmd.env("TMPDIR", &agent_tmpdir);
    cmd.env("TEMP", &agent_tmpdir);
    cmd.env("TMP", &agent_tmpdir);

    // In Docker, HOME is /home/kronn (the container user), but agents store their
    // auth and config in the real user's home (KRONN_HOST_HOME). Override HOME so
    // all agents (Claude, Codex, Copilot, Kiro, etc.) find their config files.
    // On native (Tauri desktop, direct binary), HOME is already correct.
    let real_home = std::env::var("KRONN_HOST_HOME").ok().filter(|rh| {
        let exists = std::path::Path::new(rh).is_dir();
        if !exists {
            tracing::warn!("KRONN_HOST_HOME={} does not exist, ignoring", rh);
        }
        exists
    });
    if let Some(ref rh) = real_home {
        cmd.env("HOME", rh);
        cmd.env("USERPROFILE", rh); // Windows agents use USERPROFILE
    }

    // Resolve the effective home for agent config lookups (cross-platform).
    let effective_home = real_home.clone()
        .or_else(|| std::env::var("HOME").ok())
        .or_else(|| std::env::var("USERPROFILE").ok());

    // Copilot CLI supports COPILOT_HOME to override config location.
    // Set it explicitly as a safety net (works on all platforms).
    if (binary == "copilot" || npx_package == Some("@github/copilot"))
        && std::env::var("COPILOT_HOME").is_err()
    {
        if let Some(ref home) = effective_home {
            let copilot_dir = std::path::Path::new(home).join(".copilot");
            if copilot_dir.exists() {
                cmd.env("COPILOT_HOME", &copilot_dir);
            }
        }
    }

    // Tell Claude Code we're in a containerized environment.
    // This bypasses the root/sudo check for --dangerously-skip-permissions.
    // Note: use CLAUDE_CODE_BUBBLEWRAP, not IS_SANDBOX — IS_SANDBOX also
    // suppresses 529 overloaded errors causing infinite silent retries.
    cmd.env("CLAUDE_CODE_BUBBLEWRAP", "1");
    // Hint shell-aware tools to use bash (dash does not support `-l`).
    // Only on Unix — Windows doesn't use SHELL env var.
    #[cfg(unix)]
    cmd.env("SHELL", "/bin/bash");

    // Only set API key env var if explicitly configured (override)
    // Otherwise let the agent use its own local auth
    if let Some(key) = api_key {
        cmd.env(env_key, key);
    }

    // Forward GitHub token so agents can create branches, PRs, etc.
    // Priority: env var GH_TOKEN/GITHUB_TOKEN > `gh auth token` (gh CLI config).
    // Also sets COPILOT_GITHUB_TOKEN for GitHub Copilot CLI.
    let gh_token = std::env::var("GH_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .or_else(|_| {
            // Fallback: extract token from `gh auth token` (stored in ~/.config/gh/hosts.yml).
            // Use sync_cmd so the gh subprocess does not flash a console window on Windows.
            crate::core::cmd::sync_cmd("gh")
                .args(["auth", "token"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if t.is_empty() { None } else { Some(t) }
                })
                .ok_or(std::env::VarError::NotPresent)
        });
    if let Ok(ref token) = gh_token {
        cmd.env("GH_TOKEN", token);
        cmd.env("GITHUB_TOKEN", token);
        cmd.env("COPILOT_GITHUB_TOKEN", token);
    }
    // If an API key was explicitly set (e.g. for CopilotCli), also set COPILOT_GITHUB_TOKEN
    if let Some(key) = api_key {
        if env_key == "GH_TOKEN" {
            cmd.env("COPILOT_GITHUB_TOKEN", key);
        }
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
                        return StreamJsonEvent::Usage { input_tokens: input, output_tokens: output, cost_usd: None };
                    }
                }
            }

            // Tool input delta — accumulate partial JSON
            if let Some(delta) = event.get("delta") {
                if delta.get("type").and_then(|v| v.as_str()) == Some("input_json_delta") {
                    if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                        return StreamJsonEvent::ToolInputDelta(partial.to_string());
                    }
                }
            }

            // Content block start — tool use or thinking
            if let Some(content_block) = event.get("content_block") {
                let block_type = content_block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if block_type == "tool_use" {
                    if let Some(name) = content_block.get("name").and_then(|v| v.as_str()) {
                        return StreamJsonEvent::ToolStart(name.to_string());
                    }
                }
            }

            // Content block stop — tool input complete
            let event_type_inner = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if event_type_inner == "content_block_stop" {
                return StreamJsonEvent::ToolEnd;
            }

            StreamJsonEvent::Skip
        }

        // Final result line — contains token usage and cost
        "result" => {
            let cost = json.get("cost_usd").and_then(|v| v.as_f64()).filter(|c| *c > 0.0);
            if let Some(usage) = json.get("usage") {
                let input = usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                let output = usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                if input > 0 || output > 0 {
                    return StreamJsonEvent::Usage { input_tokens: input, output_tokens: output, cost_usd: cost };
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

/// Strip ANSI escape codes from a string.
/// Handles CSI sequences (\x1b[...m), OSC, and other common escape patterns.
pub fn strip_ansi(s: &str) -> String {
    static RE: std::sync::LazyLock<regex_lite::Regex> = std::sync::LazyLock::new(|| {
        regex_lite::Regex::new(r"\x1b\[[0-9;?]*[a-zA-Z]|\x1b\][^\x07]*\x07|\x1b[()][0-9A-B]").unwrap()
    });
    RE.replace_all(s, "").to_string()
}

/// Clean Kiro CLI output: strip ANSI codes, remove the "> " prefix, and filter noise lines.
/// Kiro mixes tool execution logs with actual response text. Filter out tool noise.
pub fn clean_kiro_line(line: &str) -> Option<String> {
    let clean = strip_ansi(line);
    let trimmed = clean.trim();
    // Skip empty lines, cursor control artifacts, and the Kiro banner/spinner
    if trimmed.is_empty()
        || trimmed.chars().all(|c| c.is_whitespace() || c == '\u{2800}') // braille blank chars in banner
        || trimmed.starts_with("Credits:")
        || trimmed.starts_with("▸ Credits:")
        // ── Kiro tool execution logs (structural patterns, language-independent) ──
        // Unicode marker lines
        || trimmed.starts_with("✓ ")       // ✓ Successfully read/found/etc.
        || trimmed.starts_with("↱ ")       // ↱ Operation N: ...
        || trimmed.starts_with("⋮")        // truncation marker
        || trimmed.starts_with("❗ ")       // ❗ No matches found ...
        // Tool invocation patterns (always in English — Kiro CLI log format)
        || trimmed.contains("(using tool:")           // "Reading file: X (using tool: read)"
        || trimmed.contains("(from mcp server:")      // "Running tool X ... (from mcp server: Y)"
        // Structured result lines (start with "- " followed by keyword)
        || trimmed.starts_with("- Completed in ")
        || trimmed.starts_with("- Summary: ")
        // Batch operation headers
        || trimmed.starts_with("Batch fs_read")
        || trimmed.starts_with("Batch ")
    {
        return None;
    }
    // Strip the "> " prefix Kiro adds to responses
    let result = if let Some(stripped) = trimmed.strip_prefix("> ") {
        stripped.to_string()
    } else {
        trimmed.to_string()
    };
    if result.is_empty() { None } else { Some(result) }
}

/// Parse token usage from agent output.
/// Codex outputs "tokens used\nN,NNN" on stderr (StdoutOnly mode captures it).
/// Kiro outputs "Credits: 0.05 • Time: 3s" on stderr.
/// Returns (cleaned_response, tokens_used) — token lines are stripped if found in response.
pub fn parse_token_usage(agent_type: &AgentType, response: &str, stderr_lines: &[String]) -> (String, u64) {
    match agent_type {
        AgentType::Kiro => {
            // Kiro outputs "Credits: X.XX" or "▸ Credits: X.XX" on stderr.
            // Format observed: "Credits: 0.05 • Time: 3s" (may vary across versions).
            // We parse the float after "Credits:" and before the next "•" or EOL.
            // Store as integer: credits × 10000 for precision (0.05 → 500).
            for line in stderr_lines {
                let clean = strip_ansi(line);
                if let Some(credits_part) = clean.split("Credits:").nth(1) {
                    let credits_str = credits_part.split('•').next().unwrap_or(credits_part).trim();
                    if let Ok(credits) = credits_str.parse::<f64>() {
                        let tokens = (credits * 10000.0) as u64;
                        return (response.to_string(), tokens);
                    } else {
                        tracing::warn!("Kiro credits parse failed for: {:?}", credits_str);
                    }
                }
            }
            if !stderr_lines.is_empty() {
                tracing::debug!("Kiro stderr ({} lines), no Credits found", stderr_lines.len());
            }
            (response.to_string(), 0)
        }
        AgentType::Codex => {
            // Codex outputs "tokens used" then the count on stderr
            // Check stderr first (primary source)
            if stderr_lines.len() >= 2 {
                let last = stderr_lines[stderr_lines.len() - 1].trim();
                let second_last = stderr_lines[stderr_lines.len() - 2].trim();
                if second_last == "tokens used" {
                    let count_str: String = last.chars().filter(|c| *c != ',' && *c != '.').collect();
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
                    let count_str: String = last.chars().filter(|c| *c != ',' && *c != '.').collect();
                    if let Ok(count) = count_str.parse::<u64>() {
                        let clean = lines[..lines.len() - 2].join("\n");
                        return (clean, count);
                    }
                }
            }
            (response.to_string(), 0)
        }
        AgentType::Ollama => {
            // Ollama HTTP streaming puts "ollama_tokens:prompt:eval" in stderr_capture
            // (written by start_ollama_http when the final `done: true` chunk arrives).
            for line in stderr_lines {
                if let Some(rest) = line.strip_prefix("ollama_tokens:") {
                    let parts: Vec<&str> = rest.split(':').collect();
                    if parts.len() == 2 {
                        let prompt = parts[0].parse::<u64>().unwrap_or(0);
                        let eval = parts[1].parse::<u64>().unwrap_or(0);
                        return (response.to_string(), prompt + eval);
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

#[cfg(test)]
#[path = "runner_test.rs"]
mod runner_test;

fn get_api_key(env_key: &str, tokens: &TokensConfig) -> Option<String> {
    let provider = match env_key {
        "ANTHROPIC_API_KEY" => "anthropic",
        "OPENAI_API_KEY" => "openai",
        "GEMINI_API_KEY" => "google",
        "MISTRAL_API_KEY" => "mistral",
        "OLLAMA_HOST" => return std::env::var(env_key).ok(), // Ollama: no API key, just host URL
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
