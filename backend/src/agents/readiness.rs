//! Readiness of the agents a multi-agent discussion or room is about to start
//! (KT-1107): installed, allowed to run, signed in, and able to open an ACP
//! session. Every check runs in parallel and is bounded; none calls a model.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use super::acp_start::AcpStartFailure;
use super::runner::{self, AcpProbeBounds};
use crate::models::{
    AgentDetection, AgentReadiness, AgentReadinessReason, AgentReadinessStatus, AgentType,
    TokensConfig,
};

/// How long a result is served without probing again.
pub const CACHE_TTL: Duration = Duration::from_secs(180);

/// Bound of a CLI's sign-in status command.
pub const LOGIN_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// What one readiness request probes with.
pub struct ProbeContext {
    pub project_id: Option<String>,
    /// Empty for a discussion without a project.
    pub project_path: String,
    pub detections: Vec<AgentDetection>,
    pub tokens: TokensConfig,
    /// Hash of the saved agent settings: any change invalidates the cache.
    pub agents_config: u64,
    pub bounds: AcpProbeBounds,
    pub login_timeout: Duration,
    pub force: bool,
}

struct Entry {
    result: AgentReadiness,
    at: Instant,
    fingerprint: u64,
}

static CACHE: LazyLock<Mutex<HashMap<(String, String), Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Probe `agents` (deduplicated, in request order) in parallel.
pub async fn check_agents(ctx: &ProbeContext, agents: &[AgentType]) -> Vec<AgentReadiness> {
    let mut unique: Vec<&AgentType> = Vec::new();
    for agent in agents {
        if !unique.contains(&agent) {
            unique.push(agent);
        }
    }
    futures::future::join_all(unique.into_iter().map(|agent| check_cached(ctx, agent))).await
}

/// Hash of everything a result depends on but the clock: the agent's settings,
/// its install, its key and the MCP servers its session would start.
fn fingerprint(ctx: &ProbeContext, agent: &AgentType) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    ctx.agents_config.hash(&mut hasher);
    // The probe runs there: a project moved to another folder is probed again.
    ctx.project_path.hash(&mut hasher);
    crate::core::config::saved_full_access(agent).hash(&mut hasher);
    if let Some(detection) = detection_for(ctx, agent) {
        (
            detection.installed,
            detection.runtime_available,
            &detection.path,
            &detection.version,
        )
            .hash(&mut hasher);
    }
    runner::configured_api_key(agent, &ctx.tokens).hash(&mut hasher);
    format!("{:?}", runner::probe_mcp_servers(agent, &ctx.project_path)).hash(&mut hasher);
    hasher.finish()
}

async fn check_cached(ctx: &ProbeContext, agent: &AgentType) -> AgentReadiness {
    let key = (
        format!("{agent:?}"),
        ctx.project_id.clone().unwrap_or_default(),
    );
    let fingerprint = fingerprint(ctx, agent);
    if !ctx.force {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = cache.get(&key) {
            if entry.fingerprint == fingerprint && entry.at.elapsed() < CACHE_TTL {
                return AgentReadiness {
                    cached: true,
                    ..entry.result.clone()
                };
            }
        }
    }
    let result = check(ctx, agent).await;
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(
        key,
        Entry {
            result: result.clone(),
            at: Instant::now(),
            fingerprint,
        },
    );
    result
}

fn detection_for<'a>(ctx: &'a ProbeContext, agent: &AgentType) -> Option<&'a AgentDetection> {
    ctx.detections
        .iter()
        .find(|detection| &detection.agent_type == agent)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Login {
    SignedIn,
    SignedOut,
    Unknown,
}

async fn check(ctx: &ProbeContext, agent: &AgentType) -> AgentReadiness {
    if runner::is_http_chat_agent(agent) {
        return verdict(agent, AgentReadinessReason::NotProbed);
    }
    let native = runner::requires_explicit_full_access(agent);
    let detection = detection_for(ctx, agent);
    // A native runtime is spawned by name: a package-runner fallback cannot start it.
    let installed = detection
        .is_some_and(|detection| detection.installed || (!native && detection.runtime_available));
    if !installed {
        return verdict(agent, AgentReadinessReason::NotInstalled);
    }
    if native && !crate::core::config::saved_full_access(agent) {
        return verdict(agent, AgentReadinessReason::FullAccessRequired);
    }
    let binary = detection
        .filter(|detection| detection.installed)
        .and_then(|detection| detection.path.clone());
    let session = async {
        if !native {
            // The adapters open no process before the first prompt.
            return None;
        }
        Some(
            runner::probe_native_acp_session(
                agent,
                &ctx.project_path,
                ctx.project_id.as_deref(),
                &ctx.tokens,
                ctx.bounds,
            )
            .await,
        )
    };
    let (login, session) = tokio::join!(
        login_status(agent, binary.as_deref(), ctx.login_timeout),
        session
    );
    // A key Kronn hands the launch may sign it in where the CLI's own login does not.
    let login = match login {
        Login::SignedOut if runner::configured_api_key(agent, &ctx.tokens).is_some() => {
            Login::Unknown
        }
        other => other,
    };
    if login == Login::SignedOut {
        return verdict(agent, AgentReadinessReason::NotLoggedIn);
    }
    if let Some(Err(failure)) = session {
        return session_failure(agent, failure);
    }
    if login == Login::Unknown {
        return verdict(agent, AgentReadinessReason::LoginUnverified);
    }
    verdict(agent, AgentReadinessReason::Ready)
}

fn status_of(reason: AgentReadinessReason) -> AgentReadinessStatus {
    match reason {
        AgentReadinessReason::Ready => AgentReadinessStatus::Ready,
        AgentReadinessReason::LoginUnverified | AgentReadinessReason::NotProbed => {
            AgentReadinessStatus::Unknown
        }
        _ => AgentReadinessStatus::NotReady,
    }
}

fn reason_key(reason: AgentReadinessReason) -> &'static str {
    match reason {
        AgentReadinessReason::Ready => "ready",
        AgentReadinessReason::NotInstalled => "not_installed",
        AgentReadinessReason::FullAccessRequired => "full_access_required",
        AgentReadinessReason::NotLoggedIn => "not_logged_in",
        AgentReadinessReason::LoginUnverified => "login_unverified",
        AgentReadinessReason::SessionTimeout => "session_timeout",
        AgentReadinessReason::SessionFailed => "session_failed",
        AgentReadinessReason::NotProbed => "not_probed",
    }
}

fn verdict(agent: &AgentType, reason: AgentReadinessReason) -> AgentReadiness {
    AgentReadiness {
        agent_type: agent.clone(),
        status: status_of(reason),
        reason,
        message_key: format!("readiness.reason.{}", reason_key(reason)),
        servers: Vec::new(),
        secs: None,
        detail: None,
        cached: false,
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn session_failure(agent: &AgentType, failure: AcpStartFailure) -> AgentReadiness {
    let reason = if failure.detail.is_some() {
        AgentReadinessReason::SessionFailed
    } else {
        AgentReadinessReason::SessionTimeout
    };
    let mut result = verdict(agent, reason);
    result.secs = (reason == AgentReadinessReason::SessionTimeout).then_some(failure.secs);
    result.servers = failure.servers;
    result.detail = failure.detail;
    result
}

/// The CLI's own, model-free sign-in status command, where one exists.
fn login_command(agent: &AgentType) -> Option<&'static [&'static str]> {
    match agent {
        AgentType::ClaudeCode => Some(&["auth", "status", "--json"]),
        AgentType::Codex => Some(&["login", "status"]),
        AgentType::Kiro => Some(&["whoami"]),
        _ => None,
    }
}

async fn login_status(agent: &AgentType, binary: Option<&str>, bound: Duration) -> Login {
    let (Some(args), Some(binary)) = (login_command(agent), binary) else {
        return Login::Unknown;
    };
    let family = crate::core::child_env::AgentFamily::from_agent_type(agent);
    let mut command = crate::core::cmd::discovery_cmd(binary, family);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(bound, command.output()).await {
        Ok(Ok(output)) => parse_login(
            agent,
            output.status.success(),
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
        ),
        _ => Login::Unknown,
    }
}

/// Only a recognised answer is trusted: anything else, an unknown subcommand
/// included, stays unknown.
fn parse_login(agent: &AgentType, success: bool, stdout: &str, stderr: &str) -> Login {
    let text = format!("{stdout}\n{stderr}").to_lowercase();
    match agent {
        AgentType::ClaudeCode => serde_json::from_str::<serde_json::Value>(stdout.trim())
            .ok()
            .and_then(|status| status.get("loggedIn").and_then(|value| value.as_bool()))
            .map_or(Login::Unknown, |signed_in| {
                if signed_in {
                    Login::SignedIn
                } else {
                    Login::SignedOut
                }
            }),
        AgentType::Codex | AgentType::Kiro if text.contains("not logged in") => Login::SignedOut,
        AgentType::Codex if success && text.contains("logged in") => Login::SignedIn,
        AgentType::Kiro if success && !text.trim().is_empty() => Login::SignedIn,
        _ => Login::Unknown,
    }
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;
