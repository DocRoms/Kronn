//! Real process boundaries, with isolated fake CLIs and no provider requests.
#![cfg(unix)]

use std::{ffi::OsString, os::unix::fs::PermissionsExt, sync::Arc, time::Duration};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

use kronn::agents::runner::{start_agent_with_config, AgentStartConfig, TaskWorkerBridgeContext};
use kronn::db::{model_catalog, Database};
use kronn::models::{AgentType, ModelTier};

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn set(&mut self, name: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        if !self.0.iter().any(|(key, _)| *key == name) {
            self.0.push((name, std::env::var_os(name)));
        }
        // This binary contains one current-thread test. No other test can
        // observe its temporary PATH or transport switches.
        unsafe { std::env::set_var(name, value) };
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

async fn unresolved_qp_effort_is_an_atomic_configuration_error(db: Arc<Database>) {
    let mut cfg = kronn::core::config::default_config();
    cfg.encryption_secret = Some(kronn::core::crypto::generate_secret());
    let state = kronn::AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(cfg)),
        db.clone(),
        kronn::DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    let mut created = 0_i64;
    for agent in ["ClaudeCode", "Codex"] {
        for (case, effort) in [
            ("legacy", None),
            ("blank", Some("  ")),
            ("explicit", Some("low")),
        ] {
            let id = format!("qp-{agent}-{case}");
            let qp = serde_json::from_value(serde_json::json!({
                "id": id, "name": "Unresolved effort", "icon": "", "prompt_template": "test",
                "variables": [], "agent": agent, "project_id": null, "skill_ids": [],
                "profile_ids": [], "directive_ids": [], "tier": "default",
                "agent_settings": {"reasoning_effort": effort}, "description": "",
                "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now()
            }))
            .unwrap();
            db.with_conn(move |conn| kronn::db::quick_prompts::insert_quick_prompt(conn, &qp))
                .await
                .unwrap();
            let request = Request::builder()
                .method("POST")
                .uri("/api/discussions")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "title": "Unresolved QP", "agent": agent, "initial_prompt": "test",
                        "originating_qp_id": id, "tier": "default", "no_agent": true
                    })
                    .to_string(),
                ))
                .unwrap();
            let response = kronn::build_router_with_auth(state.clone(), false)
                .oneshot(request)
                .await
                .unwrap();
            let result: serde_json::Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            if case == "explicit" {
                assert_eq!(result["success"], false, "{agent}: {result}");
                assert_eq!(result["error_code"], "validation", "{agent}: {result}");
                let error = result["error"].as_str().unwrap();
                assert!(
                    error.contains("choose a model or clear the effort"),
                    "{error}"
                );
                assert!(!error.contains("DB error"), "{error}");
            } else {
                assert_eq!(result["success"], true, "{agent}: {result}");
                created += 1;
            }
            db.with_read_conn(move |conn| {
                let discussions: i64 =
                    conn.query_row("SELECT count(*) FROM discussions", [], |row| row.get(0))?;
                let snapshots: i64 = conn.query_row(
                    "SELECT count(*) FROM discussion_effort_snapshots",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(
                    discussions, created,
                    "a refused launch must not create a discussion"
                );
                assert_eq!(snapshots, 0, "no unresolved or empty-effort snapshot");
                Ok(())
            })
            .await
            .unwrap();
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn discovered_effort_reaches_real_cli_and_adapter_children_including_resume_and_workers() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for name in ["host", "data", "project", "host-bin"] {
        std::fs::create_dir(dir.path().join(name)).unwrap();
    }
    let mut env = Environment(Vec::new());
    env.set("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    env.set("KRONN_HOST_HOME", dir.path().join("host"));
    env.set("KRONN_DATA_DIR", dir.path().join("data"));
    env.set("KRONN_HOST_BIN", dir.path().join("host-bin"));
    let project = dir.path().join("project");
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"kronn-internal":{"command":"false","args":[]}}}"#,
    )
    .unwrap();
    for binary in ["claude", "codex"] {
        let file = bin.join(binary);
        std::fs::write(&file, r#"#!/bin/sh
if [ "$1" = auth ]; then
  printf '%s\n' '{"loggedIn":true}'
  exit 0
fi
printf '%s\n' "$@" > "$KRONN_EFFORT_ARGV"
cat >/dev/null
case "$0" in
  *claude) printf '%s\n' '{"type":"result","subtype":"success","result":"fixture"}' ;;
  *codex) printf '%s\n' '{"type":"thread.started","thread_id":"11111111-1111-4111-8111-111111111111"}' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}' ;;
esac
"#).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            kronn::agents::find_binary(binary).unwrap().path,
            file.to_string_lossy()
        );
    }
    let db = Arc::new(Database::open_in_memory().unwrap());
    db.with_conn(|conn| {
        for agent in [AgentType::ClaudeCode, AgentType::Codex] {
            model_catalog::reconcile_live(
                conn,
                &model_catalog::agent_runtime_target_id(&agent),
                &agent,
                &[
                    model_catalog::DiscoveredModel {
                        model_id: "effort-model".into(),
                        display_name: "Test".into(),
                        capabilities: vec!["chat".into()],
                        reasoning_modes: vec!["low".into(), "high".into()],
                        default_reasoning_mode: None,
                    },
                    model_catalog::DiscoveredModel {
                        model_id: "other-model".into(),
                        display_name: "Other".into(),
                        capabilities: vec!["chat".into()],
                        reasoning_modes: vec!["low".into()],
                        default_reasoning_mode: None,
                    },
                ],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    kronn::core::model_catalog::refresh_runtime_cache(&db)
        .await
        .unwrap();
    unresolved_qp_effort_is_an_atomic_configuration_error(db.clone()).await;
    let base = kronn::core::config::default_config();
    let resume_id = "11111111-1111-4111-8111-111111111111";
    let mut count = 0;
    for (agent, binary, switch) in [
        (AgentType::ClaudeCode, "claude", "KRONN_ACP_ADAPTER_CLAUDE"),
        (AgentType::Codex, "codex", "KRONN_ACP_ADAPTER_CODEX"),
    ] {
        for (adapted, resume, worker) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            env.set(switch, if adapted { "1" } else { "0" });
            for (preset, override_effort, pin, expected) in [
                (None, None, None, None),
                (Some("low"), None, None, Some("low")),
                (Some("low"), Some("high"), None, Some("high")),
                (Some("high"), None, Some("other-model"), None),
            ] {
                let mut tiers = base.agents.model_tiers.clone();
                let tier = if agent == AgentType::ClaudeCode {
                    &mut tiers.claude_code
                } else {
                    &mut tiers.codex
                };
                tier.default = Some("effort-model".into());
                tier.default_effort = preset.map(str::to_owned);
                let argv_file = dir.path().join(format!("argv-{count}"));
                env.set("KRONN_EFFORT_ARGV", &argv_file);
                let worker_context = TaskWorkerBridgeContext {
                    execution_id: "fake-execution".into(),
                    discussion_id: "fake-discussion".into(),
                    agent_type: binary.into(),
                    dispatch_job_id: "fake-job".into(),
                    source_message_id: "fake-message".into(),
                };
                let config = AgentStartConfig {
                    tier: ModelTier::Default,
                    model_tiers: Some(&tiers),
                    model_override: pin,
                    reasoning_effort_override: override_effort,
                    mcp_context_override: Some(""),
                    cli_resume_id: resume.then_some(resume_id),
                    task_worker_context: worker.then_some(&worker_context),
                    ..AgentStartConfig::new(
                        &agent,
                        project.to_str().unwrap(),
                        "fixture prompt",
                        &base.tokens,
                    )
                };
                let mut process =
                    tokio::time::timeout(Duration::from_secs(10), start_agent_with_config(config))
                        .await
                        .unwrap()
                        .unwrap();
                let completion = tokio::time::timeout(Duration::from_secs(10), async {
                    while process.next_line().await.is_some() {}
                    process.child.wait().await.unwrap()
                })
                .await;
                if completion.is_err() {
                    process.child.kill().await.unwrap();
                    panic!("fake runtime timed out: {agent:?} adapted={adapted} resume={resume} worker={worker}");
                }
                assert!(completion.unwrap().success());
                let raw = std::fs::read_to_string(&argv_file).unwrap();
                let args: Vec<_> = raw.lines().collect();
                assert_eq!(
                    args.windows(2)
                        .find(|pair| pair[0] == "--model" || pair[0] == "-m")
                        .map(|pair| pair[1]),
                    Some(pin.unwrap_or("effort-model"))
                );
                let actual = if agent == AgentType::ClaudeCode {
                    args.windows(2)
                        .find(|pair| pair[0] == "--effort")
                        .map(|pair| pair[1].to_owned())
                } else {
                    args.windows(2)
                        .find(|pair| {
                            pair[0] == "-c" && pair[1].starts_with("model_reasoning_effort=")
                        })
                        .map(|pair| {
                            serde_json::from_str::<String>(pair[1].split_once('=').unwrap().1)
                                .unwrap()
                        })
                };
                assert_eq!(
                    actual.as_deref(),
                    expected,
                    "{agent:?} adapted={adapted} resume={resume} worker={worker}"
                );
                // The direct Codex runner intentionally starts fresh; only its
                // ACP adapter consumes the resume hint. Effort must survive both.
                let expects_resume =
                    resume && !worker && (agent == AgentType::ClaudeCode || adapted);
                assert_eq!(
                    args.iter()
                        .any(|arg| *arg == "--resume" || *arg == "resume"),
                    expects_resume,
                    "{agent:?} adapted={adapted} resume={resume} worker={worker}"
                );
                if worker {
                    assert!(args.contains(&if agent == AgentType::ClaudeCode {
                        "--setting-sources"
                    } else {
                        "--ignore-user-config"
                    }));
                }
                count += 1;
            }
        }
        let forbidden = dir.path().join(format!("forbidden-{binary}"));
        env.set("KRONN_EFFORT_ARGV", &forbidden);
        let result = start_agent_with_config(AgentStartConfig {
            model_override: Some("other-model"),
            reasoning_effort_override: Some("high"),
            mcp_context_override: Some(""),
            ..AgentStartConfig::new(
                &agent,
                project.to_str().unwrap(),
                "must not spawn",
                &base.tokens,
            )
        })
        .await;
        assert!(matches!(result, Err(error) if error.contains("Cannot apply reasoning effort")));
        assert!(
            !forbidden.exists(),
            "unsupported effort must fail before spawning"
        );
    }
    assert_eq!(count, 40);
}
