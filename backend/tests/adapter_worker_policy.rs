//! KT-652 / KT-806: exercise production dispatch and inspect the actual CLI argv.
#![cfg(unix)]

use std::{collections::HashSet, ffi::OsString, os::unix::fs::PermissionsExt, time::Duration};

use kronn::agents::runner::{start_agent_with_config, AgentStartConfig, TaskWorkerBridgeContext};
use kronn::models::AgentType;

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn change(&mut self, name: &'static str, value: Option<OsString>) {
        if !self.0.iter().any(|(key, _)| *key == name) {
            self.0.push((name, kronn::core::child_env::var_os(name)));
        }
        // nextest, the suite's runner, gives this test its own process.
        {
            match value {
                Some(value) => kronn::core::child_env::set_var(name, value),
                None => kronn::core::child_env::remove_var(name),
            }
        }
    }
    fn set(&mut self, name: &'static str, value: impl Into<OsString>) {
        self.change(name, Some(value.into()));
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (name, value) in self.0.iter().rev() {
            {
                match value {
                    Some(value) => kronn::core::child_env::set_var(name, value),
                    None => kronn::core::child_env::remove_var(name),
                }
            }
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn default_adapters_and_explicit_fallback_keep_worker_spawn_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["bin", "host", "host-bin", "data", "project"] {
        std::fs::create_dir(dir.path().join(name)).unwrap();
    }
    let bin = dir.path().join("bin");
    let project = dir.path().join("project");
    let linked = dir.path().join("linked repo");
    let initialized = kronn::core::cmd::git_cmd()
        .arg("init")
        .arg(&linked)
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let read_only_repos = vec![linked.to_string_lossy().to_string()];
    let mut env = Environment(Vec::new());
    env.set("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    env.set("KRONN_HOST_HOME", dir.path().join("host"));
    env.set("KRONN_HOST_BIN", dir.path().join("host-bin"));
    env.set("KRONN_DATA_DIR", dir.path().join("data"));
    env.set(
        "KRONN_INTROSPECTION_PUBLIC_PATH",
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/disc-introspection-mcp.py"
        ),
    );
    let mut registry = serde_json::from_value(serde_json::json!({"mcpServers":{
        "project-safe": {"command": "safe-server", "args": ["serve"]}
    }}))
    .unwrap();
    assert!(kronn::core::mcp_scanner::inject_kronn_internal(
        &mut registry
    ));
    std::fs::write(
        project.join(".mcp.json"),
        serde_json::to_string(&registry).unwrap(),
    )
    .unwrap();
    // An inherited permissive marker must not weaken a worker's OS sandbox.
    env.set("CLAUDE_CODE_BUBBLEWRAP", "1");
    env.set("KRONN_TASK_WORKER_CONTEXT", "unrelated-parent-context");
    for binary in ["claude", "codex"] {
        let file = bin.join(binary);
        std::fs::write(&file, r#"#!/bin/sh
if [ "$1" = auth ]; then
  printf '%s\n' '{"loggedIn":true}'
  exit 0
fi
# Agents get a built environment: the fixture's paths ride in names each owns.
KRONN_POLICY_ARGV="${CLAUDE_POLICY_ARGV:-$CODEX_POLICY_ARGV}"
KRONN_POLICY_ENV="${CLAUDE_POLICY_ENV:-$CODEX_POLICY_ENV}"
printf '%s\n' "$@" > "$KRONN_POLICY_ARGV"
printf '%s\n' "${KRONN_TASK_WORKER_CONTEXT-unset}" "${KRONN_DISCUSSION_ID-unset}" "${TMPDIR-unset}" "${CLAUDE_CODE_BUBBLEWRAP-unset}" > "$KRONN_POLICY_ENV"
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
    let base = kronn::core::config::default_config();
    let mut failures = Vec::new();
    let mut count = 0;
    for (agent, switch) in [
        (AgentType::ClaudeCode, "KRONN_ACP_ADAPTER_CLAUDE"),
        (AgentType::Codex, "KRONN_ACP_ADAPTER_CODEX"),
    ] {
        for toggle in [None, Some("0"), Some("1")] {
            env.change(switch, toggle.map(OsString::from));
            for mode in [
                "ordinary",
                "worker",
                "read-only",
                "ordinary-resume",
                "read-only-resume",
            ] {
                let resume = mode.ends_with("-resume");
                if resume && (agent != AgentType::Codex || toggle == Some("0")) {
                    continue;
                }
                let worker = mode == "worker";
                let read_only = mode.starts_with("read-only");
                let label = format!("{agent:?} toggle={toggle:?} mode={mode}");
                let argv = dir.path().join(format!("argv-{count}"));
                let child_env = dir.path().join(format!("env-{count}"));
                env.set("CLAUDE_POLICY_ARGV", argv.clone());
                env.set("CODEX_POLICY_ARGV", argv.clone());
                env.set("CLAUDE_POLICY_ENV", child_env.clone());
                env.set("CODEX_POLICY_ENV", child_env.clone());
                let context = TaskWorkerBridgeContext {
                    execution_id: "fixture-execution".into(),
                    discussion_id: "fixture-discussion".into(),
                    agent_type: format!("{agent:?}"),
                    dispatch_job_id: "fixture-job".into(),
                    source_message_id: "fixture-message".into(),
                };
                let mut process = tokio::time::timeout(
                    Duration::from_secs(10),
                    start_agent_with_config(AgentStartConfig {
                        full_access: true,
                        read_only_repos: if read_only { &read_only_repos } else { &[] },
                        discussion_id: Some("fixture-discussion"),
                        mcp_context_override: Some(""),
                        // Even an explicit hint must not resume a task worker.
                        cli_resume_id: (worker || resume)
                            .then_some("11111111-1111-4111-8111-111111111111"),
                        task_worker_context: worker.then_some(&context),
                        ..AgentStartConfig::new(
                            &agent,
                            project.to_str().unwrap(),
                            "fixture prompt",
                            &base.tokens,
                        )
                    }),
                )
                .await
                .unwrap()
                .unwrap();
                let finished = tokio::time::timeout(Duration::from_secs(10), async {
                    while process.next_line().await.is_some() {}
                    process.child.wait().await.unwrap()
                })
                .await;
                if finished.is_err() {
                    process.child.kill().await.unwrap();
                    panic!("owned fixture did not complete: {label}");
                }
                assert!(finished.unwrap().success(), "{label}");
                let raw = std::fs::read_to_string(argv).unwrap();
                let args: Vec<_> = raw.lines().collect();
                let raw_env = std::fs::read_to_string(child_env).unwrap();
                let observed: Vec<_> = raw_env.lines().collect();
                let mut check = |ok: bool, reason: &str| {
                    if !ok {
                        failures.push(format!("{label}: {reason}"));
                    }
                };
                let adapted = if agent == AgentType::ClaudeCode {
                    args.contains(&"--session-id")
                } else {
                    args.contains(&"-") // Adapter streams its prompt through stdin.
                };
                check(
                    adapted == (toggle != Some("0")),
                    "wrong actual dispatch route",
                );
                if agent == AgentType::Codex {
                    let trust_overrides: Vec<_> = args
                        .windows(2)
                        .filter(|pair| pair[0] == "-c")
                        .map(|pair| pair[1])
                        .filter(|setting| {
                            setting.starts_with("projects=") || setting.starts_with("projects.")
                        })
                        .collect();
                    check(
                        trust_overrides.len() == 1,
                        "Codex must receive exactly one projects trust override",
                    );
                    for setting in trust_overrides {
                        check(
                            setting.starts_with("projects={"),
                            "Codex project trust must use an inline table, never a dotted key",
                        );
                        let config: toml::Value = toml::from_str(setting).unwrap();
                        let projects = config["projects"].as_table().unwrap();
                        check(projects.len() == 1, "unrelated project was trusted");
                        check(
                            projects
                                .get(project.to_str().unwrap())
                                .and_then(|entry| entry.get("trust_level"))
                                .and_then(toml::Value::as_str)
                                == Some("trusted"),
                            "working directory trust is missing",
                        );
                    }
                }
                if adapted && agent == AgentType::Codex {
                    let mut options = HashSet::new();
                    let mut config_keys = HashSet::new();
                    let mut configs = Vec::new();
                    for (index, arg) in args.iter().enumerate() {
                        if *arg == "-c" {
                            let setting = args.get(index + 1).expect("-c needs a value");
                            let (key, _) = setting.split_once('=').expect("-c needs key=value");
                            check(
                                config_keys.insert(key),
                                &format!("duplicate Codex config key: {key}"),
                            );
                            configs.push(toml::from_str::<toml::Value>(setting).unwrap());
                        } else if arg.starts_with('-') && *arg != "-" {
                            let option = arg.split('=').next().unwrap();
                            check(
                                options.insert(option),
                                &format!("duplicate Codex option: {option}"),
                            );
                        }
                    }
                    for required in ["--json", "--skip-git-repo-check"] {
                        check(options.contains(required), &format!("missing {required}"));
                    }
                    check(
                        args.iter().filter(|arg| **arg == "exec").count() == 1,
                        "exec must occur once",
                    );
                    check(
                        args.iter().filter(|arg| **arg == "resume").count() == usize::from(resume),
                        "unexpected resume count",
                    );
                    if resume {
                        check(
                            args.windows(2)
                                .any(|p| p == ["resume", "11111111-1111-4111-8111-111111111111"]),
                            "resume lost the supplied thread id",
                        );
                    }
                    check(
                        options.contains("--sandbox") == (!read_only && !resume),
                        "sandbox flag must not override the read-only profile or a resumed policy",
                    );
                    let mcp: Vec<_> = configs
                        .iter()
                        .filter_map(|config| config.get("mcp_servers"))
                        .collect();
                    // A dotted env_vars override from the direct builder is
                    // distinct from the complete, project-scoped MCP table.
                    check(
                        config_keys.contains("mcp_servers"),
                        "complete MCP override missing",
                    );
                    let project_mcp: Vec<_> = mcp
                        .iter()
                        .filter(|servers| servers.get("project-safe").is_some())
                        .collect();
                    check(
                        project_mcp.len() == usize::from(!worker),
                        "project MCP registry must occur once for non-workers only",
                    );
                    if !worker {
                        let servers = project_mcp.first().expect("project MCP server missing");
                        check(
                            servers["project-safe"]["command"].as_str() == Some("safe-server"),
                            "project MCP command changed",
                        );
                        check(
                            servers["kronn-internal"]["command"].is_str(),
                            "internal MCP bridge missing",
                        );
                    }
                    if read_only {
                        check(
                            configs.iter().any(|config| {
                                config
                                    .get("default_permissions")
                                    .and_then(toml::Value::as_str)
                                    == Some("kronn_read_only_repos")
                            }),
                            "read-only permissions profile is not selected",
                        );
                        check(
                            configs.iter().any(|config| {
                                config
                                    .get("permissions")
                                    .and_then(|profiles| profiles.get("kronn_read_only_repos"))
                                    .is_some()
                            }),
                            "read-only permissions profile is not defined",
                        );
                    }
                }
                if adapted && agent == AgentType::ClaudeCode {
                    let registry = args
                        .windows(2)
                        .find(|p| p[0] == "--mcp-config")
                        .and_then(|p| serde_json::from_str::<serde_json::Value>(p[1]).ok());
                    check(
                        registry.as_ref().is_some_and(|value| {
                            value
                                .pointer("/mcpServers/kronn-internal/command")
                                .is_some()
                        }),
                        "the real generated internal bridge disappeared during authorization",
                    );
                }
                check(
                    observed[1] == "fixture-discussion",
                    "discussion context missing",
                );
                check(
                    observed[2] == project.join(".kronn/tmp").to_string_lossy(),
                    "temporary files escape worktree",
                );
                if worker {
                    check(
                        observed[0] == serde_json::to_string(&context).unwrap(),
                        "delivery context missing or inherited",
                    );
                    check(
                        observed[3] == "unset",
                        "permissive container marker inherited",
                    );
                    check(
                        !args.contains(&"--resume") && !args.contains(&"resume"),
                        "worker resumes prior conversation",
                    );
                    check(
                        !args.contains(&"--dangerously-skip-permissions")
                            && !args.contains(&"--sandbox=danger-full-access"),
                        "full-access overrides worker isolation",
                    );
                    if agent == AgentType::ClaudeCode {
                        check(
                            args.windows(2).any(|p| p == ["--setting-sources", ""]),
                            "global settings not disabled",
                        );
                        check(
                            args.contains(&"--strict-mcp-config"),
                            "MCP registry not isolated",
                        );
                        check(
                            args.windows(2)
                                .any(|p| p == ["--tools", "Bash,Edit,Read,Write,Glob,Grep"]),
                            "worker tool set widened",
                        );
                        for tool in ["task_exec_status", "task_exec_commit", "task_exec_deliver"] {
                            check(
                                args.contains(&format!("mcp__kronn-internal__{tool}").as_str()),
                                "delivery allowlist incomplete",
                            );
                        }
                    } else {
                        check(
                            args.contains(&"--ignore-user-config")
                                && args.contains(&"--ignore-rules"),
                            "global Codex configuration inherited",
                        );
                        check(
                            args.contains(&"--sandbox=workspace-write"),
                            "Codex worker sandbox missing",
                        );
                    }
                } else if read_only {
                    check(
                        !args.contains(&"--dangerously-skip-permissions")
                            && !args.iter().any(|arg| arg.starts_with("--sandbox=")),
                        "full access or container policy overrides read-only repositories",
                    );
                    if agent == AgentType::ClaudeCode {
                        check(
                            observed[3] == "unset",
                            "read-only Claude inherited sandbox bypass marker",
                        );
                        let settings: serde_json::Value = serde_json::from_str(
                            args[args.iter().position(|arg| *arg == "--settings").unwrap() + 1],
                        )
                        .unwrap();
                        check(
                            settings["sandbox"]["filesystem"]["denyWrite"]
                                == serde_json::json!([linked.canonicalize().unwrap()]),
                            "linked repo must be unwritable by subprocesses",
                        );
                        check(
                            settings["permissions"]["deny"]
                                .as_array()
                                .is_some_and(|rules| !rules.is_empty()),
                            "built-in edits must also be denied",
                        );
                        check(
                            args.windows(2).any(|p| {
                                p[0] == "--add-dir"
                                    && p[1] == linked.canonicalize().unwrap().to_str().unwrap()
                            }),
                            "linked repo read access missing",
                        );
                    } else {
                        check(
                            args.contains(&"approval_policy=\"never\""),
                            "Codex escalation must be disabled",
                        );
                        check(
                            args.contains(&"--ignore-user-config")
                                && args.contains(&"--strict-config"),
                            "Codex policy can be silently ignored",
                        );
                        check(
                            args.iter()
                                .any(|arg| arg.starts_with("permissions={kronn_read_only_repos=")),
                            "Codex read-only profile missing",
                        );
                        check(
                            !args.contains(&"--add-dir"),
                            "Codex linked repo became writable",
                        );
                    }
                } else {
                    check(
                        observed[0] == "unset",
                        "ordinary turn inherits another worker's capability",
                    );
                }
                count += 1;
            }
        }
    }
    assert_eq!(count, 22);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
