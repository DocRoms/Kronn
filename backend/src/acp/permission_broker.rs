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

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
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
    audit_log: Mutex<Vec<AcpAuditEntry>>,
}

impl AcpPermissionBroker {
    pub fn new(full_access: bool) -> Self {
        Self {
            full_access,
            scope: None,
            protocol_session_id: Mutex::new(None),
            authorized_tools: Mutex::new(BTreeMap::new()),
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

    /// Reconstruct candidates from this session's canonical project registry.
    /// Matching an id is insufficient: a caller could attach another command
    /// or arguments to an authorized name. Only an exact candidate match is
    /// retained, and the returned value is the independently reconstructed
    /// canonical declaration rather than caller-owned data.
    pub fn authorize_mcp_servers(
        &self,
        candidates: Vec<super::AcpMcpServer>,
    ) -> Vec<super::AcpMcpServer> {
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
                            (!command.trim().is_empty()
                                && !crate::core::mcp_scanner::mcp_entry_leaks_secret(&entry))
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

    pub fn register_trusted_mcp_server(&self, server: &super::AcpMcpServer) {
        self.register_authorized_servers(std::slice::from_ref(server));
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
    pub fn decide_tool_call_permission(&self, method: &str, params: &Value) -> Value {
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
        let (server, tool) = tool_identity(tool_call);
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
        // Scope trumps `full_access`: it broadens operations inside the bound
        // project/server only. A missing location and missing server/tool
        // identity is unverifiable and therefore denied.
        let allow = session_matches && resource_scoped && (self.full_access || safe_kind);
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
                "tool_call kind={} full_access={} session_matches={} resource_scoped={} -> {}",
                kind.unwrap_or("unspecified"),
                self.full_access,
                session_matches,
                resource_scoped,
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
            codex_sandbox: self.full_access.then_some("danger-full-access"),
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
                } else {
                    "restrictive runtime default kept (deny-by-default)"
                }
            ),
        );
        policy
    }
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
    fn session_policy_keeps_the_runtime_default_unless_full_access_is_set() {
        let restricted = AcpPermissionBroker::new(false).session_policy();
        assert!(!restricted.claude_skip_permissions);
        assert_eq!(restricted.codex_sandbox, None);

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
}
