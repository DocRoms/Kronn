//! ACP host boundary.
//!
//! This module deliberately owns the control-plane contract and not a CLI's
//! command-line syntax. Native and adapted runtimes implement `AcpTransport`;
//! callers only deal with negotiated capabilities and opaque session targets.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};

use crate::models::AgentType;

mod claude_adapter;
mod codex_adapter;
mod permission_broker;

pub use claude_adapter::ClaudeAcpAdapter;
pub use codex_adapter::CodexAcpAdapter;
pub use permission_broker::{
    AcpAuditEntry, AcpPermissionBroker, AcpPermissionVerdict, AcpSessionPolicy, AcpSessionScope,
};

/// Shared by the Claude/Codex adapter test modules.
#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// Write an executable shell fixture and return its path. The adapters
    /// always append their own CLI-specific flags (`--print`,
    /// `--output-format`, `--session-id`, `--resume`, …); a fixture script
    /// ignores whatever it does not recognize and reacts only to the
    /// substrings it cares about, exactly like a real shell script would.
    /// The fixture executes on POSIX hosts; its helper must also compile for
    /// Windows, where the portability gate builds the complete test library.
    pub(crate) fn write_fixture_script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fixture-cli");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fixture script");
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&path)
                .expect("stat fixture script")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod fixture script");
        }
        path
    }
}

struct PendingRequest {
    sender: oneshot::Sender<Result<Value, AcpError>>,
    /// Only an OpenCode session/resume request may classify the verified
    /// missing-session contract as safe to replace, for this exact id.
    missing_session_id: Option<String>,
}

type PendingRequests = Arc<Mutex<HashMap<u64, PendingRequest>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AcpAgent {
    OpenCode,
    GeminiCli,
    CopilotCli,
    Kiro,
    Vibe,
    Codex,
    ClaudeCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpRuntime {
    Native,
    Adapter,
    DirectCliMigration,
}

/// The production transport Kronn can actually start today. Candidate ACP
/// support and an active ACP route are deliberately separate: callers must
/// never label a direct CLI invocation as native/adapted ACP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpProductionRoute {
    NativeAcp,
    /// Codex/Claude via `ClaudeAcpAdapter`/`CodexAcpAdapter` — the same
    /// `AcpHost` as native agents, but the wire is each CLI's own
    /// non-interactive protocol rather than ACP JSON-RPC. Only reachable
    /// through [`resolve_acp_route`]; [`production_route`] never returns it.
    AdaptedAcp,
    DirectCliMigration,
    HttpModelProvider,
}

/// The default/candidate route for an agent, assuming no adapter opt-in.
/// Deliberately pure and unaware of any runtime toggle: Codex/Claude keep
/// returning `DirectCliMigration` here even after the adapters exist, so the
/// conservative default never silently changes. Use [`resolve_acp_route`] at
/// actual dispatch time to honor the explicit, observable opt-in toggle.
pub fn production_route(agent: &AgentType) -> AcpProductionRoute {
    match agent {
        AgentType::OpenCode
        | AgentType::GeminiCli
        | AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe => AcpProductionRoute::NativeAcp,
        AgentType::ClaudeCode | AgentType::Codex => AcpProductionRoute::DirectCliMigration,
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom => {
            AcpProductionRoute::HttpModelProvider
        }
    }
}

/// Strict boolean parse for a `KRONN_ACP_ADAPTER_*` toggle. Only `1`/`true`
/// (case-insensitive, surrounding whitespace ignored) activate the adapter;
/// unset, empty, `0`, `false`, or any other value all mean "off" and keep the
/// conservative direct-CLI default. Presence-only parsing (`.is_ok()`) used to
/// activate the adapter for ANY value including `"0"`/`"false"` — an operator
/// clearing the toggle by setting it to a falsy string instead of unsetting it
/// would silently keep routing through the ACP adapter (KT-542 review fix).
fn env_flag_enabled(var: &str) -> bool {
    std::env::var(var)
        .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"))
        .unwrap_or(false)
}

/// Per-agent, explicit, environment-driven opt-in for the Codex/Claude ACP
/// adapters. Off by default: direct-CLI migration stays the production
/// default for both agents until an operator turns the adapter on for that
/// specific agent. Reading the toggle at call time (rather than baking it
/// into a `once_cell`) keeps it test-friendly and trivially observable —
/// `kronn doctor`/logs can report the exact variable an operator would set.
pub fn acp_adapter_enabled(agent: &AgentType) -> bool {
    match agent {
        AgentType::Codex => env_flag_enabled("KRONN_ACP_ADAPTER_CODEX"),
        AgentType::ClaudeCode => env_flag_enabled("KRONN_ACP_ADAPTER_CLAUDE"),
        _ => false,
    }
}

/// The route actually taken for one dispatch, honoring the explicit opt-in
/// toggle on top of the conservative default from [`production_route`].
/// Never widens any OTHER agent's route: only a `DirectCliMigration` default
/// can become `AdaptedAcp`, and only when that agent's toggle is set.
pub fn resolve_acp_route(agent: &AgentType) -> AcpProductionRoute {
    let default_route = production_route(agent);
    if default_route == AcpProductionRoute::DirectCliMigration && acp_adapter_enabled(agent) {
        AcpProductionRoute::AdaptedAcp
    } else {
        default_route
    }
}

/// The exact, vendor-documented ACP subprocess command for each native
/// runtime. Kept pure so the command surface is unit-tested without spawning a
/// process. An agent with no verified command returns `None` and stays on the
/// observable direct-CLI migration route rather than guessing a flag.
pub fn native_acp_command(agent: AcpAgent) -> Option<(&'static str, Vec<&'static str>)> {
    match agent {
        AcpAgent::OpenCode => Some(("opencode", vec!["acp"])),
        AcpAgent::GeminiCli => Some(("gemini", vec!["--acp"])),
        AcpAgent::CopilotCli => Some(("copilot", vec!["--acp"])),
        AcpAgent::Kiro => Some(("kiro-cli", vec!["acp"])),
        AcpAgent::Vibe => Some(("vibe-acp", vec![])),
        AcpAgent::Codex | AcpAgent::ClaudeCode => None,
    }
}

pub fn acp_agent(agent: &AgentType) -> Option<AcpAgent> {
    match agent {
        AgentType::OpenCode => Some(AcpAgent::OpenCode),
        AgentType::GeminiCli => Some(AcpAgent::GeminiCli),
        AgentType::CopilotCli => Some(AcpAgent::CopilotCli),
        AgentType::Kiro => Some(AcpAgent::Kiro),
        AgentType::Vibe => Some(AcpAgent::Vibe),
        AgentType::Codex => Some(AcpAgent::Codex),
        AgentType::ClaudeCode => Some(AcpAgent::ClaudeCode),
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AcpCapability {
    Sessions,
    /// `agentCapabilities.loadSession`: loading replays the prior transcript.
    LoadSession,
    /// `agentCapabilities.sessionCapabilities.resume`: reconnect without a
    /// replay, so it is safe to pair with a discussion delta.
    Resume,
    Streaming,
    Cancellation,
    Permissions,
    McpInjection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpRuntimeProfile {
    pub agent: AcpAgent,
    pub runtime: AcpRuntime,
    pub advertised_capabilities: BTreeSet<AcpCapability>,
}

impl AcpRuntimeProfile {
    pub fn requires(&self, capability: AcpCapability) -> Result<(), AcpError> {
        self.advertised_capabilities
            .contains(&capability)
            .then_some(())
            .ok_or(AcpError::CapabilityUnavailable { capability })
    }
}

/// Product defaults are intentionally conservative. A runtime's initialize
/// response remains authoritative for a concrete session.
pub fn runtime_profile(agent: AcpAgent) -> AcpRuntimeProfile {
    let runtime = match agent {
        AcpAgent::OpenCode
        | AcpAgent::GeminiCli
        | AcpAgent::CopilotCli
        | AcpAgent::Kiro
        | AcpAgent::Vibe => AcpRuntime::Native,
        AcpAgent::Codex | AcpAgent::ClaudeCode => AcpRuntime::Adapter,
    };
    AcpRuntimeProfile {
        agent,
        runtime,
        advertised_capabilities: BTreeSet::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSessionTarget {
    pub agent: AcpAgent,
    pub session_id: String,
}

impl AcpSessionTarget {
    pub fn new(agent: AcpAgent, session_id: impl Into<String>) -> Result<Self, AcpError> {
        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            return Err(AcpError::InvalidSessionTarget);
        }
        Ok(Self { agent, session_id })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpInitialize {
    pub protocol_version: u32,
    /// ACP v1 negotiates client capabilities at `initialize`; MCP declarations
    /// belong to `session/new`, where they are scoped to one workspace.
    pub cwd: String,
    pub mcp_servers: Vec<AcpMcpServer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpMcpServer {
    pub id: String,
    pub command: String,
    pub args: Vec<String>,
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpNegotiatedCapabilities {
    pub protocol_version: u32,
    pub capabilities: BTreeSet<AcpCapability>,
}

/// One selectable model/mode exposed by an ACP session. Per the ACP
/// session-config-options contract these are returned in the `session/new`
/// (and `session/load`) *response*, never guessed from `initialize`. Kronn maps
/// a tier/model onto an option value and applies it with
/// `session/set_config_option`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfigOption {
    pub id: String,
    pub current: Option<String>,
    pub available: Vec<AcpConfigValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpConfigValue {
    pub id: String,
    pub name: String,
}

/// Parse the `configOptions` array of a `session/new`/`session/load` response.
/// Tolerant of the documented shape and minor casing variants; an absent or
/// malformed array yields no options rather than a fabricated catalogue.
fn parse_config_options(result: &Value) -> Vec<AcpConfigOption> {
    let Some(options) = result
        .get("configOptions")
        .or_else(|| result.get("config_options"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    options
        .iter()
        .filter_map(|option| {
            let id = option
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())?
                .to_owned();
            // Every runtime spells this differently and none of it is
            // standardized. OpenCode sends `currentValue` + `options[].value`;
            // reading only the shapes we knew made its whole catalogue look
            // absent, and an agent with no catalogue is refused at preflight.
            let current = option
                .get("value")
                .or_else(|| option.get("current"))
                .or_else(|| option.get("currentValue"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let available = option
                .get("availableValues")
                .or_else(|| option.get("available_values"))
                .or_else(|| option.get("values"))
                .or_else(|| option.get("options"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| {
                            let vid = value
                                .get("id")
                                .or_else(|| value.get("value"))
                                .and_then(Value::as_str)
                                .filter(|id| !id.trim().is_empty())?
                                .to_owned();
                            let name = value
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(&vid)
                                .to_owned();
                            Some(AcpConfigValue { id: vid, name })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(AcpConfigOption {
                id,
                current,
                available,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpSessionEvent {
    /// Runtime-owned conversation identifier discovered after session
    /// creation. This is control metadata consumed by the runner, never text
    /// forwarded to the discussion or an agent-visible event payload.
    NativeSessionId(String),
    TextDelta(String),
    ToolCall {
        name: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Completed,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AcpError {
    #[error("ACP capability is unavailable: {capability:?}")]
    CapabilityUnavailable { capability: AcpCapability },
    #[error("ACP session target is empty")]
    InvalidSessionTarget,
    #[error("ACP protocol version {actual} is unsupported; maximum is {maximum}")]
    UnsupportedProtocolVersion { actual: u32, maximum: u32 },
    #[error("ACP transport failed: {0}")]
    Transport(String),
    #[error("ACP response did not contain a valid session identifier")]
    InvalidSessionResponse,
    #[error("ACP request timed out: {0}")]
    Timeout(String),
    /// Only a transport that has positively identified this condition may use
    /// the safe fresh-session fallback. Authentication and I/O failures remain
    /// distinct and must be returned to the caller.
    #[error("ACP session does not exist")]
    SessionNotFound,
}

#[async_trait]
pub trait AcpTransport: Send + Sync {
    async fn initialize(
        &self,
        request: AcpInitialize,
    ) -> Result<AcpNegotiatedCapabilities, AcpError>;
    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError>;
    /// The selectable model/mode options returned by the last `session/new` or
    /// `session/load` response. Empty when the runtime exposes none.
    async fn config_options(&self) -> Vec<AcpConfigOption>;
    /// Apply one option value via `session/set_config_option` and update the
    /// stored option set from the response.
    async fn set_config_option(
        &self,
        target: &AcpSessionTarget,
        config_id: &str,
        value_id: &str,
    ) -> Result<(), AcpError>;
    async fn resume_session(&self, target: &AcpSessionTarget) -> Result<(), AcpError>;
    /// Run one prompt turn, forwarding each normalized event on `events` as it
    /// arrives so the caller streams live during the turn instead of receiving
    /// an accumulated batch only after the response. A closed receiver is not an
    /// error: the transport keeps draining the turn to completion.
    async fn prompt(
        &self,
        target: &AcpSessionTarget,
        prompt: &str,
        events: mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError>;
    async fn cancel(&self, target: &AcpSessionTarget) -> Result<(), AcpError>;
    async fn shutdown(&self) -> Result<(), AcpError>;
    /// The runtime's own native session/thread identifier, when it differs
    /// from Kronn's opaque `AcpSessionTarget.session_id`. Native ACP agents
    /// and the Claude adapter let Kronn choose the id up front, so it is
    /// always `None` there. The Codex adapter cannot: Codex only assigns a
    /// `thread_id` after the first turn, so this exposes it once known so the
    /// caller can persist it for a cross-restart resume. Default `None` keeps
    /// every other implementor unchanged.
    async fn native_session_id(&self, _target: &AcpSessionTarget) -> Option<String> {
        None
    }
}

/// A sequential ND-JSON ACP client for native ACP CLIs. The wire is kept here
/// rather than sharing the Claude stream parser: ACP messages are JSON-RPC
/// request/response objects and notifications, not model output.
pub struct AcpJsonRpcTransport {
    agent: AcpAgent,
    stdin: Arc<Mutex<ChildStdin>>,
    process: Mutex<AcpProcess>,
    next_id: AtomicU64,
    pending: PendingRequests,
    notifications: broadcast::Sender<Value>,
    session_setup: Mutex<Option<AcpSessionSetup>>,
    config_options: Mutex<Vec<AcpConfigOption>>,
    broker: Arc<AcpPermissionBroker>,
}

/// How long `shutdown` waits for the stdout dispatcher after the child is
/// reaped. Only a descendant holding the inherited pipe can exceed this, and
/// blocking shutdown on that is worse than abandoning the drain.
const DISPATCHER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns the stdout dispatcher and cancels it when dropped. A bare `JoinHandle`
/// only DETACHES on drop, so without this every path that does not reach an
/// explicit join would leave the drain running: a `shutdown` future cancelled
/// mid-await, or a transport dropped without `shutdown` at all. `kill_on_drop`
/// covers the `Child`; nothing covered the task.
struct DispatcherOwner(Option<JoinHandle<()>>);

impl DispatcherOwner {
    fn new(handle: JoinHandle<()>) -> Self {
        Self(Some(handle))
    }

    /// Wait for the drain to finish, bounded. On timeout, abort and then WAIT
    /// for the cancellation to land: `abort` only requests it, so returning
    /// here without the join would claim a stop that has not happened.
    ///
    /// The handle STAYS in `self.0` across every await. Taking it into a local
    /// first — which this did — defeats the whole guard precisely where it is
    /// needed: a `finish` cancelled mid-await would drop that local, and
    /// dropping a `JoinHandle` detaches, while `Drop` here would find `None`
    /// and do nothing. Held in place, any cancellation leaves `Some(..)` for
    /// `Drop` to abort.
    async fn finish(mut self) -> Result<(), String> {
        let Some(handle) = self.0.as_mut() else {
            return Ok(());
        };
        let outcome = match timeout(DISPATCHER_JOIN_TIMEOUT, &mut *handle).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(format!("join ACP dispatcher: {error}")),
            Err(_) => {
                handle.abort();
                // The cancellation is what we wait for; its `JoinError` is the
                // expected outcome, not a failure to report.
                let _ = (&mut *handle).await;
                Ok(())
            }
        };
        // Observed finished: release it so `Drop` has nothing left to abort.
        self.0.take();
        outcome
    }
}

impl Drop for DispatcherOwner {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// The process and the task draining its stdout have one lifecycle. Keeping
/// them together lets shutdown reap the process before joining the dispatcher.
struct AcpProcess {
    child: Option<Child>,
    dispatcher: Option<DispatcherOwner>,
}

/// `session/new` inputs captured at `initialize` time. Retained so that
/// `session/resume` can resend `cwd` + `mcpServers` (the workspace scope), not
/// only the opaque session id, as the ACP session-resume contract requires.
#[derive(Debug, Clone)]
struct AcpSessionSetup {
    cwd: String,
    mcp_servers: Vec<Value>,
}

impl AcpJsonRpcTransport {
    /// Start a runtime whose ACP subprocess command is documented by its
    /// vendor. Agents without a verified command remain on the observable
    /// direct-CLI migration route instead of guessing a flag.
    pub async fn spawn_native(
        agent: AcpAgent,
        cwd: &str,
        full_access: bool,
        discussion_id: Option<&str>,
        scope: AcpSessionScope,
    ) -> Result<Self, AcpError> {
        let (program, args) = native_acp_command(agent).ok_or_else(|| {
            AcpError::Transport(format!("no verified production ACP command for {agent:?}"))
        })?;
        let mut command = crate::core::cmd::async_cmd(program);
        command.args(args).current_dir(cwd);
        if let Some(discussion_id) = discussion_id {
            command.env("KRONN_DISCUSSION_ID", discussion_id);
        }
        Self::spawn_scoped(agent, command, full_access, Some(scope)).await
    }

    pub async fn spawn(
        agent: AcpAgent,
        command: tokio::process::Command,
        full_access: bool,
    ) -> Result<Self, AcpError> {
        Self::spawn_scoped(agent, command, full_access, None).await
    }

    async fn spawn_scoped(
        agent: AcpAgent,
        mut command: tokio::process::Command,
        full_access: bool,
        scope: Option<AcpSessionScope>,
    ) -> Result<Self, AcpError> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| AcpError::Transport(format!("spawn ACP process: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AcpError::Transport("ACP stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::Transport("ACP stdout unavailable".into()))?;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending: PendingRequests = Arc::new(Mutex::new(HashMap::new()));
        let (notifications, _) = broadcast::channel(256);
        let broker = Arc::new(match scope {
            Some(scope) => AcpPermissionBroker::scoped(full_access, scope),
            None => AcpPermissionBroker::new(full_access),
        });
        let dispatcher = Self::start_dispatcher(
            BufReader::new(stdout),
            stdin.clone(),
            pending.clone(),
            notifications.clone(),
            broker.clone(),
        );
        Ok(Self {
            agent,
            stdin,
            process: Mutex::new(AcpProcess {
                child: Some(child),
                dispatcher: Some(DispatcherOwner::new(dispatcher)),
            }),
            next_id: AtomicU64::new(1),
            pending,
            notifications,
            session_setup: Mutex::new(None),
            config_options: Mutex::new(Vec::new()),
            broker,
        })
    }

    /// Audit trail of every permission/fs/terminal decision the dispatcher
    /// made for incoming agent->client requests during this session.
    pub fn permission_audit_log(&self) -> Vec<AcpAuditEntry> {
        self.broker.audit_log()
    }

    fn start_dispatcher(
        mut stdout: BufReader<ChildStdout>,
        stdin: Arc<Mutex<ChildStdin>>,
        pending: PendingRequests,
        notifications: broadcast::Sender<Value>,
        broker: Arc<AcpPermissionBroker>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let mut line = String::new();
                let read = match stdout.read_line(&mut line).await {
                    Ok(read) => read,
                    Err(error) => {
                        fail_pending(
                            &pending,
                            AcpError::Transport(format!("read ACP response: {error}")),
                        )
                        .await;
                        return;
                    }
                };
                if read == 0 {
                    fail_pending(
                        &pending,
                        AcpError::Transport("ACP process closed stdout".into()),
                    )
                    .await;
                    return;
                }
                let message: Value = match serde_json::from_str(line.trim()) {
                    Ok(message) => message,
                    Err(error) => {
                        tracing::warn!("discarding malformed ACP frame: {error}");
                        continue;
                    }
                };
                if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    if let Some(method) = message.get("method").and_then(Value::as_str) {
                        // Agent -> client requests cannot wait behind a prompt response.
                        // Every one is routed through the scoped, audited broker instead
                        // of hanging or being silently granted.
                        let params = message.get("params").cloned().unwrap_or(Value::Null);
                        let envelope = match handle_client_request(&broker, method, &params) {
                            Ok(result) => json!({"jsonrpc":"2.0", "id":id, "result": result}),
                            Err((code, error_message)) => {
                                json!({"jsonrpc":"2.0", "id":id, "error": {"code": code, "message": error_message}})
                            }
                        };
                        let _ = write_frame(&stdin, envelope).await;
                    } else if let Some(request) = pending.lock().await.remove(&id) {
                        let result = if let Some(error) = message.get("error") {
                            Err(classify_response_error(
                                error,
                                request.missing_session_id.as_deref(),
                            ))
                        } else {
                            message.get("result").cloned().ok_or_else(|| {
                                AcpError::Transport("ACP response returned no result".into())
                            })
                        };
                        let _ = request.sender.send(result);
                    }
                } else {
                    let _ = notifications.send(message);
                }
            }
        })
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let missing_session_id = (self.agent == AcpAgent::OpenCode && method == "session/resume")
            .then(|| {
                params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .flatten();
        self.pending.lock().await.insert(
            id,
            PendingRequest {
                sender,
                missing_session_id,
            },
        );
        if let Err(error) = self
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        timeout(Duration::from_secs(30), receiver)
            .await
            .map_err(|_| AcpError::Timeout(method.to_owned()))?
            .map_err(|_| {
                AcpError::Transport(format!("ACP dispatcher stopped before {method} completed"))
            })?
    }

    async fn send(&self, frame: Value) -> Result<(), AcpError> {
        write_frame(&self.stdin, frame).await
    }
}

async fn write_frame(stdin: &Arc<Mutex<ChildStdin>>, frame: Value) -> Result<(), AcpError> {
    let encoded = serde_json::to_string(&frame)
        .map_err(|error| AcpError::Transport(format!("encode ACP request: {error}")))?;
    let mut stdin = stdin.lock().await;
    stdin
        .write_all(encoded.as_bytes())
        .await
        .map_err(|error| AcpError::Transport(format!("write ACP request: {error}")))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| AcpError::Transport(format!("terminate ACP request: {error}")))?;
    stdin
        .flush()
        .await
        .map_err(|error| AcpError::Transport(format!("flush ACP request: {error}")))?;
    Ok(())
}

/// JSON-RPC "Invalid params" per the spec Kronn's ACP transports speak.
/// OpenCode's `toRequestError(ACPSessionNotFoundError)` (verified against
/// `packages/opencode/src/acp/error.ts` at tag v1.18.27) uses this code with
/// `message: "session not found: <id>"` and `data: {"sessionId": "<id>"}`.
const JSON_RPC_INVALID_PARAMS: i64 = -32602;

/// Turn one JSON-RPC error response into an `AcpError`, promoting it to the
/// safe `SessionNotFound` fallback ONLY for the exact structured shape a
/// server can use to positively identify a missing session.
///
/// This is deliberately narrow. OpenCode's own `resumeSession` (same source,
/// `service.ts`) calls the SDK with `throwOnError: true` and its
/// `fromUnknownError` keeps only an ACP-tagged error or an auth error —
/// anything else, including a genuine missing session reached through that
/// path, collapses into a generic `ServiceFailureError` with no code and no
/// `sessionId` data. Such a response is indistinguishable from a transport
/// hiccup here on purpose: this function must NOT guess "missing" from a
/// generic code, an HTTP-style 404, or free text, because doing so would
/// turn an ambiguous failure (auth, timeout, a real bug) into an automatic
/// fresh-session replay after a prompt may already have had an external
/// effect. A session that disappears through a path this narrow match does
/// not cover is a known upstream gap, not something this function should
/// paper over — see docs/gotchas/native-acp-resume-continuity.md.
fn classify_response_error(error: &Value, requested_session: Option<&str>) -> AcpError {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
    let session_id = error
        .get("data")
        .and_then(|data| data.get("sessionId"))
        .and_then(Value::as_str);
    if code == Some(JSON_RPC_INVALID_PARAMS)
        && requested_session.is_some_and(|id| {
            !id.is_empty()
                && id.trim() == id
                && !id.chars().any(char::is_control)
                && session_id == Some(id)
                && message == format!("session not found: {id}")
        })
    {
        return AcpError::SessionNotFound;
    }
    AcpError::Transport(format!("ACP response error: {error}"))
}

async fn fail_pending(pending: &PendingRequests, error: AcpError) {
    let waiters = std::mem::take(&mut *pending.lock().await);
    for (_, request) in waiters {
        let _ = request
            .sender
            .send(Err(AcpError::Transport(error.to_string())));
    }
}

/// Route one incoming agent->client JSON-RPC request through the broker.
/// `session/request_permission` always gets a "result" (a spec-shaped
/// selected/cancelled outcome, allow or deny); `fs/*`, `terminal/*` and any
/// other method Kronn does not implement get a spec-correct JSON-RPC error
/// instead of a fabricated result object.
fn handle_client_request(
    broker: &AcpPermissionBroker,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "session/request_permission" => Ok(broker.decide_tool_call_permission(method, params)),
        method if method.starts_with("fs/") || method.starts_with("terminal/") => {
            Err(broker.deny_unbound_capability(method))
        }
        other => Err(broker.deny_unknown_method(other)),
    }
}

/// Map an ACP v1 `initialize` result into Kronn's capability set.
///
/// ACP v1 baseline: every conformant agent supports session creation
/// (`session/new`), prompt turns (`session/prompt`), cancellation
/// (`session/cancel` notification) and stdio MCP servers. These are core
/// methods, not negotiated flags, so they must never be gated behind an
/// `agentCapabilities` sub-object. Session loading and session resumption are
/// separate optional capabilities: loading replays history, while resumption
/// does not. Scoped permission negotiation is optional too.
///
/// A model/mode catalogue is deliberately NOT derived here: the ACP
/// session-config-options contract returns selectable options in the
/// `session/new`/`session/load` *response*, so Kronn discovers them per session
/// instead of guessing a `modelCapabilities`/`models` object at initialize.
fn agent_capabilities(agent_caps: &Value) -> BTreeSet<AcpCapability> {
    let mut caps: BTreeSet<AcpCapability> = [
        AcpCapability::Sessions,
        AcpCapability::Streaming,
        AcpCapability::Cancellation,
        AcpCapability::McpInjection,
    ]
    .into_iter()
    .collect();
    if let Some(object) = agent_caps.as_object() {
        // `loadSession` replays history, so it is never evidence that a delta
        // can safely use `session/resume`.
        if object
            .get("loadSession")
            .map(|value| value.as_bool().unwrap_or(true))
            .unwrap_or(false)
        {
            caps.insert(AcpCapability::LoadSession);
        }
        if object
            .get("sessionCapabilities")
            .and_then(Value::as_object)
            .and_then(|capabilities| capabilities.get("resume"))
            .and_then(Value::as_object)
            .is_some()
        {
            caps.insert(AcpCapability::Resume);
        }
        if object.contains_key("permissionCapabilities") {
            caps.insert(AcpCapability::Permissions);
        }
    }
    caps
}

fn session_id(result: &Value) -> Result<String, AcpError> {
    result
        .get("sessionId")
        .or_else(|| result.get("session_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .ok_or(AcpError::InvalidSessionResponse)
}

/// The turn's usage as `session/prompt` reports it. ACP puts it on the
/// response; only some runtimes also stream it.
fn usage_from_prompt_result(result: &Value) -> Option<AcpSessionEvent> {
    let usage = result.get("usage")?;
    let input_tokens = usage
        .get("inputTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .get("outputTokens")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    (input_tokens > 0 || output_tokens > 0).then_some(AcpSessionEvent::Usage {
        input_tokens,
        output_tokens,
    })
}

/// A `{"type":"text","text":"…"}` content block, wherever it appears — on its
/// own or inside an array.
fn push_text_block(block: &Value, events: &mut Vec<AcpSessionEvent>) {
    if block.get("type").and_then(Value::as_str) != Some("text") {
        return;
    }
    if let Some(text) = block.get("text").and_then(Value::as_str) {
        events.push(AcpSessionEvent::TextDelta(text.to_owned()));
    }
}

fn events_from_notifications(messages: Vec<Value>, session_id: &str) -> Vec<AcpSessionEvent> {
    messages
        .into_iter()
        .filter_map(|message| {
            let params = message.get("params")?;
            // ACP session/update carries the session identifier. Fixtures from
            // early compatible runtimes did not, so retain those frames only
            // when the field is absent; never attribute an explicit other
            // session's update to this prompt.
            if params
                .get("sessionId")
                .and_then(Value::as_str)
                .is_some_and(|received| received != session_id)
            {
                return None;
            }
            let update = params
                .get("update")
                .or_else(|| params.get("sessionUpdate"))?;
            let mut events = Vec::new();
            // Which kind of chunk this is. A runtime that does not say keeps the
            // old behaviour — its text is the answer.
            let kind = update.get("sessionUpdate").and_then(Value::as_str);
            // The model's private reasoning, which several runtimes stream
            // before the answer. It is deliberately never shown: it is a
            // scratchpad, and concatenating it into the reply would leak it.
            let is_thought = matches!(kind, Some("agent_thought_chunk"));
            if let (Some(content), false) = (update.get("content"), is_thought) {
                match content {
                    Value::String(text) => events.push(AcpSessionEvent::TextDelta(text.to_owned())),
                    Value::Array(blocks) => {
                        for block in blocks {
                            push_text_block(block, &mut events);
                        }
                    }
                    // A single block, unwrapped: `{"type":"text","text":"…"}`.
                    // OpenCode streams every chunk this way, so the whole reply
                    // fell through this match and the room showed nothing at
                    // all — no text, and no reason for its absence.
                    Value::Object(_) => push_text_block(content, &mut events),
                    _ => {}
                }
            }
            if update.get("toolCallId").is_some() || update.get("toolCall").is_some() {
                events.push(AcpSessionEvent::ToolCall {
                    name: update
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_owned(),
                });
            }
            if let Some(usage) = update.get("usage") {
                events.push(AcpSessionEvent::Usage {
                    input_tokens: usage
                        .get("inputTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    output_tokens: usage
                        .get("outputTokens")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                });
            }
            (!events.is_empty()).then_some(events)
        })
        .flatten()
        .collect()
}

#[async_trait]
impl AcpTransport for AcpJsonRpcTransport {
    async fn initialize(
        &self,
        request: AcpInitialize,
    ) -> Result<AcpNegotiatedCapabilities, AcpError> {
        let servers: Vec<Value> = self
            .broker
            .authorize_mcp_servers(request.mcp_servers)
            .into_iter()
            .map(|server| {
                json!({
                    "name": server.id, "command": server.command, "args": server.args, "env": [],
                })
            })
            .collect();
        let result = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": request.protocol_version,
                    "clientInfo": {"name": "Kronn", "version": env!("CARGO_PKG_VERSION")},
                    // Kronn does not yet bind ACP file/terminal callbacks to
                    // its scoped workspace executor. Do not advertise those
                    // capabilities: incoming requests are refused explicitly
                    // by the dispatcher instead of becoming a false grant.
                    "clientCapabilities": {},
                }),
            )
            .await?;
        *self.session_setup.lock().await = Some(AcpSessionSetup {
            cwd: request.cwd,
            mcp_servers: servers,
        });
        Ok(AcpNegotiatedCapabilities {
            protocol_version: result
                .get("protocolVersion")
                .or_else(|| result.get("protocol_version"))
                .and_then(Value::as_u64)
                .unwrap_or(request.protocol_version as u64) as u32,
            capabilities: agent_capabilities(
                result.get("agentCapabilities").unwrap_or(&Value::Null),
            ),
        })
    }

    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        let setup = self.session_setup.lock().await.clone().ok_or_else(|| {
            AcpError::Transport("ACP session/new was called before initialize".into())
        })?;
        // Model/mode selection is NOT sent in the request: the ACP
        // session-config-options contract returns the selectable options in the
        // response, and the client applies a choice afterwards with
        // `session/set_config_option`.
        let result = self
            .request(
                "session/new",
                json!({"cwd": setup.cwd, "mcpServers": setup.mcp_servers}),
            )
            .await?;
        *self.config_options.lock().await = parse_config_options(&result);
        let id = session_id(&result)?;
        self.broker
            .bind_protocol_session(&id)
            .map_err(AcpError::Transport)?;
        AcpSessionTarget::new(self.agent, id)
    }

    async fn config_options(&self) -> Vec<AcpConfigOption> {
        self.config_options.lock().await.clone()
    }

    async fn set_config_option(
        &self,
        target: &AcpSessionTarget,
        config_id: &str,
        value_id: &str,
    ) -> Result<(), AcpError> {
        let result = self
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": target.session_id,
                    "configId": config_id,
                    "value": value_id,
                }),
            )
            .await?;
        // The response echoes the updated option set; keep it authoritative so a
        // subsequent read reflects the applied selection.
        let updated = parse_config_options(&result);
        if !updated.is_empty() {
            *self.config_options.lock().await = updated;
        } else if let Some(option) = self
            .config_options
            .lock()
            .await
            .iter_mut()
            .find(|option| option.id == config_id)
        {
            option.current = Some(value_id.to_owned());
        }
        Ok(())
    }

    async fn resume_session(&self, target: &AcpSessionTarget) -> Result<(), AcpError> {
        self.broker
            .bind_protocol_session(&target.session_id)
            .map_err(AcpError::Transport)?;
        // `session/resume` restores the session without the history replay that
        // `session/load` mandates, so it is the only lifecycle call paired with
        // a delta prompt.
        let setup = self.session_setup.lock().await.clone().ok_or_else(|| {
            AcpError::Transport("ACP session/resume was called before initialize".into())
        })?;
        let result = self
            .request(
                "session/resume",
                json!({
                    "sessionId": target.session_id,
                    "cwd": setup.cwd,
                    "mcpServers": setup.mcp_servers,
                }),
            )
            .await?;
        // A resumed session may re-advertise its config options; refresh them.
        let options = parse_config_options(&result);
        if !options.is_empty() {
            *self.config_options.lock().await = options;
        }
        Ok(())
    }

    async fn prompt(
        &self,
        target: &AcpSessionTarget,
        prompt: &str,
        events: mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        let mut notifications = self.notifications.subscribe();
        // Subscribe before sending the request. ACP delivers session/update
        // notifications while the prompt request is outstanding; each is
        // forwarded immediately so the UI streams during the turn rather than
        // receiving one batch after the response.
        let request = self.request(
            "session/prompt",
            json!({"sessionId": target.session_id, "prompt": [{"type": "text", "text": prompt}]}),
        );
        tokio::pin!(request);
        loop {
            tokio::select! {
                result = &mut request => {
                    let outcome = result?;
                    while let Ok(frame) = notifications.try_recv() {
                        for event in events_from_notifications(vec![frame], &target.session_id) {
                            let _ = events.send(event).await;
                        }
                    }
                    // The turn's own token count lives on the response, not in
                    // the stream: a runtime that reports usage only here left
                    // the turn recorded at zero tokens, so a room showed a
                    // reply that had apparently cost nothing.
                    if let Some(usage) = usage_from_prompt_result(&outcome) {
                        let _ = events.send(usage).await;
                    }
                    let _ = events.send(AcpSessionEvent::Completed).await;
                    return Ok(());
                }
                received = notifications.recv() => match received {
                    Ok(frame) => {
                        for event in events_from_notifications(vec![frame], &target.session_id) {
                            let _ = events.send(event).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(dropped)) => {
                        return Err(AcpError::Transport(format!(
                            "ACP session updates exceeded the bounded host buffer ({dropped} frames lost)"
                        )));
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err(AcpError::Transport("ACP notification dispatcher stopped".into()));
                    }
                },
            }
        }
    }

    async fn cancel(&self, target: &AcpSessionTarget) -> Result<(), AcpError> {
        // ACP cancellation is a notification: waiting for a response can block
        // behind the active `session/prompt` request and prevent interruption.
        self.send(json!({"jsonrpc":"2.0", "method":"session/cancel", "params":{"sessionId": target.session_id}})).await
    }

    async fn shutdown(&self) -> Result<(), AcpError> {
        // Hold the lifecycle lock through completion so concurrent calls are
        // idempotent: the first caller reaps and joins, later callers observe
        // an already-finished lifecycle instead of racing the reap.
        let mut process = self.process.lock().await;
        let mut errors = Vec::new();
        if let Some(mut child) = process.child.take() {
            if let Err(error) = child.start_kill() {
                errors.push(format!("stop ACP process: {error}"));
            }
            if let Err(error) = child.wait().await {
                errors.push(format!("wait for ACP process: {error}"));
            }
        }
        if let Some(dispatcher) = process.dispatcher.take() {
            // Reaping the child does not close its stdout when a DESCENDANT
            // inherited the pipe, so the drain can outlive the process. The
            // owner bounds the join and cancels; a cancellation is not an error
            // here because the task IS stopped and nothing leaks — and
            // `acp_discovery::discover_with_transport` maps any shutdown error
            // onto its outcome, so failing would turn a successful catalogue
            // discovery into a provider error. Claims stop at the drain: this
            // says nothing about a descendant, which Kronn does not own.
            if let Err(error) = dispatcher.finish().await {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AcpError::Transport(errors.join("; ")))
        }
    }
}

pub struct AcpHost {
    maximum_protocol_version: u32,
    transport: Arc<dyn AcpTransport>,
    negotiated: Option<AcpNegotiatedCapabilities>,
}

impl AcpHost {
    pub fn new(maximum_protocol_version: u32, transport: Arc<dyn AcpTransport>) -> Self {
        Self {
            maximum_protocol_version,
            transport,
            negotiated: None,
        }
    }

    pub async fn negotiate(
        &mut self,
        request: AcpInitialize,
    ) -> Result<&AcpNegotiatedCapabilities, AcpError> {
        let response = self.transport.initialize(request).await?;
        if response.protocol_version > self.maximum_protocol_version {
            return Err(AcpError::UnsupportedProtocolVersion {
                actual: response.protocol_version,
                maximum: self.maximum_protocol_version,
            });
        }
        self.negotiated = Some(response);
        Ok(self
            .negotiated
            .as_ref()
            .expect("negotiated response was just stored"))
    }

    pub async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        self.require(AcpCapability::Sessions)?;
        self.transport.create_session().await
    }

    /// The model/mode options discovered from the current session response.
    pub async fn config_options(&self) -> Vec<AcpConfigOption> {
        self.transport.config_options().await
    }

    /// Apply a tier/model choice to an existing session by matching it against
    /// the options the session actually returned, then calling
    /// `session/set_config_option`. Returns `true` when a matching option value
    /// was found and applied; `false` is a deliberate no-op (no catalogue or no
    /// match) so a catalogue-less agent keeps its own default rather than
    /// receiving a spurious selection.
    pub async fn select_model(
        &self,
        target: &AcpSessionTarget,
        model: &str,
    ) -> Result<bool, AcpError> {
        let options = self.transport.config_options().await;
        for option in &options {
            if let Some(value) = option
                .available
                .iter()
                .find(|value| value.id == model || value.name == model)
            {
                self.transport
                    .set_config_option(target, &option.id, &value.id)
                    .await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub async fn resume_session(&self, target: &AcpSessionTarget) -> Result<(), AcpError> {
        self.require(AcpCapability::Resume)?;
        self.transport.resume_session(target).await
    }

    pub async fn prompt(
        &self,
        target: &AcpSessionTarget,
        prompt: &str,
        events: mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        self.require(AcpCapability::Streaming)?;
        self.transport.prompt(target, prompt, events).await
    }

    pub async fn cancel(&self, target: &AcpSessionTarget) -> Result<(), AcpError> {
        self.require(AcpCapability::Cancellation)?;
        self.transport.cancel(target).await
    }

    pub async fn shutdown(&self) -> Result<(), AcpError> {
        self.transport.shutdown().await
    }

    /// Refuse an optional session feature unless the runtime negotiated it.
    /// Callers use this for declarations which must never be silently ignored
    /// (for example, Kronn's project-scoped MCP registry).
    pub fn require_capability(&self, capability: AcpCapability) -> Result<(), AcpError> {
        self.require(capability)
    }

    fn require(&self, capability: AcpCapability) -> Result<(), AcpError> {
        self.negotiated
            .as_ref()
            .is_some_and(|negotiated| negotiated.capabilities.contains(&capability))
            .then_some(())
            .ok_or(AcpError::CapabilityUnavailable { capability })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Only the unix liveness fixtures read from a pipe; gate it so a non-unix
    /// build does not carry an unused import.
    #[cfg(unix)]
    use tokio::io::AsyncReadExt;

    struct FakeTransport;

    #[async_trait]
    impl AcpTransport for FakeTransport {
        async fn initialize(
            &self,
            _: AcpInitialize,
        ) -> Result<AcpNegotiatedCapabilities, AcpError> {
            Ok(AcpNegotiatedCapabilities {
                protocol_version: 1,
                capabilities: [AcpCapability::Sessions, AcpCapability::Streaming]
                    .into_iter()
                    .collect(),
            })
        }
        async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
            AcpSessionTarget::new(AcpAgent::OpenCode, "session-1")
        }
        async fn config_options(&self) -> Vec<AcpConfigOption> {
            Vec::new()
        }
        async fn set_config_option(
            &self,
            _: &AcpSessionTarget,
            _: &str,
            _: &str,
        ) -> Result<(), AcpError> {
            Ok(())
        }
        async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
            Ok(())
        }
        async fn prompt(
            &self,
            _: &AcpSessionTarget,
            _: &str,
            events: mpsc::Sender<AcpSessionEvent>,
        ) -> Result<(), AcpError> {
            let _ = events.send(AcpSessionEvent::Completed).await;
            Ok(())
        }
        async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<(), AcpError> {
            Ok(())
        }
    }

    fn request() -> AcpInitialize {
        AcpInitialize {
            protocol_version: 1,
            cwd: "/workspace".into(),
            mcp_servers: vec![],
        }
    }

    /// Exposes a session-scoped config option (mirroring the ACP
    /// session-config-options response) and records the exact
    /// `session/set_config_option` call, so the host's model selection is
    /// verified without a live agent.
    struct ModelTransport {
        options: Vec<AcpConfigOption>,
        recorded: Arc<Mutex<Option<(String, String)>>>,
    }

    #[async_trait]
    impl AcpTransport for ModelTransport {
        async fn initialize(
            &self,
            _: AcpInitialize,
        ) -> Result<AcpNegotiatedCapabilities, AcpError> {
            Ok(AcpNegotiatedCapabilities {
                protocol_version: 1,
                capabilities: [AcpCapability::Sessions, AcpCapability::Streaming]
                    .into_iter()
                    .collect(),
            })
        }
        async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
            AcpSessionTarget::new(AcpAgent::OpenCode, "session-model")
        }
        async fn config_options(&self) -> Vec<AcpConfigOption> {
            self.options.clone()
        }
        async fn set_config_option(
            &self,
            _: &AcpSessionTarget,
            config_id: &str,
            value_id: &str,
        ) -> Result<(), AcpError> {
            *self.recorded.lock().await = Some((config_id.to_owned(), value_id.to_owned()));
            Ok(())
        }
        async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
            Ok(())
        }
        async fn prompt(
            &self,
            _: &AcpSessionTarget,
            _: &str,
            events: mpsc::Sender<AcpSessionEvent>,
        ) -> Result<(), AcpError> {
            let _ = events.send(AcpSessionEvent::Completed).await;
            Ok(())
        }
        async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<(), AcpError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn model_is_selected_via_set_config_option_only_when_the_session_offers_it() {
        let model_option = AcpConfigOption {
            id: "model".into(),
            current: Some("sonnet".into()),
            available: vec![
                AcpConfigValue {
                    id: "sonnet".into(),
                    name: "Claude Sonnet".into(),
                },
                AcpConfigValue {
                    id: "opus".into(),
                    name: "Claude Opus".into(),
                },
            ],
        };

        // A session offering the option applies the exact configId/value via
        // session/set_config_option, matching by id.
        let recorded = Arc::new(Mutex::new(None));
        let mut host = AcpHost::new(
            1,
            Arc::new(ModelTransport {
                options: vec![model_option.clone()],
                recorded: recorded.clone(),
            }),
        );
        host.negotiate(request()).await.unwrap();
        let target = host.create_session().await.unwrap();
        assert!(host.select_model(&target, "opus").await.unwrap());
        assert_eq!(
            recorded
                .lock()
                .await
                .as_ref()
                .map(|(c, v)| (c.as_str(), v.as_str())),
            Some(("model", "opus"))
        );

        // A session that offers no matching option is a deliberate no-op: the
        // agent keeps its own default instead of a spurious selection.
        let recorded = Arc::new(Mutex::new(None));
        let mut host = AcpHost::new(
            1,
            Arc::new(ModelTransport {
                options: Vec::new(),
                recorded: recorded.clone(),
            }),
        );
        host.negotiate(request()).await.unwrap();
        let target = host.create_session().await.unwrap();
        assert!(!host.select_model(&target, "opus").await.unwrap());
        assert_eq!(*recorded.lock().await, None);
    }

    #[test]
    fn config_options_are_parsed_from_the_session_response_not_initialize() {
        // The ACP session-config-options contract returns options in the
        // session/new response; parse them tolerantly and ignore initialize.
        let options = parse_config_options(&json!({
            "sessionId": "s1",
            "configOptions": [{
                "id": "model",
                "value": "sonnet",
                "availableValues": [
                    {"id": "sonnet", "name": "Claude Sonnet"},
                    {"id": "opus", "name": "Claude Opus"}
                ]
            }]
        }));
        assert_eq!(
            options,
            vec![AcpConfigOption {
                id: "model".into(),
                current: Some("sonnet".into()),
                available: vec![
                    AcpConfigValue {
                        id: "sonnet".into(),
                        name: "Claude Sonnet".into()
                    },
                    AcpConfigValue {
                        id: "opus".into(),
                        name: "Claude Opus".into()
                    },
                ],
            }]
        );
        // No configOptions => no fabricated catalogue.
        assert!(parse_config_options(&json!({"sessionId": "s1"})).is_empty());
    }

    /// Captured verbatim from `opencode acp` on 2026-09-03. None of these key
    /// names is standardized, and reading only the ones we already knew made
    /// OpenCode's whole catalogue look absent — which gets the agent refused
    /// at preflight, for every turn, with "no live discovery path".
    #[test]
    fn config_options_are_parsed_from_the_shape_opencode_sends() {
        let options = parse_config_options(&json!({
            "sessionId": "ses_f96d2f31",
            "configOptions": [{
                "id": "model",
                "name": "Model",
                "category": "model",
                "type": "select",
                "currentValue": "opencode/big-pickle",
                "options": [
                    {"value": "opencode/big-pickle", "name": "OpenCode Zen/Big Pickle"},
                    {"value": "opencode/mimo-v2.5-free", "name": "OpenCode Zen/MiMo V2.5 Free"}
                ]
            }]
        }));
        assert_eq!(
            options,
            vec![AcpConfigOption {
                id: "model".into(),
                current: Some("opencode/big-pickle".into()),
                available: vec![
                    AcpConfigValue {
                        id: "opencode/big-pickle".into(),
                        name: "OpenCode Zen/Big Pickle".into()
                    },
                    AcpConfigValue {
                        id: "opencode/mimo-v2.5-free".into(),
                        name: "OpenCode Zen/MiMo V2.5 Free".into()
                    },
                ],
            }]
        );
    }

    #[tokio::test]
    async fn host_negotiates_then_rejects_an_unadvertised_capability() {
        let mut host = AcpHost::new(1, Arc::new(FakeTransport));
        host.negotiate(request()).await.unwrap();

        let target = host.create_session().await.unwrap();
        assert_eq!(target.agent, AcpAgent::OpenCode);
        assert_eq!(
            host.resume_session(&target).await.unwrap_err(),
            AcpError::CapabilityUnavailable {
                capability: AcpCapability::Resume
            }
        );
        assert_eq!(
            host.require_capability(AcpCapability::McpInjection)
                .unwrap_err(),
            AcpError::CapabilityUnavailable {
                capability: AcpCapability::McpInjection
            }
        );
    }

    #[tokio::test]
    async fn host_rejects_a_newer_protocol_before_using_it() {
        struct NewerTransport;
        #[async_trait]
        impl AcpTransport for NewerTransport {
            async fn initialize(
                &self,
                _: AcpInitialize,
            ) -> Result<AcpNegotiatedCapabilities, AcpError> {
                Ok(AcpNegotiatedCapabilities {
                    protocol_version: 2,
                    capabilities: BTreeSet::new(),
                })
            }
            async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
                unreachable!()
            }
            async fn config_options(&self) -> Vec<AcpConfigOption> {
                unreachable!()
            }
            async fn set_config_option(
                &self,
                _: &AcpSessionTarget,
                _: &str,
                _: &str,
            ) -> Result<(), AcpError> {
                unreachable!()
            }
            async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
                unreachable!()
            }
            async fn prompt(
                &self,
                _: &AcpSessionTarget,
                _: &str,
                _: mpsc::Sender<AcpSessionEvent>,
            ) -> Result<(), AcpError> {
                unreachable!()
            }
            async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
                unreachable!()
            }
            async fn shutdown(&self) -> Result<(), AcpError> {
                unreachable!()
            }
        }

        let mut host = AcpHost::new(1, Arc::new(NewerTransport));
        assert_eq!(
            host.negotiate(request()).await.unwrap_err(),
            AcpError::UnsupportedProtocolVersion {
                actual: 2,
                maximum: 1
            }
        );
    }

    #[test]
    fn session_target_never_accepts_an_unknown_empty_identifier() {
        assert_eq!(
            AcpSessionTarget::new(AcpAgent::OpenCode, "  ").unwrap_err(),
            AcpError::InvalidSessionTarget
        );
    }

    #[test]
    fn production_routes_never_overstate_unwired_acp_adapters() {
        for agent in [
            AgentType::OpenCode,
            AgentType::GeminiCli,
            AgentType::CopilotCli,
            AgentType::Kiro,
            AgentType::Vibe,
        ] {
            assert_eq!(production_route(&agent), AcpProductionRoute::NativeAcp);
            let acp = acp_agent(&agent).expect("native ACP agent maps to a runtime");
            assert!(
                native_acp_command(acp).is_some(),
                "{agent:?} native route must have a verified ACP command"
            );
        }
        for agent in [AgentType::Codex, AgentType::ClaudeCode] {
            assert_eq!(
                production_route(&agent),
                AcpProductionRoute::DirectCliMigration
            );
            assert_ne!(acp_agent(&agent), None);
        }
        for agent in [
            AgentType::Ollama,
            AgentType::LiteLlm,
            AgentType::Nvidia,
            AgentType::Custom,
        ] {
            assert_eq!(
                production_route(&agent),
                AcpProductionRoute::HttpModelProvider
            );
            assert_eq!(acp_agent(&agent), None);
        }
    }

    #[test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    fn the_adapted_route_is_off_by_default_and_never_widens_other_agents() {
        std::env::remove_var("KRONN_ACP_ADAPTER_CODEX");
        std::env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::DirectCliMigration
        );
        assert_eq!(
            resolve_acp_route(&AgentType::ClaudeCode),
            AcpProductionRoute::DirectCliMigration
        );
        // A route that was never DirectCliMigration to begin with must never
        // become AdaptedAcp, no matter what the toggle says.
        for agent in [AgentType::OpenCode, AgentType::Ollama] {
            assert_eq!(resolve_acp_route(&agent), production_route(&agent));
        }
    }

    #[test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    fn each_agent_s_toggle_only_widens_that_agent_s_own_route() {
        std::env::remove_var("KRONN_ACP_ADAPTER_CODEX");
        std::env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
        std::env::set_var("KRONN_ACP_ADAPTER_CODEX", "1");
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::AdaptedAcp
        );
        assert_eq!(
            resolve_acp_route(&AgentType::ClaudeCode),
            AcpProductionRoute::DirectCliMigration,
            "Claude's route must stay unaffected by Codex's toggle"
        );
        std::env::remove_var("KRONN_ACP_ADAPTER_CODEX");

        std::env::set_var("KRONN_ACP_ADAPTER_CLAUDE", "1");
        assert_eq!(
            resolve_acp_route(&AgentType::ClaudeCode),
            AcpProductionRoute::AdaptedAcp
        );
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::DirectCliMigration,
            "Codex's route must stay unaffected by Claude's toggle"
        );
        std::env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
    }

    #[test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    fn only_1_or_true_activate_the_adapter_toggle_every_other_value_stays_direct_cli() {
        // Regression: `.is_ok()` used to activate the adapter for ANY
        // present value, including the exact strings an operator would type
        // to mean "off" (`"0"`, `"false"`) without realizing only unsetting
        // the variable actually disables it.
        for falsy in ["0", "false", "False", "FALSE", "no", "off", "", "  ", "2"] {
            std::env::set_var("KRONN_ACP_ADAPTER_CODEX", falsy);
            assert_eq!(
                resolve_acp_route(&AgentType::Codex),
                AcpProductionRoute::DirectCliMigration,
                "KRONN_ACP_ADAPTER_CODEX={falsy:?} must NOT activate the adapter"
            );
        }
        for truthy in ["1", "true", "True", "TRUE", " 1 ", " true "] {
            std::env::set_var("KRONN_ACP_ADAPTER_CODEX", truthy);
            assert_eq!(
                resolve_acp_route(&AgentType::Codex),
                AcpProductionRoute::AdaptedAcp,
                "KRONN_ACP_ADAPTER_CODEX={truthy:?} must activate the adapter"
            );
        }
        std::env::remove_var("KRONN_ACP_ADAPTER_CODEX");
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::DirectCliMigration,
            "unset must stay direct-CLI"
        );
    }

    #[test]
    fn native_acp_commands_match_the_verified_vendor_syntax() {
        assert_eq!(
            native_acp_command(AcpAgent::OpenCode),
            Some(("opencode", vec!["acp"]))
        );
        assert_eq!(
            native_acp_command(AcpAgent::GeminiCli),
            Some(("gemini", vec!["--acp"]))
        );
        // Regression: Copilot must be `copilot --acp`, never an unjustified
        // `--stdio`; Kiro must be `kiro-cli acp`, not a direct-CLI fallback.
        assert_eq!(
            native_acp_command(AcpAgent::CopilotCli),
            Some(("copilot", vec!["--acp"]))
        );
        assert_eq!(
            native_acp_command(AcpAgent::Kiro),
            Some(("kiro-cli", vec!["acp"]))
        );
        assert_eq!(
            native_acp_command(AcpAgent::Vibe),
            Some(("vibe-acp", vec![]))
        );
        assert_eq!(native_acp_command(AcpAgent::Codex), None);
        assert_eq!(native_acp_command(AcpAgent::ClaudeCode), None);
    }

    #[test]
    fn acp_v1_baseline_capabilities_are_never_gated_behind_optional_flags() {
        // A minimal ACP agent that advertises no optional capabilities still
        // supports sessions, prompt turns, cancellation and stdio MCP.
        let baseline = agent_capabilities(&json!({}));
        assert!(baseline.contains(&AcpCapability::Sessions));
        assert!(baseline.contains(&AcpCapability::Streaming));
        assert!(baseline.contains(&AcpCapability::Cancellation));
        assert!(baseline.contains(&AcpCapability::McpInjection));
        assert!(!baseline.contains(&AcpCapability::LoadSession));
        assert!(!baseline.contains(&AcpCapability::Resume));

        // Optional capabilities are added only when genuinely advertised. A
        // model catalogue is NOT one of them: it is discovered from the
        // session response, never from a fabricated initialize object.
        let extended = agent_capabilities(&json!({
            "loadSession": true,
            "sessionCapabilities": {"resume": {}},
            "modelCapabilities": {}, "permissionCapabilities": {}
        }));
        assert!(extended.contains(&AcpCapability::LoadSession));
        assert!(extended.contains(&AcpCapability::Resume));
        assert!(extended.contains(&AcpCapability::Permissions));

        // Loading is not resumption: neither a false nor a true load flag may
        // enable `session/resume` by itself.
        assert!(
            !agent_capabilities(&json!({"loadSession": false})).contains(&AcpCapability::Resume)
        );
        assert!(!agent_capabilities(&json!({"loadSession": true})).contains(&AcpCapability::Resume));
    }

    #[test]
    fn null_or_malformed_resume_capabilities_do_not_advertise_resume() {
        // ACP explicitly permits null to mean unsupported; a present key is
        // not sufficient. Other non-object values cannot negotiate support.
        for value in [
            Value::Null,
            json!(false),
            json!(true),
            json!(0),
            json!("yes"),
            json!([]),
        ] {
            assert!(
                !agent_capabilities(&json!({"sessionCapabilities": {"resume": value}}))
                    .contains(&AcpCapability::Resume),
                "non-object resume advertisement must not cause session/resume: {value}"
            );
        }
        for value in [json!({}), json!({"_meta": {"vendor": "fixture"}})] {
            assert!(
                agent_capabilities(&json!({"sessionCapabilities": {"resume": value}}))
                    .contains(&AcpCapability::Resume)
            );
        }
    }

    #[test]
    fn json_rpc_notifications_are_not_interpreted_as_claude_stream_json() {
        let events = events_from_notifications(
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"update": {"content": [{"type":"text", "text":"ACP delta"}]}}
            })],
            "fixture-session",
        );
        assert_eq!(events, vec![AcpSessionEvent::TextDelta("ACP delta".into())]);
    }

    /// KT-543 — captured from a live `opencode acp` run. Every chunk arrives as
    /// a bare content OBJECT, which the parser handled neither as a string nor
    /// as an array: the whole reply fell through and the room showed nothing,
    /// with no reason for the blank.
    #[test]
    fn a_single_content_block_is_read_as_text() {
        let events = events_from_notifications(
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "ses_live",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": "msg_1",
                        "content": {"type": "text", "text": "Bonjour"}
                    }
                }
            })],
            "ses_live",
        );
        assert_eq!(events, vec![AcpSessionEvent::TextDelta("Bonjour".into())]);
    }

    /// The same runtime streams its reasoning first, in the same shape. It is a
    /// scratchpad: reading the block must not mean showing it.
    #[test]
    fn a_thought_chunk_is_never_forwarded_as_text() {
        let events = events_from_notifications(
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "ses_live",
                    "update": {
                        "sessionUpdate": "agent_thought_chunk",
                        "messageId": "msg_1",
                        "content": {"type": "text", "text": "The user is greeting me in French."}
                    }
                }
            })],
            "ses_live",
        );
        assert!(
            events.is_empty(),
            "reasoning leaked into the reply: {events:?}"
        );
    }

    /// A runtime that does not label its chunks keeps the behaviour it had.
    #[test]
    fn an_unlabelled_chunk_is_still_treated_as_the_answer() {
        let events = events_from_notifications(
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {"sessionId": "s1", "update": {"content": "hello"}}
            })],
            "s1",
        );
        assert_eq!(events, vec![AcpSessionEvent::TextDelta("hello".into())]);
    }

    /// ACP reports the turn's tokens on the response. Dropping it recorded the
    /// turn at zero — a reply that had apparently cost nothing.
    #[test]
    fn usage_is_taken_from_the_prompt_response() {
        let result = json!({
            "stopReason": "end_turn",
            "usage": {"inputTokens": 6126, "outputTokens": 28, "totalTokens": 7946}
        });
        assert_eq!(
            usage_from_prompt_result(&result),
            Some(AcpSessionEvent::Usage {
                input_tokens: 6126,
                output_tokens: 28
            }),
        );
    }

    #[test]
    fn a_response_without_usage_reports_none() {
        assert_eq!(
            usage_from_prompt_result(&json!({"stopReason": "end_turn"})),
            None
        );
        assert_eq!(
            usage_from_prompt_result(&json!({"usage": {"inputTokens": 0, "outputTokens": 0}})),
            None,
        );
    }

    #[test]
    fn session_updates_are_not_attributed_across_sessions() {
        let events = events_from_notifications(
            vec![json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "other-session",
                    "update": {"content": [{"type":"text", "text":"wrong"}]}
                }
            })],
            "expected-session",
        );
        assert!(events.is_empty());
    }

    #[test]
    fn fs_and_terminal_requests_get_a_json_rpc_error_never_a_fabricated_result() {
        let broker = AcpPermissionBroker::new(true);
        for method in ["fs/read_text_file", "fs/write_text_file", "terminal/create"] {
            let outcome = handle_client_request(&broker, method, &Value::Null);
            let (code, message) = outcome.expect_err("fs/terminal must never be granted a result");
            assert_eq!(code, permission_broker::ACP_CAPABILITY_NOT_GRANTED);
            assert!(message.contains(method));
        }
    }

    #[test]
    fn an_unimplemented_method_is_a_standard_json_rpc_method_not_found() {
        let broker = AcpPermissionBroker::new(true);
        let (code, message) =
            handle_client_request(&broker, "session/some_future_method", &Value::Null)
                .expect_err("unknown methods must be refused");
        assert_eq!(code, permission_broker::ACP_METHOD_NOT_FOUND);
        assert!(message.contains("session/some_future_method"));
    }

    #[test]
    fn a_permission_request_is_routed_to_the_broker_and_returns_a_result_not_an_error() {
        let broker = AcpPermissionBroker::new(false);
        let params = json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "call-1", "kind": "read"},
            "options": [{"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"}]
        });
        let result = handle_client_request(&broker, "session/request_permission", &params)
            .expect("a read-kind tool call must be granted a result, not an error");
        assert_eq!(
            result,
            json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
        );
    }

    #[tokio::test]
    async fn json_rpc_transport_keeps_updates_emitted_before_prompt_response() {
        let mut command = crate::core::cmd::async_cmd("sh");
        command.args(["-c", "while IFS= read -r line; do case \"$line\" in *'\"method\":\"initialize\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"sessionCapabilities\":{},\"promptCapabilities\":{},\"sessionCancellation\":{}}}}' ;; *'\"method\":\"session/new\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"fixture-session\"}}' ;; *'\"method\":\"session/prompt\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"update\":{\"content\":[{\"type\":\"text\",\"text\":\"before response\"}],\"usage\":{\"inputTokens\":3,\"outputTokens\":5}}}}'; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}' ;; esac; done"]);
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        transport.initialize(request()).await.unwrap();
        let target = transport.create_session().await.unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        transport
            .prompt(&target, "fixture prompt", tx)
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert_eq!(
            events,
            vec![
                AcpSessionEvent::TextDelta("before response".into()),
                AcpSessionEvent::Usage {
                    input_tokens: 3,
                    output_tokens: 5
                },
                AcpSessionEvent::Completed,
            ]
        );
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn dispatcher_denies_a_live_fs_request_with_a_spec_correct_json_rpc_error() {
        // End-to-end coverage that the running dispatcher — not just the pure
        // `handle_client_request` helper — actually routes an inbound
        // agent->client request through the broker. The fixture agent sends
        // an `fs/read_text_file` request mid-turn, captures Kronn's reply to
        // a file, then completes the turn normally.
        let response_file = tempfile::NamedTempFile::new().unwrap();
        let response_path = response_file.path().to_path_buf();
        let mut command = crate::core::cmd::async_cmd("sh");
        command
            .env("RESPONSE_FILE", &response_path)
            .args(["-c", "while IFS= read -r line; do case \"$line\" in *'\"method\":\"initialize\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1}}' ;; *'\"method\":\"session/new\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"sessionId\":\"fixture-session\"}}' ;; *'\"method\":\"session/prompt\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"fs/read_text_file\",\"params\":{\"sessionId\":\"fixture-session\",\"path\":\"/tmp/x\"}}'; IFS= read -r reply; printf '%s' \"$reply\" > \"$RESPONSE_FILE\"; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{}}' ;; esac; done"]);
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        transport.initialize(request()).await.unwrap();
        let target = transport.create_session().await.unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        transport
            .prompt(&target, "fixture prompt", tx)
            .await
            .unwrap();
        while rx.recv().await.is_some() {}
        transport.shutdown().await.unwrap();

        let raw = std::fs::read_to_string(&response_path).unwrap();
        let reply: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(reply["id"], json!(99));
        assert_eq!(
            reply["error"]["code"],
            json!(permission_broker::ACP_CAPABILITY_NOT_GRANTED)
        );
        assert!(reply.get("result").is_none());

        let audited = transport
            .permission_audit_log()
            .into_iter()
            .find(|entry| entry.method == "fs/read_text_file")
            .expect("the fs/read_text_file decision must be audited");
        assert_eq!(audited.verdict, AcpPermissionVerdict::Deny);
    }

    #[test]
    fn session_response_rejects_missing_or_blank_ids() {
        assert_eq!(
            session_id(&json!({})).unwrap_err(),
            AcpError::InvalidSessionResponse
        );
        assert_eq!(
            session_id(&json!({"sessionId": " "})).unwrap_err(),
            AcpError::InvalidSessionResponse
        );
        assert_eq!(session_id(&json!({"sessionId": "acp-1"})).unwrap(), "acp-1");
    }

    #[test]
    fn classify_response_error_maps_only_the_exact_opencode_missing_session_shape() {
        let matching = json!({"code": -32602, "message": "session not found: native-id", "data": {"sessionId": "native-id"}});
        assert_eq!(
            classify_response_error(&matching, Some("native-id")),
            AcpError::SessionNotFound
        );
        for requested in [None, Some("different-id")] {
            assert!(matches!(
                classify_response_error(&matching, requested),
                AcpError::Transport(_)
            ));
        }
        for error in [
            json!({"code": -32602, "message": "session not found: native-id", "data": {"sessionId": "different-id"}}),
            json!({"code": -32602, "message": "session not found: native-id because authentication failed", "data": {"sessionId": "native-id"}}),
            json!({"code": -32602, "message": "session not found: native-id"}),
            json!({"code": -32000, "message": "session not found: native-id", "data": {"sessionId": "native-id"}}),
            json!({"code": -32603, "message": "internal error"}),
            json!({"code": -32002, "message": "resource not found"}),
        ] {
            assert!(
                matches!(
                    classify_response_error(&error, Some("native-id")),
                    AcpError::Transport(_)
                ),
                "only the exact structured error for the requested session permits replay"
            );
        }
    }

    #[tokio::test]
    async fn resume_session_over_the_real_transport_maps_the_structured_session_not_found_error() {
        let mut command = crate::core::cmd::async_cmd("sh");
        command.args(["-c", "while IFS= read -r line; do case \"$line\" in *'\"method\":\"initialize\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"sessionCapabilities\":{\"resume\":{}}}}}' ;; *'\"method\":\"session/resume\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"error\":{\"code\":-32602,\"message\":\"session not found: gone-id\",\"data\":{\"sessionId\":\"gone-id\"}}}' ;; esac; done"]);
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        transport.initialize(request()).await.unwrap();
        let target = AcpSessionTarget::new(AcpAgent::OpenCode, "gone-id").unwrap();
        assert_eq!(
            transport.resume_session(&target).await.unwrap_err(),
            AcpError::SessionNotFound
        );
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn resume_session_over_the_real_transport_keeps_an_ambiguous_error_as_transport() {
        // A timeout/auth/unstructured failure from the real process must never
        // be read as a positively-identified missing session: the caller
        // (run_acp_session) fails closed on `Transport` instead of silently
        // starting a replacement session and resending the prompt.
        let mut command = crate::core::cmd::async_cmd("sh");
        command.args(["-c", "while IFS= read -r line; do case \"$line\" in *'\"method\":\"initialize\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":1,\"agentCapabilities\":{\"sessionCapabilities\":{\"resume\":{}}}}}' ;; *'\"method\":\"session/resume\"'*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"error\":{\"code\":-32603,\"message\":\"internal error\"}}' ;; esac; done"]);
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        transport.initialize(request()).await.unwrap();
        let target = AcpSessionTarget::new(AcpAgent::OpenCode, "ambiguous-id").unwrap();
        let error = transport.resume_session(&target).await.unwrap_err();
        assert!(matches!(error, AcpError::Transport(_)));
        assert_ne!(error, AcpError::SessionNotFound);
        transport.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn missing_session_fallback_is_bound_to_the_exact_opencode_resume_request() {
        for (agent, method, requested, expected_missing) in [
            (AcpAgent::OpenCode, "session/resume", "gone-id", true),
            (AcpAgent::OpenCode, "session/resume", "another-id", false),
            (AcpAgent::OpenCode, "session/prompt", "gone-id", false),
            (AcpAgent::Codex, "session/resume", "gone-id", false),
        ] {
            let mut command = crate::core::cmd::async_cmd("sh");
            command.args(["-c", r#"while IFS= read -r line; do
case "$line" in
*'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"resume":{}}}}}' ;;
*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32602,"message":"session not found: gone-id","data":{"sessionId":"gone-id"}}}' ;;
esac
done"#]);
            let transport = AcpJsonRpcTransport::spawn(agent, command, false)
                .await
                .unwrap();
            transport.initialize(request()).await.unwrap();
            let error = transport
                .request(method, json!({"sessionId": requested}))
                .await
                .unwrap_err();
            if expected_missing {
                assert_eq!(error, AcpError::SessionNotFound);
            } else {
                assert!(matches!(error, AcpError::Transport(_)));
            }
            transport.shutdown().await.unwrap();
        }
    }

    /// Ownership of the drain, proven without a subprocess and without timing:
    /// the spawned task holds a `oneshot` sender, so the receiver resolves the
    /// moment that task is dropped. Awaiting the receiver IS the event — there
    /// is nothing to poll and nothing to sleep on.
    #[tokio::test]
    async fn dropping_the_dispatcher_owner_cancels_the_drain() {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let owner = DispatcherOwner::new(tokio::spawn(async move {
            let _sender = sender;
            std::future::pending::<()>().await;
        }));

        drop(owner);

        receiver
            .await
            .expect_err("dropping the owner must cancel the drain, dropping its sender");
    }

    /// A transport dropped WITHOUT `shutdown` must not leave the drain running.
    /// `kill_on_drop` covers the child; this covers the task.
    #[tokio::test]
    async fn dropping_the_process_record_cancels_the_drain() {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let process = AcpProcess {
            child: None,
            dispatcher: Some(DispatcherOwner::new(tokio::spawn(async move {
                let _sender = sender;
                std::future::pending::<()>().await;
            }))),
        };

        drop(process);

        receiver
            .await
            .expect_err("dropping the process record must cancel the drain");
    }

    /// `finish` returns only once the drain has actually stopped. A task that
    /// ends on its own is joined, not abandoned.
    #[tokio::test]
    async fn finishing_the_owner_waits_for_a_drain_that_ends_by_itself() {
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let owner = DispatcherOwner::new(tokio::spawn(async move {
            drop(sender);
        }));

        owner.finish().await.unwrap();

        receiver
            .await
            .expect_err("the drain must have run to completion before finish returned");
    }

    /// The owner is consumed by `finish`, so a second shutdown finds no
    /// dispatcher left and must stay a no-op rather than double-abort.
    #[tokio::test]
    async fn finishing_an_owner_twice_is_not_possible_and_an_empty_one_succeeds() {
        DispatcherOwner(None).finish().await.unwrap();
    }

    /// A `shutdown` future cancelled MID-AWAIT must still stop the drain.
    ///
    /// The future is polled once before being dropped, on purpose: an unpolled
    /// `async fn` has not run its body at all, so dropping it would exercise
    /// nothing and pass for the wrong reason. Polling first puts it inside the
    /// join, which is the exact window where taking the handle into a local
    /// would detach it.
    #[tokio::test]
    async fn cancelling_a_shutdown_in_flight_still_cancels_the_drain() {
        use std::future::Future;
        use std::task::Poll;

        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let owner = DispatcherOwner::new(tokio::spawn(async move {
            let _sender = sender;
            std::future::pending::<()>().await;
        }));

        let mut in_flight = Box::pin(owner.finish());
        let entered = std::future::poll_fn(|cx| Poll::Ready(in_flight.as_mut().poll(cx))).await;
        assert!(
            entered.is_pending(),
            "the drain never ends on its own, so finish must be waiting"
        );
        drop(in_flight);

        receiver
            .await
            .expect_err("a cancelled shutdown must not leave the drain detached");
    }

    /// A liveness rendezvous the TEST owns, with no PID anywhere.
    ///
    /// The fixture holds a FIFO open for writing; the test holds the read end.
    /// While the process lives the pipe stays open, and when it dies the kernel
    /// closes its end — the test observes EOF. That EOF is an event, so nothing
    /// is polled and nothing is slept on. On a failure path the test simply
    /// drops its end: the fixture, blocked on a stdin that the transport owns,
    /// is terminated by `kill_on_drop`, and no signal is ever sent to a number
    /// that may have been recycled.
    #[cfg(unix)]
    struct FixtureLiveness {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl FixtureLiveness {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("alive");
            let status = std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("mkfifo must be available on a unix test host");
            assert!(status.success(), "mkfifo failed for {path:?}");
            Self { _dir: dir, path }
        }

        /// Opens the read end and waits for the fixture's READY byte, proving
        /// the process reached its blocking state before the test acts.
        async fn wait_ready(&self) -> tokio::fs::File {
            let mut reader = timeout(FIXTURE_GUARD, tokio::fs::File::open(&self.path))
                .await
                .expect("opening the liveness pipe must not hang")
                .expect("the fixture must open its liveness pipe");
            let mut ready = [0u8; 5];
            timeout(FIXTURE_GUARD, reader.read_exact(&mut ready))
                .await
                .expect("the fixture must announce itself")
                .expect("the fixture must announce itself");
            assert_eq!(&ready, b"READY");
            reader
        }
    }

    /// Anti-hang bound only: every wait below resolves on an event, and this
    /// exists so a broken build fails instead of blocking the suite forever.
    #[cfg(unix)]
    const FIXTURE_GUARD: Duration = Duration::from_secs(30);

    /// EOF on the liveness pipe means every writer is gone — the fixture and
    /// anything that inherited its descriptors. An IO error is NOT treated as
    /// success: it would prove nothing about the process.
    #[cfg(unix)]
    async fn fixture_is_gone(reader: &mut tokio::fs::File) -> bool {
        let mut rest = Vec::new();
        matches!(
            timeout(FIXTURE_GUARD, reader.read_to_end(&mut rest)).await,
            Ok(Ok(_))
        )
    }

    #[cfg(unix)]
    fn fixture_command(liveness: &FixtureLiveness, script: &str) -> tokio::process::Command {
        let mut command = crate::core::cmd::async_cmd("sh");
        command.env("ALIVE", &liveness.path).args(["-c", script]);
        command
    }

    /// DoD — abandonment: a transport dropped after a REJECTED negotiation must
    /// not leave its process behind. Observed through the pipe closing, never
    /// through a PID.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_rejected_negotiation_dropped_terminates_the_owned_fixture() {
        let liveness = FixtureLiveness::new();
        let command = fixture_command(
            &liveness,
            r#"exec 3>"$ALIVE"
printf 'READY' >&3
while IFS= read -r line; do
    case "$line" in
        *'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2}}' ;;
    esac
done"#,
        );
        let transport = Arc::new(
            AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
                .await
                .unwrap(),
        );
        let mut alive = liveness.wait_ready().await;
        let mut host = AcpHost::new(1, transport.clone());

        assert_eq!(
            host.negotiate(request()).await.unwrap_err(),
            AcpError::UnsupportedProtocolVersion {
                actual: 2,
                maximum: 1,
            }
        );
        drop(host);
        drop(transport);

        assert!(
            fixture_is_gone(&mut alive).await,
            "dropping the rejected transport must terminate its owned fixture"
        );
    }

    /// DoD — idempotent reap: two shutdowns succeed, and the process is gone.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_is_idempotent_and_reaps_the_owned_fixture() {
        let liveness = FixtureLiveness::new();
        let command = fixture_command(
            &liveness,
            r#"exec 3>"$ALIVE"
printf 'READY' >&3
while IFS= read -r _; do :; done"#,
        );
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        let mut alive = liveness.wait_ready().await;

        transport.shutdown().await.unwrap();
        transport.shutdown().await.unwrap();

        assert!(
            fixture_is_gone(&mut alive).await,
            "shutdown must wait until the owned fixture is reaped"
        );
    }
}
