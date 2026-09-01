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
    audit_log: Mutex<Vec<AcpAuditEntry>>,
}

impl AcpPermissionBroker {
    pub fn new(full_access: bool) -> Self {
        Self {
            full_access,
            audit_log: Mutex::new(Vec::new()),
        }
    }

    pub fn full_access(&self) -> bool {
        self.full_access
    }

    /// Full audit trail recorded so far, in decision order.
    pub fn audit_log(&self) -> Vec<AcpAuditEntry> {
        self.audit_log
            .lock()
            .expect("ACP permission broker audit log mutex poisoned")
            .clone()
    }

    fn record(&self, method: &str, verdict: AcpPermissionVerdict, reason: String) {
        tracing::info!(
            method,
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
            });
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
        let kind = params
            .get("toolCall")
            .and_then(|tool_call| tool_call.get("kind"))
            .and_then(Value::as_str);
        let safe_kind = matches!(
            kind,
            Some("read") | Some("search") | Some("think") | Some("fetch")
        );
        let allow = self.full_access || safe_kind;
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
        self.record(
            method,
            if allow {
                AcpPermissionVerdict::Allow
            } else {
                AcpPermissionVerdict::Deny
            },
            format!(
                "tool_call kind={} full_access={} -> {}",
                kind.unwrap_or("unspecified"),
                self.full_access,
                if allow { "allow" } else { "deny" }
            ),
        );
        match selected {
            Some(option_id) => json!({"outcome": {"outcome": "selected", "optionId": option_id}}),
            None => json!({"outcome": {"outcome": "cancelled"}}),
        }
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
}
