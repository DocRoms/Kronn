//! KT-910 / KT-908: workflow Agent launch policies, read on the argv the
//! production adapters pass to a fake CLI.
#![cfg(unix)]

use std::{ffi::OsString, os::unix::fs::PermissionsExt, path::Path, time::Duration};

use kronn::agents::runner::{start_agent_with_config, AgentStartConfig};
use kronn::models::AgentType;

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn set(&mut self, name: &'static str, value: impl Into<OsString>) {
        if !self.0.iter().any(|(key, _)| *key == name) {
            self.0.push((name, kronn::core::child_env::var_os(name)));
        }
        // This integration binary deliberately has one current-thread test.
        unsafe { std::env::set_var(name, value.into()) }
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

struct Fixture {
    dir: tempfile::TempDir,
    project: std::path::PathBuf,
    env: Environment,
    count: usize,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for name in ["bin", "host", "host-bin", "data", "project"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        let bin = dir.path().join("bin");
        let project = dir.path().join("project");
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
        for binary in ["claude", "codex"] {
            let file = bin.join(binary);
            std::fs::write(&file, r#"#!/bin/sh
if [ "$1" = auth ]; then
  printf '%s\n' '{"loggedIn":true}'
  exit 0
fi
# Agents get a built environment: the fixture's paths ride in names each owns.
KRONN_POLICY_ARGV="${CLAUDE_POLICY_ARGV:-$CODEX_POLICY_ARGV}"
printf '%s\n' "$@" > "$KRONN_POLICY_ARGV"
printf '%s\n' "${ENABLE_TOOL_SEARCH-unset}" > "$KRONN_POLICY_ARGV.env"
cat >/dev/null
case "$0" in
  *claude) printf '%s\n' '{"type":"result","subtype":"success","result":"fixture"}' ;;
  *codex) printf '%s\n' '{"type":"thread.started","thread_id":"11111111-1111-4111-8111-111111111111"}' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}' ;;
esac
"#).unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            dir,
            project,
            env,
            count: 0,
        }
    }

    async fn launch(&mut self, agent: AgentType, options: &LaunchOptions) -> Vec<String> {
        let argv = self.dir.path().join(format!("argv-{}", self.count));
        self.count += 1;
        self.env.set("CLAUDE_POLICY_ARGV", argv.clone());
        self.env.set("CODEX_POLICY_ARGV", argv.clone());
        let base = kronn::core::config::default_config();
        let project = self.project.to_string_lossy().to_string();
        let config = AgentStartConfig {
            full_access: true,
            discussion_id: Some("fixture-discussion"),
            mcp_context_override: Some(""),
            read_only_dirs: &options.read_only_dirs,
            step_tools: options.step_tools.as_ref(),
            ..AgentStartConfig::new(&agent, &project, "fixture prompt", &base.tokens)
        };
        let mut process =
            tokio::time::timeout(Duration::from_secs(10), start_agent_with_config(config))
                .await
                .unwrap()
                .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while process.next_line().await.is_some() {}
            process.child.wait().await.unwrap()
        })
        .await
        .expect("fake CLI completes");
        std::fs::read_to_string(argv)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[derive(Default)]
struct LaunchOptions {
    read_only_dirs: Vec<String>,
    step_tools: Option<kronn::models::StepTools>,
}

fn last_env(fixture: &Fixture) -> String {
    let argv = fixture
        .dir
        .path()
        .join(format!("argv-{}.env", fixture.count - 1));
    std::fs::read_to_string(argv).unwrap().trim().to_string()
}

fn mcp_config(args: &[String]) -> serde_json::Value {
    serde_json::from_str(value_after(args, "--mcp-config").expect("--mcp-config")).unwrap()
}

/// Every value following `--allowedTools`, up to the next flag.
fn allowed_tools(args: &[String]) -> Vec<String> {
    let Some(start) = args.iter().position(|arg| arg == "--allowedTools") else {
        return Vec::new();
    };
    args[start + 1..]
        .iter()
        .take_while(|arg| !arg.starts_with("--"))
        .cloned()
        .collect()
}

fn codex_mcp(args: &[String]) -> toml::Value {
    let setting = args
        .windows(2)
        .filter(|pair| pair[0] == "-c")
        .map(|pair| pair[1].as_str())
        .find(|setting| setting.starts_with("mcp_servers="))
        .expect("Codex MCP override");
    toml::from_str(setting).unwrap()
}

async fn declared_tools_replace_the_catalogue(fixture: &mut Fixture) {
    // Undeclared: exactly today's adapter argv.
    let undeclared = fixture
        .launch(AgentType::ClaudeCode, &LaunchOptions::default())
        .await;
    for absent in ["--tools", "--disable-slash-commands"] {
        assert!(!undeclared.contains(&absent.to_string()), "{undeclared:?}");
    }
    let registry = mcp_config(&undeclared);
    assert!(registry.pointer("/mcpServers/project-safe").is_some());
    let bridge_args = registry
        .pointer("/mcpServers/kronn-internal/args")
        .and_then(serde_json::Value::as_array)
        .expect("bridge");
    assert!(!bridge_args.iter().any(|arg| arg
        .as_str()
        .is_some_and(|arg| arg.starts_with("--step-tools"))));
    assert!(undeclared.contains(&"--strict-mcp-config".to_string()));
    assert!(undeclared.contains(&"--dangerously-skip-permissions".to_string()));
    let mut granted = allowed_tools(&undeclared);
    granted.sort();
    assert_eq!(
        granted,
        vec!["mcp__kronn-internal__*", "mcp__project-safe__*"]
    );
    assert_eq!(last_env(fixture), "unset");

    // tools: none — no built-in tool, no MCP server, no skill listing.
    let none = LaunchOptions {
        step_tools: Some(kronn::models::StepTools::default()),
        ..LaunchOptions::default()
    };
    let bare = fixture.launch(AgentType::ClaudeCode, &none).await;
    assert_eq!(value_after(&bare, "--tools"), Some(""));
    assert!(bare.contains(&"--disable-slash-commands".to_string()));
    assert_eq!(mcp_config(&bare), serde_json::json!({"mcpServers": {}}));
    assert!(bare.contains(&"--strict-mcp-config".to_string()));
    assert!(allowed_tools(&bare).is_empty(), "{bare:?}");

    // Declared Kronn tools: those alone, loaded up front.
    let declared = LaunchOptions {
        step_tools: Some(kronn::models::StepTools {
            cli: vec!["Read".into(), "Grep".into()],
            kronn_internal: vec!["task_get".into(), "plan_get".into()],
        }),
        ..LaunchOptions::default()
    };
    let narrowed = fixture.launch(AgentType::ClaudeCode, &declared).await;
    assert_eq!(value_after(&narrowed, "--tools"), Some("Read,Grep"));
    let registry = mcp_config(&narrowed);
    let servers = registry["mcpServers"].as_object().unwrap();
    assert_eq!(servers.keys().collect::<Vec<_>>(), vec!["kronn-internal"]);
    assert_eq!(
        servers["kronn-internal"]["args"]
            .as_array()
            .and_then(|args| args.last())
            .and_then(serde_json::Value::as_str),
        Some("--step-tools=task_get,plan_get")
    );
    assert_eq!(
        allowed_tools(&narrowed),
        vec![
            "mcp__kronn-internal__task_get",
            "mcp__kronn-internal__plan_get"
        ]
    );
    assert_eq!(last_env(fixture), "false", "schemas loaded, not deferred");

    // Codex: the same narrowing in its MCP override.
    let codex_undeclared = codex_mcp(
        &fixture
            .launch(AgentType::Codex, &LaunchOptions::default())
            .await,
    );
    assert!(codex_undeclared["mcp_servers"]
        .get("project-safe")
        .is_some());
    assert!(codex_undeclared["mcp_servers"]["kronn-internal"]
        .get("enabled_tools")
        .is_none());
    let codex_none = codex_mcp(&fixture.launch(AgentType::Codex, &none).await);
    assert_eq!(
        codex_none["mcp_servers"]
            .as_table()
            .map(|table| table.len()),
        Some(0)
    );
    let declared_codex = LaunchOptions {
        step_tools: Some(kronn::models::StepTools {
            cli: vec![],
            kronn_internal: vec!["task_get".into()],
        }),
        ..LaunchOptions::default()
    };
    let codex_narrowed = codex_mcp(&fixture.launch(AgentType::Codex, &declared_codex).await);
    let servers = codex_narrowed["mcp_servers"].as_table().unwrap();
    assert_eq!(servers.keys().collect::<Vec<_>>(), vec!["kronn-internal"]);
    assert_eq!(
        servers["kronn-internal"]["enabled_tools"],
        toml::Value::Array(vec![toml::Value::String("task_get".into())])
    );
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| pair[1].as_str())
}

async fn artifacts_directory_is_read_only(fixture: &mut Fixture) {
    let artifacts = fixture.dir.path().join("run artifacts");
    std::fs::create_dir(&artifacts).unwrap();
    std::fs::write(artifacts.join("S1.png"), [0x89, b'P', b'N', b'G']).unwrap();
    let artifacts = artifacts.canonicalize().unwrap();
    let dirs = vec![artifacts.to_string_lossy().to_string()];
    let options = LaunchOptions {
        read_only_dirs: dirs.clone(),
        ..LaunchOptions::default()
    };

    let claude = fixture.launch(AgentType::ClaudeCode, &options).await;
    assert!(
        claude.contains(&"--session-id".to_string()),
        "adapter route: {claude:?}"
    );
    assert_eq!(value_after(&claude, "--add-dir"), artifacts.to_str());
    assert!(!claude.contains(&"--dangerously-skip-permissions".to_string()));
    let settings: serde_json::Value =
        serde_json::from_str(value_after(&claude, "--settings").unwrap()).unwrap();
    assert_eq!(
        settings["permissions"]["deny"],
        serde_json::json!([format!("Edit(/{}/**)", artifacts.display())])
    );
    assert_eq!(
        settings["sandbox"]["filesystem"]["denyWrite"],
        serde_json::json!([artifacts])
    );
    assert_eq!(settings["sandbox"]["failIfUnavailable"], true);

    let codex = fixture.launch(AgentType::Codex, &options).await;
    let profile = codex
        .iter()
        .find(|arg| arg.starts_with("permissions={kronn_read_only_repos="))
        .expect("read-only profile");
    assert!(
        profile.contains(&format!(
            "{}=\"read\"",
            serde_json::to_string(&artifacts).unwrap()
        )),
        "{profile}"
    );
    assert!(
        !codex.contains(&"--add-dir".to_string()),
        "a write grant in Codex"
    );
    assert!(!codex.iter().any(|arg| arg.starts_with("--sandbox=")));
    assert!(
        Path::new(&dirs[0]).join("S1.png").is_file(),
        "nothing was removed"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn workflow_agent_launch_policies_reach_the_adapter_argv() {
    let mut fixture = Fixture::new();
    artifacts_directory_is_read_only(&mut fixture).await;
    declared_tools_replace_the_catalogue(&mut fixture).await;
}
