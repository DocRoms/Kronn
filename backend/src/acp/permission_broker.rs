//! Scoped, audited, deny-by-default broker for ACP client-side requests.
//!
//! One broker type backs two different enforcement mechanisms because the
//! transports it serves are structurally different:
//! - Native ACP agents (`AcpJsonRpcTransport`) can call back into Kronn live,
//!   over the JSON-RPC channel, mid-session (`session/request_permission`,
//!   `fs/*`, `terminal/*`). The dispatcher consults the broker per request.
//! - The Codex/Claude adapters wrap non-interactive CLIs with no live
//!   callback (verified: neither `codex exec` nor `claude --print` exposes a
//!   permission-prompt callback flag). The broker's decision is computed once
//!   per session and translated into static CLI flags instead.
//!
//! Both paths share the same policy (`full_access` gate) and the same audit
//! trail shape, so "one broker" is true of the decision logic even though the
//! wire mechanism differs.

use super::secret_files::is_secret_file;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// JSON-RPC 2.0 reserves -32000..-32099 for implementation-defined server
/// errors. Kronn has not bound a scoped executor for `fs/*`/`terminal/*`
/// (ADR-003): every such request is refused with this code rather than
/// silently granted.
pub const ACP_CAPABILITY_NOT_GRANTED: i64 = -32001;
/// Standard JSON-RPC 2.0 "Method not found".
pub const ACP_METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionVerdict {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAuditEntry {
    pub method: String,
    pub verdict: AcpPermissionVerdict,
    pub reason: String,
    /// Correlates this entry back to one ACP session — a discussion id when
    /// known, else a caller-supplied label. `"unscoped"` for a broker built
    /// with [`AcpPermissionBroker::new`] (no [`AcpSessionScope`]), which is
    /// never true in production (every real transport now scopes its
    /// broker) but keeps existing unit tests and call sites compiling
    /// (KT-542 review: audit entries must be correlable across sessions).
    pub session: String,
    pub protocol_session_id: Option<String>,
    pub server: Option<String>,
    pub tool: Option<String>,
    pub locations: Vec<String>,
}

/// Label the runner gives a session that serves no discussion.
pub(crate) const UNBOUND_SESSION_LABEL: &str = "unbound-discussion";

/// Project/session scope a broker enforces on top of the `full_access` gate.
/// Two independent, defense-in-depth checks key off this:
/// - [`AcpPermissionBroker::authorize_mcp_servers`] re-derives the project's
///   OWN authorized server set from `project_root` rather than trusting the
///   caller's candidate list, and drops anything not in it.
/// - [`AcpPermissionBroker::decide_tool_call_permission`] denies a `read`/
///   `search`/`think`/`fetch` tool call whose reported `locations` escape
///   `project_root`, even under `full_access` — `full_access` broadens which
///   OPERATIONS are allowed inside this session's own project, never which
///   project/session it may touch.
#[derive(Debug, Clone)]
pub struct AcpSessionScope {
    /// The project this session is bound to. `None` disables path scoping
    /// (e.g. a global, non-project-bound discussion).
    pub project_root: Option<PathBuf>,
    /// Opaque, non-secret label correlating every audit entry from this
    /// broker back to one session (a discussion id, when known).
    pub session_label: String,
}

impl AcpSessionScope {
    pub fn new(project_root: Option<PathBuf>, session_label: impl Into<String>) -> Self {
        Self {
            project_root,
            session_label: session_label.into(),
        }
    }
}

/// Static CLI policy for a direct-CLI adapter session (Codex/Claude), as
/// computed once by the broker from the same `full_access` gate the live
/// dispatcher path uses. `None` means "the runtime's own default applies" —
/// deny-by-default is expressed by omitting a bypass flag, not by adding one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcpSessionPolicy {
    /// Claude Code: pass `--dangerously-skip-permissions` when true.
    pub claude_skip_permissions: bool,
    /// Codex: `-s <value>` sandbox override. `None` keeps Codex's own default
    /// (read-only) rather than asserting a flag Kronn cannot justify.
    pub codex_sandbox: Option<&'static str>,
}

/// Scoped decision-maker for one ACP session. Every decision — live or
/// pre-session — is recorded in `audit_log` with a normalized, actionable
/// reason so operators can see exactly why a request was allowed or refused.
pub struct AcpPermissionBroker {
    full_access: bool,
    scope: Option<AcpSessionScope>,
    protocol_session_id: Mutex<Option<String>>,
    authorized_tools: Mutex<BTreeMap<String, BTreeSet<String>>>,
    trusted_internal_mcp: AtomicBool,
    /// Vibe's permission request carries only a `toolCallId`; the tool it names
    /// is the harness-resolved `_meta.tool_name` of the preceding `tool_call`.
    harness_tool_names: AtomicBool,
    observed_tools: Mutex<BTreeMap<String, String>>,
    audit_log: Mutex<Vec<AcpAuditEntry>>,
}

/// Bound on remembered tool calls: one is consumed by its permission request.
const MAX_OBSERVED_TOOL_CALLS: usize = 256;

impl AcpPermissionBroker {
    pub fn new(full_access: bool) -> Self {
        Self {
            full_access,
            scope: None,
            protocol_session_id: Mutex::new(None),
            authorized_tools: Mutex::new(BTreeMap::new()),
            trusted_internal_mcp: AtomicBool::new(false),
            harness_tool_names: AtomicBool::new(false),
            observed_tools: Mutex::new(BTreeMap::new()),
            audit_log: Mutex::new(Vec::new()),
        }
    }

    /// A broker bound to a project/session scope (KT-542 review). Every real
    /// transport (native ACP, the Codex adapter, the Claude adapter)
    /// constructs its broker this way; `new` remains for callers/tests with
    /// no project to scope to.
    pub fn scoped(full_access: bool, scope: AcpSessionScope) -> Self {
        Self {
            full_access,
            scope: Some(scope),
            protocol_session_id: Mutex::new(None),
            authorized_tools: Mutex::new(BTreeMap::new()),
            trusted_internal_mcp: AtomicBool::new(false),
            harness_tool_names: AtomicBool::new(false),
            observed_tools: Mutex::new(BTreeMap::new()),
            audit_log: Mutex::new(Vec::new()),
        }
    }

    pub fn full_access(&self) -> bool {
        self.full_access
    }

    fn session_label(&self) -> &str {
        self.scope
            .as_ref()
            .map(|scope| scope.session_label.as_str())
            .unwrap_or("unscoped")
    }

    /// Full audit trail recorded so far, in decision order.
    pub fn audit_log(&self) -> Vec<AcpAuditEntry> {
        self.audit_log
            .lock()
            .expect("ACP permission broker audit log mutex poisoned")
            .clone()
    }

    fn record(&self, method: &str, verdict: AcpPermissionVerdict, reason: String) {
        self.record_context(method, verdict, reason, None, None, Vec::new());
    }

    fn record_context(
        &self,
        method: &str,
        verdict: AcpPermissionVerdict,
        reason: String,
        server: Option<String>,
        tool: Option<String>,
        locations: Vec<String>,
    ) {
        let session = self.session_label().to_owned();
        let protocol_session_id = self
            .protocol_session_id
            .lock()
            .expect("ACP protocol-session mutex poisoned")
            .clone();
        tracing::info!(
            method,
            session = %session,
            protocol_session_id = protocol_session_id.as_deref().unwrap_or("unbound"),
            server = server.as_deref().unwrap_or("none"),
            tool = tool.as_deref().unwrap_or("none"),
            verdict = if verdict == AcpPermissionVerdict::Allow { "allow" } else { "deny" },
            reason = %reason,
            "ACP permission decision"
        );
        self.audit_log
            .lock()
            .expect("ACP permission broker audit log mutex poisoned")
            .push(AcpAuditEntry {
                method: method.to_owned(),
                verdict,
                reason,
                session,
                protocol_session_id,
                server,
                tool,
                locations,
            });
    }

    pub fn bind_protocol_session(&self, session_id: &str) -> Result<(), String> {
        let session_id = session_id.trim();
        if session_id.is_empty()
            || session_id.len() > 512
            || session_id.chars().any(char::is_control)
        {
            return Err("invalid ACP protocol session id".to_string());
        }
        *self
            .protocol_session_id
            .lock()
            .expect("ACP protocol-session mutex poisoned") = Some(session_id.to_owned());
        Ok(())
    }

    /// An audit step's session: its project is being audited and it serves no
    /// discussion (the runner labels such a session `unbound-discussion`). A
    /// room participant on the same project keeps its tools during an audit.
    fn is_audit_session(&self) -> bool {
        self.scope.as_ref().is_some_and(|scope| {
            scope.session_label == UNBOUND_SESSION_LABEL
                && scope
                    .project_root
                    .as_deref()
                    .is_some_and(crate::core::audit_mcp_filter::audit_in_progress)
        })
    }

    /// Whether the audit rule (`core::audit_mcp_filter::AUDIT_MCP_EXCLUDED`)
    /// withholds `server_id` from this session. A withheld server is recorded
    /// as an explicit refusal, never silently missing.
    pub fn audit_excludes(&self, server_id: &str) -> bool {
        let excluded = self.is_audit_session()
            && crate::core::audit_mcp_filter::excluded_from_audit(server_id);
        if excluded {
            self.record_context(
                "mcp/authorize_server",
                AcpPermissionVerdict::Deny,
                "excluded from audits by the audit MCP rule".to_owned(),
                Some(server_id.to_owned()),
                None,
                Vec::new(),
            );
        }
        excluded
    }

    /// `servers` without those the audit rule withholds from this session.
    pub fn without_audit_excluded(
        &self,
        servers: Vec<super::AcpMcpServer>,
    ) -> Vec<super::AcpMcpServer> {
        servers
            .into_iter()
            .filter(|server| !self.audit_excludes(&server.id))
            .collect()
    }

    /// Reconstruct candidates from this session's canonical project registry.
    /// Matching an id is insufficient: a caller could attach another command
    /// or arguments to an authorized name. Only an exact candidate match is
    /// retained, and the returned value is the independently reconstructed
    /// canonical declaration rather than caller-owned data.
    pub fn authorize_mcp_servers(
        &self,
        candidates: Vec<super::AcpMcpServer>,
    ) -> Vec<super::AcpMcpServer> {
        let candidates = self.without_audit_excluded(candidates);
        let Some(scope) = self.scope.as_ref() else {
            self.register_authorized_servers(&candidates);
            return candidates;
        };
        let Some(root) = scope.project_root.as_deref() else {
            for server in candidates {
                self.record_context(
                    "mcp/authorize_server",
                    AcpPermissionVerdict::Deny,
                    "project-less session cannot receive a project MCP server".to_owned(),
                    Some(server.id),
                    None,
                    Vec::new(),
                );
            }
            return Vec::new();
        };
        let canonical: BTreeMap<String, super::AcpMcpServer> =
            crate::core::mcp_scanner::read_mcp_json(&root.to_string_lossy())
                .map(|file| {
                    file.mcp_servers
                        .into_iter()
                        .filter_map(|(id, entry)| {
                            let command = entry.command.clone()?;
                            // Only command and args are compared and passed on:
                            // env values never leave the adapter that owns them.
                            (!command.trim().is_empty()
                                && !entry
                                    .args
                                    .as_deref()
                                    .is_some_and(crate::core::mcp_scanner::mcp_args_carry_secret))
                            .then_some((
                                id.clone(),
                                super::AcpMcpServer {
                                    id,
                                    command,
                                    args: entry.args.unwrap_or_default(),
                                    allowed_tools: Vec::new(),
                                },
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
        let mut authorized = Vec::new();
        for candidate in candidates {
            match canonical.get(&candidate.id) {
                Some(server) if server == &candidate => authorized.push(server.clone()),
                Some(_) => self.record_context(
                    "mcp/authorize_server",
                    AcpPermissionVerdict::Deny,
                    "candidate declaration differs from the canonical project server".to_owned(),
                    Some(candidate.id),
                    None,
                    Vec::new(),
                ),
                None => self.record_context(
                    "mcp/authorize_server",
                    AcpPermissionVerdict::Deny,
                    format!(
                        "server is not authorized by project registry {}",
                        root.display()
                    ),
                    Some(candidate.id),
                    None,
                    Vec::new(),
                ),
            }
        }
        self.register_authorized_servers(&authorized);
        authorized
    }

    /// Identify a permission request by the tool its `tool_call` announced
    /// (Vibe). Only for a runtime whose harness sets `_meta.tool_name`.
    pub fn identify_tools_by_harness_name(&self) {
        self.harness_tool_names.store(true, Ordering::Relaxed);
    }

    /// Remember the harness tool name of a `tool_call` update from the bound
    /// session. `_meta.tool_name` is the registered tool Vibe resolved and is
    /// about to run, never model text; the first announcement of an id wins.
    pub fn observe_tool_call_update(&self, params: &Value) {
        if !self.harness_tool_names.load(Ordering::Relaxed)
            || !self.protocol_session_matches(params.get("sessionId").and_then(Value::as_str))
        {
            return;
        }
        let Some(update) = params.get("update") else {
            return;
        };
        if update.get("sessionUpdate").and_then(Value::as_str) != Some("tool_call") {
            return;
        }
        let Some(id) = update
            .get("toolCallId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            return;
        };
        // Every first announcement is kept, a non-MCP one as "": a later frame
        // reusing its id must not turn a shell call into a bridge tool.
        let name = update
            .pointer("/_meta/tool_name")
            .and_then(Value::as_str)
            .filter(|name| name.len() <= 512)
            .filter(|_| {
                update.pointer("/_meta/effect_kind").and_then(Value::as_str) == Some("tool")
            })
            .unwrap_or_default();
        let mut observed = self
            .observed_tools
            .lock()
            .expect("ACP observed-tools mutex poisoned");
        if observed.len() >= MAX_OBSERVED_TOOL_CALLS {
            observed.clear();
        }
        observed
            .entry(id.to_owned())
            .or_insert_with(|| name.to_owned());
    }

    /// Server and tool of a harness-named call. Vibe routes an MCP tool as
    /// `mcp_<server>.<tool>`, both halves normalized: the group must be that of
    /// exactly one authorized server, and a server limited to some tools must
    /// list one that normalizes to `<tool>`.
    fn harness_identity(&self, tool_call: Option<&Value>) -> Option<(String, String)> {
        if !self.harness_tool_names.load(Ordering::Relaxed) {
            return None;
        }
        let id = tool_call?.get("toolCallId")?.as_str()?;
        let name = self
            .observed_tools
            .lock()
            .expect("ACP observed-tools mutex poisoned")
            .remove(id)
            .filter(|name| !name.is_empty())?;
        let authorized = self
            .authorized_tools
            .lock()
            .expect("ACP authorized-tools mutex poisoned");
        let (group, routed_tool) = name.split_once('.')?;
        let group = group.strip_prefix("mcp_")?;
        if routed_tool.is_empty() {
            return None;
        }
        let mut matches = authorized
            .iter()
            .filter(|(server, _)| vibe_identifier(server) == group);
        let (server, tools) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let tool = if tools.is_empty() {
            routed_tool.to_owned()
        } else {
            tools
                .iter()
                .find(|tool| vibe_identifier(tool) == routed_tool)?
                .clone()
        };
        Some((server.clone(), tool))
    }

    pub fn register_trusted_mcp_server(&self, server: &super::AcpMcpServer) {
        self.register_authorized_servers(std::slice::from_ref(server));
        if server.id == "kronn-internal" {
            self.trusted_internal_mcp.store(true, Ordering::Relaxed);
        }
    }

    fn register_authorized_servers(&self, servers: &[super::AcpMcpServer]) {
        let mut authorized = self
            .authorized_tools
            .lock()
            .expect("ACP authorized-tools mutex poisoned");
        for server in servers {
            authorized.insert(
                server.id.clone(),
                server.allowed_tools.iter().cloned().collect(),
            );
        }
    }

    /// Decide a live `session/request_permission` request. Deny-by-default:
    /// only conservative, non-mutating tool-call kinds (`read`, `search`,
    /// `think`, `fetch`) are auto-approved without `full_access`; everything
    /// else (`edit`, `delete`, `move`, `execute`, `other`, or an absent/
    /// unrecognized kind) is refused. Returns the exact ACP v1 result shape:
    /// `{"outcome": {"outcome": "selected", "optionId": ...}}` when the agent
    /// offered a matching option, `{"outcome": {"outcome": "cancelled"}}`
    /// otherwise — never Kronn's own ad hoc shape.
    ///
    /// An MCP call is identified by `rawInput.{server,tool}` (adapters) or, for
    /// a harness-named runtime (Vibe), by the tool its `tool_call` announced.
    /// Either way only the runtime-registered bridge is allowed without
    /// `full_access`. The title is never an identity: it can be model text.
    pub fn decide_tool_call_permission(&self, method: &str, params: &Value) -> Value {
        tracing::trace!(shape = %value_shape(params, 0), "ACP permission request shape");
        let tool_call = params.get("toolCall");
        let kind = tool_call
            .and_then(|tool_call| tool_call.get("kind"))
            .and_then(Value::as_str);
        let safe_kind = matches!(
            kind,
            Some("read") | Some("search") | Some("think") | Some("fetch")
        );
        let request_session = params.get("sessionId").and_then(Value::as_str);
        let session_matches = self.protocol_session_matches(request_session);
        let (mut server, mut tool) = tool_identity(tool_call);
        let mut harness_identified = false;
        if server.is_none() && tool.is_none() {
            if let Some((harness_server, harness_tool)) = self.harness_identity(tool_call) {
                (server, tool) = (Some(harness_server), Some(harness_tool));
                harness_identified = true;
            }
        }
        let tool_scoped = self.tool_identity_is_authorized(server.as_deref(), tool.as_deref());
        let parsed_locations = tool_locations(tool_call);
        let locations = parsed_locations.clone().unwrap_or_default();
        let locations_scoped = parsed_locations
            .as_deref()
            .is_some_and(|locations| self.locations_are_scoped(locations));
        let resource_scoped = if self.scope.is_none() {
            true
        } else if server.is_some() || tool.is_some() {
            tool_scoped
        } else if parsed_locations.is_some() {
            locations_scoped
        } else {
            kind == Some("think")
        };
        // A real secret file (`.env`, a key) is never read, `full_access` or
        // not. A versioned template (`.env.dist`) is not one. Only reads are
        // concerned: what an agent may write is the `full_access` gate's call.
        let secret_target = safe_kind
            && parsed_locations
                .as_deref()
                .is_some_and(|locations| locations.iter().any(|path| location_is_secret(path)));
        // Scope trumps `full_access`: it broadens operations inside the bound
        // project/server only. A missing location and missing server/tool
        // identity is unverifiable and therefore denied.
        let trusted_internal_call = server.as_deref() == Some("kronn-internal")
            && tool_scoped
            && self.trusted_internal_mcp.load(Ordering::Relaxed);
        let allow = session_matches
            && resource_scoped
            && !secret_target
            && (self.full_access || safe_kind || trusted_internal_call);
        let options = params
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let preferred_kinds: &[&str] = if allow {
            &["allow_once", "allow_always"]
        } else {
            &["reject_once", "reject_always"]
        };
        let selected = pick_option(&options, preferred_kinds);
        self.record_context(
            method,
            if allow {
                AcpPermissionVerdict::Allow
            } else {
                AcpPermissionVerdict::Deny
            },
            format!(
                "tool_call kind={}{} full_access={} session_matches={} resource_scoped={} secret_file={} -> {}",
                kind.unwrap_or("unspecified"),
                if harness_identified { " identity=harness" } else { "" },
                self.full_access,
                session_matches,
                resource_scoped,
                secret_target,
                if allow { "allow" } else { "deny" }
            ),
            server,
            tool,
            locations,
        );
        match selected {
            Some(option_id) => json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
            None => json!({"outcome": {"outcome": "cancelled"}}),
        }
    }

    fn protocol_session_matches(&self, received: Option<&str>) -> bool {
        if self.scope.is_none() {
            return true;
        }
        let expected = self
            .protocol_session_id
            .lock()
            .expect("ACP protocol-session mutex poisoned");
        expected
            .as_deref()
            .zip(received)
            .is_some_and(|(expected, received)| expected == received)
    }

    fn tool_identity_is_authorized(&self, server: Option<&str>, tool: Option<&str>) -> bool {
        let (Some(server), Some(tool)) = (server, tool) else {
            return false;
        };
        let authorized = self
            .authorized_tools
            .lock()
            .expect("ACP authorized-tools mutex poisoned");
        authorized.get(server).is_some_and(|tools| {
            !tool.trim().is_empty() && (tools.is_empty() || tools.contains(tool))
        })
    }

    fn locations_are_scoped(&self, locations: &[String]) -> bool {
        let Some(root) = self
            .scope
            .as_ref()
            .and_then(|scope| scope.project_root.as_deref())
        else {
            return false;
        };
        !locations.is_empty()
            && locations
                .iter()
                .all(|path| path_is_within(root, Path::new(path)))
    }

    /// `fs/read_text_file` / `fs/write_text_file` / `terminal/*`: Kronn has
    /// never advertised these capabilities at `initialize` (empty
    /// `clientCapabilities`), so a conformant agent should not call them. A
    /// non-conformant or defensive one gets a spec-correct JSON-RPC error
    /// (code, message) instead of a fabricated "result" object.
    pub fn deny_unbound_capability(&self, method: &str) -> (i64, String) {
        let message = format!(
            "Kronn has not bound a scoped executor for '{method}'; this capability is not granted"
        );
        self.record(
            method,
            AcpPermissionVerdict::Deny,
            "capability not advertised at initialize".to_owned(),
        );
        (ACP_CAPABILITY_NOT_GRANTED, message)
    }

    /// Any other incoming client request Kronn does not implement.
    pub fn deny_unknown_method(&self, method: &str) -> (i64, String) {
        self.record(
            method,
            AcpPermissionVerdict::Deny,
            "method not implemented by Kronn's ACP client".to_owned(),
        );
        (ACP_METHOD_NOT_FOUND, format!("Method not found: {method}"))
    }

    /// Static CLI policy for a Codex/Claude adapter session, computed once
    /// (no live callback exists) and audited exactly like a live decision.
    pub fn session_policy(&self) -> AcpSessionPolicy {
        let policy = AcpSessionPolicy {
            claude_skip_permissions: self.full_access,
            codex_sandbox: codex_sandbox_for(self.full_access, crate::core::env::is_docker()),
        };
        self.record(
            "session/policy",
            if self.full_access {
                AcpPermissionVerdict::Allow
            } else {
                AcpPermissionVerdict::Deny
            },
            format!(
                "full_access={} -> {}",
                self.full_access,
                if self.full_access {
                    "broadened CLI bypass granted"
                } else if policy.codex_sandbox.is_some() {
                    "restrictive runtime default kept; Codex runs unsandboxed inside the container"
                } else {
                    "restrictive runtime default kept (deny-by-default)"
                }
            ),
        );
        policy
    }
}

/// Codex's bwrap sandbox cannot start inside the Kronn container (no
/// unprivileged user namespaces): every command would fail. There the container
/// and the project's mounts are the boundary, as on the direct CLI path.
fn codex_sandbox_for(full_access: bool, in_container: bool) -> Option<&'static str> {
    (full_access || in_container).then_some("danger-full-access")
}

/// A location names a secret file by its own name or, when it exists, through the
/// file it resolves to: a link called `notes.txt` that points at `.env` is one.
fn location_is_secret(path: &str) -> bool {
    let path = Path::new(path);
    is_secret_file(path)
        || std::fs::canonicalize(path)
            .ok()
            .is_some_and(|resolved| is_secret_file(&resolved))
}

/// Filesystem-aware containment check. The project root and the candidate's
/// nearest existing ancestor are canonicalized, which catches symlink escapes
/// while still allowing a not-yet-created file below a real in-project
/// directory. Any ambiguity fails closed.
fn path_is_within(root: &Path, candidate: &Path) -> bool {
    fn normalize(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str()),
            }
        }
        out
    }
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let candidate = normalize(
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        }
        .as_path(),
    );
    let mut ancestor = candidate.as_path();
    while !ancestor.exists() {
        let Some(parent) = ancestor.parent() else {
            return false;
        };
        ancestor = parent;
    }
    let Ok(canonical_ancestor) = ancestor.canonicalize() else {
        return false;
    };
    let Ok(suffix) = candidate.strip_prefix(ancestor) else {
        return false;
    };
    canonical_ancestor.join(suffix).starts_with(root)
}

/// Extract only structured, non-secret correlation fields. The title is
/// intentionally ignored: it is display text controlled by the agent and is
/// not an authorization identity.
fn tool_identity(tool_call: Option<&Value>) -> (Option<String>, Option<String>) {
    let raw = tool_call
        .and_then(|call| call.get("rawInput"))
        .and_then(Value::as_object);
    let pick = |names: &[&str]| {
        raw.and_then(|raw| {
            names.iter().find_map(|name| {
                raw.get(*name)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && value.len() <= 256)
                    .map(str::to_owned)
            })
        })
    };
    (
        pick(&["server", "serverName", "mcpServer"]),
        pick(&["tool", "toolName"]),
    )
}

/// A name as Vibe's runtime normalizes it into a route identifier
/// (`kronn-internal` -> `kronn_internal`). A name it would normalize otherwise
/// matches nothing and stays denied.
fn vibe_identifier(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Field names of an ACP frame with short harness labels, to capture each
/// runtime's request shape. Argument values are never rendered: only `kind`,
/// `status`, `title`, `toolCallId`, `optionId`, `sessionUpdate` and Vibe's
/// `_meta.tool_name`/`effect_kind`, truncated.
pub(crate) fn value_shape(value: &Value, depth: usize) -> String {
    const LABELS: &[&str] = &[
        "kind",
        "status",
        "title",
        "toolCallId",
        "optionId",
        "sessionUpdate",
        "tool_name",
        "effect_kind",
    ];
    match value {
        Value::Object(map) if depth < 4 => {
            let fields: Vec<String> = map
                .iter()
                .map(|(key, value)| match value {
                    Value::String(text) if LABELS.contains(&key.as_str()) => {
                        format!("{key}={:?}", text.chars().take(48).collect::<String>())
                    }
                    // Argument payloads: names only.
                    Value::Object(inner) if key == "rawInput" || key == "arguments" => format!(
                        "{key}{{{}}}",
                        inner.keys().cloned().collect::<Vec<_>>().join(",")
                    ),
                    _ => format!("{key}:{}", value_shape(value, depth + 1)),
                })
                .collect();
            format!("{{{}}}", fields.join(" "))
        }
        Value::Array(items) if depth < 4 => format!(
            "[{}]",
            items
                .iter()
                .take(4)
                .map(|item| value_shape(item, depth + 1))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(_) => "{..}".into(),
        Value::Array(_) => "[..]".into(),
        Value::String(_) => "str".into(),
        Value::Number(_) => "num".into(),
        Value::Bool(_) => "bool".into(),
        Value::Null => "null".into(),
    }
}

/// Parse ACP tool-call locations without silently discarding malformed
/// entries. `None` means absent or unverifiable and therefore fails closed in
/// a scoped session.
fn tool_locations(tool_call: Option<&Value>) -> Option<Vec<String>> {
    let values = tool_call?.get("locations")?.as_array()?;
    if values.is_empty() {
        return None;
    }
    values
        .iter()
        .map(|location| {
            let path = location.get("path")?.as_str()?.trim();
            (!path.is_empty() && path.len() <= 4096).then(|| path.to_owned())
        })
        .collect()
}

fn pick_option(options: &[Value], preferred_kinds: &[&str]) -> Option<String> {
    for kind in preferred_kinds {
        if let Some(option_id) = options
            .iter()
            .find(|option| option.get("kind").and_then(Value::as_str) == Some(*kind))
            .and_then(|option| option.get("optionId"))
            .and_then(Value::as_str)
        {
            return Some(option_id.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission_request(kind: Option<&str>) -> Value {
        let mut tool_call = json!({"toolCallId": "call-1"});
        if let Some(kind) = kind {
            tool_call["kind"] = json!(kind);
        }
        json!({
            "sessionId": "s1",
            "toolCall": tool_call,
            "options": [
                {"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "reject-once", "name": "Reject once", "kind": "reject_once"},
            ]
        })
    }

    #[test]
    fn read_like_kinds_are_allowed_without_full_access() {
        let broker = AcpPermissionBroker::new(false);
        for kind in ["read", "search", "think", "fetch"] {
            let result = broker.decide_tool_call_permission(
                "session/request_permission",
                &permission_request(Some(kind)),
            );
            assert_eq!(
                result,
                json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}}),
                "kind={kind} should be auto-approved"
            );
        }
        assert!(broker
            .audit_log()
            .iter()
            .all(|entry| entry.verdict == AcpPermissionVerdict::Allow));
    }

    #[test]
    fn mutating_kinds_are_denied_by_default() {
        let broker = AcpPermissionBroker::new(false);
        for kind in ["edit", "delete", "move", "execute", "other"] {
            let result = broker.decide_tool_call_permission(
                "session/request_permission",
                &permission_request(Some(kind)),
            );
            assert_eq!(
                result,
                json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}}),
                "kind={kind} should be denied by default"
            );
        }
        assert!(broker
            .audit_log()
            .iter()
            .all(|entry| entry.verdict == AcpPermissionVerdict::Deny));
    }

    #[test]
    fn an_absent_or_unrecognized_kind_is_denied_by_default() {
        let broker = AcpPermissionBroker::new(false);
        let result = broker
            .decide_tool_call_permission("session/request_permission", &permission_request(None));
        assert_eq!(
            result,
            json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
        );
    }

    #[test]
    fn full_access_allows_every_kind() {
        let broker = AcpPermissionBroker::new(true);
        let result = broker.decide_tool_call_permission(
            "session/request_permission",
            &permission_request(Some("execute")),
        );
        assert_eq!(
            result,
            json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
        );
    }

    #[test]
    fn a_denial_without_a_reject_option_offered_is_cancelled_not_fabricated() {
        let broker = AcpPermissionBroker::new(false);
        let params = json!({
            "sessionId": "s1",
            "toolCall": {"toolCallId": "call-1", "kind": "execute"},
            "options": [{"optionId": "allow-once", "name": "Allow once", "kind": "allow_once"}]
        });
        let result = broker.decide_tool_call_permission("session/request_permission", &params);
        assert_eq!(result, json!({"outcome": {"outcome": "cancelled"}}));
    }

    #[test]
    fn fs_and_terminal_requests_get_a_spec_correct_json_rpc_error_never_a_result_object() {
        let broker = AcpPermissionBroker::new(true);
        for method in ["fs/read_text_file", "fs/write_text_file", "terminal/create"] {
            let (code, message) = broker.deny_unbound_capability(method);
            assert_eq!(code, ACP_CAPABILITY_NOT_GRANTED);
            assert!(message.contains(method));
        }
        // full_access never grants fs/terminal — those aren't a "session
        // policy" gate, they're a genuinely unbound capability.
        assert!(broker
            .audit_log()
            .iter()
            .all(|entry| entry.verdict == AcpPermissionVerdict::Deny));
    }

    #[test]
    fn codex_runs_without_its_sandbox_inside_the_container() {
        // Recette 0.14.2 — under Docker, a Codex discussion could not run a
        // single command: "bwrap: No permissions to create a new namespace".
        assert_eq!(codex_sandbox_for(false, true), Some("danger-full-access"));
        assert_eq!(codex_sandbox_for(false, false), None);
        assert_eq!(codex_sandbox_for(true, false), Some("danger-full-access"));
    }

    #[test]
    fn session_policy_keeps_the_runtime_default_unless_full_access_is_set() {
        let restricted = AcpPermissionBroker::new(false).session_policy();
        assert!(!restricted.claude_skip_permissions);
        assert_eq!(
            restricted.codex_sandbox,
            codex_sandbox_for(false, crate::core::env::is_docker())
        );

        let broadened = AcpPermissionBroker::new(true).session_policy();
        assert!(broadened.claude_skip_permissions);
        assert_eq!(broadened.codex_sandbox, Some("danger-full-access"));
    }

    #[test]
    fn every_decision_is_audited_with_a_normalized_reason() {
        let broker = AcpPermissionBroker::new(false);
        broker.decide_tool_call_permission(
            "session/request_permission",
            &permission_request(Some("read")),
        );
        broker.deny_unbound_capability("fs/write_text_file");
        broker.deny_unknown_method("session/weird_future_method");
        let log = broker.audit_log();
        assert_eq!(log.len(), 3);
        assert!(log.iter().all(|entry| !entry.reason.is_empty()));
    }

    #[test]
    fn an_unscoped_broker_stamps_every_audit_entry_unscoped() {
        let broker = AcpPermissionBroker::new(false);
        broker.deny_unknown_method("session/weird_future_method");
        assert_eq!(broker.audit_log()[0].session, "unscoped");
    }

    #[test]
    fn a_scoped_broker_stamps_every_audit_entry_with_its_session_label() {
        let scope = AcpSessionScope::new(None, "disc-abc123");
        let broker = AcpPermissionBroker::scoped(false, scope);
        broker.deny_unknown_method("session/weird_future_method");
        assert_eq!(broker.audit_log()[0].session, "disc-abc123");
    }

    #[test]
    fn authorize_mcp_servers_drops_a_server_outside_the_project_registry_and_audits_it() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers": {"in-scope": {"command": "in-scope-server"}}}"#,
        )
        .unwrap();
        let scope = AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-1");
        let broker = AcpPermissionBroker::scoped(false, scope);

        let candidates = vec![
            crate::acp::AcpMcpServer {
                id: "in-scope".into(),
                command: "in-scope-server".into(),
                args: vec![],
                allowed_tools: vec![],
            },
            crate::acp::AcpMcpServer {
                id: "other-project-server".into(),
                command: "sneaky".into(),
                args: vec![],
                allowed_tools: vec![],
            },
        ];
        let authorized = broker.authorize_mcp_servers(candidates);

        assert_eq!(authorized.len(), 1);
        assert_eq!(authorized[0].id, "in-scope");
        let log = broker.audit_log();
        assert!(log
            .iter()
            .any(|entry| entry.verdict == AcpPermissionVerdict::Deny
                && entry.server.as_deref() == Some("other-project-server")
                && entry.session == "disc-1"));
    }

    #[test]
    fn authorize_mcp_servers_is_a_no_op_for_an_unscoped_broker() {
        let broker = AcpPermissionBroker::new(false);
        let candidates = vec![crate::acp::AcpMcpServer {
            id: "anything".into(),
            command: "anything".into(),
            args: vec![],
            allowed_tools: vec![],
        }];
        assert_eq!(broker.authorize_mcp_servers(candidates.clone()).len(), 1);
        let _ = candidates;
    }

    #[test]
    fn matching_server_id_never_authorizes_a_different_command_or_arguments() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers":{"safe":{"command":"safe-server","args":["serve"]}}}"#,
        )
        .unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-canonical"),
        );
        let authorized = broker.authorize_mcp_servers(vec![crate::acp::AcpMcpServer {
            id: "safe".into(),
            command: "malicious-server".into(),
            args: vec!["--token".into(), "secret".into()],
            allowed_tools: vec![],
        }]);
        assert!(authorized.is_empty());
        assert!(broker.audit_log().iter().any(|entry| {
            entry.verdict == AcpPermissionVerdict::Deny
                && entry.server.as_deref() == Some("safe")
                && entry.reason.contains("differs")
        }));
    }

    #[test]
    fn an_audit_session_is_refused_kronn_internal_and_memory_with_the_filter_rule() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers": {"Memory": {"command": "memory-mcp"}, "Git": {"command": "git-mcp"}}}"#,
        )
        .unwrap();
        let server = |id: &str, command: &str| crate::acp::AcpMcpServer {
            id: id.into(),
            command: command.into(),
            args: vec![],
            allowed_tools: vec![],
        };
        let scoped = |label: &str| {
            AcpPermissionBroker::scoped(
                false,
                AcpSessionScope::new(Some(project.path().to_path_buf()), label),
            )
        };
        let candidates = || vec![server("Memory", "memory-mcp"), server("Git", "git-mcp")];

        // No audit in progress: the registry decides, as before.
        let before = scoped(UNBOUND_SESSION_LABEL);
        assert_eq!(before.authorize_mcp_servers(candidates()).len(), 2);
        assert!(!before.audit_excludes("kronn-internal"));

        let _audit = crate::core::audit_mcp_filter::AuditSessionGuard::enter(project.path());
        let audit = scoped(UNBOUND_SESSION_LABEL);
        let kept = audit.authorize_mcp_servers(candidates());
        assert_eq!(
            kept.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["Git"]
        );
        assert!(audit.audit_excludes("kronn-internal"));
        let refusals: Vec<_> = audit
            .audit_log()
            .into_iter()
            .filter(|e| e.reason.contains("excluded from audits"))
            .filter_map(|e| e.server)
            .collect();
        assert_eq!(refusals, ["Memory", "kronn-internal"]);
        // The same rule as the `.mcp.json` filter, server for server.
        for id in crate::core::audit_mcp_filter::AUDIT_MCP_EXCLUDED {
            assert!(audit.audit_excludes(id), "{id}");
        }
        assert!(!audit.audit_excludes("Git"));

        // A discussion on the audited project keeps Kronn's bridge.
        let room = scoped("disc-1");
        assert!(!room.audit_excludes("kronn-internal"));
        assert_eq!(room.authorize_mcp_servers(candidates()).len(), 2);
    }

    #[test]
    fn a_scoped_call_without_locations_or_an_authorized_tool_identity_is_denied() {
        let project = tempfile::tempdir().unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-no-location"),
        );
        broker.bind_protocol_session("s1").unwrap();
        let result = broker.decide_tool_call_permission(
            "session/request_permission",
            &permission_request(Some("read")),
        );
        assert_eq!(
            result,
            json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
        );
    }

    #[test]
    fn a_scoped_call_requires_the_bound_protocol_session() {
        let project = tempfile::tempdir().unwrap();
        let scope = AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-session");
        let broker = AcpPermissionBroker::scoped(false, scope);
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] =
            json!([{"path": project.path().join("README.md").to_string_lossy()}]);
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request),
            json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
        );
        broker.bind_protocol_session("another-session").unwrap();
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request),
            json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
        );
    }

    #[test]
    fn mcp_permission_is_scoped_to_the_exact_registered_server_and_tool() {
        let project = tempfile::tempdir().unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-tool"),
        );
        broker.bind_protocol_session("s1").unwrap();
        broker.register_trusted_mcp_server(&crate::acp::AcpMcpServer {
            id: "kronn-internal".into(),
            command: "python3".into(),
            args: vec![],
            allowed_tools: vec!["disc_get".into()],
        });
        let request = |server: &str, tool: &str| {
            let mut request = permission_request(Some("read"));
            request["toolCall"]["rawInput"] = json!({"server": server, "tool": tool});
            request
        };
        assert_eq!(
            broker.decide_tool_call_permission(
                "session/request_permission",
                &request("kronn-internal", "disc_get"),
            ),
            json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
        );
        for request in [
            request("another-server", "disc_get"),
            request("kronn-internal", "disc_delete"),
        ] {
            assert_eq!(
                broker.decide_tool_call_permission("session/request_permission", &request),
                json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
            );
        }
        let allowed = &broker.audit_log()[0];
        assert_eq!(allowed.protocol_session_id.as_deref(), Some("s1"));
        assert_eq!(allowed.server.as_deref(), Some("kronn-internal"));
        assert_eq!(allowed.tool.as_deref(), Some("disc_get"));
        assert!(!allowed.reason.contains("rawInput"));
    }

    #[test]
    fn only_the_runtime_registered_kronn_bridge_can_write_without_full_access() {
        let broker = AcpPermissionBroker::scoped(false, AcpSessionScope::new(None, "room"));
        broker.bind_protocol_session("s1").unwrap();
        let server = crate::acp::AcpMcpServer {
            id: "kronn-internal".into(),
            command: "owned-bridge".into(),
            args: vec![],
            allowed_tools: vec!["disc_append".into()],
        };
        let mut request = permission_request(Some("other"));
        request["toolCall"]["rawInput"] = json!({"server":"kronn-internal", "tool":"disc_append"});
        broker.register_authorized_servers(std::slice::from_ref(&server));
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request)["outcome"]
                ["optionId"],
            "reject-once",
            "a project declaration does not confer runtime trust"
        );
        broker.register_trusted_mcp_server(&server);
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request)["outcome"]
                ["optionId"],
            "allow-once"
        );
        for (session, server, tool) in [
            ("other", "kronn-internal", "disc_append"),
            ("s1", "another-server", "disc_append"),
            ("s1", "kronn-internal", "disc_delete"),
        ] {
            request["sessionId"] = json!(session);
            request["toolCall"]["rawInput"] = json!({"server":server, "tool":tool});
            assert_eq!(
                broker.decide_tool_call_permission("session/request_permission", &request)
                    ["outcome"]["optionId"],
                "reject-once"
            );
        }
    }

    #[test]
    fn any_safe_kind_targeting_a_location_outside_the_project_is_denied_even_without_full_access_or_with_it(
    ) {
        let project = tempfile::tempdir().unwrap();
        let scope = AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-2");
        for full_access in [false, true] {
            let broker = AcpPermissionBroker::scoped(full_access, scope.clone());
            broker.bind_protocol_session("s1").unwrap();
            for kind in ["read", "search", "think", "fetch"] {
                let mut request = permission_request(Some(kind));
                request["toolCall"]["locations"] = json!([{"path": "/etc/passwd"}]);
                let result =
                    broker.decide_tool_call_permission("session/request_permission", &request);
                assert_eq!(
                    result,
                    json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}}),
                    "kind={kind} full_access={full_access} outside the project must be denied"
                );
            }
        }
    }

    #[test]
    fn a_read_targeting_a_location_inside_the_project_is_still_allowed() {
        let project = tempfile::tempdir().unwrap();
        let scope = AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-3");
        let broker = AcpPermissionBroker::scoped(false, scope);
        broker.bind_protocol_session("s1").unwrap();
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] =
            json!([{"path": project.path().join("src/main.rs").to_string_lossy()}]);
        let result = broker.decide_tool_call_permission("session/request_permission", &request);
        assert_eq!(
            result,
            json!({"outcome": {"outcome": "selected", "optionId": "allow-once"}})
        );
    }

    #[test]
    fn a_relative_location_that_climbs_out_of_the_project_via_dotdot_is_denied() {
        let project = tempfile::tempdir().unwrap();
        let scope = AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-4");
        let broker = AcpPermissionBroker::scoped(false, scope);
        broker.bind_protocol_session("s1").unwrap();
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] = json!([{"path": "../../etc/passwd"}]);
        let result = broker.decide_tool_call_permission("session/request_permission", &request);
        assert_eq!(
            result,
            json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_location_reached_through_an_in_project_symlink_to_outside_is_denied() {
        use std::os::unix::fs::symlink;

        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), project.path().join("escape")).unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-symlink"),
        );
        broker.bind_protocol_session("s1").unwrap();
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] = json!([{
            "path": project.path().join("escape/new-file.txt").to_string_lossy()
        }]);
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request),
            json!({"outcome": {"outcome": "selected", "optionId": "reject-once"}})
        );
    }

    /// What the broker answers to a read of `relative` inside a scoped project.
    fn read_outcome(full_access: bool, relative: &str) -> (Value, AcpAuditEntry) {
        let project = tempfile::tempdir().unwrap();
        let broker = AcpPermissionBroker::scoped(
            full_access,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-env"),
        );
        broker.bind_protocol_session("s1").unwrap();
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] =
            json!([{"path": project.path().join(relative).to_string_lossy()}]);
        let outcome = broker.decide_tool_call_permission("session/request_permission", &request);
        let entry = broker.audit_log().pop().unwrap();
        (outcome, entry)
    }

    fn selected(option: &str) -> Value {
        json!({"outcome": {"outcome": "selected", "optionId": option}})
    }

    #[test]
    fn a_versioned_environment_template_is_readable_by_an_agent() {
        for template in [".env.dist", ".env.example", ".env.sample", ".env.template"] {
            for full_access in [false, true] {
                let (outcome, entry) = read_outcome(full_access, template);
                assert_eq!(outcome, selected("allow-once"), "{template}");
                assert_eq!(entry.verdict, AcpPermissionVerdict::Allow, "{template}");
                assert!(
                    entry.reason.contains("secret_file=false"),
                    "{}",
                    entry.reason
                );
            }
        }
    }

    #[test]
    fn a_real_secret_file_stays_refused_and_the_refusal_is_an_answer_not_a_failure() {
        for secret in [
            ".env",
            ".env.local",
            "config/.env.production",
            "tls/server.key",
        ] {
            // `full_access` widens what an agent may do, never what it may read
            // of a secret.
            for full_access in [false, true] {
                let (outcome, entry) = read_outcome(full_access, secret);
                assert_eq!(
                    outcome,
                    selected("reject-once"),
                    "{secret}: a refusal is a selected reject option the agent reads as \
                     the tool's answer, not a cancelled turn"
                );
                assert_eq!(entry.verdict, AcpPermissionVerdict::Deny, "{secret}");
                assert!(
                    entry.reason.contains("secret_file=true"),
                    "{}",
                    entry.reason
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_real_secret_file_is_refused_under_its_innocent_name() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(project.path().join(".env"), "TOKEN=x\n").unwrap();
        std::os::unix::fs::symlink(
            project.path().join(".env"),
            project.path().join("notes.txt"),
        )
        .unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-link"),
        );
        broker.bind_protocol_session("s1").unwrap();
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] =
            json!([{"path": project.path().join("notes.txt").to_string_lossy()}]);
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request),
            selected("reject-once")
        );
    }

    #[test]
    fn a_read_that_names_no_path_is_refused_with_an_answer_never_left_unanswered() {
        // OpenCode asks about a `read` of an environment file with empty
        // `locations`: the broker cannot tell `.env` from `.env.dist`, so it
        // fails closed. The refusal must still come back as a selected reject
        // option, which is what lets the agent carry on.
        let project = tempfile::tempdir().unwrap();
        let broker = AcpPermissionBroker::scoped(
            false,
            AcpSessionScope::new(Some(project.path().to_path_buf()), "disc-blind"),
        );
        broker.bind_protocol_session("s1").unwrap();
        let mut request = permission_request(Some("read"));
        request["toolCall"]["locations"] = json!([]);
        assert_eq!(
            broker.decide_tool_call_permission("session/request_permission", &request),
            selected("reject-once")
        );
    }

    // ─── Native runtimes' real request shapes (captured live, 2026-10-06) ───

    fn bridge(allowed_tools: &[&str]) -> crate::acp::AcpMcpServer {
        crate::acp::AcpMcpServer {
            id: "kronn-internal".into(),
            command: "python3".into(),
            args: vec![],
            allowed_tools: allowed_tools.iter().map(|tool| tool.to_string()).collect(),
        }
    }

    fn vibe_broker(full_access: bool, root: Option<PathBuf>) -> AcpPermissionBroker {
        let broker =
            AcpPermissionBroker::scoped(full_access, AcpSessionScope::new(root, "disc-vibe"));
        broker.bind_protocol_session("s-vibe").unwrap();
        broker.identify_tools_by_harness_name();
        broker
    }

    /// Mistral Vibe 2.25.8: the `tool_call` update that precedes a permission request.
    fn vibe_tool_call(
        id: &str,
        effect_kind: &str,
        tool_name: &str,
        kind: &str,
        title: &str,
    ) -> Value {
        json!({
            "sessionId": "s-vibe",
            "update": {
                "_meta": {"effect_kind": effect_kind, "tool_name": tool_name},
                "kind": kind,
                "rawInput": {},
                "sessionUpdate": "tool_call",
                "status": "in_progress",
                "title": title,
                "toolCallId": id,
            }
        })
    }

    /// Mistral Vibe 2.25.8: its permission request names nothing but the call id.
    fn vibe_permission_request(id: &str) -> Value {
        json!({
            "sessionId": "s-vibe",
            "toolCall": {"toolCallId": id},
            "options": [
                {"optionId": "allow_once", "name": "Allow once", "kind": "allow_once"},
                {"optionId": "allow_always", "name": "Allow always", "kind": "allow_always"},
                {"optionId": "allow_always_permanent", "name": "Always", "kind": "allow_always"},
                {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
            ]
        })
    }

    fn vibe_decision(broker: &AcpPermissionBroker, update: &Value, id: &str) -> Value {
        broker.observe_tool_call_update(update);
        broker
            .decide_tool_call_permission("session/request_permission", &vibe_permission_request(id))
            ["outcome"]["optionId"]
            .clone()
    }

    #[test]
    fn vibe_calls_the_kronn_bridge_without_full_access() {
        let broker = vibe_broker(false, None);
        broker.register_trusted_mcp_server(&bridge(&[]));
        for (id, tool) in [("effect-1", "bridge_info"), ("effect-2", "disc_meta")] {
            let name = format!("mcp_kronn_internal.{tool}");
            assert_eq!(
                vibe_decision(
                    &broker,
                    &vibe_tool_call(id, "tool", &name, "other", &name),
                    id
                ),
                "allow_once",
                "{tool}"
            );
        }
        let allowed = broker.audit_log();
        assert_eq!(allowed[1].server.as_deref(), Some("kronn-internal"));
        assert_eq!(allowed[1].tool.as_deref(), Some("disc_meta"));
        assert!(
            allowed[1].reason.contains("identity=harness"),
            "{}",
            allowed[1].reason
        );
    }

    #[test]
    fn vibe_shell_unknown_and_unauthorized_calls_stay_denied() {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join(".mcp.json"),
            r#"{"mcpServers": {"github-tools": {"command": "gh-mcp"}}}"#,
        )
        .unwrap();
        let broker = vibe_broker(false, Some(project.path().to_path_buf()));
        broker.register_trusted_mcp_server(&bridge(&[]));
        broker.authorize_mcp_servers(vec![crate::acp::AcpMcpServer {
            id: "github-tools".into(),
            command: "gh-mcp".into(),
            args: vec![],
            allowed_tools: vec![],
        }]);
        for (id, effect_kind, tool_name, kind, title) in [
            // A shell command, even one titled like the bridge's tool.
            (
                "e-shell",
                "shell",
                "bash",
                "execute",
                "bash: env | cut -d= -f1",
            ),
            (
                "e-forged",
                "shell",
                "bash",
                "execute",
                "mcp_kronn_internal.bridge_info",
            ),
            // A server this session never declared.
            (
                "e-unknown",
                "tool",
                "mcp_other_server.do_it",
                "other",
                "mcp_other_server.do_it",
            ),
            // A project server: authorized, but a declaration confers no trust.
            (
                "e-project",
                "tool",
                "mcp_github_tools.create_issue",
                "other",
                "mcp_github_tools.create_issue",
            ),
            // Not an MCP route at all.
            (
                "e-builtin",
                "tool",
                "write_file",
                "edit",
                "Writing notes.txt",
            ),
        ] {
            assert_eq!(
                vibe_decision(
                    &broker,
                    &vibe_tool_call(id, effect_kind, tool_name, kind, title),
                    id
                ),
                "reject_once",
                "{id}"
            );
        }
    }

    #[test]
    fn vibe_identity_needs_this_session_s_own_announcement() {
        let broker = vibe_broker(false, None);
        broker.register_trusted_mcp_server(&bridge(&[]));
        let name = "mcp_kronn_internal.disc_meta";
        // Announced by another session.
        let mut update = vibe_tool_call("e-1", "tool", name, "other", name);
        update["sessionId"] = json!("another-session");
        assert_eq!(vibe_decision(&broker, &update, "e-1"), "reject_once");
        // Never announced.
        assert_eq!(
            broker.decide_tool_call_permission(
                "session/request_permission",
                &vibe_permission_request("e-unannounced")
            )["outcome"]["optionId"],
            "reject_once"
        );
        // Announced once, consumed by the first request.
        let update = vibe_tool_call("e-2", "tool", name, "other", name);
        assert_eq!(vibe_decision(&broker, &update, "e-2"), "allow_once");
        assert_eq!(
            broker.decide_tool_call_permission(
                "session/request_permission",
                &vibe_permission_request("e-2")
            )["outcome"]["optionId"],
            "reject_once"
        );
        // A later update cannot rename an announced call.
        broker.observe_tool_call_update(&vibe_tool_call(
            "e-3", "shell", "bash", "execute", "bash: ls",
        ));
        broker.observe_tool_call_update(&vibe_tool_call("e-3", "tool", name, "other", name));
        assert_eq!(
            broker.decide_tool_call_permission(
                "session/request_permission",
                &vibe_permission_request("e-3")
            )["outcome"]["optionId"],
            "reject_once"
        );
    }

    #[test]
    fn vibe_identity_honours_a_step_s_tool_list_and_refuses_ambiguity() {
        let broker = vibe_broker(false, None);
        broker.register_trusted_mcp_server(&bridge(&["disc_meta"]));
        let call = |id: &str, tool: &str| {
            let name = format!("mcp_kronn_internal.{tool}");
            vibe_tool_call(id, "tool", &name, "other", &name)
        };
        assert_eq!(
            vibe_decision(&broker, &call("e-1", "disc_meta"), "e-1"),
            "allow_once"
        );
        assert_eq!(
            vibe_decision(&broker, &call("e-2", "disc_append"), "e-2"),
            "reject_once"
        );

        // Two authorized servers that normalize to one route group.
        let ambiguous = vibe_broker(true, None);
        ambiguous.register_authorized_servers(&[
            crate::acp::AcpMcpServer {
                id: "a-b".into(),
                ..bridge(&[])
            },
            crate::acp::AcpMcpServer {
                id: "a_b".into(),
                ..bridge(&[])
            },
        ]);
        let update = vibe_tool_call("e-3", "tool", "mcp_a_b.t", "other", "mcp_a_b.t");
        assert_eq!(vibe_decision(&ambiguous, &update, "e-3"), "reject_once");
    }

    #[test]
    fn a_runtime_without_harness_names_ignores_vibe_style_announcements() {
        let broker = AcpPermissionBroker::scoped(false, AcpSessionScope::new(None, "disc-x"));
        broker.bind_protocol_session("s-vibe").unwrap();
        broker.register_trusted_mcp_server(&bridge(&[]));
        let name = "mcp_kronn_internal.bridge_info";
        assert_eq!(
            vibe_decision(
                &broker,
                &vibe_tool_call("e-1", "tool", name, "other", name),
                "e-1"
            ),
            "reject_once"
        );
    }

    /// GitHub Copilot CLI 1.0.92: the request names the tool, never its server,
    /// so the broker cannot authorize it; Kronn's bridge is granted at launch
    /// (`acp::native_mcp_launch_grants`) and a shell call stays refused.
    #[test]
    fn copilot_requests_stay_denied_by_the_broker() {
        let broker = AcpPermissionBroker::scoped(false, AcpSessionScope::new(None, "disc-cop"));
        broker.bind_protocol_session("s-cop").unwrap();
        broker.register_trusted_mcp_server(&bridge(&[]));
        let options = json!([
            {"optionId": "allow_once", "name": "Allow", "kind": "allow_once"},
            {"optionId": "allow_always", "name": "Always", "kind": "allow_always"},
            {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
        ]);
        for tool_call in [
            json!({"kind": "other", "rawInput": {}, "status": "pending",
                   "title": "bridge_info", "toolCallId": "call_1"}),
            json!({"kind": "execute", "rawInput": {"command": "env", "commands": ["env"]},
                   "status": "pending", "title": "List environment variable names",
                   "toolCallId": "call_2"}),
        ] {
            let request = json!({"sessionId": "s-cop", "toolCall": tool_call, "options": options});
            assert_eq!(
                broker.decide_tool_call_permission("session/request_permission", &request)
                    ["outcome"]["optionId"],
                "reject_once"
            );
        }
    }

    #[test]
    fn vibe_identifiers_follow_the_runtime_normalization() {
        assert_eq!(vibe_identifier("kronn-internal"), "kronn_internal");
        assert_eq!(vibe_identifier("GitHub.Tools"), "github_tools");
    }
}
