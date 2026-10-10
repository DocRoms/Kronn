use super::*;
use crate::acp::{
    AcpAgent, AcpCapability, AcpConfigOption, AcpError, AcpInitialize, AcpNegotiatedCapabilities,
    AcpSessionEvent, AcpSessionTarget, AcpTransport,
};
use crate::core::config::test_saved_access;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

#[derive(Clone, Copy, PartialEq)]
enum Session {
    Opens,
    Hangs,
    Fails,
}

/// A fake ACP runtime that counts every request, a prompt above all.
struct ProbeAgent {
    session: Session,
    initializes: AtomicUsize,
    sessions: AtomicUsize,
    prompts: AtomicUsize,
    config_writes: AtomicUsize,
    shutdowns: AtomicUsize,
}

impl ProbeAgent {
    fn new(session: Session) -> Arc<Self> {
        Arc::new(Self {
            session,
            initializes: AtomicUsize::new(0),
            sessions: AtomicUsize::new(0),
            prompts: AtomicUsize::new(0),
            config_writes: AtomicUsize::new(0),
            shutdowns: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl AcpTransport for ProbeAgent {
    async fn initialize(&self, _: AcpInitialize) -> Result<AcpNegotiatedCapabilities, AcpError> {
        self.initializes.fetch_add(1, Ordering::SeqCst);
        Ok(AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: BTreeSet::from([
                AcpCapability::Sessions,
                AcpCapability::Streaming,
                AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        self.sessions.fetch_add(1, Ordering::SeqCst);
        match self.session {
            Session::Opens => AcpSessionTarget::new(AcpAgent::OpenCode, "probe-session"),
            Session::Hangs => std::future::pending().await,
            Session::Fails => Err(AcpError::Transport(
                "MCP server Broken exited with code 1".into(),
            )),
        }
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
        self.config_writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        _: Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), AcpError> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

const SHORT: AcpProbeBounds = AcpProbeBounds {
    initialize: Duration::from_millis(400),
    session: Duration::from_millis(400),
    shutdown: Duration::from_millis(400),
};

fn detection(agent: &str, installed: bool, path: Option<&Path>) -> AgentDetection {
    serde_json::from_value(serde_json::json!({
        "name": agent, "agent_type": agent, "installed": installed,
        "path": path.map(|path| path.to_string_lossy().into_owned()),
        "version": "1.0.0", "latest_version": null, "origin": "test",
        "install_command": null,
    }))
    .unwrap()
}

struct Project {
    dir: tempfile::TempDir,
    id: String,
}

impl Project {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            id: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn path(&self) -> String {
        self.dir.path().to_string_lossy().into_owned()
    }

    /// Route every ACP session opened in this project to `agent`.
    fn route(&self, agent: &Arc<ProbeAgent>) -> runner::test_acp_routes::RouteGuard {
        let work_dir = runner::resolve_agent_work_dir(None, &self.path()).unwrap();
        runner::test_acp_routes::route(&work_dir, agent.clone())
    }

    fn declare_mcp(&self, names: &[&str]) {
        let servers: serde_json::Map<String, serde_json::Value> = names
            .iter()
            .map(|name| {
                (
                    (*name).to_string(),
                    serde_json::json!({"command": "sh", "args": []}),
                )
            })
            .collect();
        std::fs::write(
            self.dir.path().join(".mcp.json"),
            serde_json::json!({ "mcpServers": servers }).to_string(),
        )
        .unwrap();
    }

    fn context(&self, detections: Vec<AgentDetection>) -> ProbeContext {
        ProbeContext {
            project_id: Some(self.id.clone()),
            project_path: self.path(),
            detections,
            tokens: crate::core::config::default_config().tokens,
            agents_config: 0,
            bounds: SHORT,
            // A fresh script's first exec can be slow on macOS (code scanning).
            login_timeout: Duration::from_secs(30),
            force: false,
        }
    }
}

fn login_fixture(dir: &Path, body: &str) -> std::path::PathBuf {
    crate::acp::test_support::write_fixture_script(dir, body)
}

async fn one(ctx: &ProbeContext, agent: AgentType) -> AgentReadiness {
    check_agents(ctx, &[agent]).await.remove(0)
}

#[tokio::test]
async fn an_agent_that_is_not_installed_is_not_ready_and_nothing_starts() {
    let project = Project::new();
    let agent = ProbeAgent::new(Session::Opens);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::OpenCode, true);
    let ctx = project.context(vec![detection("OpenCode", false, None)]);
    let result = one(&ctx, AgentType::OpenCode).await;
    assert_eq!(result.status, AgentReadinessStatus::NotReady);
    assert_eq!(result.reason, AgentReadinessReason::NotInstalled);
    assert_eq!(result.message_key, "readiness.reason.not_installed");
    assert_eq!(agent.initializes.load(Ordering::SeqCst), 0);
    // An agent absent from the sweep is not installed either.
    let absent = one(&project.context(Vec::new()), AgentType::Vibe).await;
    assert_eq!(absent.reason, AgentReadinessReason::NotInstalled);
}

#[tokio::test]
async fn a_native_agent_without_full_access_is_not_ready_and_never_spawned() {
    let project = Project::new();
    let agent = ProbeAgent::new(Session::Opens);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::CopilotCli, false);
    let ctx = project.context(vec![detection("CopilotCli", true, None)]);
    let result = one(&ctx, AgentType::CopilotCli).await;
    assert_eq!(result.status, AgentReadinessStatus::NotReady);
    assert_eq!(result.reason, AgentReadinessReason::FullAccessRequired);
    assert_eq!(agent.initializes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_signed_out_cli_is_not_ready_and_a_signed_in_one_is_ready() {
    let bin = tempfile::tempdir().unwrap();
    let signed_out = login_fixture(
        bin.path(),
        r#"case "$*" in whoami) echo 'Not logged in'; exit 1;; *) exit 2;; esac"#,
    );
    let project = Project::new();
    let agent = ProbeAgent::new(Session::Opens);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::Kiro, true);
    let ctx = project.context(vec![detection("Kiro", true, Some(&signed_out))]);
    let result = one(&ctx, AgentType::Kiro).await;
    assert_eq!(result.status, AgentReadinessStatus::NotReady);
    assert_eq!(result.reason, AgentReadinessReason::NotLoggedIn);
    assert_eq!(result.message_key, "readiness.reason.not_logged_in");

    let bin = tempfile::tempdir().unwrap();
    let signed_in = login_fixture(
        bin.path(),
        r#"case "$*" in "auth status --json") printf '{"loggedIn": true}';; *) exit 2;; esac"#,
    );
    let project = Project::new();
    let ctx = project.context(vec![detection("ClaudeCode", true, Some(&signed_in))]);
    let result = one(&ctx, AgentType::ClaudeCode).await;
    assert_eq!(result.status, AgentReadinessStatus::Ready, "{result:?}");
    assert_eq!(result.reason, AgentReadinessReason::Ready);
    assert_eq!(result.message_key, "readiness.reason.ready");
}

#[tokio::test]
async fn a_status_command_the_cli_does_not_know_is_unknown_never_ready() {
    let bin = tempfile::tempdir().unwrap();
    let unknown = login_fixture(
        bin.path(),
        "echo \"error: unknown command 'login'\" >&2; exit 2",
    );
    let project = Project::new();
    let ctx = project.context(vec![detection("Codex", true, Some(&unknown))]);
    let result = one(&ctx, AgentType::Codex).await;
    assert_eq!(result.status, AgentReadinessStatus::Unknown);
    assert_eq!(result.reason, AgentReadinessReason::LoginUnverified);
}

#[tokio::test]
async fn a_signed_out_cli_with_a_key_kronn_hands_it_is_unknown_not_refused() {
    let bin = tempfile::tempdir().unwrap();
    let signed_out = login_fixture(bin.path(), r#"printf '{"loggedIn": false}'"#);
    let project = Project::new();
    let mut ctx = project.context(vec![detection("ClaudeCode", true, Some(&signed_out))]);
    ctx.tokens.keys.push(crate::models::ApiKey {
        id: "k1".into(),
        name: "test".into(),
        provider: "anthropic".into(),
        value: "sk-ant-test-0000".into(),
        active: true,
    });
    let result = one(&ctx, AgentType::ClaudeCode).await;
    assert_eq!(result.status, AgentReadinessStatus::Unknown);
    assert_eq!(result.reason, AgentReadinessReason::LoginUnverified);
}

#[test]
fn only_a_recognised_sign_in_answer_is_trusted() {
    use super::Login::*;
    let parse = super::parse_login;
    assert_eq!(
        parse(&AgentType::ClaudeCode, true, r#"{"loggedIn":true}"#, ""),
        SignedIn
    );
    assert_eq!(
        parse(&AgentType::ClaudeCode, false, r#"{"loggedIn":false}"#, ""),
        SignedOut
    );
    assert_eq!(
        parse(&AgentType::ClaudeCode, true, "Logged in", ""),
        Unknown
    );
    assert_eq!(
        parse(&AgentType::Codex, true, "Logged in using ChatGPT", ""),
        SignedIn
    );
    assert_eq!(
        parse(&AgentType::Codex, false, "", "Not logged in"),
        SignedOut
    );
    assert_eq!(
        parse(&AgentType::Codex, false, "", "unexpected argument"),
        Unknown
    );
    assert_eq!(
        parse(&AgentType::Kiro, false, "Not logged in", ""),
        SignedOut
    );
    assert_eq!(
        parse(&AgentType::Kiro, true, "user@example.com", ""),
        SignedIn
    );
    assert_eq!(parse(&AgentType::OpenCode, true, "Logged in", ""), Unknown);
}

#[tokio::test]
async fn a_blocking_mcp_server_is_caught_at_session_new_within_its_bound() {
    let project = Project::new();
    project.declare_mcp(&["Hang"]);
    let agent = ProbeAgent::new(Session::Hangs);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::OpenCode, true);
    let ctx = project.context(vec![detection("OpenCode", true, None)]);
    let started = Instant::now();
    let result = one(&ctx, AgentType::OpenCode).await;
    assert!(
        started.elapsed() < SHORT.session * 3,
        "{:?}",
        started.elapsed()
    );
    assert_eq!(result.status, AgentReadinessStatus::NotReady);
    assert_eq!(result.reason, AgentReadinessReason::SessionTimeout);
    assert_eq!(result.servers, vec!["Hang".to_string()]);
    assert!(result.secs.is_some());
    assert_eq!(
        agent.shutdowns.load(Ordering::SeqCst),
        1,
        "the runtime is stopped"
    );
    assert_eq!(agent.prompts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_session_that_fails_to_open_reports_its_redacted_error() {
    let project = Project::new();
    let agent = ProbeAgent::new(Session::Fails);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::Vibe, true);
    let ctx = project.context(vec![detection("Vibe", true, None)]);
    let result = one(&ctx, AgentType::Vibe).await;
    assert_eq!(result.reason, AgentReadinessReason::SessionFailed);
    assert!(
        result
            .detail
            .as_deref()
            .unwrap_or_default()
            .contains("Broken"),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_opening_session_proves_the_runtime_starts_with_zero_tokens() {
    let project = Project::new();
    project.declare_mcp(&["Memory"]);
    let agent = ProbeAgent::new(Session::Opens);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::OpenCode, true);
    let ctx = project.context(vec![detection("OpenCode", true, None)]);
    let result = one(&ctx, AgentType::OpenCode).await;
    // OpenCode has no sign-in status command: never claimed ready.
    assert_eq!(result.status, AgentReadinessStatus::Unknown);
    assert_eq!(result.reason, AgentReadinessReason::LoginUnverified);
    assert_eq!(agent.initializes.load(Ordering::SeqCst), 1);
    assert_eq!(agent.sessions.load(Ordering::SeqCst), 1);
    assert_eq!(agent.shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.prompts.load(Ordering::SeqCst),
        0,
        "no prompt, no token"
    );
    assert_eq!(agent.config_writes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn agents_are_probed_in_parallel_each_within_its_bound() {
    let project = Project::new();
    let agent = ProbeAgent::new(Session::Hangs);
    let _route = project.route(&agent);
    let _opencode = test_saved_access::set(&AgentType::OpenCode, true);
    let _vibe = test_saved_access::set(&AgentType::Vibe, true);
    let _gemini = test_saved_access::set(&AgentType::GeminiCli, true);
    let ctx = project.context(vec![
        detection("OpenCode", true, None),
        detection("Vibe", true, None),
        detection("GeminiCli", true, None),
    ]);
    let started = Instant::now();
    let results = check_agents(
        &ctx,
        &[
            AgentType::OpenCode,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Vibe,
            AgentType::Ollama,
        ],
    )
    .await;
    let elapsed = started.elapsed();
    assert!(elapsed >= SHORT.session);
    assert!(
        elapsed < SHORT.session * 2,
        "probes ran one after another: {elapsed:?}"
    );
    let agents: Vec<_> = results
        .iter()
        .map(|result| result.agent_type.clone())
        .collect();
    assert_eq!(
        agents,
        vec![
            AgentType::OpenCode,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Ollama
        ]
    );
    for result in &results[..3] {
        assert_eq!(result.reason, AgentReadinessReason::SessionTimeout);
    }
    assert_eq!(results[3].status, AgentReadinessStatus::Unknown);
    assert_eq!(results[3].reason, AgentReadinessReason::NotProbed);
    assert_eq!(agent.prompts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_result_is_cached_until_the_agent_or_project_mcp_configuration_changes() {
    let project = Project::new();
    let agent = ProbeAgent::new(Session::Opens);
    let _route = project.route(&agent);
    let _access = test_saved_access::set(&AgentType::OpenCode, true);
    let mut ctx = project.context(vec![detection("OpenCode", true, None)]);
    let first = one(&ctx, AgentType::OpenCode).await;
    assert!(!first.cached);
    let second = one(&ctx, AgentType::OpenCode).await;
    assert!(second.cached);
    assert_eq!(
        agent.initializes.load(Ordering::SeqCst),
        1,
        "served from the cache"
    );

    // The project's MCP servers change.
    project.declare_mcp(&["Added"]);
    assert!(!one(&ctx, AgentType::OpenCode).await.cached);
    assert_eq!(agent.initializes.load(Ordering::SeqCst), 2);

    // The saved agent settings change.
    ctx.agents_config = 42;
    assert!(!one(&ctx, AgentType::OpenCode).await.cached);
    assert_eq!(agent.initializes.load(Ordering::SeqCst), 3);

    // An explicit recheck.
    ctx.force = true;
    assert!(!one(&ctx, AgentType::OpenCode).await.cached);
    assert_eq!(agent.initializes.load(Ordering::SeqCst), 4);
    ctx.force = false;

    // Full access turned off: not served stale.
    let _off = test_saved_access::set(&AgentType::OpenCode, false);
    let revoked = one(&ctx, AgentType::OpenCode).await;
    assert!(!revoked.cached);
    assert_eq!(revoked.reason, AgentReadinessReason::FullAccessRequired);

    // The same project moved to another folder, with the same MCP servers.
    let moved = Project::new();
    std::fs::copy(
        project.dir.path().join(".mcp.json"),
        moved.dir.path().join(".mcp.json"),
    )
    .unwrap();
    let _moved_route = moved.route(&agent);
    let _back_on = test_saved_access::set(&AgentType::OpenCode, true);
    // Cache the original context again, so only the folder differs below.
    one(&ctx, AgentType::OpenCode).await;
    assert!(one(&ctx, AgentType::OpenCode).await.cached);
    let mut moved_ctx = moved.context(vec![detection("OpenCode", true, None)]);
    moved_ctx.project_id = ctx.project_id.clone();
    moved_ctx.agents_config = ctx.agents_config;
    let before = agent.initializes.load(Ordering::SeqCst);
    assert!(!one(&moved_ctx, AgentType::OpenCode).await.cached);
    assert_eq!(
        agent.initializes.load(Ordering::SeqCst),
        before + 1,
        "probed in the new folder"
    );
    assert!(one(&moved_ctx, AgentType::OpenCode).await.cached);

    // Another project has its own entry.
    let other = Project::new();
    let _other_route = other.route(&agent);
    let _on = test_saved_access::set(&AgentType::OpenCode, true);
    assert!(
        !one(
            &other.context(vec![detection("OpenCode", true, None)]),
            AgentType::OpenCode
        )
        .await
        .cached
    );
}

/// Through Kronn's real spawn of `opencode acp`: the runtime receives
/// `initialize` and `session/new`, never `session/prompt`.
#[cfg(unix)]
mod real_spawn {
    use super::*;

    const FIXTURE: &str = r#"
import json, os, sys

out = os.environ["OPENCODE_FIXTURE_OUT"]
hang = os.path.exists(os.path.join(out, "hang"))
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    with open(os.path.join(out, "methods"), "a") as handle:
        handle.write(str(method) + "\n")
    if method == "initialize":
        reply = {"protocolVersion": 1}
    elif method == "session/new":
        if hang:
            continue
        reply = {"sessionId": "probe-session"}
    elif method == "session/prompt":
        open(os.path.join(out, "prompted"), "w").close()
        reply = {"stopReason": "end_turn"}
    else:
        continue
    if "id" in message:
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": reply}) + "\n")
        sys.stdout.flush()
"#;

    struct OnPath(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl OnPath {
        fn install(bin: &Path, out: &Path) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let script = bin.join("opencode");
            std::fs::write(&script, format!("#!/usr/bin/env python3{FIXTURE}")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            let saved: Vec<_> = ["PATH", "OPENCODE_FIXTURE_OUT", "OPENCODE_CONFIG_CONTENT"]
                .into_iter()
                .map(|name| (name, crate::core::child_env::var_os(name)))
                .collect();
            let path = std::env::join_paths(std::iter::once(bin.to_path_buf()).chain(
                std::env::split_paths(&crate::core::child_env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
            crate::core::child_env::set_var("PATH", path);
            crate::core::child_env::set_var("OPENCODE_FIXTURE_OUT", out);
            crate::core::child_env::remove_var("OPENCODE_CONFIG_CONTENT");
            Self(saved)
        }
    }

    impl Drop for OnPath {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                match value {
                    Some(value) => crate::core::child_env::set_var(name, value),
                    None => crate::core::child_env::remove_var(name),
                }
            }
        }
    }

    fn methods(out: &Path) -> Vec<String> {
        std::fs::read_to_string(out.join("methods"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[tokio::test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    async fn the_real_spawn_opens_a_session_and_never_prompts() {
        let bin = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let _path = OnPath::install(bin.path(), out.path());
        let _access = test_saved_access::set(&AgentType::OpenCode, true);
        let project = Project::new();
        let mut ctx = project.context(vec![detection("OpenCode", true, None)]);
        ctx.bounds = AcpProbeBounds {
            initialize: Duration::from_secs(20),
            session: Duration::from_secs(20),
            shutdown: Duration::from_secs(5),
        };
        let result = one(&ctx, AgentType::OpenCode).await;
        assert_eq!(
            result.reason,
            AgentReadinessReason::LoginUnverified,
            "{result:?}"
        );
        assert_eq!(methods(out.path()), vec!["initialize", "session/new"]);
        assert!(
            !out.path().join("prompted").exists(),
            "the model was never asked"
        );
    }

    #[tokio::test]
    #[serial_test::serial(acp_adapter_env_toggle)]
    async fn the_real_spawn_of_a_stalled_session_times_out_and_never_prompts() {
        let bin = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("hang"), "").unwrap();
        let _path = OnPath::install(bin.path(), out.path());
        let _access = test_saved_access::set(&AgentType::OpenCode, true);
        let project = Project::new();
        let mut ctx = project.context(vec![detection("OpenCode", true, None)]);
        ctx.bounds = AcpProbeBounds {
            initialize: Duration::from_secs(20),
            session: Duration::from_millis(800),
            shutdown: Duration::from_secs(5),
        };
        let result = one(&ctx, AgentType::OpenCode).await;
        assert_eq!(
            result.reason,
            AgentReadinessReason::SessionTimeout,
            "{result:?}"
        );
        assert_eq!(methods(out.path()), vec!["initialize", "session/new"]);
        assert!(!out.path().join("prompted").exists());
    }
}
