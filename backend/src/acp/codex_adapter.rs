//! Adapts Codex CLI's own non-interactive protocol to `AcpTransport`.
//!
//! Codex has no vendor ACP subcommand either (verified: `codex --help` on the
//! installed CLI, codex-cli 0.151.0, lists no `acp`/`--acp`; `codex app-server`
//! exists but is explicitly `[experimental]`, daemon/proxy-shaped rather than
//! a simple spawn-and-speak-stdio process, and not a "vendor-documented"
//! command in the sense `native_acp_command` requires — so this adapter drives
//! the stable, documented `codex exec` surface instead:
//! - `codex exec --json <prompt>` starts a new thread and streams `ThreadEvent`
//!   JSONL to stdout; the very first event is `{"type":"thread.started",
//!   "thread_id":"..."}` (verified against the upstream `codex-rs/exec/src/
//!   exec_events.rs` source, since no live API access was available to
//!   observe it directly).
//! - `codex exec resume <thread_id> --json <prompt>` continues that thread —
//!   but `--sandbox` is NOT accepted on `resume` (verified via `codex exec
//!   resume --help`, unlike the top-level `codex exec`), so the sandbox
//!   policy only applies to a session's first turn.
//!
//! Codex cannot hand out a session id before the first turn runs (unlike
//! Claude's `--session-id`), so `create_session` allocates Kronn's own
//! correlation id as the opaque `AcpSessionTarget`, and the real Codex
//! `thread_id` is tracked separately, exposed via `native_session_id` once
//! known so the caller can persist it for a cross-restart resume.
//!
//! `codex exec`/`codex exec resume` have no live permission callback either
//! (only static `--sandbox`/`--dangerously-bypass-approvals-and-sandbox`
//! flags), so — exactly like the Claude adapter — permission policy is
//! computed once per session by [`AcpPermissionBroker::session_policy`].
//! MCP servers are not inlined into any Kronn-controlled payload: Codex reads
//! its own already-synced `~/.codex/config.toml` (project-authorized servers,
//! real credentials, written by the existing MCP sync); the adapter only
//! narrows the `kronn-internal` server's forwarded env var *names* — never
//! values — exactly like today's direct-CLI Codex invocation.

use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::{mpsc, Mutex};

use super::permission_broker::{AcpAuditEntry, AcpPermissionBroker};
use super::{
    AcpAgent, AcpCapability, AcpConfigOption, AcpError, AcpInitialize, AcpNegotiatedCapabilities,
    AcpSessionEvent, AcpSessionTarget, AcpTransport,
};

pub struct CodexAcpAdapter {
    program: String,
    cwd: Mutex<Option<PathBuf>>,
    model: Option<String>,
    broker: AcpPermissionBroker,
    /// The real Codex `thread_id`, once known. Seeded at construction to
    /// resume a thread from a previous Kronn process; otherwise populated
    /// from the first turn's `thread.started` event.
    thread_id: Mutex<Option<String>>,
    current_child: Mutex<Option<Child>>,
}

impl CodexAcpAdapter {
    pub fn new(
        model: Option<String>,
        full_access: bool,
        seed_native_thread_id: Option<String>,
    ) -> Self {
        Self {
            program: "codex".to_owned(),
            cwd: Mutex::new(None),
            model,
            broker: AcpPermissionBroker::new(full_access),
            thread_id: Mutex::new(seed_native_thread_id),
            current_child: Mutex::new(None),
        }
    }

    /// Test-only: drive a fixture script instead of the real `codex` binary.
    #[cfg(test)]
    fn new_with_program(
        program: impl Into<String>,
        model: Option<String>,
        full_access: bool,
        seed_native_thread_id: Option<String>,
    ) -> Self {
        Self {
            program: program.into(),
            ..Self::new(model, full_access, seed_native_thread_id)
        }
    }

    pub fn permission_audit_log(&self) -> Vec<AcpAuditEntry> {
        self.broker.audit_log()
    }
}

/// One line of `codex exec --json` JSONL output, reduced to what the adapter
/// forwards. Mirrors the upstream `ThreadEvent`/`ThreadItemDetails` shapes
/// (`type` tag, `thread_id`, `item.type`, `item.text`, `usage.*_tokens`)
/// without depending on the `codex-protocol` crate.
enum CodexLineEvent {
    ThreadStarted(Option<String>),
    Text(String),
    ToolCall(String),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Fatal(String),
    Skip,
}

fn parse_codex_line(line: &str) -> CodexLineEvent {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return CodexLineEvent::Skip;
    }
    let Ok(json) = serde_json::from_str::<Value>(trimmed) else {
        // Non-JSON noise (should not happen under --json) — never surfaced
        // as model text.
        return CodexLineEvent::Skip;
    };
    match json.get("type").and_then(Value::as_str).unwrap_or("") {
        "thread.started" => CodexLineEvent::ThreadStarted(
            json.get("thread_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
        "item.completed" => match json
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
        {
            Some("agent_message") => {
                let text = json
                    .pointer("/item/text")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if text.is_empty() {
                    CodexLineEvent::Skip
                } else {
                    CodexLineEvent::Text(text.to_owned())
                }
            }
            Some(
                kind @ ("command_execution" | "file_change" | "mcp_tool_call" | "collab_tool_call"
                | "web_search"),
            ) => CodexLineEvent::ToolCall(kind.to_owned()),
            _ => CodexLineEvent::Skip,
        },
        "turn.completed" => {
            let input_tokens = json
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output_tokens = json
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            CodexLineEvent::Usage {
                input_tokens,
                output_tokens,
            }
        }
        "turn.failed" => CodexLineEvent::Fatal(
            json.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("Codex turn failed")
                .to_owned(),
        ),
        "error" => CodexLineEvent::Fatal(
            json.get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex reported a fatal error")
                .to_owned(),
        ),
        // item.started / item.updated / turn.started / unrecognized: no
        // user-visible content to forward yet.
        _ => CodexLineEvent::Skip,
    }
}

#[async_trait]
impl AcpTransport for CodexAcpAdapter {
    async fn initialize(
        &self,
        request: AcpInitialize,
    ) -> Result<AcpNegotiatedCapabilities, AcpError> {
        *self.cwd.lock().await = Some(PathBuf::from(&request.cwd));
        let capabilities = [
            AcpCapability::Sessions,
            AcpCapability::Streaming,
            AcpCapability::Cancellation,
            AcpCapability::Resume,
            AcpCapability::McpInjection,
        ]
        .into_iter()
        .collect();
        Ok(AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities,
        })
    }

    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        // Codex only assigns a real thread id after the first turn runs (no
        // "create an empty thread" mode exists). Kronn's own correlation id
        // stands in as the opaque ACP target.
        AcpSessionTarget::new(AcpAgent::Codex, uuid::Uuid::new_v4().to_string())
    }

    async fn config_options(&self) -> Vec<AcpConfigOption> {
        Vec::new()
    }

    async fn set_config_option(
        &self,
        _target: &AcpSessionTarget,
        _config_id: &str,
        _value_id: &str,
    ) -> Result<(), AcpError> {
        Ok(())
    }

    async fn resume_session(&self, _target: &AcpSessionTarget) -> Result<(), AcpError> {
        if self.thread_id.lock().await.is_some() {
            Ok(())
        } else {
            Err(AcpError::Transport(
                "no Codex thread known to resume".into(),
            ))
        }
    }

    async fn prompt(
        &self,
        _target: &AcpSessionTarget,
        prompt: &str,
        events: mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        let cwd = self.cwd.lock().await.clone().ok_or_else(|| {
            AcpError::Transport("Codex ACP adapter prompted before initialize".into())
        })?;
        let known_thread = self.thread_id.lock().await.clone();

        let mut args: Vec<String> = vec!["exec".into()];
        if let Some(thread) = &known_thread {
            args.push("resume".into());
            args.push(thread.clone());
        }
        args.push("--json".into());
        args.push("--skip-git-repo-check".into());
        args.push("-c".into());
        args.push(crate::agents::runner::codex_kronn_internal_env_override());
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        // `--sandbox` is not accepted by `codex exec resume` (verified via
        // `codex exec resume --help`): only the first turn of a thread can
        // set it.
        if known_thread.is_none() {
            if let Some(sandbox) = self.broker.session_policy().codex_sandbox {
                args.push(format!("--sandbox={sandbox}"));
            }
        }
        args.push(prompt.to_owned());

        let mut command = crate::core::cmd::async_cmd(&self.program);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| AcpError::Transport(format!("spawn codex: {error}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::Transport("codex stdout unavailable".into()))?;
        *self.current_child.lock().await = Some(child);

        let mut lines = BufReader::new(stdout).lines();
        let mut fatal: Option<String> = None;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match parse_codex_line(&line) {
                    CodexLineEvent::ThreadStarted(Some(thread)) => {
                        *self.thread_id.lock().await = Some(thread);
                    }
                    CodexLineEvent::ThreadStarted(None) => {}
                    CodexLineEvent::Text(text) => {
                        let _ = events.send(AcpSessionEvent::TextDelta(text)).await;
                    }
                    CodexLineEvent::ToolCall(name) => {
                        let _ = events.send(AcpSessionEvent::ToolCall { name }).await;
                    }
                    CodexLineEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        let _ = events
                            .send(AcpSessionEvent::Usage {
                                input_tokens,
                                output_tokens,
                            })
                            .await;
                    }
                    CodexLineEvent::Fatal(message) => fatal = Some(message),
                    CodexLineEvent::Skip => {}
                },
                Ok(None) => break,
                Err(error) => {
                    return Err(AcpError::Transport(format!("read codex stdout: {error}")));
                }
            }
        }

        let status = {
            let mut guard = self.current_child.lock().await;
            match guard.as_mut() {
                Some(child) => child
                    .wait()
                    .await
                    .map_err(|error| AcpError::Transport(format!("wait for codex: {error}")))?,
                None => return Err(AcpError::Transport("codex turn was cancelled".into())),
            }
        };
        *self.current_child.lock().await = None;

        if let Some(message) = fatal {
            return Err(AcpError::Transport(message));
        }
        if !status.success() {
            return Err(AcpError::Transport(format!(
                "codex exited with status {status}"
            )));
        }
        let _ = events.send(AcpSessionEvent::Completed).await;
        Ok(())
    }

    async fn cancel(&self, _target: &AcpSessionTarget) -> Result<(), AcpError> {
        if let Some(mut child) = self.current_child.lock().await.take() {
            let _ = child.start_kill();
            tokio::spawn(async move {
                let _ = child.wait().await;
            });
        }
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), AcpError> {
        if let Some(mut child) = self.current_child.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        Ok(())
    }

    async fn native_session_id(&self, _target: &AcpSessionTarget) -> Option<String> {
        self.thread_id.lock().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::AcpHost;

    fn init_request(cwd: &str) -> AcpInitialize {
        AcpInitialize {
            protocol_version: 1,
            cwd: cwd.into(),
            mcp_servers: vec![],
        }
    }

    async fn drain(mut rx: mpsc::Receiver<AcpSessionEvent>) -> Vec<AcpSessionEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[test]
    fn parses_the_verified_openai_codex_exec_events_shape() {
        assert!(matches!(
            parse_codex_line(r#"{"type":"thread.started","thread_id":"th-1"}"#),
            CodexLineEvent::ThreadStarted(Some(id)) if id == "th-1"
        ));
        assert!(matches!(
            parse_codex_line(
                r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"hi"}}"#
            ),
            CodexLineEvent::Text(text) if text == "hi"
        ));
        assert!(matches!(
            parse_codex_line(
                r#"{"type":"item.completed","item":{"id":"i2","type":"command_execution","command":"ls","aggregated_output":"","exit_code":0,"status":"completed"}}"#
            ),
            CodexLineEvent::ToolCall(kind) if kind == "command_execution"
        ));
        assert!(matches!(
            parse_codex_line(
                r#"{"type":"turn.completed","usage":{"input_tokens":3,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}"#
            ),
            CodexLineEvent::Usage {
                input_tokens: 3,
                output_tokens: 5
            }
        ));
        assert!(matches!(
            parse_codex_line(r#"{"type":"turn.failed","error":{"message":"boom"}}"#),
            CodexLineEvent::Fatal(message) if message == "boom"
        ));
        assert!(matches!(
            parse_codex_line(r#"{"type":"turn.started"}"#),
            CodexLineEvent::Skip
        ));
    }

    /// A fixture "codex" that always reports `thread.started` with a fixed
    /// id derived from whether `resume` was passed, so the test can verify
    /// which invocation shape actually ran. Every other flag the adapter
    /// passes (`--json`, `--skip-git-repo-check`, `-c ...`, `--sandbox=...`)
    /// is simply ignored, the way a real shell script would.
    const FIXTURE_BODY: &str = r#"
        case "$*" in
          *resume*)
            printf '%s\n' '{"type":"thread.started","thread_id":"th-resumed"}'
            printf '%s\n' '{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"resumed"}}'
            ;;
          *)
            printf '%s\n' '{"type":"thread.started","thread_id":"th-first"}'
            printf '%s\n' '{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"created"}}'
            ;;
        esac
        printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":2}}'
        "#;

    #[tokio::test]
    async fn create_session_does_not_spawn_a_process_first_prompt_captures_the_thread_id() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), FIXTURE_BODY);
        let adapter = std::sync::Arc::new(CodexAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
            None,
        ));
        let mut host = AcpHost::new(1, adapter.clone());
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();
        assert_eq!(target.agent, AcpAgent::Codex);
        assert!(adapter.native_session_id(&target).await.is_none());

        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx)
            .await
            .unwrap_or_else(|error| panic!("first prompt failed: {error}"));
        let events = drain(rx).await;
        assert!(events.contains(&AcpSessionEvent::TextDelta("created".into())));
        assert_eq!(
            adapter.native_session_id(&target).await,
            Some("th-first".to_owned())
        );
    }

    #[tokio::test]
    async fn a_second_turn_resumes_the_captured_thread() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), FIXTURE_BODY);
        let adapter = std::sync::Arc::new(CodexAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
            None,
        ));
        let mut host = AcpHost::new(1, adapter.clone());
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();

        let (tx, _rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx).await.unwrap();

        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello again", tx).await.unwrap();
        let events = drain(rx).await;
        assert!(events.contains(&AcpSessionEvent::TextDelta("resumed".into())));
    }

    #[tokio::test]
    async fn a_restart_reconstructs_the_adapter_seeded_with_the_persisted_thread_id() {
        // Simulates a Kronn restart: no in-process history, but a previously
        // persisted native thread id is fed back in at construction time.
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), FIXTURE_BODY);
        let adapter = std::sync::Arc::new(CodexAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
            Some("th-from-before-restart".to_owned()),
        ));
        let mut host = AcpHost::new(1, adapter.clone());
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();
        host.resume_session(&target)
            .await
            .expect("a seeded thread id must make resume_session succeed");

        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx).await.unwrap();
        let events = drain(rx).await;
        assert!(events.contains(&AcpSessionEvent::TextDelta("resumed".into())));
    }

    #[tokio::test]
    async fn resume_session_without_any_known_thread_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let adapter = CodexAcpAdapter::new_with_program("sh", None, false, None);
        let mut host = AcpHost::new(1, std::sync::Arc::new(adapter));
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();
        let error = host.resume_session(&target).await.unwrap_err();
        assert!(matches!(error, AcpError::Transport(_)));
    }

    #[tokio::test]
    async fn a_missing_binary_surfaces_as_a_transport_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let adapter =
            CodexAcpAdapter::new_with_program("kronn-nonexistent-codex-binary", None, false, None);
        let mut host = AcpHost::new(1, std::sync::Arc::new(adapter));
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();
        let (tx, _rx) = mpsc::channel(16);
        let error = host.prompt(&target, "hi", tx).await.unwrap_err();
        assert!(matches!(error, AcpError::Transport(_)));
    }

    #[tokio::test]
    async fn cancel_kills_the_live_subprocess_and_the_turn_reports_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), "sleep 30");
        let adapter = std::sync::Arc::new(CodexAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
            None,
        ));
        let mut host = AcpHost::new(1, adapter.clone());
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();

        let (tx, _rx) = mpsc::channel(16);
        let prompt_target = target.clone();
        let prompt_adapter = adapter.clone();
        let handle =
            tokio::spawn(async move { prompt_adapter.prompt(&prompt_target, "hi", tx).await });
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        adapter.cancel(&target).await.unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_err(), "a killed turn must not report success");
    }

    #[tokio::test]
    async fn kronn_internal_mcp_override_forwards_only_env_var_names_never_values() {
        // Codex reads project-authorized MCP servers (with real credentials)
        // from its own already-synced `~/.codex/config.toml`; the adapter
        // only narrows which env vars the kronn-internal server may forward,
        // by name. This proves the exact same override string used by
        // today's direct-CLI Codex invocation is present, and that nothing
        // resembling a secret VALUE ever appears in argv.
        let dir = tempfile::tempdir().unwrap();
        let argv_file = dir.path().join("argv.txt");
        let fixture = crate::acp::test_support::write_fixture_script(
            dir.path(),
            &format!(
                r#"printf '%s\n' "$*" > '{}'
                printf '%s\n' '{{"type":"thread.started","thread_id":"th-1"}}'
                printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":2}}}}'"#,
                argv_file.display()
            ),
        );
        let adapter =
            CodexAcpAdapter::new_with_program(fixture.to_string_lossy(), None, false, None);
        let mut host = AcpHost::new(1, std::sync::Arc::new(adapter));
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();
        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx).await.unwrap();
        drain(rx).await;

        let argv = std::fs::read_to_string(&argv_file).unwrap();
        assert!(
            argv.contains(&crate::agents::runner::codex_kronn_internal_env_override()),
            "the exact narrow env-var-name override must be forwarded: {argv}"
        );
        for var in crate::agents::runner::KRONN_INTERNAL_CODEX_ENV_VARS {
            assert!(argv.contains(var), "{var} must be listed by name: {argv}");
        }
    }
}
