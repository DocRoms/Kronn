use super::*;
use std::collections::BTreeMap;

/// A backend environment holding everything a child must not see, plus what
/// it needs.
const PARENT: &[(&str, &str)] = &[
    ("PATH", "/usr/bin:/bin"),
    ("HOME", "/home/u"),
    ("LANG", "fr_FR.UTF-8"),
    ("LC_ALL", "fr_FR.UTF-8"),
    ("HTTPS_PROXY", "http://proxy:3128"),
    ("https_proxy", "http://proxy:3128"),
    ("SSL_CERT_FILE", "/etc/ca.pem"),
    ("KRONN_AUTH_TOKEN", "admin-bearer"),
    ("KRONN_ENCRYPTION_KEK", "raw-key"),
    ("KRONN_KEK", "raw-key"),
    ("KRONN_DATA_DIR", "/data"),
    ("ANTHROPIC_API_KEY", "sk-ant"),
    ("CLAUDE_CONFIG_DIR", "/home/u/.claude"),
    ("OPENAI_API_KEY", "sk-oai"),
    ("CODEX_HOME", "/home/u/.codex"),
    ("GEMINI_API_KEY", "gm"),
    ("MISTRAL_API_KEY", "ms"),
    ("GH_TOKEN", "ghp"),
    ("AZURE_CONFIG_DIR", "/home/u/.azure"),
    ("AZURE_CLIENT_SECRET", "az-secret"),
    ("DATABASE_PASSWORD", "pw"),
    ("RANDOM_SERVICE_TOKEN", "t"),
];

fn names(vars: &[(OsString, OsString)]) -> Vec<String> {
    vars.iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect()
}

fn parent() -> Vec<(OsString, OsString)> {
    PARENT
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

const EVERY_ROUTE: &[ChildRoute] = &[
    ChildRoute::Agent(AgentFamily::Claude),
    ChildRoute::Agent(AgentFamily::Codex),
    ChildRoute::Agent(AgentFamily::Gemini),
    ChildRoute::Agent(AgentFamily::Copilot),
    ChildRoute::Agent(AgentFamily::Kiro),
    ChildRoute::Agent(AgentFamily::Vibe),
    ChildRoute::Agent(AgentFamily::OpenCode),
    ChildRoute::Agent(AgentFamily::Other),
    ChildRoute::ProjectExec,
    ChildRoute::WorkflowExec,
    ChildRoute::QuickExec,
    ChildRoute::CredentialCli,
    ChildRoute::Git,
    ChildRoute::GitHost,
    ChildRoute::DependencyCheck,
    ChildRoute::Docker,
    ChildRoute::Tool,
];

/// Each Kronn-owned route keeps its program's own settings and nothing that
/// looks like a credential, whatever family it belongs to.
#[test]
fn kronn_owned_routes_keep_their_settings_and_no_credential() {
    let parent: Vec<(OsString, OsString)> = [
        ("PATH", "/usr/bin"),
        ("GH_CONFIG_DIR", "/home/u/.config/gh"),
        ("GLAB_CONFIG_DIR", "/home/u/.config/glab-cli"),
        ("GIT_AUTHOR_NAME", "U"),
        ("GITLAB_TOKEN", "glpat"),
        ("GH_TOKEN", "ghp"),
        ("GOPROXY", "https://proxy.golang.org"),
        ("COMPOSER_HOME", "/home/u/.composer"),
        ("NPM_TOKEN", "npm"),
        ("CARGO_REGISTRIES_ACME_TOKEN", "cargo"),
        ("DOCKER_HOST", "unix:///var/run/docker.sock"),
        ("COMPOSE_PROJECT_NAME", "p"),
        ("DOCKER_AUTH_TOKEN", "d"),
        ("DOCKER_PASSWORD", "d"),
        ("KRONN_AUTH_TOKEN", "admin"),
        ("ANTHROPIC_API_KEY", "sk-ant"),
    ]
    .iter()
    .map(|(name, value)| (OsString::from(name), OsString::from(value)))
    .collect();
    let expectations: &[(ChildRoute, &[&str])] = &[
        (
            ChildRoute::GitHost,
            &["GH_CONFIG_DIR", "GLAB_CONFIG_DIR", "GIT_AUTHOR_NAME"],
        ),
        (ChildRoute::DependencyCheck, &["GOPROXY", "COMPOSER_HOME"]),
        (ChildRoute::Docker, &["DOCKER_HOST", "COMPOSE_PROJECT_NAME"]),
        (ChildRoute::Tool, &[]),
    ];
    for (route, own) in expectations {
        let inherited = names(&inherited_from(*route, parent.clone()));
        let mut expected: Vec<&str> = own.to_vec();
        expected.push("PATH");
        let mut got: Vec<&str> = inherited.iter().map(String::as_str).collect();
        expected.sort_unstable();
        got.sort_unstable();
        assert_eq!(got, expected, "{route:?}");
    }
}

#[test]
fn no_route_inherits_the_admin_token_the_key_or_the_data_dir() {
    for route in EVERY_ROUTE {
        let inherited = names(&inherited_from(*route, parent()));
        for forbidden in [
            "KRONN_AUTH_TOKEN",
            "KRONN_ENCRYPTION_KEK",
            "KRONN_KEK",
            "KRONN_DATA_DIR",
            "DATABASE_PASSWORD",
            "RANDOM_SERVICE_TOKEN",
        ] {
            assert!(
                !inherited.iter().any(|name| name == forbidden),
                "{route:?} inherited {forbidden}"
            );
        }
        // The allow-list is kept on every route.
        for kept in [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "HTTPS_PROXY",
            "https_proxy",
            "SSL_CERT_FILE",
        ] {
            assert!(
                inherited.iter().any(|name| name == kept),
                "{route:?} lost {kept}"
            );
        }
    }
}

#[test]
fn a_provider_key_reaches_only_its_own_agent() {
    let expectations: &[(ChildRoute, &[&str])] = &[
        (
            ChildRoute::Agent(AgentFamily::Claude),
            &["ANTHROPIC_API_KEY", "CLAUDE_CONFIG_DIR"],
        ),
        (
            ChildRoute::Agent(AgentFamily::Codex),
            &["OPENAI_API_KEY", "CODEX_HOME"],
        ),
        (ChildRoute::Agent(AgentFamily::Gemini), &["GEMINI_API_KEY"]),
        (ChildRoute::Agent(AgentFamily::Vibe), &["MISTRAL_API_KEY"]),
        // GitHub variables come only from a connected project, never inherited.
        (ChildRoute::Agent(AgentFamily::Copilot), &[]),
        (ChildRoute::Agent(AgentFamily::OpenCode), &[]),
        (ChildRoute::ProjectExec, &[]),
        (ChildRoute::WorkflowExec, &[]),
        (ChildRoute::QuickExec, &[]),
    ];
    let keys = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "GEMINI_API_KEY",
        "MISTRAL_API_KEY",
        "GH_TOKEN",
        "CLAUDE_CONFIG_DIR",
        "CODEX_HOME",
    ];
    for (route, own) in expectations {
        let inherited = names(&inherited_from(*route, parent()));
        for key in keys {
            assert_eq!(
                inherited.iter().any(|name| name == key),
                own.contains(&key),
                "{route:?} and {key}"
            );
        }
    }
}

#[test]
fn a_credential_cli_keeps_its_cloud_configuration_only() {
    let inherited = names(&inherited_from(ChildRoute::CredentialCli, parent()));
    assert!(inherited.contains(&"AZURE_CONFIG_DIR".to_string()));
    assert!(inherited.contains(&"AZURE_CLIENT_SECRET".to_string()));
    assert!(!inherited.contains(&"ANTHROPIC_API_KEY".to_string()));

    let mut command = std::process::Command::new("az");
    with_parent_env(PARENT, || isolate(&mut command, ChildRoute::CredentialCli));
    let set = env_of(&command);
    assert!(set.contains_key("AZURE_CLIENT_SECRET"), "az reads it");
    assert!(!set.contains_key("KRONN_AUTH_TOKEN"));
}

fn env_of(command: &std::process::Command) -> BTreeMap<String, String> {
    command
        .get_envs()
        .filter_map(|(name, value)| {
            value.map(|value| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect()
}

#[test]
fn seal_drops_forbidden_names_and_other_agents_keys_whatever_set_them() {
    let mut command = std::process::Command::new("codex");
    command.env_clear();
    command
        .env("KRONN_AUTH_TOKEN", "x")
        .env("KRONN_ENCRYPTION_KEK", "x")
        .env("ANTHROPIC_API_KEY", "x")
        .env("OPENAI_API_KEY", "own")
        .env("KRONN_BRIDGE_TOKEN", "kbt_x")
        .env("KRONN_MCP_ABC_GITHUB_TOKEN", "mcp")
        .env("SOME_TOKEN", "x")
        .env("GH_TOKEN", "github-module")
        .env("KRONN_DISCUSSION_ID", "d1");
    seal(
        &mut command,
        ChildRoute::Agent(AgentFamily::Codex),
        &["KRONN_BRIDGE_TOKEN", "KRONN_MCP_*", "GH_TOKEN"],
    );
    let set = env_of(&command);
    for gone in [
        "KRONN_AUTH_TOKEN",
        "KRONN_ENCRYPTION_KEK",
        "ANTHROPIC_API_KEY",
        "SOME_TOKEN",
    ] {
        assert!(!set.contains_key(gone), "{gone} survived the seal");
    }
    for kept in [
        "OPENAI_API_KEY",
        "KRONN_BRIDGE_TOKEN",
        "KRONN_MCP_ABC_GITHUB_TOKEN",
        "GH_TOKEN",
        "KRONN_DISCUSSION_ID",
    ] {
        assert!(set.contains_key(kept), "{kept} was dropped");
    }
    // A grant never lifts the forbidden list.
    let mut command = std::process::Command::new("x");
    command.env_clear().env("KRONN_AUTH_TOKEN", "x");
    seal(&mut command, ChildRoute::ProjectExec, &["KRONN_AUTH_TOKEN"]);
    assert!(env_of(&command).is_empty());
}

#[test]
fn an_agent_launch_gets_its_contexts_its_token_and_only_its_key() {
    let work = tempfile::tempdir().unwrap();
    let room = RoomAgentBridgeContext {
        discussion_id: "room-1".into(),
        agent_type: "GeminiCli".into(),
        dispatch_job_id: "job".into(),
        source_message_id: "m".into(),
    };
    let launch = AgentLaunch {
        discussion_id: Some("room-1"),
        room_agent: Some(&room),
        bridge_token: Some("kbt_launch"),
        api_key: Some(("GEMINI_API_KEY", "configured-in-kronn")),
        ..Default::default()
    };
    let route = ChildRoute::Agent(AgentFamily::Gemini);
    let mut command = std::process::Command::new("gemini");
    with_parent_env(PARENT, || {
        reset(&mut command, route);
        apply_agent_launch(
            &mut command,
            work.path(),
            &launch,
            Some("http://127.0.0.1:3140".into()),
        )
        .unwrap();
        seal(&mut command, route, &launch.granted());
    });
    let set = env_of(&command);
    assert_eq!(set["GEMINI_API_KEY"], "configured-in-kronn");
    assert_eq!(set["KRONN_DISCUSSION_ID"], "room-1");
    assert_eq!(set["KRONN_BRIDGE_TOKEN"], "kbt_launch");
    assert!(set["KRONN_ROOM_AGENT_CONTEXT"].contains("room-1"));
    assert!(set["TMPDIR"].ends_with("tmp"));
    assert!(!set.contains_key("KRONN_AUTH_TOKEN"));
    assert!(!set.contains_key("OPENAI_API_KEY"));
    assert!(!set.contains_key("ANTHROPIC_API_KEY"));
}

#[test]
fn a_worker_launch_carries_no_room_or_step_identity() {
    let work = tempfile::tempdir().unwrap();
    let worker = TaskWorkerBridgeContext {
        execution_id: "exec-1".into(),
        discussion_id: "d1".into(),
        agent_type: "ClaudeCode".into(),
        dispatch_job_id: "j".into(),
        source_message_id: "m".into(),
    };
    let room = RoomAgentBridgeContext {
        discussion_id: "d1".into(),
        agent_type: "ClaudeCode".into(),
        dispatch_job_id: "j".into(),
        source_message_id: "m".into(),
    };
    let mut command = std::process::Command::new("claude");
    command.env_clear();
    let launch = AgentLaunch {
        task_worker: Some(&worker),
        room_agent: Some(&room),
        ..Default::default()
    };
    apply_agent_launch(&mut command, work.path(), &launch, None).unwrap();
    let set = env_of(&command);
    assert!(set["KRONN_TASK_WORKER_CONTEXT"].contains("exec-1"));
    assert!(!set.contains_key("KRONN_ROOM_AGENT_CONTEXT"));
    assert!(!set.contains_key("KRONN_BACKEND_URL"));
}

#[test]
fn wslenv_still_lists_every_injected_name() {
    let work = tempfile::tempdir().unwrap();
    let route = ChildRoute::Agent(AgentFamily::Claude);
    let launch = AgentLaunch {
        discussion_id: Some("d1"),
        bridge_token: Some("kbt_x"),
        api_key: Some(("ANTHROPIC_API_KEY", "k")),
        ..Default::default()
    };
    let mut command = tokio::process::Command::new("wsl.exe");
    let inherited = with_parent_env(
        &[
            ("PATH", "/bin"),
            ("SystemRoot", r"C:\Windows"),
            ("TMPDIR", r"C:\Temp"),
            ("CLAUDE_CONFIG_DIR", r"C:\Users\u\.claude"),
        ],
        || {
            reset(command.as_std_mut(), route);
            let inherited = names_of(command.as_std());
            apply_agent_launch(
                command.as_std_mut(),
                work.path(),
                &launch,
                Some("http://172.20.0.1:3140".into()),
            )
            .unwrap();
            seal(command.as_std_mut(), route, &launch.granted());
            inherited
        },
    );
    let granted = launch.granted();
    crate::agents::wsl::apply_wslenv(&mut command, |name| {
        forwarded_into_wsl(name, &inherited, &granted)
    });
    let wslenv = env_of(command.as_std())["WSLENV"].clone();
    let entries: Vec<&str> = wslenv.split(':').collect();
    for expected in [
        "KRONN_DISCUSSION_ID",
        "KRONN_BACKEND_URL",
        "KRONN_BRIDGE_TOKEN",
        "ANTHROPIC_API_KEY",
        "TMPDIR/up",
    ] {
        assert!(entries.contains(&expected), "{expected} missing: {wslenv}");
    }
    // Inherited from Windows and untouched: stays on the Windows side.
    for absent in ["PATH", "SystemRoot", "CLAUDE_CONFIG_DIR"] {
        assert!(
            !entries.iter().any(|entry| entry.starts_with(absent)),
            "{absent} forwarded: {wslenv}"
        );
    }
}

#[test]
fn secret_looking_names_are_recognised() {
    for name in [
        "GH_TOKEN",
        "DB_PASSWORD",
        "X_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
        "KRONN_KEK",
        "NPM_CONFIG__AUTH",
        "npm_config__auth",
        "npm_config_//registry.example.com/:_auth",
        "NGROK_AUTHTOKEN",
        "npm_config__password",
    ] {
        assert!(looks_secret(name), "{name}");
    }
    for name in [
        "PATH",
        "HOME",
        "KRONN_DISCUSSION_ID",
        "LANG",
        "SSH_AUTH_SOCK",
        "GIT_AUTHOR_NAME",
        "XAUTHORITY",
        "NPM_CONFIG_REGISTRY",
    ] {
        assert!(!looks_secret(name), "{name}");
    }
}

/// npm's registry credentials ride on an allow-listed prefix: every route
/// drops them all the same (B3-09).
#[test]
fn npm_registry_credentials_reach_no_route() {
    let parent: &[(&str, &str)] = &[
        ("PATH", "/usr/bin"),
        ("NPM_CONFIG_REGISTRY", "https://registry.example.com/"),
        ("NPM_CONFIG__AUTH", "dXNlcjpwYXNz"),
        ("npm_config__auth", "dXNlcjpwYXNz"),
        ("npm_config_//registry.example.com/:_auth", "dXNlcjpwYXNz"),
        ("npm_config_//registry.example.com/:_authToken", "npm-token"),
        ("npm_config__password", "cGFzcw=="),
    ];
    for route in EVERY_ROUTE {
        let mut command = std::process::Command::new("node");
        with_parent_env(parent, || isolate(&mut command, *route));
        let names: Vec<String> = env_of(&command).into_keys().collect();
        assert_eq!(
            names,
            vec!["NPM_CONFIG_REGISTRY".to_string(), "PATH".to_string()],
            "{route:?}"
        );
    }
}

#[test]
fn agent_families_are_recognised_from_a_launch() {
    assert_eq!(
        AgentFamily::from_launch("claude", None, ""),
        AgentFamily::Claude
    );
    assert_eq!(
        AgentFamily::from_launch("/opt/bin/claude", None, ""),
        AgentFamily::Claude
    );
    assert_eq!(
        AgentFamily::from_launch("npx", Some("@openai/codex"), ""),
        AgentFamily::Codex
    );
    assert_eq!(
        AgentFamily::from_launch("python3", None, "MISTRAL_API_KEY"),
        AgentFamily::Vibe
    );
    assert_eq!(AgentFamily::from_launch("sh", None, ""), AgentFamily::Other);
}

/// The direct route end to end: the spawned process's real environment.
#[cfg(unix)]
#[tokio::test]
async fn a_direct_launch_spawns_with_the_built_environment_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for program in ["claude", "codex"] {
        let path = dir.path().join(program);
        std::fs::write(&path, "#!/bin/sh\n/usr/bin/env\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (env_key, key) = if program == "claude" {
            ("ANTHROPIC_API_KEY", Some("vault-key"))
        } else {
            ("OPENAI_API_KEY", None)
        };
        let child = with_parent_env(PARENT, || {
            crate::agents::runner::try_spawn(
                path.to_str().unwrap(),
                None,
                &[],
                dir.path(),
                env_key,
                key,
                crate::agents::runner::SpawnIo::Direct(None),
                Some("disc-1"),
                None,
                None,
                None,
                &[],
                Some("kbt_direct"),
            )
            .unwrap()
        });
        let output = child.wait_with_output().await.unwrap();
        let env: BTreeMap<String, String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect();
        for forbidden in [
            "KRONN_AUTH_TOKEN",
            "KRONN_ENCRYPTION_KEK",
            "KRONN_KEK",
            "KRONN_DATA_DIR",
        ] {
            assert!(
                !env.contains_key(forbidden),
                "{program} received {forbidden}"
            );
        }
        assert_eq!(
            env.get("KRONN_BRIDGE_TOKEN").map(String::as_str),
            Some("kbt_direct")
        );
        assert_eq!(
            env.get("KRONN_DISCUSSION_ID").map(String::as_str),
            Some("disc-1")
        );
        if program == "claude" {
            assert_eq!(
                env.get("ANTHROPIC_API_KEY").map(String::as_str),
                Some("vault-key")
            );
            assert!(
                !env.contains_key("OPENAI_API_KEY"),
                "claude received Codex's key"
            );
        } else {
            assert_eq!(
                env.get("OPENAI_API_KEY").map(String::as_str),
                Some("sk-oai")
            );
            assert!(
                !env.contains_key("ANTHROPIC_API_KEY"),
                "codex received Claude's key"
            );
        }
    }
}

/// The adapter route: whatever the adapter's own launch env carries, the seal
/// runs after it, so the admin token and another agent's key never pass.
#[cfg(unix)]
#[tokio::test]
async fn an_adapter_launch_env_cannot_reinject_forbidden_names() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex");
    std::fs::write(&path, "#!/bin/sh\n/usr/bin/env\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let adapter_env = vec![
        ("KRONN_AUTH_TOKEN".to_string(), "admin".to_string()),
        ("ANTHROPIC_API_KEY".to_string(), "sk-ant".to_string()),
        ("KRONN_MCP_ABC_TOKEN".to_string(), "mcp-ref".to_string()),
        ("ENABLE_TOOL_SEARCH".to_string(), "false".to_string()),
    ];
    let mut child = with_parent_env(&[("PATH", "/usr/bin:/bin")], || {
        crate::agents::runner::try_spawn(
            path.to_str().unwrap(),
            None,
            &[],
            dir.path(),
            "OPENAI_API_KEY",
            None,
            crate::agents::runner::SpawnIo::Adapter(&adapter_env),
            None,
            None,
            None,
            None,
            &[],
            Some("kbt_adapter"),
        )
        .unwrap()
    });
    drop(child.stdin.take());
    let output = child.wait_with_output().await.unwrap();
    let env: BTreeMap<String, String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .collect();
    assert!(!env.contains_key("KRONN_AUTH_TOKEN"));
    assert!(!env.contains_key("ANTHROPIC_API_KEY"));
    assert_eq!(
        env.get("KRONN_MCP_ABC_TOKEN").map(String::as_str),
        Some("mcp-ref")
    );
    assert_eq!(
        env.get("ENABLE_TOOL_SEARCH").map(String::as_str),
        Some("false")
    );
    assert_eq!(
        env.get("KRONN_BRIDGE_TOKEN").map(String::as_str),
        Some("kbt_adapter")
    );
}

#[test]
fn an_approved_script_step_gets_its_launch_values_on_the_built_environment() {
    with_parent_env(PARENT, || {
        let mut command = std::process::Command::new("node");
        isolate_with_github_and_values(
            &mut command,
            ChildRoute::WorkflowExec,
            &[],
            &[
                ("KRONN_WORKTREE", OsStr::new("/repo/wt")),
                ("KRONN_APPROVED_SCRIPTS_DIR", OsStr::new("/data/copy")),
                ("SOME_TOKEN", OsStr::new("not granted")),
            ],
        );
        let set = env_of(&command);
        assert_eq!(
            set.get("KRONN_WORKTREE").map(String::as_str),
            Some("/repo/wt")
        );
        assert_eq!(
            set.get("KRONN_APPROVED_SCRIPTS_DIR").map(String::as_str),
            Some("/data/copy")
        );
        assert_eq!(set.get("PATH").map(String::as_str), Some("/usr/bin:/bin"));
        for gone in [
            "SOME_TOKEN",
            "KRONN_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "GH_TOKEN",
        ] {
            assert!(!set.contains_key(gone), "{gone} reached the step");
        }
    });
}

/// What the desktop keeps live for the webview helpers: an allow-list, so a
/// credential under any name, and a name that is not Unicode, is withheld
/// (B7-01), while Kronn's own settings stay (B7-02).
#[test]
fn the_webview_keeps_an_allow_list_only() {
    for kept in [
        "PATH",
        "HOME",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XDG_RUNTIME_DIR",
        "GDK_BACKEND",
        "WEBKIT_DISABLE_COMPOSITING_MODE",
        "WEBVIEW2_USER_DATA_FOLDER",
        "RUST_LOG",
        "KRONN_DATA_DIR",
        "KRONN_USE_KEYCHAIN",
        "KRONN_MCP_SECRET_REFERENCES",
    ] {
        assert!(webview_keeps(OsStr::new(kept)), "{kept}");
    }
    for withheld in [
        "MYSQL_PWD",
        "DEPLOY_PASSPHRASE",
        "SENTRY_DSN",
        "DATABASE_URL",
        "REDIS_URL",
        "BW_SESSION",
        "OP_SESSION_acme",
        "ANTHROPIC_API_KEY",
        "GH_TOKEN",
        "KRONN_BRIDGE_TOKEN",
        "KRONN_AUTH_TOKEN",
        "OLLAMA_HOST",
    ] {
        assert!(!webview_keeps(OsStr::new(withheld)), "{withheld}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(!webview_keeps(OsStr::from_bytes(b"X_TOKEN\xff")));
    }
}

/// Withheld variables leave the process environment but still reach the
/// builder and Kronn's own reads (B7-01, B7-02).
#[test]
#[serial_test::serial]
fn withheld_variables_leave_the_process_but_reach_kronn() {
    let sentinels = [
        ("MYSQL_PWD", "sentinel-mysql"),
        ("DEPLOY_PASSPHRASE", "sentinel-passphrase"),
        ("SENTRY_DSN", "https://key@sentry.example/1"),
        ("KRONN_WITHHOLD_SENTINEL_API_KEY", "sentinel"),
        ("KRONN_USE_KEYCHAIN", "1"),
    ];
    for (name, value) in sentinels {
        std::env::set_var(name, value);
    }
    let names: Vec<&str> = sentinels.iter().map(|(name, _)| *name).collect();
    let held = take_process_environment_where(|name| {
        !names.iter().any(|sentinel| name == *sentinel) || webview_keeps(name)
    });
    for (name, value) in sentinels {
        let live = std::env::var_os(name);
        if name == "KRONN_USE_KEYCHAIN" {
            assert_eq!(live.as_deref(), Some(OsStr::new(value)), "{name}");
        } else {
            assert!(live.is_none(), "{name} stayed live");
            let value_held = held
                .iter()
                .find_map(|(key, held)| (key == name).then_some(held));
            assert_eq!(value_held.map(|v| v.to_str().unwrap()), Some(value));
        }
    }
    // The keychain switch the error message recommends still works.
    assert!(crate::core::keyvault::use_os_keychain());
    for (name, _) in sentinels {
        std::env::remove_var(name);
    }

    let live = vec![(OsString::from("PATH"), OsString::from("/usr/bin"))];
    let withheld = vec![
        (
            OsString::from("ANTHROPIC_API_KEY"),
            OsString::from("sk-ant"),
        ),
        (OsString::from("MYSQL_PWD"), OsString::from("pw")),
    ];
    let parent = with_withheld(live, &withheld);
    let claude = names_of_vars(&inherited_from(
        ChildRoute::Agent(AgentFamily::Claude),
        parent.clone(),
    ));
    assert!(claude.contains(&"ANTHROPIC_API_KEY".to_string()));
    assert!(!claude.contains(&"MYSQL_PWD".to_string()));
    let tool = names_of_vars(&inherited_from(ChildRoute::Tool, parent));
    assert!(!tool.contains(&"ANTHROPIC_API_KEY".to_string()));
}

fn names_of_vars(vars: &[(OsString, OsString)]) -> Vec<String> {
    names(vars)
}

/// Windows compares environment names without case: a key exported in
/// another case is still found (B7-04).
#[test]
fn environment_names_compare_like_the_platform() {
    let parent = with_withheld(
        vec![(OsString::from("PATH"), OsString::from("/usr/bin"))],
        &[(OsString::from("Gh_Token"), OsString::from("x"))],
    );
    let found = with_parent_env(&[("Gh_Token", "x")], || parent_var_string("GH_TOKEN"));
    if cfg!(windows) {
        assert_eq!(found.as_deref(), Some("x"));
        assert!(same_name(OsStr::new("Gh_Token"), OsStr::new("GH_TOKEN")));
        // No duplicate when the live environment has another case.
        let merged = with_withheld(
            vec![(OsString::from("gh_token"), OsString::from("live"))],
            &[(OsString::from("GH_TOKEN"), OsString::from("held"))],
        );
        assert_eq!(merged.len(), 1);
    } else {
        assert_eq!(found, None);
        assert!(!same_name(OsStr::new("Gh_Token"), OsStr::new("GH_TOKEN")));
    }
    assert_eq!(parent.len(), 2);
}

/// The desktop withholds its environment before Tauri starts its webview.
#[test]
fn the_desktop_withholds_its_environment_before_the_webview_starts() {
    let main = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../desktop/src-tauri/src/main.rs"),
    )
    .unwrap();
    let withhold = main
        .find("child_env::withhold_process_environment()")
        .expect("the desktop never withholds its environment");
    assert!(withhold < main.find("tauri::Builder::default()").unwrap());
    assert!(withhold < main.find("tracing_subscriber::").unwrap());
}
