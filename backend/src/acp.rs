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
use tokio::time::{timeout, Duration};

use crate::models::AgentType;

mod claude_adapter;
mod codex_adapter;
mod permission_broker;

pub use claude_adapter::ClaudeAcpAdapter;
pub use codex_adapter::CodexAcpAdapter;
pub use permission_broker::{
    AcpAuditEntry, AcpPermissionBroker, AcpPermissionVerdict, AcpSessionPolicy,
};

/// Shared by the Claude/Codex adapter test modules.
#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// Write an executable shell fixture and return its path. The adapters
    /// always append their own CLI-specific flags (`--print`,
    /// `--output-format`, `--session-id`, `--resume`, …); a fixture script
    /// ignores whatever it does not recognize and reacts only to the
    /// substrings it cares about, exactly like a real shell script would.
    pub(crate) fn write_fixture_script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fixture-cli");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fixture script");
        let mut perms = fs::metadata(&path)
            .expect("stat fixture script")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod fixture script");
        path
    }
}

type PendingRequests = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, AcpError>>>>>;

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

/// Per-agent, explicit, environment-driven opt-in for the Codex/Claude ACP
/// adapters. Off by default: direct-CLI migration stays the production
/// default for both agents until an operator turns the adapter on for that
/// specific agent. Reading the toggle at call time (rather than baking it
/// into a `once_cell`) keeps it test-friendly and trivially observable —
/// `kronn doctor`/logs can report the exact variable an operator would set.
pub fn acp_adapter_enabled(agent: &AgentType) -> bool {
    match agent {
        AgentType::Codex => std::env::var("KRONN_ACP_ADAPTER_CODEX").is_ok(),
        AgentType::ClaudeCode => std::env::var("KRONN_ACP_ADAPTER_CLAUDE").is_ok(),
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
            let current = option
                .get("value")
                .or_else(|| option.get("current"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let available = option
                .get("availableValues")
                .or_else(|| option.get("available_values"))
                .or_else(|| option.get("values"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| {
                            let vid = value
                                .get("id")
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
    child: Mutex<Child>,
    next_id: AtomicU64,
    pending: PendingRequests,
    notifications: broadcast::Sender<Value>,
    session_setup: Mutex<Option<AcpSessionSetup>>,
    config_options: Mutex<Vec<AcpConfigOption>>,
    broker: Arc<AcpPermissionBroker>,
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
    ) -> Result<Self, AcpError> {
        let (program, args) = native_acp_command(agent).ok_or_else(|| {
            AcpError::Transport(format!("no verified production ACP command for {agent:?}"))
        })?;
        let mut command = crate::core::cmd::async_cmd(program);
        command.args(args).current_dir(cwd);
        Self::spawn(agent, command, full_access).await
    }

    pub async fn spawn(
        agent: AcpAgent,
        mut command: tokio::process::Command,
        full_access: bool,
    ) -> Result<Self, AcpError> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
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
        let pending = Arc::new(Mutex::new(HashMap::<
            u64,
            oneshot::Sender<Result<Value, AcpError>>,
        >::new()));
        let (notifications, _) = broadcast::channel(256);
        let broker = Arc::new(AcpPermissionBroker::new(full_access));
        Self::start_dispatcher(
            BufReader::new(stdout),
            stdin.clone(),
            pending.clone(),
            notifications.clone(),
            broker.clone(),
        );
        Ok(Self {
            agent,
            stdin,
            child: Mutex::new(child),
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
    ) {
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
                    } else if let Some(sender) = pending.lock().await.remove(&id) {
                        let result = if let Some(error) = message.get("error") {
                            Err(AcpError::Transport(format!("ACP response error: {error}")))
                        } else {
                            message.get("result").cloned().ok_or_else(|| {
                                AcpError::Transport("ACP response returned no result".into())
                            })
                        };
                        let _ = sender.send(result);
                    }
                } else {
                    let _ = notifications.send(message);
                }
            }
        });
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
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

async fn fail_pending(pending: &PendingRequests, error: AcpError) {
    let waiters = std::mem::take(&mut *pending.lock().await);
    for (_, sender) in waiters {
        let _ = sender.send(Err(AcpError::Transport(error.to_string())));
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
/// `agentCapabilities` sub-object. Only session loading (`loadSession`) and
/// scoped permission negotiation are genuinely optional and advertised per
/// agent at `initialize`.
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
        // `loadSession` is an ACP bool; an explicit `false` means resume is not
        // supported, so never assume it works from mere key presence.
        if object
            .get("loadSession")
            .map(|value| value.as_bool().unwrap_or(true))
            .unwrap_or(false)
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
            if let Some(content) = update.get("content") {
                match content {
                    Value::String(text) => events.push(AcpSessionEvent::TextDelta(text.to_owned())),
                    Value::Array(blocks) => {
                        for block in blocks {
                            if block.get("type").and_then(Value::as_str) == Some("text") {
                                if let Some(text) = block.get("text").and_then(Value::as_str) {
                                    events.push(AcpSessionEvent::TextDelta(text.to_owned()));
                                }
                            }
                        }
                    }
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
        let servers: Vec<Value> = request
            .mcp_servers
            .into_iter()
            .map(|server| {
                json!({
                    "name": server.id, "command": server.command, "args": server.args,
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
        AcpSessionTarget::new(self.agent, session_id(&result)?)
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
        // ACP session-resume must restate the workspace scope (`cwd` +
        // `mcpServers`), not only the opaque session id, so the resumed session
        // keeps Kronn's project MCP registry and working directory.
        let setup = self.session_setup.lock().await.clone().ok_or_else(|| {
            AcpError::Transport("ACP session/resume was called before initialize".into())
        })?;
        let result = self
            .request(
                "session/load",
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
                    result?;
                    while let Ok(frame) = notifications.try_recv() {
                        for event in events_from_notifications(vec![frame], &target.session_id) {
                            let _ = events.send(event).await;
                        }
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
        let mut child = self.child.lock().await;
        child
            .start_kill()
            .map_err(|error| AcpError::Transport(format!("stop ACP process: {error}")))
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
        assert!(!baseline.contains(&AcpCapability::Resume));

        // Optional capabilities are added only when genuinely advertised. A
        // model catalogue is NOT one of them: it is discovered from the
        // session response, never from a fabricated initialize object.
        let extended = agent_capabilities(&json!({
            "loadSession": true, "modelCapabilities": {}, "permissionCapabilities": {}
        }));
        assert!(extended.contains(&AcpCapability::Resume));
        assert!(extended.contains(&AcpCapability::Permissions));

        // An explicit `loadSession: false` must not enable resume.
        assert!(
            !agent_capabilities(&json!({"loadSession": false})).contains(&AcpCapability::Resume)
        );
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
}
