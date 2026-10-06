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

mod adapter_process;
mod claude_adapter;
mod codex_adapter;
mod permission_broker;
mod secret_files;
mod vibe_policy;

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
    /// A Claude Code `stream-json` turn shaped like a real one: the served
    /// model on the assistant event, one `Read` call, a reply, and a `result`
    /// whose usage counts cache reads and writes apart from `input_tokens`.
    pub(crate) const CLAUDE_TURN_WITH_CACHE: &str = r#"
cat >/dev/null
printf '%s\n' '{"type":"system","subtype":"init","session_id":"fixture-session","model":"claude-opus-5-5"}'
printf '%s\n' '{"type":"assistant","message":{"model":"claude-opus-5-5-20260915","content":[]}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"Read","input":{}}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"src/lib.rs\"}"}}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_stop","index":0}}'
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"orchestrated"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":48,"cache_creation_input_tokens":80271,"cache_read_input_tokens":1554330,"output_tokens":21545}}'
"#;

    /// An `opencode acp` stand-in, run with python3: ids are read from the
    /// requests, the shapes are those of OpenCode 1.18 (`session/request_permission`
    /// with `kind: read`, empty `locations`, and the `once`/`always`/`reject`
    /// options). It records the inline configuration it was started with and the
    /// answer it got under `$OPENCODE_FIXTURE_OUT` (an `OPENCODE_` name, so the
    /// built environment passes it to OpenCode), then goes on with its turn.
    #[cfg(unix)]
    pub(crate) const OPENCODE_ACP_FIXTURE: &str = r#"
import json, os, sys

def send(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()

out = os.environ["OPENCODE_FIXTURE_OUT"]
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"protocolVersion": 1}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {"sessionId": "fixture-session"}})
    elif method == "session/prompt":
        with open(out + "/config.json", "w") as handle:
            handle.write(os.environ.get("OPENCODE_CONFIG_CONTENT", ""))
        send({"jsonrpc": "2.0", "id": 99, "method": "session/request_permission", "params": {
            "sessionId": "fixture-session",
            "toolCall": {"toolCallId": "call-1", "title": "read", "kind": "read",
                         "status": "pending", "locations": [], "rawInput": {}},
            "options": [
                {"optionId": "once", "kind": "allow_once", "name": "Allow once"},
                {"optionId": "always", "kind": "allow_always", "name": "Always allow"},
                {"optionId": "reject", "kind": "reject_once", "name": "Reject"},
            ]}})
        reply = sys.stdin.readline()
        with open(out + "/reply.json", "w") as handle:
            handle.write(reply)
        send({"jsonrpc": "2.0", "method": "session/update", "params": {
            "sessionId": "fixture-session",
            "update": {"sessionUpdate": "agent_message_chunk",
                       "content": {"type": "text", "text": "carried on after the refusal"}}}})
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "stopReason": "end_turn",
            "usage": {"inputTokens": 100, "outputTokens": 20, "totalTokens": 160,
                      "cachedReadTokens": 30, "cachedWriteTokens": 10}}})
"#;

    /// Write the stand-in as an executable named `opencode`, for a test that puts
    /// `dir` first on `PATH` so that Kronn's own spawn of `opencode acp` finds it.
    #[cfg(unix)]
    pub(crate) fn write_fake_opencode(dir: &Path) -> PathBuf {
        let path = dir.join("opencode");
        fs::write(
            &path,
            format!("#!/usr/bin/env python3{OPENCODE_ACP_FIXTURE}"),
        )
        .expect("write fake opencode");
        let mut perms = fs::metadata(&path)
            .expect("stat fake opencode")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod fake opencode");
        path
    }

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
    /// non-interactive protocol rather than ACP JSON-RPC.
    AdaptedAcp,
    DirectCliMigration,
    HttpModelProvider,
}

/// Product defaults, independent of per-agent runtime overrides. Claude and
/// Codex use the shared ACP host; an explicit override can retain direct CLI.
pub fn production_route(agent: &AgentType) -> AcpProductionRoute {
    match agent {
        AgentType::OpenCode
        | AgentType::GeminiCli
        | AgentType::CopilotCli
        | AgentType::Kiro
        | AgentType::Vibe => AcpProductionRoute::NativeAcp,
        AgentType::ClaudeCode | AgentType::Codex => AcpProductionRoute::AdaptedAcp,
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom => {
            AcpProductionRoute::HttpModelProvider
        }
    }
}

/// Absence uses the product default. Explicit values retain the established
/// strict boolean interpretation: only `1`/`true` enable, including whitespace
/// and case normalization. Empty, malformed and false values keep direct CLI.
fn env_flag_enabled(var: &str) -> bool {
    match crate::core::child_env::var(var) {
        Ok(value) => matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true"),
        Err(std::env::VarError::NotPresent) => true,
        Err(std::env::VarError::NotUnicode(_)) => false,
    }
}

/// Per-agent runtime override, read for each launch. Unset means adapted ACP;
/// `0` or `false` selects the explicit direct-CLI compatibility route.
pub fn acp_adapter_enabled(agent: &AgentType) -> bool {
    match agent {
        AgentType::Codex => env_flag_enabled("KRONN_ACP_ADAPTER_CODEX"),
        AgentType::ClaudeCode => env_flag_enabled("KRONN_ACP_ADAPTER_CLAUDE"),
        _ => false,
    }
}

/// Resolve the actual dispatch without changing any other agent's route.
pub fn resolve_acp_route(agent: &AgentType) -> AcpProductionRoute {
    let default_route = production_route(agent);
    if default_route == AcpProductionRoute::AdaptedAcp && !acp_adapter_enabled(agent) {
        AcpProductionRoute::DirectCliMigration
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
                    let mut flat = Vec::new();
                    collect_config_values(values, &mut flat);
                    flat
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

/// Read the values of one select option. ACP lets `options` be either a flat
/// list of `{value, name}` or a list of groups `{group, name, options: [...]}`
/// (the ACP SDK bundled with OpenCode 1.18.33 declares the option list as a
/// union of both shapes). A group is not a selectable value, so it is
/// flattened: reading only the flat shape made every grouped value look absent.
/// A value offered twice is kept once.
fn collect_config_values(values: &[Value], out: &mut Vec<AcpConfigValue>) {
    for value in values {
        let id = value
            .get("id")
            .or_else(|| value.get("value"))
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty());
        match id {
            Some(id) => {
                if out.iter().any(|known| known.id == id) {
                    continue;
                }
                let name = value.get("name").and_then(Value::as_str).unwrap_or(id);
                out.push(AcpConfigValue {
                    id: id.to_owned(),
                    name: name.to_owned(),
                });
            }
            None => {
                if let Some(members) = value.get("options").and_then(Value::as_array) {
                    collect_config_values(members, out);
                }
            }
        }
    }
}

/// ACP does not standardize which option carries the model catalogue, so a
/// runtime's option is the catalogue when its id names a model. Shared by
/// discovery (what to list) and launch (what a chosen model must be found in).
pub(crate) fn is_model_option_id(id: &str) -> bool {
    id.to_lowercase().contains("model")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpSessionEvent {
    /// Model identifier from a structured runtime response, not configuration.
    ModelObserved(String),
    /// The CLI's own session id, as its init line reports it. Unlike
    /// `NativeSessionId` it is observation only: nothing is persisted for a
    /// later resume, it just names the transcript this launch writes.
    CliSessionObserved(String),
    /// Runtime-owned conversation identifier discovered after session
    /// creation. This is control metadata consumed by the runner, never text
    /// forwarded to the discussion or an agent-visible event payload.
    NativeSessionId(String),
    TextDelta(String),
    ToolCall {
        name: String,
    },
    /// The tool call most recently announced reached a terminal status
    /// (`completed`, `failed` or `cancelled`). KT-932's watchdog spends a
    /// tool call's own, much wider bound on it rather than the model's own
    /// inactivity delay; this is what hands the clock back to the model the
    /// moment the tool is actually done.
    ToolCallEnded,
    /// A tool call's start or target for the live views, built from its
    /// structured fields only (`agents::activity`). Updates carrying the same
    /// call id refine one call; they never announce another.
    ToolActivity(crate::agents::activity::ToolActivityUpdate),
    /// Correlated tool metadata for the durable transcript, redacted and bounded.
    ToolTrace(crate::agents::tool_trace::ToolTraceUpdate),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        prompt_cache: crate::agents::runner::PromptCacheUsage,
    },
    /// The turn's cost as the runtime itself reported it, in micro-USD.
    Cost {
        usd_micros: u64,
    },
    /// A frame from the agent that carries nothing to show — a reasoning chunk,
    /// a plan, a status update. It is proof of life and nothing else: without
    /// it, a model thinking for ten minutes before it answers looks exactly like
    /// one whose connection died, and KT-932's inactivity watchdog cannot tell
    /// them apart.
    Activity,
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
    /// Candidates and authorized MCP servers computed at launch, so the launch
    /// grant (Copilot) and the `session/new` declaration come from one decision.
    launch_mcp_servers: Mutex<Option<(Vec<AcpMcpServer>, Vec<AcpMcpServer>)>>,
    /// The name this session declares Kronn's bridge under.
    bridge_id: String,
    /// A per-launch directory Kronn wrote for the runtime, removed at shutdown.
    launch_dir: Option<std::path::PathBuf>,
}

/// How long `shutdown` waits for the stdout dispatcher after the child is
/// reaped. Only a descendant holding the inherited pipe can exceed this, and
/// blocking shutdown on that is worse than abandoning the drain.
const DISPATCHER_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

/// Budget for a control request — `initialize`, `session/new`, `session/cancel`.
/// These are local handshakes; a runtime that has not answered in half a minute
/// is not going to.
const CONTROL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Backstop for a prompt turn.
///
/// `session/prompt` is not a handshake: it is the agent reading, thinking,
/// calling its tools and writing an answer. It shared the 30 s control budget
/// until 0.13.0, so every ACP turn longer than half a minute died with
/// "ACP request timed out: session/prompt" — while the CLI path grants the same
/// work fifteen minutes of silence and up to the operator's configured global
/// timeout.
///
/// This is only a backstop, and it is derived from the ceiling the operator's
/// own setting is clamped to rather than picked — so it can never be tighter
/// than what they configured, and it follows that ceiling if it ever moves.
/// The real limits live in the streaming layer, which owns the configured
/// stall and global timeouts and kills the process when either fires; this one
/// exists only so a runtime that answers nothing at all cannot wedge the
/// dispatcher forever.
///
/// The margin is what keeps the two from racing: at exactly the ceiling, a run
/// hitting its configured global timeout could lose the race and be reported
/// as an ACP protocol timeout instead of the operator's own limit.
const PROMPT_REQUEST_TIMEOUT: Duration =
    Duration::from_secs((crate::models::MAX_AGENT_GLOBAL_TIMEOUT_MIN as u64 + 10) * 60);

fn request_timeout(method: &str) -> Duration {
    if method == "session/prompt" {
        PROMPT_REQUEST_TIMEOUT
    } else {
        CONTROL_REQUEST_TIMEOUT
    }
}

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
                match (&mut *handle).await {
                    // Raced to completion between the timeout and the abort.
                    Ok(()) => Ok(()),
                    // The cancellation we asked for: expected, so not fatal —
                    // but never silent, or a drain that had to be killed would
                    // leave no trace at all.
                    Err(error) if error.is_cancelled() => {
                        tracing::warn!(
                            timeout_secs = DISPATCHER_JOIN_TIMEOUT.as_secs(),
                            "ACP stdout drain outlived the reaped process and was cancelled"
                        );
                        Ok(())
                    }
                    // A PANIC is not what we asked for. Report it exactly like
                    // the nominal join does instead of hiding it behind the
                    // expected-cancellation policy.
                    Err(error) => Err(format!("join ACP dispatcher: {error}")),
                }
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
    /// The process group the child leads (Unix), so that stopping the agent
    /// stops what it started too: its shell commands, its MCP servers.
    group: Option<u32>,
}

impl Drop for AcpProcess {
    fn drop(&mut self) {
        // Still owning the child means nobody shut it down: `kill_on_drop` only
        // reaches the child itself, not what it spawned.
        if self.child.is_some() {
            kill_process_group(self.group);
        }
    }
}

/// SIGKILL a process group Kronn created for an ACP child. The child is still
/// owned, so it has not been reaped and its pid cannot have been reused.
#[cfg(unix)]
fn kill_process_group(group: Option<u32>) {
    if let Some(pgid) = group
        .and_then(|group| i32::try_from(group).ok())
        .filter(|pgid| *pgid > 1)
    {
        // SAFETY: a plain signal to a group that `process_group(0)` made at spawn.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_group: Option<u32>) {}

/// Native ACP runtimes are spawned as Windows programs and their protocol
/// payloads carry Windows paths, so one found only inside WSL is refused.
fn native_acp_wsl_only_refusal(program: &str) -> Option<String> {
    if !cfg!(target_os = "windows") {
        return None;
    }
    crate::agents::find_binary(program)
        .filter(|location| location.via_wsl)
        .map(|_| crate::agents::wsl::native_acp_wsl_refusal(program))
}

/// What one native ACP launch adds to its built environment (KT-1013): the
/// same per-launch values the direct and adapter routes receive.
#[derive(Default, Clone)]
pub struct NativeLaunchEnv {
    pub discussion_id: Option<String>,
    pub room_agent: Option<crate::agents::runner::RoomAgentBridgeContext>,
    pub workflow_step: Option<crate::agents::runner::WorkflowStepBridgeContext>,
    /// This agent's configured provider key, set under its own variable.
    pub api_key: Option<String>,
    /// The launch's scoped bridge token (layer B).
    pub bridge_token: Option<String>,
    /// `core::github_connection::env_for_launch` for the agent's project (D2).
    pub github_env: Vec<(String, String)>,
}

fn native_family(agent: AcpAgent) -> crate::core::child_env::AgentFamily {
    use crate::core::child_env::AgentFamily;
    match agent {
        AcpAgent::OpenCode => AgentFamily::OpenCode,
        AcpAgent::GeminiCli => AgentFamily::Gemini,
        AcpAgent::CopilotCli => AgentFamily::Copilot,
        AcpAgent::Kiro => AgentFamily::Kiro,
        AcpAgent::Vibe => AgentFamily::Vibe,
        AcpAgent::Codex => AgentFamily::Codex,
        AcpAgent::ClaudeCode => AgentFamily::Claude,
    }
}

/// The subprocess of a native ACP session: its command, working directory and
/// the environment Kronn builds for it.
fn native_command(
    agent: AcpAgent,
    program: &str,
    args: &[&str],
    cwd: &str,
    launch: &NativeLaunchEnv,
) -> Result<tokio::process::Command, String> {
    use crate::core::child_env;
    let family = native_family(agent);
    let route = child_env::ChildRoute::Agent(family);
    let mut command = crate::core::cmd::async_cmd(program, route);
    command.args(args).current_dir(cwd);
    child_env::reset(command.as_std_mut(), route);
    // Under Docker, the MCP values the project's MCP files refer to (KT-964).
    crate::core::mcp_secret_refs::apply_to(&mut command, std::path::Path::new(cwd));
    let values = child_env::AgentLaunch {
        discussion_id: launch.discussion_id.as_deref(),
        task_worker: None,
        room_agent: launch.room_agent.as_ref(),
        workflow_step: launch.workflow_step.as_ref(),
        bridge_token: launch.bridge_token.as_deref(),
        api_key: family.provider_key_env().zip(launch.api_key.as_deref()),
    };
    // Native runtimes are refused inside WSL, so the backend URL is the plain one.
    let backend_url = launch.discussion_id.is_some().then(|| {
        crate::core::child_env::var("KRONN_BACKEND_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3140".into())
    });
    child_env::apply_agent_launch(
        command.as_std_mut(),
        std::path::Path::new(cwd),
        &values,
        backend_url,
    )?;
    // Only a project connected to GitHub hands its agent a token (D2).
    crate::core::github_connection::apply_launch_env(command.as_std_mut(), &launch.github_env);
    let mut granted = values.granted();
    granted.extend_from_slice(child_env::GITHUB_ENV);
    child_env::seal(command.as_std_mut(), route, &granted);
    Ok(command)
}

/// Environment Kronn gives `opencode acp`, on top of the user's own: the inline
/// configuration of [`secret_files::opencode_launch_config`]. With full access
/// an operator's own inline configuration is left alone; without it, Kronn's
/// permissions are merged over it, or the launch is refused.
fn apply_opencode_policy(
    command: &mut tokio::process::Command,
    full_access: bool,
    servers: &[AcpMcpServer],
) -> Result<(), String> {
    const VARIABLE: &str = "OPENCODE_CONFIG_CONTENT";
    let operator = crate::core::child_env::var(VARIABLE).ok();
    if full_access && operator.is_some() {
        tracing::info!("{VARIABLE} is already set; Kronn's OpenCode read policy is not applied");
    }
    let bridge_tools: Vec<String> = servers
        .iter()
        .find(|server| permission_broker::is_bridge_id(&server.id))
        .map(|bridge| {
            if bridge.allowed_tools.is_empty() {
                vec![format!("{}_*", opencode_identifier(&bridge.id))]
            } else {
                bridge
                    .allowed_tools
                    .iter()
                    .map(|tool| {
                        format!(
                            "{}_{}",
                            opencode_identifier(&bridge.id),
                            opencode_identifier(tool)
                        )
                    })
                    .collect()
            }
        })
        .unwrap_or_default();
    if let Some(config) =
        secret_files::opencode_launch_config(full_access, &bridge_tools, operator.as_deref())?
    {
        command.env(VARIABLE, config);
    }
    Ok(())
}

/// An MCP server or tool name as OpenCode builds its tool ids (`<server>_<tool>`).
fn opencode_identifier(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
        launch: NativeLaunchEnv,
        scope: AcpSessionScope,
        mcp_candidates: Vec<AcpMcpServer>,
    ) -> Result<Self, AcpError> {
        let (program, args) = native_acp_command(agent).ok_or_else(|| {
            AcpError::Transport(format!("no verified production ACP command for {agent:?}"))
        })?;
        if let Some(refusal) = native_acp_wsl_only_refusal(program) {
            return Err(AcpError::Transport(refusal));
        }
        let mut command =
            native_command(agent, program, &args, cwd, &launch).map_err(AcpError::Transport)?;
        // The servers are authorized once, by the session's own broker: the
        // launch grant never names a server `session/new` does not declare.
        let broker = session_broker(agent, full_access, Some(scope));
        let bridge_id = session_bridge_id(agent);
        let servers = native_session_mcp_servers(
            &broker,
            mcp_candidates.clone(),
            crate::agents::runner::disc_introspection_mcp_command(),
            &bridge_id,
        );
        if agent == AcpAgent::OpenCode {
            apply_opencode_policy(&mut command, full_access, &servers)
                .map_err(AcpError::Transport)?;
        }
        // Without full access Vibe runs under Kronn's agent profile: nothing it
        // or the user's config would approve unasked skips the broker.
        let vibe_profile_dir = if agent == AcpAgent::Vibe && !full_access {
            let vibe_home = vibe_policy::vibe_home().ok_or_else(|| {
                AcpError::Transport(
                    "cannot locate Vibe's home (VIBE_HOME or HOME): Vibe is not started without full access".into(),
                )
            })?;
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            let policy = vibe_policy::prepare(std::path::Path::new(cwd), &vibe_home, &suffix[..12])
                .map_err(AcpError::Transport)?;
            for (name, value) in &policy.env {
                command.env(name, value);
            }
            Some(policy.dir)
        } else {
            None
        };
        let launch_args = native_mcp_launch_args(agent, &servers);
        if let Some(bridge) = servers
            .iter()
            .find(|server| permission_broker::is_bridge_id(&server.id))
            .filter(|_| {
                launch_args
                    .iter()
                    .any(|arg| arg.starts_with("--allow-tool="))
            })
        {
            broker.record_launch_grant(&bridge.id, &bridge.allowed_tools);
        }
        command.args(launch_args);
        let mut transport = match Self::spawn_with_broker(agent, command, broker).await {
            Ok(transport) => transport,
            Err(error) => {
                if let Some(dir) = &vibe_profile_dir {
                    let _ = std::fs::remove_dir_all(dir);
                }
                return Err(error);
            }
        };
        transport.bridge_id = bridge_id;
        transport.launch_dir = vibe_profile_dir;
        *transport.launch_mcp_servers.lock().await = Some((mcp_candidates, servers));
        Ok(transport)
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
        command: tokio::process::Command,
        full_access: bool,
        scope: Option<AcpSessionScope>,
    ) -> Result<Self, AcpError> {
        Self::spawn_with_broker(agent, command, session_broker(agent, full_access, scope)).await
    }

    async fn spawn_with_broker(
        agent: AcpAgent,
        mut command: tokio::process::Command,
        broker: AcpPermissionBroker,
    ) -> Result<Self, AcpError> {
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // Its own group: stopping the agent then stops the processes it started,
        // which killing the agent alone leaves running.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| AcpError::Transport(format!("spawn ACP process: {error}")))?;
        let group = child.id().filter(|_| cfg!(unix));
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
        let broker = Arc::new(broker);
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
                group,
            }),
            next_id: AtomicU64::new(1),
            pending,
            notifications,
            session_setup: Mutex::new(None),
            config_options: Mutex::new(Vec::new()),
            broker,
            launch_mcp_servers: Mutex::new(None),
            bridge_id: "kronn-internal".into(),
            launch_dir: None,
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
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                // A stray non-UTF-8 byte must not end the session: frames are
                // decoded lossily and a malformed one is discarded below.
                let read = match stdout.read_until(b'\n', &mut bytes).await {
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
                let line = String::from_utf8_lossy(&bytes);
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
                    observe_notification(&broker, &message);
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
        timeout(request_timeout(method), receiver)
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

/// A tool call announced before its permission request: the broker may need
/// the harness tool name it carries (Vibe).
fn observe_notification(broker: &AcpPermissionBroker, message: &Value) {
    if message.get("method").and_then(Value::as_str) != Some("session/update")
        || message.pointer("/params/update/toolCallId").is_none()
    {
        return;
    }
    tracing::trace!(
        shape = %permission_broker::value_shape(&message["params"]["update"], 0),
        "ACP tool-call update shape"
    );
    broker.observe_tool_call_update(&message["params"]);
}

/// The broker of one native session. Vibe's permission request names no tool:
/// its broker identifies MCP calls by the tool the harness announced.
fn session_broker(
    agent: AcpAgent,
    full_access: bool,
    scope: Option<AcpSessionScope>,
) -> AcpPermissionBroker {
    let broker = match scope {
        Some(scope) => AcpPermissionBroker::scoped(full_access, scope),
        None => AcpPermissionBroker::new(full_access),
    };
    if agent == AcpAgent::Vibe {
        broker.identify_tools_by_harness_name();
    }
    broker
}

/// The name a native session declares Kronn's bridge under. Copilot, Vibe and
/// OpenCode approve it without asking Kronn (a launch grant, a harness
/// identity, a config rule), and they also load MCP servers from the user's and
/// the repository's own configs: a per-launch name keeps any of those from
/// claiming the bridge's approval.
fn session_bridge_id(agent: AcpAgent) -> String {
    match agent {
        AcpAgent::CopilotCli | AcpAgent::Vibe | AcpAgent::OpenCode => {
            let suffix = uuid::Uuid::new_v4().simple().to_string();
            format!("kronn-internal-{}", &suffix[..12])
        }
        _ => "kronn-internal".into(),
    }
}

/// Copilot's launch arguments for Kronn's bridge. Copilot rejects the stdio
/// servers a client declares over ACP and reads its own config, the
/// workspace's `.mcp.json`/`.github/mcp.json` included, where a server may be
/// named `kronn-internal`. So the well-known name is disabled, the bridge is
/// added under its per-launch name, and only that name is granted; its
/// permission request names the tool but not the server, so the broker could
/// not grant it. A step's tool list is granted tool by tool.
pub(crate) fn native_mcp_launch_args(agent: AcpAgent, servers: &[AcpMcpServer]) -> Vec<String> {
    if agent != AcpAgent::CopilotCli {
        return Vec::new();
    }
    let mut args = vec![
        "--disable-mcp-server".to_owned(),
        "kronn-internal".to_owned(),
    ];
    let Some(bridge) = servers.iter().find(|server| {
        permission_broker::is_bridge_id(&server.id) && server.id != "kronn-internal"
    }) else {
        return args;
    };
    let config = json!({"mcpServers": {bridge.id.clone(): {
        "command": bridge.command, "args": bridge.args, "env": {},
    }}});
    args.push("--additional-mcp-config".to_owned());
    args.push(config.to_string());
    if bridge.allowed_tools.is_empty() {
        args.push(format!("--allow-tool={}", bridge.id));
    } else {
        args.extend(
            bridge
                .allowed_tools
                .iter()
                .filter(|tool| {
                    !tool.is_empty()
                        && tool.len() <= 128
                        && tool
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                })
                .map(|tool| format!("--allow-tool={}({tool})", bridge.id)),
        );
    }
    args
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
    // A usage block that counts nothing is a runtime that did not measure, not
    // a run that cost nothing: it reports no usage at all.
    (input_tokens > 0 || output_tokens > 0).then_some(AcpSessionEvent::Usage {
        input_tokens,
        output_tokens,
        // `None` when the runtime did not say — which is not zero.
        prompt_cache: crate::agents::runner::PromptCacheUsage {
            cached_prompt_tokens: usage.get("cachedReadTokens").and_then(Value::as_u64),
            cache_write_prompt_tokens: usage.get("cachedWriteTokens").and_then(Value::as_u64),
        },
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
            // Vibe echoes the injected prompt as user_message_chunk. Only
            // agent_message_chunk is an answer: user echoes, private thoughts,
            // tool content and future labelled variants must not become text.
            // Keep compatibility with older unlabelled runtime frames.
            let is_answer = matches!(kind, None | Some("agent_message_chunk"));
            if let (Some(content), true) = (update.get("content"), is_answer) {
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
                // KT-932 — a `tool_call`/`tool_call_update` that has reached a
                // terminal status is the tool ending, not another sign that it
                // is still running: the watchdog must hand the clock back to
                // the model, not restart the tool's own wider bound again.
                let status = update.get("status").and_then(Value::as_str);
                let terminal = matches!(status, Some("completed" | "failed" | "cancelled"));
                if terminal {
                    events.push(AcpSessionEvent::ToolCallEnded);
                } else {
                    events.push(AcpSessionEvent::ToolCall {
                        name: update
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_owned(),
                    });
                }
                // A terminal update refines a call; it never announces one.
                if let Some(activity) = crate::agents::activity::acp_tool_update(update)
                    .filter(|activity| !terminal || activity.carries_detail())
                {
                    events.push(AcpSessionEvent::ToolActivity(activity));
                }
                if let Some(trace) = crate::agents::tool_trace::from_acp(update) {
                    events.push(AcpSessionEvent::ToolTrace(trace));
                }
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
                    prompt_cache: Default::default(),
                });
            }
            (!events.is_empty()).then_some(events)
        })
        .flatten()
        .collect()
}

/// Kronn's bridge is supplied by the runtime, independently of the project
/// registry. Reconstruct its command instead of trusting a reserved-name
/// candidate, and keep ordinary project servers under the broker's checks.
///
/// A step's tool list (KT-908) travels with the rebuilt bridge: its allowed
/// tools for the broker and the launch grants, its `--step-tools` argument for
/// the bridge's own filter. `bridge_id` is the name it is declared under.
fn native_session_mcp_servers(
    broker: &AcpPermissionBroker,
    candidates: Vec<AcpMcpServer>,
    internal: Option<crate::agents::runner::InternalMcpCommand>,
    bridge_id: &str,
) -> Vec<AcpMcpServer> {
    let candidates = broker.without_audit_excluded(candidates);
    let requested_internal = candidates
        .iter()
        .find(|server| server.id == "kronn-internal")
        .map(|server| server.allowed_tools.clone());
    let mut servers = broker.authorize_mcp_servers(
        candidates
            .into_iter()
            .filter(|server| server.id != "kronn-internal")
            .collect(),
    );
    if let (Some(launch), Some(allowed_tools)) = (internal, requested_internal) {
        let mut args = launch.args;
        if !allowed_tools.is_empty() {
            args.push(format!("--step-tools={}", allowed_tools.join(",")));
        }
        let bridge = AcpMcpServer {
            id: bridge_id.into(),
            command: launch.command,
            args,
            allowed_tools,
        };
        broker.register_trusted_mcp_server(&bridge);
        servers.insert(0, bridge);
    }
    servers
}

#[async_trait]
impl AcpTransport for AcpJsonRpcTransport {
    async fn initialize(
        &self,
        request: AcpInitialize,
    ) -> Result<AcpNegotiatedCapabilities, AcpError> {
        let prepared = self.launch_mcp_servers.lock().await.take();
        let servers = match prepared {
            Some((candidates, servers)) if candidates == request.mcp_servers => servers,
            _ => native_session_mcp_servers(
                &self.broker,
                request.mcp_servers,
                crate::agents::runner::disc_introspection_mcp_command(),
                &self.bridge_id,
            ),
        };
        let servers: Vec<Value> = servers
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
                        let derived = events_from_notifications(vec![frame], &target.session_id);
                        // A frame is progress even when it says nothing the
                        // reader can see; tell the host so it is not mistaken
                        // for silence.
                        if derived.is_empty() {
                            let _ = events.send(AcpSessionEvent::Activity).await;
                        }
                        for event in derived {
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
        if let Some(dir) = &self.launch_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        let mut errors = Vec::new();
        if let Some(mut child) = process.child.take() {
            // The group first: what the agent started must not outlive it.
            kill_process_group(process.group.take());
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

    /// Whether the current session lists models at all. A chosen model that is
    /// absent from such a list will not be the one the agent runs; against a
    /// session that lists none, `select_model` keeps its deliberate no-op.
    pub async fn offers_model_catalogue(&self) -> bool {
        self.transport
            .config_options()
            .await
            .iter()
            .any(|option| is_model_option_id(&option.id) && !option.available.is_empty())
    }

    /// Apply a tier/model choice to an existing session by matching it against
    /// the options the session actually returned, then calling
    /// `session/set_config_option`. The model catalogue is searched before any
    /// other option, so an effort or mode value never stands in for a model.
    /// Returns `true` when a matching option value was found and applied;
    /// `false` is a deliberate no-op (no catalogue or no match) so a
    /// catalogue-less agent keeps its own default rather than receiving a
    /// spurious selection. A caller that must run exactly the chosen model
    /// checks `offers_model_catalogue` before accepting `false`.
    pub async fn select_model(
        &self,
        target: &AcpSessionTarget,
        model: &str,
    ) -> Result<bool, AcpError> {
        let options = self.transport.config_options().await;
        let (catalogue, others): (Vec<_>, Vec<_>) = options
            .iter()
            .partition(|option| is_model_option_id(&option.id));
        for option in catalogue.into_iter().chain(others) {
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
    // Only the Unix liveness fixtures read from a socket.
    #[cfg(unix)]
    use tokio::io::AsyncReadExt;

    fn command_env(command: &tokio::process::Command) -> HashMap<String, String> {
        command
            .as_std()
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect()
    }

    /// KT-1013 — a native ACP agent gets the values the other routes give:
    /// its configured key, its room and workflow contexts, the bridge token and
    /// a project temp dir; never the admin token nor another agent's key.
    #[test]
    fn a_native_launch_receives_its_key_and_contexts_through_the_builder() {
        let project = tempfile::tempdir().unwrap();
        let launch = NativeLaunchEnv {
            discussion_id: Some("room-7".into()),
            room_agent: Some(crate::agents::runner::RoomAgentBridgeContext {
                discussion_id: "room-7".into(),
                agent_type: "GeminiCli".into(),
                dispatch_job_id: "job-1".into(),
                source_message_id: "msg-1".into(),
            }),
            workflow_step: None,
            api_key: Some("kronn-stored-gemini-key".into()),
            bridge_token: Some("kbt_native".into()),
            github_env: Vec::new(),
        };
        let command = crate::core::child_env::with_parent_env(
            &[
                ("PATH", "/usr/bin"),
                ("KRONN_AUTH_TOKEN", "admin"),
                ("KRONN_ENCRYPTION_KEK", "raw"),
                ("OPENAI_API_KEY", "sk-oai"),
                ("GOOGLE_CLOUD_PROJECT", "p"),
                ("GH_TOKEN", "backend-gh"),
            ],
            || {
                native_command(
                    AcpAgent::GeminiCli,
                    "gemini",
                    &["--acp"],
                    &project.path().to_string_lossy(),
                    &launch,
                )
                .unwrap()
            },
        );
        let env = command_env(&command);
        assert_eq!(env["GEMINI_API_KEY"], "kronn-stored-gemini-key");
        assert_eq!(env["KRONN_DISCUSSION_ID"], "room-7");
        assert!(env["KRONN_ROOM_AGENT_CONTEXT"].contains("job-1"));
        assert_eq!(env["KRONN_BRIDGE_TOKEN"], "kbt_native");
        assert!(env.contains_key("KRONN_BACKEND_URL"));
        assert!(env["TMPDIR"].contains(".kronn"));
        assert_eq!(env["GOOGLE_CLOUD_PROJECT"], "p");
        assert!(
            !env.contains_key("GH_TOKEN"),
            "no connected project, no GitHub token"
        );
        for forbidden in ["KRONN_AUTH_TOKEN", "KRONN_ENCRYPTION_KEK", "OPENAI_API_KEY"] {
            assert!(
                !env.contains_key(forbidden),
                "{forbidden} reached the agent"
            );
        }

        // A workflow step's capability rides the same way.
        let step = NativeLaunchEnv {
            discussion_id: Some("step-room".into()),
            workflow_step: Some(crate::agents::runner::WorkflowStepBridgeContext {
                discussion_id: "step-room".into(),
                run_id: "run-1".into(),
                step_key: "s1".into(),
                capability: "cap".into(),
            }),
            ..Default::default()
        };
        let command = native_command(
            AcpAgent::Kiro,
            "kiro-cli",
            &[],
            &project.path().to_string_lossy(),
            &step,
        )
        .unwrap();
        assert!(command_env(&command)["KRONN_WORKFLOW_STEP_CONTEXT"].contains("run-1"));

        // A connected project's GitHub variables reach the native agent.
        let connected = NativeLaunchEnv {
            github_env: vec![
                ("GH_TOKEN".into(), "project-gh".into()),
                ("COPILOT_GITHUB_TOKEN".into(), "project-gh".into()),
            ],
            ..Default::default()
        };
        let command = native_command(
            AcpAgent::CopilotCli,
            "copilot",
            &[],
            &project.path().to_string_lossy(),
            &connected,
        )
        .unwrap();
        let env = command_env(&command);
        assert_eq!(env["GH_TOKEN"], "project-gh");
        assert_eq!(env["COPILOT_GITHUB_TOKEN"], "project-gh");
    }

    #[test]
    fn native_mcp_registry_keeps_the_owned_bridge_without_trusting_projectless_candidates() {
        let broker = AcpPermissionBroker::scoped(false, AcpSessionScope::new(None, "discussion"));
        let declared = |id: &str| AcpMcpServer {
            id: id.into(),
            command: "untrusted".into(),
            args: vec!["--token=fixture".into()],
            allowed_tools: Vec::new(),
        };
        let launch = || crate::agents::runner::InternalMcpCommand {
            command: "owned-kronn-mcp".into(),
            args: vec![],
            env: Default::default(),
        };
        let servers = native_session_mcp_servers(
            &broker,
            vec![declared("kronn-internal"), declared("other-project")],
            Some(launch()),
            "kronn-internal",
        );
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].id, "kronn-internal");
        assert_eq!(servers[0].command, "owned-kronn-mcp");
        assert!(servers[0].args.is_empty());
        assert!(
            native_session_mcp_servers(
                &broker,
                vec![declared("kronn-internal")],
                None,
                "kronn-internal"
            )
            .is_empty(),
            "a missing owned bridge cannot fall back to the candidate command"
        );
        assert!(
            native_session_mcp_servers(&broker, vec![], Some(launch()), "kronn-internal")
                .is_empty(),
            "catalogue probes that do not request MCP must remain tool-free"
        );
    }

    fn owned_bridge() -> crate::agents::runner::InternalMcpCommand {
        crate::agents::runner::InternalMcpCommand {
            command: "owned-kronn-mcp".into(),
            args: vec!["bridge.py".into()],
            env: Default::default(),
        }
    }

    fn declared_bridge() -> AcpMcpServer {
        AcpMcpServer {
            id: "kronn-internal".into(),
            command: "owned-kronn-mcp".into(),
            args: vec!["bridge.py".into()],
            allowed_tools: Vec::new(),
        }
    }

    /// P1-1 — a step's tool list survives the whole native path: the step's
    /// declaration, the session's servers, Copilot's launch grants and the
    /// broker's decision.
    #[test]
    fn a_step_s_tool_list_narrows_the_native_bridge_end_to_end() {
        let step = crate::models::StepTools {
            cli: Vec::new(),
            kronn_internal: vec!["disc_meta".into()],
        };
        let declared =
            crate::agents::runner::declared_mcp_servers(vec![declared_bridge()], Some(&step));
        let bridge_id = session_bridge_id(AcpAgent::CopilotCli);
        let broker = session_broker(
            AcpAgent::CopilotCli,
            false,
            Some(AcpSessionScope::new(None, "step")),
        );
        let servers =
            native_session_mcp_servers(&broker, declared, Some(owned_bridge()), &bridge_id);
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].id, bridge_id);
        assert_eq!(servers[0].allowed_tools, vec!["disc_meta"]);
        assert_eq!(servers[0].args, vec!["bridge.py", "--step-tools=disc_meta"]);

        let args = native_mcp_launch_args(AcpAgent::CopilotCli, &servers);
        let grants: Vec<_> = args
            .iter()
            .filter(|arg| arg.starts_with("--allow-tool"))
            .collect();
        assert_eq!(
            grants,
            vec![&format!("--allow-tool={bridge_id}(disc_meta)")]
        );
        let config: Value = serde_json::from_str(
            &args[args
                .iter()
                .position(|arg| arg == "--additional-mcp-config")
                .unwrap()
                + 1],
        )
        .unwrap();
        assert_eq!(
            config["mcpServers"][&bridge_id]["args"],
            json!(["bridge.py", "--step-tools=disc_meta"])
        );

        // The same session on Vibe: the declared tool passes, another does not.
        let broker = session_broker(
            AcpAgent::Vibe,
            false,
            Some(AcpSessionScope::new(None, "step")),
        );
        broker.bind_protocol_session("s1").unwrap();
        let bridge_id = session_bridge_id(AcpAgent::Vibe);
        let declared =
            crate::agents::runner::declared_mcp_servers(vec![declared_bridge()], Some(&step));
        native_session_mcp_servers(&broker, declared, Some(owned_bridge()), &bridge_id);
        let group = bridge_id.replace('-', "_");
        let decide = |id: &str, tool: &str| {
            let name = format!("mcp_{group}.{tool}");
            observe_notification(
                &broker,
                &json!({"method": "session/update", "params": {
                "sessionId": "s1",
                "update": {"sessionUpdate": "tool_call", "toolCallId": id, "kind": "other",
                           "_meta": {"effect_kind": "tool", "tool_name": name}}}}),
            );
            handle_client_request(
                &broker,
                "session/request_permission",
                &json!({
                "sessionId": "s1", "toolCall": {"toolCallId": id},
                "options": [{"optionId": "allow", "kind": "allow_once"},
                            {"optionId": "reject", "kind": "reject_once"}]}),
            )
            .unwrap()["outcome"]["optionId"]
                .clone()
        };
        assert_eq!(decide("e1", "disc_meta"), "allow");
        assert_eq!(decide("e2", "task_exec_launch"), "reject");
    }

    /// P2-1 — Copilot never loads a server called `kronn-internal` and is
    /// granted only the bridge's per-launch name, whatever else is authorized.
    #[test]
    fn copilot_is_granted_only_the_bridge_s_per_launch_name() {
        let bridge_id = session_bridge_id(AcpAgent::CopilotCli);
        assert!(bridge_id.starts_with("kronn-internal-") && bridge_id.len() == 27);
        assert_ne!(bridge_id, session_bridge_id(AcpAgent::CopilotCli));
        let broker = session_broker(
            AcpAgent::CopilotCli,
            false,
            Some(AcpSessionScope::new(None, "disc")),
        );
        let servers = native_session_mcp_servers(
            &broker,
            vec![declared_bridge()],
            Some(owned_bridge()),
            &bridge_id,
        );
        let args = native_mcp_launch_args(AcpAgent::CopilotCli, &servers);
        assert_eq!(&args[..2], ["--disable-mcp-server", "kronn-internal"]);
        assert_eq!(args[2], "--additional-mcp-config");
        let config: Value = serde_json::from_str(&args[3]).unwrap();
        assert_eq!(
            config,
            json!({"mcpServers": {bridge_id.clone(): {
                "command": "owned-kronn-mcp", "args": ["bridge.py"], "env": {}}}})
        );
        assert_eq!(args[4..], [format!("--allow-tool={bridge_id}")]);

        // No bridge for this session (audit, a step without Kronn tools): the
        // well-known name is still disabled and nothing is granted.
        assert_eq!(
            native_mcp_launch_args(AcpAgent::CopilotCli, &[]),
            vec!["--disable-mcp-server", "kronn-internal"]
        );
        for agent in [
            AcpAgent::Vibe,
            AcpAgent::GeminiCli,
            AcpAgent::Kiro,
            AcpAgent::OpenCode,
        ] {
            assert!(
                native_mcp_launch_args(agent, &servers).is_empty(),
                "{agent:?}"
            );
        }
        assert_eq!(session_bridge_id(AcpAgent::GeminiCli), "kronn-internal");
        assert_eq!(session_bridge_id(AcpAgent::Kiro), "kronn-internal");
    }

    /// Vibe's `tool_call` announcement reaches its broker through the
    /// dispatcher, ahead of the permission request that names only the id.
    #[test]
    fn the_dispatcher_hands_vibe_s_announcement_to_its_broker_only() {
        let announcement = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s1",
                "update": {
                    "_meta": {"effect_kind": "tool", "tool_name": "mcp_kronn_internal.disc_meta"},
                    "kind": "other", "rawInput": {}, "sessionUpdate": "tool_call",
                    "status": "in_progress", "title": "mcp_kronn_internal.disc_meta",
                    "toolCallId": "effect-1"
                }
            }
        });
        let request = json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "effect-1"},
            "options": [
                {"optionId": "allow_once", "name": "Allow", "kind": "allow_once"},
                {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"}
            ]
        });
        let decide = |agent: AcpAgent| {
            let broker = session_broker(agent, false, Some(AcpSessionScope::new(None, "disc")));
            broker.bind_protocol_session("s1").unwrap();
            broker.register_trusted_mcp_server(&AcpMcpServer {
                id: "kronn-internal".into(),
                command: "owned-kronn-mcp".into(),
                args: vec![],
                allowed_tools: Vec::new(),
            });
            observe_notification(&broker, &announcement);
            handle_client_request(&broker, "session/request_permission", &request).unwrap()
                ["outcome"]["optionId"]
                .clone()
        };
        assert_eq!(decide(AcpAgent::Vibe), "allow_once");
        assert_eq!(decide(AcpAgent::CopilotCli), "reject_once");
        assert_eq!(decide(AcpAgent::OpenCode), "reject_once");
    }

    #[test]
    fn an_audit_session_gets_no_kronn_internal_bridge() {
        let project = tempfile::tempdir().unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "unbound-discussion"),
        );
        let launch = crate::agents::runner::InternalMcpCommand {
            command: "owned-kronn-mcp".into(),
            args: vec![],
            env: Default::default(),
        };
        let bridge = AcpMcpServer {
            id: "kronn-internal".into(),
            command: "owned-kronn-mcp".into(),
            args: vec![],
            allowed_tools: Vec::new(),
        };
        let _audit = crate::core::audit_mcp_filter::AuditSessionGuard::enter(project.path());
        assert!(
            native_session_mcp_servers(&broker, vec![bridge], Some(launch), "kronn-internal")
                .is_empty()
        );
        assert!(broker
            .audit_log()
            .iter()
            .any(|e| e.server.as_deref() == Some("kronn-internal")
                && e.reason.contains("excluded from audits")));
    }

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

    /// OpenCode builds the `model` option as one flat `provider/model` value per
    /// model of every provider it loaded for the session directory, then an
    /// `effort` and a `mode` option (read from the 1.18.33 bundle). A local
    /// provider such as Ollama is a provider like another: its models must come
    /// out one for one, the `:` of a tag included, and nothing else may be added.
    #[test]
    fn a_local_provider_model_is_read_like_any_other_opencode_model() {
        let options = parse_config_options(&json!({
            "sessionId": "ses_local",
            "configOptions": [
                {
                    "id": "model", "name": "Model", "category": "model", "type": "select",
                    "currentValue": "ollama/qwen3.8:27b",
                    "options": [
                        {"value": "ollama/llama3.3:70b", "name": "Ollama/Llama 3.3 70B"},
                        {"value": "ollama/qwen3.8:27b", "name": "Ollama/Qwen3.8 27B"},
                        {"value": "opencode/big-pickle", "name": "OpenCode Zen/Big Pickle"}
                    ]
                },
                {
                    "id": "mode", "name": "Session Mode", "category": "mode", "type": "select",
                    "currentValue": "build",
                    "options": [{"value": "build", "name": "build"}, {"value": "plan", "name": "plan"}]
                }
            ]
        }));
        let model = options
            .iter()
            .find(|option| is_model_option_id(&option.id))
            .expect("model option");
        assert_eq!(model.current.as_deref(), Some("ollama/qwen3.8:27b"));
        assert_eq!(
            model
                .available
                .iter()
                .map(|value| value.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "ollama/llama3.3:70b",
                "ollama/qwen3.8:27b",
                "opencode/big-pickle"
            ]
        );
        assert!(
            options
                .iter()
                .filter(|option| is_model_option_id(&option.id))
                .count()
                == 1,
            "a session mode is not a model catalogue"
        );
    }

    /// ACP also allows a select option to group its values. A group is not a
    /// value: it has no `value` of its own, and reading only the flat shape
    /// dropped every model of a grouped catalogue.
    #[test]
    fn grouped_select_options_are_flattened_into_their_values() {
        let options = parse_config_options(&json!({
            "sessionId": "s1",
            "configOptions": [{
                "id": "model",
                "currentValue": "ollama/qwen3.8:27b",
                "options": [
                    {"group": "ollama", "name": "Ollama", "options": [
                        {"value": "ollama/qwen3.8:27b", "name": "Qwen3.8 27B"},
                        {"value": "ollama/llama3.3:70b", "name": "Llama 3.3 70B"}
                    ]},
                    {"group": "opencode", "name": "OpenCode Zen", "options": [
                        {"value": "opencode/big-pickle", "name": "Big Pickle"},
                        {"value": "ollama/qwen3.8:27b", "name": "duplicate of a value above"}
                    ]},
                    {"value": "loose/model", "name": "Loose"}
                ]
            }]
        }));
        let ids: Vec<&str> = options[0]
            .available
            .iter()
            .map(|value| value.id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![
                "ollama/qwen3.8:27b",
                "ollama/llama3.3:70b",
                "opencode/big-pickle",
                "loose/model"
            ]
        );
        assert_eq!(options[0].available[0].name, "Qwen3.8 27B");
    }

    /// A value of another option (an effort level, a session mode) must never
    /// be taken for a model, even when it carries the model's name.
    #[tokio::test]
    async fn a_chosen_model_is_matched_in_the_model_catalogue_before_any_other_option() {
        let recorded = Arc::new(Mutex::new(None));
        let mut host = AcpHost::new(
            1,
            Arc::new(ModelTransport {
                options: vec![
                    AcpConfigOption {
                        id: "effort".into(),
                        current: Some("default".into()),
                        available: vec![AcpConfigValue {
                            id: "default".into(),
                            name: "Default".into(),
                        }],
                    },
                    AcpConfigOption {
                        id: "model".into(),
                        current: Some("opencode/big-pickle".into()),
                        available: vec![
                            AcpConfigValue {
                                id: "opencode/big-pickle".into(),
                                name: "Big Pickle".into(),
                            },
                            AcpConfigValue {
                                id: "default".into(),
                                name: "A model that is literally named default".into(),
                            },
                        ],
                    },
                ],
                recorded: recorded.clone(),
            }),
        );
        host.negotiate(request()).await.unwrap();
        let target = host.create_session().await.unwrap();

        assert!(host.offers_model_catalogue().await);
        assert!(host.select_model(&target, "default").await.unwrap());
        assert_eq!(
            recorded
                .lock()
                .await
                .as_ref()
                .map(|(c, v)| (c.as_str(), v.as_str())),
            Some(("model", "default")),
            "the model option, not the effort option, receives the choice"
        );
    }

    #[tokio::test]
    async fn a_session_without_a_model_list_does_not_claim_a_model_catalogue() {
        let mut host = AcpHost::new(
            1,
            Arc::new(ModelTransport {
                options: vec![AcpConfigOption {
                    id: "mode".into(),
                    current: Some("build".into()),
                    available: vec![AcpConfigValue {
                        id: "build".into(),
                        name: "build".into(),
                    }],
                }],
                recorded: Arc::new(Mutex::new(None)),
            }),
        );
        host.negotiate(request()).await.unwrap();
        host.create_session().await.unwrap();

        assert!(!host.offers_model_catalogue().await);
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
    fn production_routes_distinguish_native_adapted_and_http_transports() {
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
            assert_eq!(production_route(&agent), AcpProductionRoute::AdaptedAcp);
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
    fn the_adapted_route_is_default_and_never_widens_other_agents() {
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CODEX");
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::AdaptedAcp
        );
        assert_eq!(
            resolve_acp_route(&AgentType::ClaudeCode),
            AcpProductionRoute::AdaptedAcp
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
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CODEX");
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
        crate::core::child_env::set_var("KRONN_ACP_ADAPTER_CODEX", "0");
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::DirectCliMigration
        );
        assert_eq!(
            resolve_acp_route(&AgentType::ClaudeCode),
            AcpProductionRoute::AdaptedAcp,
            "Claude's route must stay unaffected by Codex's toggle"
        );
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CODEX");

        crate::core::child_env::set_var("KRONN_ACP_ADAPTER_CLAUDE", "0");
        assert_eq!(
            resolve_acp_route(&AgentType::ClaudeCode),
            AcpProductionRoute::DirectCliMigration
        );
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::AdaptedAcp,
            "Codex's route must stay unaffected by Claude's toggle"
        );
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CLAUDE");
    }

    #[test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    fn only_1_or_true_activate_the_adapter_toggle_every_other_value_stays_direct_cli() {
        // Regression: `.is_ok()` used to activate the adapter for ANY
        // present value, including the exact strings an operator would type
        // to mean "off" (`"0"`, `"false"`) without realizing only unsetting
        // the variable actually disables it.
        for falsy in ["0", "false", "False", "FALSE", "no", "off", "", "  ", "2"] {
            crate::core::child_env::set_var("KRONN_ACP_ADAPTER_CODEX", falsy);
            assert_eq!(
                resolve_acp_route(&AgentType::Codex),
                AcpProductionRoute::DirectCliMigration,
                "KRONN_ACP_ADAPTER_CODEX={falsy:?} must NOT activate the adapter"
            );
        }
        for truthy in ["1", "true", "True", "TRUE", " 1 ", " true "] {
            crate::core::child_env::set_var("KRONN_ACP_ADAPTER_CODEX", truthy);
            assert_eq!(
                resolve_acp_route(&AgentType::Codex),
                AcpProductionRoute::AdaptedAcp,
                "KRONN_ACP_ADAPTER_CODEX={truthy:?} must activate the adapter"
            );
        }
        crate::core::child_env::remove_var("KRONN_ACP_ADAPTER_CODEX");
        assert_eq!(
            resolve_acp_route(&AgentType::Codex),
            AcpProductionRoute::AdaptedAcp,
            "unset must use the product adapter default"
        );
    }

    #[test]
    fn native_acp_wsl_refusal_only_applies_to_windows_hosts() {
        // Linux and macOS hosts never route through WSL, so nothing is refused.
        if !cfg!(target_os = "windows") {
            assert_eq!(native_acp_wsl_only_refusal("gemini"), None);
            assert_eq!(
                native_acp_wsl_only_refusal("definitely-not-installed-acp"),
                None
            );
        }
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

    /// A labelled non-answer must not become a successful-looking reply.
    #[test]
    fn labelled_non_agent_content_never_becomes_the_reply() {
        for kind in [
            "user_message_chunk",
            "agent_thought_chunk",
            "tool_call",
            "tool_call_update",
            "future_update",
        ] {
            for content in [
                json!("injected prompt or non-answer"),
                json!({"type": "text", "text": "injected prompt or non-answer"}),
                json!([{"type": "text", "text": "injected prompt or non-answer"}]),
            ] {
                let events = events_from_notifications(
                    vec![json!({
                        "jsonrpc": "2.0", "method": "session/update",
                        "params": {"sessionId": "vibe-session", "update": {
                            "sessionUpdate": kind, "content": content,
                        }}
                    })],
                    "vibe-session",
                );
                assert!(
                    events.is_empty(),
                    "{kind} leaked into the answer: {events:?}"
                );
            }
        }
    }

    #[test]
    fn echoed_user_prompt_keeps_agent_reply_tool_and_usage_events_distinct() {
        let events = events_from_notifications(
            vec![
                json!({"params": {"sessionId": "vibe-session", "update": {
                    "sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "Kronn instructions + user prompt"}
                }}}),
                json!({"params": {"sessionId": "vibe-session", "update": {
                    "sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "read_file",
                    "content": [{"type": "text", "text": "tool output, not the answer"}]
                }}}),
                json!({"params": {"sessionId": "vibe-session", "update": {
                    "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Bonjour 🦀"},
                    "usage": {"inputTokens": 30, "outputTokens": 4}
                }}}),
            ],
            "vibe-session",
        );
        let events = without_activity(events);
        assert_eq!(
            events,
            vec![
                AcpSessionEvent::ToolCall {
                    name: "read_file".into()
                },
                AcpSessionEvent::ToolTrace(crate::agents::tool_trace::ToolTraceUpdate {
                    id: "call-1".into(),
                    name: Some("read_file".into()),
                    input: None,
                    status: None,
                }),
                AcpSessionEvent::TextDelta("Bonjour 🦀".into()),
                AcpSessionEvent::Usage {
                    input_tokens: 30,
                    output_tokens: 4,
                    prompt_cache: Default::default(),
                },
            ]
        );
    }

    /// Generic ACP: the live-view activity is built from the kind and the raw
    /// input; the title, often the command line itself, never reaches it.
    #[test]
    fn a_generic_tool_call_feeds_the_live_views_from_its_structured_fields() {
        let events = events_from_notifications(
            vec![
                json!({"params": {"sessionId": "s1", "update": {
                    "sessionUpdate": "tool_call", "toolCallId": "call-9", "kind": "execute",
                    "title": "PGPASSWORD=hunter2 psql -h db", "status": "pending",
                    "rawInput": {"command": "PGPASSWORD=hunter2 psql -h db"}
                }}}),
                json!({"params": {"sessionId": "s1", "update": {
                    "sessionUpdate": "tool_call_update", "toolCallId": "call-9",
                    "title": "PGPASSWORD=hunter2 psql -h db...", "status": "in_progress"
                }}}),
            ],
            "s1",
        );
        let activity: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AcpSessionEvent::ToolActivity(update) => Some(update.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(activity.len(), 2, "{events:?}");
        let mut recent = crate::agents::activity::RecentActivity::default();
        let started: Vec<bool> = activity.iter().map(|update| recent.apply(update)).collect();
        assert_eq!(started, [true, false], "one call, one entry");
        let shown = recent.snapshot();
        assert_eq!(shown.entries[0].tool, "Execute");
        assert_eq!(
            shown.entries[0].target.as_deref(),
            Some("PGPASSWORD=***REDACTED*** psql")
        );
        assert!(!format!("{activity:?}").contains("hunter2"));
    }

    /// The events other than the live-view activity, which its own tests pin.
    fn without_activity(events: Vec<AcpSessionEvent>) -> Vec<AcpSessionEvent> {
        events
            .into_iter()
            .filter(|event| !matches!(event, AcpSessionEvent::ToolActivity(_)))
            .collect()
    }

    /// KT-932 follow-up — a `tool_call_update` reaching a terminal status is
    /// the tool ending, a distinct event from every other update on it (which
    /// still just prove the tool call is alive).
    #[test]
    fn a_tool_call_update_with_a_terminal_status_ends_it() {
        for status in ["completed", "failed", "cancelled"] {
            let events = events_from_notifications(
                vec![json!({"params": {"sessionId": "s1", "update": {
                    "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": status,
                }}})],
                "s1",
            );
            let events = without_activity(events);
            assert_eq!(
                events,
                vec![
                    AcpSessionEvent::ToolCallEnded,
                    AcpSessionEvent::ToolTrace(crate::agents::tool_trace::ToolTraceUpdate {
                        id: "call-1".into(),
                        name: None,
                        input: None,
                        status: Some(status.into()),
                    }),
                ],
                "{status}"
            );
        }

        for status in ["pending", "in_progress"] {
            let events = events_from_notifications(
                vec![json!({"params": {"sessionId": "s1", "update": {
                    "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": status,
                    "title": "read_file",
                }}})],
                "s1",
            );
            let events = without_activity(events);
            assert_eq!(
                events,
                vec![
                    AcpSessionEvent::ToolCall {
                        name: "read_file".into()
                    },
                    AcpSessionEvent::ToolTrace(crate::agents::tool_trace::ToolTraceUpdate {
                        id: "call-1".into(),
                        name: Some("read_file".into()),
                        input: None,
                        status: Some("in_progress".into()),
                    }),
                ],
                "{status} is still the tool call in progress, not its end"
            );
        }

        // No status at all (the initial `tool_call` announcement in most
        // fixtures): still the call starting, never its end.
        let events = events_from_notifications(
            vec![json!({"params": {"sessionId": "s1", "update": {
                "sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "cargo test",
            }}})],
            "s1",
        );
        let events = without_activity(events);
        assert_eq!(
            events,
            vec![
                AcpSessionEvent::ToolCall {
                    name: "cargo test".into()
                },
                AcpSessionEvent::ToolTrace(crate::agents::tool_trace::ToolTraceUpdate {
                    id: "call-1".into(),
                    name: Some("cargo test".into()),
                    input: None,
                    status: None,
                }),
            ]
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
                output_tokens: 28,
                prompt_cache: Default::default(),
            }),
        );
    }

    /// KT-927 — the cache the runtime reports travels with the turn's usage;
    /// what it does not report stays absent, never zero.
    #[test]
    fn prompt_usage_carries_the_cache_tokens_the_runtime_reports() {
        let with_cache = json!({
            "usage": {"inputTokens": 100, "outputTokens": 20, "totalTokens": 160,
                      "cachedReadTokens": 30, "cachedWriteTokens": 10}
        });
        assert_eq!(
            usage_from_prompt_result(&with_cache),
            Some(AcpSessionEvent::Usage {
                input_tokens: 100,
                output_tokens: 20,
                prompt_cache: crate::agents::runner::PromptCacheUsage {
                    cached_prompt_tokens: Some(30),
                    cache_write_prompt_tokens: Some(10),
                },
            }),
        );
        let read_only =
            json!({"usage": {"inputTokens": 100, "outputTokens": 20, "cachedReadTokens": 30}});
        match usage_from_prompt_result(&read_only) {
            Some(AcpSessionEvent::Usage { prompt_cache, .. }) => {
                assert_eq!(prompt_cache.cached_prompt_tokens, Some(30));
                assert_eq!(prompt_cache.cache_write_prompt_tokens, None);
            }
            other => panic!("expected usage, got {other:?}"),
        }
    }

    /// KT-927 — with full access OpenCode is started with Kronn's read policy,
    /// unless the operator already passes an inline configuration of their own.
    /// Without full access Kronn's permissions always apply (see the tests of
    /// `secret_files::opencode_launch_config`), or OpenCode is not started.
    #[test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    fn opencode_starts_with_the_read_policy_unless_the_operator_brought_their_own() {
        fn config_of(command: &tokio::process::Command) -> Option<String> {
            command
                .as_std()
                .get_envs()
                .find(|(name, _)| *name == "OPENCODE_CONFIG_CONTENT")
                .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
        }

        crate::core::child_env::remove_var("OPENCODE_CONFIG_CONTENT");
        let mut command = tokio::process::Command::new("opencode");
        apply_opencode_policy(&mut command, true, &[]).unwrap();
        assert_eq!(
            config_of(&command).as_deref(),
            Some(secret_files::opencode_config_content().as_str())
        );

        crate::core::child_env::set_var("OPENCODE_CONFIG_CONTENT", r#"{"theme":"mine"}"#);
        let mut command = tokio::process::Command::new("opencode");
        apply_opencode_policy(&mut command, true, &[]).unwrap();
        assert_eq!(
            config_of(&command),
            None,
            "with full access the operator's own inline configuration is inherited untouched"
        );

        // Without full access: merged over the operator's, bridge tools allowed.
        let bridge = AcpMcpServer {
            id: "kronn-internal-0123456789ab".into(),
            command: "python3".into(),
            args: vec![],
            allowed_tools: Vec::new(),
        };
        let mut command = tokio::process::Command::new("opencode");
        apply_opencode_policy(&mut command, false, std::slice::from_ref(&bridge)).unwrap();
        let config: Value = serde_json::from_str(&config_of(&command).unwrap()).unwrap();
        assert_eq!(config["theme"], "mine");
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(
            config["permission"]["kronn-internal-0123456789ab_*"],
            "allow"
        );

        crate::core::child_env::set_var("OPENCODE_CONFIG_CONTENT", "not json");
        let mut command = tokio::process::Command::new("opencode");
        let refused = apply_opencode_policy(&mut command, false, &[]);
        crate::core::child_env::remove_var("OPENCODE_CONFIG_CONTENT");
        assert!(refused.is_err(), "never started unrestricted");
    }

    /// KT-927 — OpenCode, scripted, on the real transport and the real broker.
    /// It asks to read an environment file WITHOUT saying which one (that is
    /// what OpenCode's `read` permission request carries), is refused, goes on
    /// with its turn and ends it reporting what it consumed. Before, the refusal
    /// ended the turn and the step with it, and the usage was never read.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    async fn a_refused_read_does_not_end_the_turn_and_the_turn_reports_its_usage() {
        let out = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        crate::core::child_env::remove_var("OPENCODE_CONFIG_CONTENT");
        // Only the program differs from `spawn_native`: the environment Kronn
        // gives OpenCode is built by the same function.
        let mut command = native_command(
            AcpAgent::OpenCode,
            "python3",
            &["-c", test_support::OPENCODE_ACP_FIXTURE],
            &project.path().to_string_lossy(),
            &NativeLaunchEnv::default(),
        )
        .unwrap();
        apply_opencode_policy(&mut command, true, &[]).unwrap();
        command.env("OPENCODE_FIXTURE_OUT", out.path());
        let transport = AcpJsonRpcTransport::spawn_scoped(
            AcpAgent::OpenCode,
            command,
            true,
            Some(AcpSessionScope::new(
                Some(project.path().to_path_buf()),
                "disc-kt927",
            )),
        )
        .await
        .unwrap();
        transport.initialize(request()).await.unwrap();
        let target = transport.create_session().await.unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        transport
            .prompt(&target, "read .env.dist then .env", tx)
            .await
            .expect("a refused read must not end the turn");
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        transport.shutdown().await.unwrap();

        let reply: Value =
            serde_json::from_str(&std::fs::read_to_string(out.path().join("reply.json")).unwrap())
                .unwrap();
        assert_eq!(reply["id"], json!(99));
        assert_eq!(
            reply["result"]["outcome"],
            json!({"outcome": "selected", "optionId": "reject"}),
            "the refusal is an answer to the tool call, which OpenCode hands its model"
        );
        assert!(
            events.contains(&AcpSessionEvent::TextDelta(
                "carried on after the refusal".into()
            )),
            "the turn went on: {events:?}"
        );
        assert!(
            events.contains(&AcpSessionEvent::Usage {
                input_tokens: 100,
                output_tokens: 20,
                prompt_cache: crate::agents::runner::PromptCacheUsage {
                    cached_prompt_tokens: Some(30),
                    cache_write_prompt_tokens: Some(10),
                },
            }),
            "input, output and cache are read from the end of the turn: {events:?}"
        );
        assert_eq!(events.last(), Some(&AcpSessionEvent::Completed));

        // What OpenCode was started with: real environment files denied,
        // templates allowed, and a refusal that does not stop the loop.
        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(out.path().join("config.json")).unwrap())
                .expect("the agent received Kronn's inline configuration");
        assert_eq!(config["experimental"]["continue_loop_on_deny"], true);
        assert_eq!(config["permission"]["read"]["*.env"], "deny");
        assert_eq!(config["permission"]["read"]["*.env.*"], "deny");
        assert_eq!(config["permission"]["read"]["*.env.dist"], "allow");
        assert_eq!(config["permission"]["read"]["*.env.example"], "allow");
    }

    /// A non-UTF-8 byte on the agent's stdout costs one discarded frame, not
    /// the session: the dispatcher keeps reading and answers the next request.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_non_utf8_stdout_line_does_not_end_the_session() {
        let peer = r#"
import json, sys
sys.stdout.buffer.write(b"\xff not json\n")
sys.stdout.flush()
for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") == "initialize":
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {"protocolVersion": 1}}) + "\n")
        sys.stdout.flush()
"#;
        let mut command = tokio::process::Command::new("python3");
        command.args(["-c", peer]);
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        timeout(FIXTURE_GUARD, transport.initialize(request()))
            .await
            .expect("the dispatcher keeps reading after the invalid byte")
            .expect("initialize succeeds");
        transport.shutdown().await.unwrap();
    }

    /// KT-927 — stopping the agent stops what it started. Killing the ACP child
    /// alone left its shell commands running.
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_stops_the_processes_the_agent_started_too() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("descendant.pid");
        let mut command = tokio::process::Command::new("sh");
        command
            .env("PID_FILE", &pid_file)
            .args(["-c", "sleep 300 & echo $! > \"$PID_FILE\"; wait"]);
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
            .await
            .unwrap();
        let alive = |pid: i32| unsafe { libc::kill(pid, 0) == 0 };
        let descendant = timeout(FIXTURE_GUARD, async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pid_file) {
                    if let Ok(pid) = text.trim().parse::<i32>() {
                        return pid;
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the agent starts its helper");
        assert!(alive(descendant), "the helper runs before the stop");

        transport.shutdown().await.unwrap();

        timeout(FIXTURE_GUARD, async {
            while alive(descendant) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the helper must not outlive the agent it belongs to");
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
        let mut command = tokio::process::Command::new("sh");
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
                    output_tokens: 5,
                    prompt_cache: Default::default(),
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
        let mut command = tokio::process::Command::new("sh");
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
    fn a_prompt_turn_is_not_held_to_the_handshake_budget() {
        // Every ACP request shared one 30 s timeout, `session/prompt` included.
        // A prompt turn is the agent reading, thinking, calling its tools and
        // writing an answer — so any turn longer than half a minute died with
        // "ACP request timed out: session/prompt", while the CLI path grants
        // the same work fifteen minutes of silence and up to two hours overall.
        assert_eq!(request_timeout("initialize"), CONTROL_REQUEST_TIMEOUT);
        assert_eq!(request_timeout("session/new"), CONTROL_REQUEST_TIMEOUT);
        assert_eq!(request_timeout("session/cancel"), CONTROL_REQUEST_TIMEOUT);

        assert_eq!(request_timeout("session/prompt"), PROMPT_REQUEST_TIMEOUT);

        // Coordinated with the operator's own setting, not picked: the backstop
        // must sit above the ceiling their configured global timeout is clamped
        // to, or it silently overrides what they set — and it must follow that
        // ceiling if it ever moves.
        let configurable_ceiling =
            Duration::from_secs(u64::from(crate::models::MAX_AGENT_GLOBAL_TIMEOUT_MIN) * 60);
        assert!(
            PROMPT_REQUEST_TIMEOUT > configurable_ceiling,
            "backstop {PROMPT_REQUEST_TIMEOUT:?} must exceed the configurable \
             ceiling {configurable_ceiling:?}"
        );
        // And not by so much that a wedged runtime is held forever.
        assert!(PROMPT_REQUEST_TIMEOUT < configurable_ceiling * 2);
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
        let mut command = tokio::process::Command::new("sh");
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
        let mut command = tokio::process::Command::new("sh");
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
            let mut command = tokio::process::Command::new("sh");
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
            group: None,
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

    /// A drain that PANICS is a failure, not a stop we asked for. The nominal
    /// join must surface it instead of folding it into the non-fatal policy
    /// that covers the cancellation.
    ///
    /// This exercises the nominal branch: the task ends at once, so the bound
    /// never elapses. The post-abort panic branch is written to the same rule
    /// but is NOT covered here — reaching it would mean waiting out the bound.
    #[tokio::test]
    async fn a_panicking_drain_is_reported_as_an_error() {
        let owner = DispatcherOwner::new(tokio::spawn(async {
            panic!("the drain fell over");
        }));

        let error = owner
            .finish()
            .await
            .expect_err("a panicking drain must not be reported as a clean stop");

        assert!(
            error.contains("join ACP dispatcher"),
            "the panic must surface through the join, got: {error}"
        );
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

    /// A liveness rendezvous the TEST owns, over a real async socket.
    ///
    /// A FIFO read through `tokio::fs` is delegated to `spawn_blocking`, where
    /// a future timeout does not cancel the blocking call — a RED could pin a
    /// blocking-pool thread for the rest of the suite. A `UnixStream` is polled
    /// by the reactor instead: every wait here is cancellable.
    ///
    /// The fixture exits on its OWN when the test drops the connection, so
    /// cleanup works against the broken implementation too. It deliberately
    /// SURVIVES stdin EOF, the way a real agent does — otherwise dropping the
    /// transport would end it for a reason that has nothing to do with process
    /// ownership, and the RED would pass without the fix.
    #[cfg(unix)]
    struct FixtureLink {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        listener: tokio::net::UnixListener,
    }

    /// Anti-hang bound only: every wait below resolves on a socket event, and
    /// this exists so a broken build fails instead of blocking the suite.
    #[cfg(unix)]
    const FIXTURE_GUARD: Duration = Duration::from_secs(30);

    #[cfg(unix)]
    impl FixtureLink {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("acp-fixture.sock");
            let listener = tokio::net::UnixListener::bind(&path).unwrap();
            Self {
                _dir: dir,
                path,
                listener,
            }
        }

        /// python3 is already a project dependency (`core::mcp_scanner`,
        /// `core::quick_exec`). Only the stdlib is used here.
        fn command(&self, initialize_reply: Option<&str>) -> tokio::process::Command {
            let mut command = tokio::process::Command::new("python3");
            command.env("ACP_SOCK", &self.path);
            if let Some(reply) = initialize_reply {
                command.env("ACP_REPLY", reply);
            }
            command.args([
                "-c",
                r#"
import os, select, socket, sys

sock = socket.socket(socket.AF_UNIX)
sock.connect(os.environ["ACP_SOCK"])
sock.sendall(b"READY")

reply = os.environ.get("ACP_REPLY", "")
stdin = sys.stdin.buffer
watch = [sock, stdin]
while True:
    ready, _, _ = select.select(watch, [], [])
    if sock in ready:
        if not sock.recv(1):
            break                 # the test released us: exit on our own
    if stdin in ready:
        line = stdin.readline()
        if not line:
            watch = [sock]        # a real agent survives stdin EOF
        elif reply and '"method":"initialize"' in line.decode("utf-8", "replace"):
            sys.stdout.write(reply + "\n")
            sys.stdout.flush()
"#,
            ]);
            command
        }

        /// Accept the fixture and read its READY, proving it reached its
        /// blocking state before the test acts on the transport.
        async fn accept_ready(&self) -> tokio::net::UnixStream {
            let (mut stream, _) = timeout(FIXTURE_GUARD, self.listener.accept())
                .await
                .expect("the fixture must connect")
                .expect("the fixture must connect");
            let mut ready = [0u8; 5];
            timeout(FIXTURE_GUARD, stream.read_exact(&mut ready))
                .await
                .expect("the fixture must announce itself")
                .expect("the fixture must announce itself");
            assert_eq!(&ready, b"READY");
            stream
        }
    }

    /// EOF on the socket means the fixture is gone. An IO error is NOT counted
    /// as success: it would prove nothing about the process.
    #[cfg(unix)]
    async fn fixture_is_gone(stream: &mut tokio::net::UnixStream) -> bool {
        let mut rest = Vec::new();
        matches!(
            timeout(FIXTURE_GUARD, stream.read_to_end(&mut rest)).await,
            Ok(Ok(_))
        )
    }

    /// DoD — abandonment: a transport dropped after a REJECTED negotiation must
    /// not leave its process behind. The fixture ignores stdin EOF, so only
    /// real process ownership can end it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_rejected_negotiation_dropped_terminates_the_owned_fixture() {
        let link = FixtureLink::new();
        let command = link.command(Some(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2}}"#,
        ));
        let transport = Arc::new(
            AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, command, false)
                .await
                .unwrap(),
        );
        let mut alive = link.accept_ready().await;
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
        let link = FixtureLink::new();
        let transport = AcpJsonRpcTransport::spawn(AcpAgent::OpenCode, link.command(None), false)
            .await
            .unwrap();
        let mut alive = link.accept_ready().await;

        transport.shutdown().await.unwrap();
        transport.shutdown().await.unwrap();

        assert!(
            fixture_is_gone(&mut alive).await,
            "shutdown must wait until the owned fixture is reaped"
        );
    }
}
