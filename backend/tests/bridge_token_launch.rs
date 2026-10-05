//! KT-1006 — a launch's bridge token through the real launch path: always
//! minted (even for a launch that owns nothing), live exactly as long as the
//! process handle, dead as soon as the launch is cancelled.
#![cfg(unix)]

use std::{ffi::OsString, os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};

use axum::{body::Body, extract::ConnectInfo, http::Request};
use kronn::agents::runner::{start_agent_with_config, AgentIo, AgentStartConfig};
use kronn::core::bridge_token::lookup;
use kronn::models::AgentType;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use tokio::sync::RwLock;
use tower::ServiceExt;

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn set(&mut self, name: &'static str, value: impl Into<OsString>) {
        if !self.0.iter().any(|(key, _)| *key == name) {
            self.0.push((name, std::env::var_os(name)));
        }
        // This integration binary holds one current-thread test.
        unsafe { std::env::set_var(name, value.into()) };
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (name, value) in self.0.iter().rev() {
            unsafe {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

/// A `claude` that records its bridge token, then holds its turn open until
/// the test releases it.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
printf '%s' "$KRONN_BRIDGE_TOKEN" > "$CLAUDE_TOKEN_OUT"
i=0
while [ ! -e "$CLAUDE_RELEASE" ] && [ $i -lt 600 ]; do
  sleep 0.05
  i=$((i + 1))
done
printf '%s\n' '{"type":"result","subtype":"success","result":"done"}'
"#;

async fn token_written_to(path: &Path) -> String {
    for _ in 0..400 {
        if let Ok(token) = std::fs::read_to_string(path) {
            if !token.is_empty() {
                return token;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the fake agent never wrote its token");
}

#[tokio::test(flavor = "current_thread")]
async fn a_launch_token_lives_exactly_as_long_as_its_process() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["bin", "host", "host-bin", "data", "project"] {
        std::fs::create_dir(dir.path().join(name)).unwrap();
    }
    let bin = dir.path().join("bin");
    let claude = bin.join("claude");
    std::fs::write(&claude, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut env = Environment(Vec::new());
    env.set("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    env.set("KRONN_HOST_HOME", dir.path().join("host"));
    env.set("KRONN_HOST_BIN", dir.path().join("host-bin"));
    env.set("KRONN_DATA_DIR", dir.path().join("data"));
    env.set("KRONN_ACP_ADAPTER_CLAUDE", "0");
    let project = dir.path().join("project");
    let base = kronn::core::config::default_config();

    // E-01: a launch with no discussion, run or project still carries a token,
    // and that token reads catalogues only.
    let token_file = dir.path().join("token-1");
    env.set("CLAUDE_TOKEN_OUT", &token_file);
    env.set("CLAUDE_RELEASE", dir.path().join("release-1"));
    let mut process = start_agent_with_config(AgentStartConfig {
        mcp_context_override: Some(""),
        ..AgentStartConfig::new(
            &AgentType::ClaudeCode,
            project.to_str().unwrap(),
            "prompt",
            &base.tokens,
        )
    })
    .await
    .expect("the launch starts");
    let token = token_written_to(&token_file).await;
    assert!(token.starts_with("kbt_"), "a launch always carries a token");
    assert!(lookup(&token).is_some(), "live while the process runs");
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_token = Some("operator".into());
    let app = build_router_with_auth(
        AppState::new_defaults(
            Arc::new(RwLock::new(config)),
            db,
            DEFAULT_MAX_CONCURRENT_AGENTS,
        ),
        true,
    );
    let mut request = Request::builder()
        .uri("/api/discussions")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            4242,
        ))));
    let status = app.oneshot(request).await.unwrap().status().as_u16();
    assert_eq!(
        status, 403,
        "a token owning nothing is refused on discussions"
    );

    // E-02: cancelling the launch kills its token at once.
    process.kill().await;
    assert!(
        lookup(&token).is_none(),
        "dead as soon as the launch is cancelled"
    );
    drop(process);

    // F-05: a token lives exactly as long as its process handle.
    let token_file = dir.path().join("token-2");
    env.set("CLAUDE_TOKEN_OUT", &token_file);
    env.set("CLAUDE_RELEASE", dir.path().join("release-2"));
    let process = start_agent_with_config(AgentStartConfig {
        mcp_context_override: Some(""),
        discussion_id: Some("launch-room"),
        ..AgentStartConfig::new(
            &AgentType::ClaudeCode,
            project.to_str().unwrap(),
            "prompt",
            &base.tokens,
        )
    })
    .await
    .expect("the launch starts");
    let token = token_written_to(&token_file).await;
    assert!(lookup(&token).is_some(), "live while the handle is held");
    std::fs::write(dir.path().join("release-2"), "").unwrap();
    drop(process);
    assert!(lookup(&token).is_none(), "revoked with the handle");
}
