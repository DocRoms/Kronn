//! Adapts Claude Code's own non-interactive protocol to `AcpTransport`.
//!
//! Claude Code has no vendor ACP subcommand (verified: `claude --help` on the
//! installed CLI, 2.1.207, exposes no `acp` flag or subcommand — unlike
//! OpenCode/Gemini/Copilot/Kiro/Vibe, which are native ACP runtimes handled by
//! `AcpJsonRpcTransport`). It does support everything ACP's session lifecycle
//! needs through documented, stable flags:
//! - `--session-id <uuid>` lets the client pin a session id up front, and
//!   `--resume <uuid>` continues it — so `create_session` never has to spawn
//!   a process just to learn an id, unlike Codex.
//! - `--output-format stream-json --include-partial-messages --verbose`
//!   streams live during a turn.
//! - `--mcp-config <file> --strict-mcp-config` is used only when the complete
//!   project file matches the broker-authorized, non-secret server set.
//!   Kronn parses the local project registry to validate that shape, but
//!   never serializes secret values into argv, prompts, events, client
//!   payloads, or audit entries; a credential-bearing or mixed file is
//!   omitted wholesale.
//!
//! There is no `--permission-prompt-tool` (or equivalent) flag in this CLI
//! version, so Claude cannot call back into Kronn mid-turn the way a native
//! ACP agent does. Permission policy is therefore computed once per session
//! by [`AcpPermissionBroker::session_policy`] and applied as static CLI flags
//! instead of a live negotiation.

use async_trait::async_trait;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::{mpsc, Mutex};

use super::permission_broker::{AcpAuditEntry, AcpPermissionBroker, AcpSessionScope};
use super::{
    AcpAgent, AcpCapability, AcpConfigOption, AcpError, AcpInitialize, AcpNegotiatedCapabilities,
    AcpSessionEvent, AcpSessionTarget, AcpTransport,
};
use crate::agents::runner::{parse_claude_stream_line, StreamJsonEvent};

pub struct ClaudeAcpAdapter {
    program: String,
    cwd: Mutex<Option<PathBuf>>,
    model: Option<String>,
    broker: AcpPermissionBroker,
    allowed_tools: Mutex<Vec<String>>,
    project_mcp_config_allowed: AtomicBool,
    discussion_id: Option<String>,
    has_run_before: AtomicBool,
    current_child: Mutex<Option<Child>>,
}

impl ClaudeAcpAdapter {
    pub fn new(
        model: Option<String>,
        full_access: bool,
        discussion_id: Option<String>,
        scope: AcpSessionScope,
    ) -> Self {
        Self {
            program: "claude".to_owned(),
            cwd: Mutex::new(None),
            model,
            broker: AcpPermissionBroker::scoped(full_access, scope),
            allowed_tools: Mutex::new(Vec::new()),
            project_mcp_config_allowed: AtomicBool::new(false),
            discussion_id,
            has_run_before: AtomicBool::new(false),
            current_child: Mutex::new(None),
        }
    }

    /// Test-only: drive a fixture script instead of the real `claude` binary.
    #[cfg(test)]
    fn new_with_program(
        program: impl Into<String>,
        model: Option<String>,
        full_access: bool,
    ) -> Self {
        Self {
            program: program.into(),
            ..Self::new(model, full_access, None, AcpSessionScope::new(None, "test"))
        }
    }

    /// Audit trail of the (static, pre-session) permission policy decision.
    pub fn permission_audit_log(&self) -> Vec<AcpAuditEntry> {
        self.broker.audit_log()
    }

    fn mcp_config_path(cwd: &std::path::Path) -> Option<PathBuf> {
        let candidate = cwd.join(".mcp.json");
        candidate.exists().then_some(candidate)
    }
}

#[async_trait]
impl AcpTransport for ClaudeAcpAdapter {
    async fn initialize(
        &self,
        request: AcpInitialize,
    ) -> Result<AcpNegotiatedCapabilities, AcpError> {
        *self.cwd.lock().await = Some(PathBuf::from(&request.cwd));
        let servers = self.broker.authorize_mcp_servers(request.mcp_servers);
        let authorized_ids: std::collections::BTreeSet<_> =
            servers.iter().map(|server| server.id.as_str()).collect();
        let config_is_exactly_authorized = crate::core::mcp_scanner::read_mcp_json(&request.cwd)
            .is_some_and(|file| {
                file.mcp_servers.len() == authorized_ids.len()
                    && file.mcp_servers.iter().all(|(id, entry)| {
                        authorized_ids.contains(id.as_str())
                            && !crate::core::mcp_scanner::mcp_entry_leaks_secret(entry)
                    })
            });
        self.project_mcp_config_allowed
            .store(config_is_exactly_authorized, Ordering::SeqCst);
        let mut allowed_tools = Vec::new();
        for server in servers {
            if server.allowed_tools.is_empty() {
                allowed_tools.push(format!("mcp__{}__*", server.id));
            } else {
                allowed_tools.extend(
                    server
                        .allowed_tools
                        .into_iter()
                        .map(|tool| format!("mcp__{}__{}", server.id, tool)),
                );
            }
        }
        *self.allowed_tools.lock().await = allowed_tools;
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
        // Claude lets the client pin a session id up front (`--session-id`).
        // No process is spawned here; the id becomes real Claude state on the
        // next `prompt` call.
        let target = AcpSessionTarget::new(AcpAgent::ClaudeCode, uuid::Uuid::new_v4().to_string())?;
        self.broker
            .bind_protocol_session(&target.session_id)
            .map_err(AcpError::Transport)?;
        Ok(target)
    }

    async fn config_options(&self) -> Vec<AcpConfigOption> {
        // Claude has no session-config-options catalogue: `--model` is a
        // direct CLI flag resolved once at adapter construction, never
        // discovered from a session response.
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

    async fn resume_session(&self, target: &AcpSessionTarget) -> Result<(), AcpError> {
        // Claude has no separate "load without prompting" call: `--resume`
        // only takes effect on the next actual `prompt` turn. Marking this
        // now covers both an in-process resume and a cross-restart one (the
        // caller reconstructs the adapter, then calls `resume_session` before
        // the first `prompt`).
        self.broker
            .bind_protocol_session(&target.session_id)
            .map_err(AcpError::Transport)?;
        self.has_run_before.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn prompt(
        &self,
        target: &AcpSessionTarget,
        prompt: &str,
        events: mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        let cwd = self.cwd.lock().await.clone().ok_or_else(|| {
            AcpError::Transport("Claude ACP adapter prompted before initialize".into())
        })?;
        let resuming = self.has_run_before.swap(true, Ordering::SeqCst);

        let mut args: Vec<String> = vec![
            "--print".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--include-partial-messages".into(),
        ];
        args.push(if resuming {
            "--resume".into()
        } else {
            "--session-id".into()
        });
        args.push(target.session_id.clone());
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.clone());
        }
        if self.project_mcp_config_allowed.load(Ordering::SeqCst) {
            let mcp_config = Self::mcp_config_path(&cwd).ok_or_else(|| {
                AcpError::Transport("authorized Claude MCP config disappeared".into())
            })?;
            args.push("--mcp-config".into());
            args.push(mcp_config.to_string_lossy().into_owned());
            args.push("--strict-mcp-config".into());
        }
        let allowed_tools = self.allowed_tools.lock().await.clone();
        if !allowed_tools.is_empty() {
            args.push("--allowedTools".into());
            args.extend(allowed_tools);
        }
        if self.broker.session_policy().claude_skip_permissions {
            args.push("--dangerously-skip-permissions".into());
        }
        let mut command = crate::core::cmd::async_cmd(&self.program);
        command
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(discussion_id) = self.discussion_id.as_deref() {
            command.env("KRONN_DISCUSSION_ID", discussion_id);
        }
        let mut child = command
            .spawn()
            .map_err(|error| AcpError::Transport(format!("spawn claude: {error}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AcpError::Transport("claude stdin unavailable".into()))?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|error| AcpError::Transport(format!("write claude prompt: {error}")))?;
        stdin
            .shutdown()
            .await
            .map_err(|error| AcpError::Transport(format!("close claude prompt stdin: {error}")))?;
        drop(stdin);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AcpError::Transport("claude stdout unavailable".into()))?;
        *self.current_child.lock().await = Some(child);

        let mut lines = BufReader::new(stdout).lines();
        let mut failure: Option<String> = None;
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match parse_claude_stream_line(&line) {
                    StreamJsonEvent::Text(text) => {
                        let _ = events.send(AcpSessionEvent::TextDelta(text)).await;
                    }
                    StreamJsonEvent::Usage {
                        input_tokens,
                        output_tokens,
                        ..
                    } => {
                        let _ = events
                            .send(AcpSessionEvent::Usage {
                                input_tokens,
                                output_tokens,
                            })
                            .await;
                    }
                    StreamJsonEvent::ToolStart(name) => {
                        let _ = events.send(AcpSessionEvent::ToolCall { name }).await;
                    }
                    StreamJsonEvent::TerminalError(terminal_failure) => {
                        failure = Some(terminal_failure.user_message());
                    }
                    StreamJsonEvent::ToolInputDelta(_)
                    | StreamJsonEvent::ToolEnd
                    | StreamJsonEvent::Skip => {}
                },
                Ok(None) => break,
                Err(error) => {
                    return Err(AcpError::Transport(format!("read claude stdout: {error}")));
                }
            }
        }

        let status = {
            let mut guard = self.current_child.lock().await;
            match guard.as_mut() {
                Some(child) => child
                    .wait()
                    .await
                    .map_err(|error| AcpError::Transport(format!("wait for claude: {error}")))?,
                // Taken by a concurrent `cancel()`: the turn was interrupted.
                None => return Err(AcpError::Transport("claude turn was cancelled".into())),
            }
        };
        *self.current_child.lock().await = None;

        if let Some(failure) = failure {
            return Err(AcpError::Transport(failure));
        }
        if !status.success() {
            return Err(AcpError::Transport(format!(
                "claude exited with status {status}"
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::{AcpHost, AcpPermissionVerdict};

    fn init_request(cwd: &str) -> AcpInitialize {
        AcpInitialize {
            protocol_version: 1,
            cwd: cwd.into(),
            mcp_servers: vec![],
        }
    }

    /// A fixture "claude" that: on the first invocation (no `--resume`)
    /// prints one stream-json text delta + a success result. On an
    /// invocation carrying `--resume` (a substring of the whole arg list),
    /// prints a different delta so the test can distinguish create vs
    /// resume. Every other flag the adapter passes (`--print`,
    /// `--output-format`, `--session-id <uuid>`, …) is simply ignored, the
    /// way a real shell script would.
    const FIXTURE_BODY: &str = r#"
        case "$*" in
          *--resume*)
            printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"resumed"}}}'
            ;;
          *)
            printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"created"}}}'
            ;;
        esac
        printf '%s\n' '{"type":"result","subtype":"success","usage":{"input_tokens":1,"output_tokens":2}}'
        "#;

    async fn drain(mut rx: mpsc::Receiver<AcpSessionEvent>) -> Vec<AcpSessionEvent> {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn create_then_prompt_uses_session_id_then_resume_uses_resume_flag() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), FIXTURE_BODY);
        let adapter = std::sync::Arc::new(ClaudeAcpAdapter {
            program: fixture.to_string_lossy().into_owned(),
            ..ClaudeAcpAdapter::new(
                None,
                false,
                None,
                AcpSessionScope::new(Some(dir.path().to_path_buf()), "disc-secret"),
            )
        });
        let mut host = AcpHost::new(1, adapter.clone());
        host.negotiate(AcpInitialize {
            protocol_version: 1,
            cwd: dir.path().to_string_lossy().into_owned(),
            mcp_servers: vec![crate::acp::AcpMcpServer {
                id: "private".into(),
                command: "private-server".into(),
                args: vec![],
                allowed_tools: vec![],
            }],
        })
        .await
        .unwrap();
        let target = host.create_session().await.unwrap();
        assert_eq!(target.agent, AcpAgent::ClaudeCode);
        assert!(!target.session_id.trim().is_empty());

        // First turn: no prior run, so `--session-id` is used, not `--resume`.
        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx)
            .await
            .unwrap_or_else(|error| panic!("first prompt failed: {error}"));
        let events = drain(rx).await;
        assert!(events.contains(&AcpSessionEvent::TextDelta("created".into())));
        assert!(events.contains(&AcpSessionEvent::Completed));

        // Second turn: has_run_before is now true, so `--resume` is used.
        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello again", tx)
            .await
            .unwrap_or_else(|error| panic!("second prompt failed: {error}"));
        let events = drain(rx).await;
        assert!(events.contains(&AcpSessionEvent::TextDelta("resumed".into())));
    }

    #[tokio::test]
    async fn resume_session_forces_the_resume_flag_even_before_any_prompt() {
        // Simulates a Kronn restart: a fresh adapter, immediately resumed
        // (the caller already knows a prior session id from durable state),
        // then prompted — must use `--resume`, not `--session-id`, on the
        // very first subprocess invocation.
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::acp::test_support::write_fixture_script(dir.path(), FIXTURE_BODY);
        let adapter = ClaudeAcpAdapter::new_with_program(fixture.to_string_lossy(), None, false);
        let mut host = AcpHost::new(1, std::sync::Arc::new(adapter));
        host.negotiate(init_request(&dir.path().to_string_lossy()))
            .await
            .unwrap();
        let target = host.create_session().await.unwrap();
        host.resume_session(&target).await.unwrap();

        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx).await.unwrap();
        let events = drain(rx).await;
        assert!(events.contains(&AcpSessionEvent::TextDelta("resumed".into())));
    }

    #[tokio::test]
    async fn a_missing_binary_surfaces_as_a_transport_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let adapter =
            ClaudeAcpAdapter::new_with_program("kronn-nonexistent-claude-binary", None, false);
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
        let adapter = std::sync::Arc::new(ClaudeAcpAdapter::new_with_program(
            fixture.to_string_lossy(),
            None,
            false,
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

    #[test]
    fn full_access_is_translated_into_a_skip_permissions_flag_and_audited() {
        let adapter = ClaudeAcpAdapter::new(None, true, None, AcpSessionScope::new(None, "test"));
        assert!(adapter.broker.session_policy().claude_skip_permissions);
        assert!(adapter
            .permission_audit_log()
            .iter()
            .any(|entry| entry.verdict == AcpPermissionVerdict::Allow));
    }

    #[tokio::test]
    async fn a_credentialed_project_mcp_config_is_omitted_instead_of_leaking_or_bypassing_scope() {
        // A project `.mcp.json` carrying a real credential value. The adapter
        // must not pass the original file wholesale: the broker excluded its
        // credential-bearing server, and loading that file anyway would let a
        // full-access Claude invocation bypass the authorized server set.
        let dir = tempfile::tempdir().unwrap();
        let secret = "sk-super-secret-token-do-not-leak";
        std::fs::write(
            dir.path().join(".mcp.json"),
            format!(
                r#"{{"mcpServers":{{"private":{{"command":"private-server","env":{{"API_KEY":"{secret}"}}}}}}}}"#
            ),
        )
        .unwrap();
        let argv_file = dir.path().join("argv.txt");
        let fixture = crate::acp::test_support::write_fixture_script(
            dir.path(),
            &format!(
                r#"printf '%s\n' "$*" > '{}'
                printf '%s\n' '{{"type":"result","subtype":"success","usage":{{"input_tokens":1,"output_tokens":2}}}}'"#,
                argv_file.display()
            ),
        );
        let adapter = std::sync::Arc::new(ClaudeAcpAdapter {
            program: fixture.to_string_lossy().into_owned(),
            ..ClaudeAcpAdapter::new(
                None,
                false,
                None,
                AcpSessionScope::new(Some(dir.path().to_path_buf()), "disc-secret"),
            )
        });
        let mut host = AcpHost::new(1, adapter.clone());
        host.negotiate(AcpInitialize {
            protocol_version: 1,
            cwd: dir.path().to_string_lossy().into_owned(),
            mcp_servers: vec![crate::acp::AcpMcpServer {
                id: "private".into(),
                command: "private-server".into(),
                args: vec![],
                allowed_tools: vec![],
            }],
        })
        .await
        .unwrap();
        let target = host.create_session().await.unwrap();
        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, "hello", tx).await.unwrap();
        drain(rx).await;

        let argv = std::fs::read_to_string(&argv_file).unwrap();
        assert!(
            !argv.contains("--mcp-config"),
            "unsafe config leaked: {argv}"
        );
        assert!(
            !argv.contains(secret),
            "the raw secret value must never appear in the adapter's own argv: {argv}"
        );
        assert!(adapter.permission_audit_log().iter().all(|entry| {
            !entry.reason.contains(secret)
                && !entry
                    .locations
                    .iter()
                    .any(|location| location.contains(secret))
        }));
    }

    #[tokio::test]
    async fn same_cardinality_with_different_server_ids_never_enables_the_project_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"canonical":{"command":"safe-server"}}}"#,
        )
        .unwrap();
        let adapter = ClaudeAcpAdapter::new(
            None,
            false,
            None,
            AcpSessionScope::new(Some(dir.path().to_path_buf()), "disc-ids"),
        );
        adapter
            .initialize(AcpInitialize {
                protocol_version: 1,
                cwd: dir.path().to_string_lossy().into_owned(),
                mcp_servers: vec![crate::acp::AcpMcpServer {
                    id: "different".into(),
                    command: "safe-server".into(),
                    args: vec![],
                    allowed_tools: vec![],
                }],
            })
            .await
            .unwrap();
        assert!(!adapter.project_mcp_config_allowed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn production_context_keeps_prompt_off_argv_and_applies_discussion_and_tool_scope() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".mcp.json"),
            r#"{"mcpServers":{"project-safe":{"command":"safe-server"}}}"#,
        )
        .unwrap();
        let argv_file = dir.path().join("argv.txt");
        let stdin_file = dir.path().join("stdin.txt");
        let discussion_file = dir.path().join("discussion.txt");
        let fixture = crate::acp::test_support::write_fixture_script(
            dir.path(),
            &format!(
                r#"printf '%s\n' "$*" > '{}'
                printf '%s\n' "$KRONN_DISCUSSION_ID" > '{}'
                IFS= read -r prompt
                printf '%s' "$prompt" > '{}'
                printf '%s\n' '{{"type":"result","subtype":"success","usage":{{"input_tokens":1,"output_tokens":2}}}}'"#,
                argv_file.display(),
                discussion_file.display(),
                stdin_file.display(),
            ),
        );
        let adapter = ClaudeAcpAdapter {
            program: fixture.to_string_lossy().into_owned(),
            ..ClaudeAcpAdapter::new(
                None,
                false,
                Some("disc-prod".into()),
                AcpSessionScope::new(Some(dir.path().to_path_buf()), "disc-prod"),
            )
        };
        let mut host = AcpHost::new(1, std::sync::Arc::new(adapter));
        host.negotiate(AcpInitialize {
            protocol_version: 1,
            cwd: dir.path().to_string_lossy().into_owned(),
            mcp_servers: vec![crate::acp::AcpMcpServer {
                id: "project-safe".into(),
                command: "safe-server".into(),
                args: vec![],
                allowed_tools: vec![],
            }],
        })
        .await
        .unwrap();
        let target = host.create_session().await.unwrap();
        let secret_prompt = "prompt-secret-must-use-stdin";
        let (tx, rx) = mpsc::channel(16);
        host.prompt(&target, secret_prompt, tx).await.unwrap();
        drain(rx).await;

        let argv = std::fs::read_to_string(argv_file).unwrap();
        assert!(!argv.contains(secret_prompt));
        assert!(argv.contains("--strict-mcp-config"));
        assert!(argv.contains("--allowedTools"));
        assert!(argv.contains("mcp__project-safe__*"));
        assert_eq!(std::fs::read_to_string(stdin_file).unwrap(), secret_prompt);
        assert_eq!(
            std::fs::read_to_string(discussion_file).unwrap().trim(),
            "disc-prod"
        );
    }
}
