//! What a Vibe session without full access runs under (mistral-vibe 2.25).
//!
//! Vibe approves some calls itself, without asking Kronn: its default agent
//! profile `accept-edits` sets `write_file`/`edit` to `always`, the bash tool
//! runs its `allowlist` (read-only defaults plus the user's additions) unasked,
//! and a user or project `config.toml` may set any tool to `always`. Vibe's
//! config layers, lowest first: defaults, GrowthBook, user TOML, project TOML,
//! `VIBE_*` environment, overrides, the agent profile, admin
//! (`core/config/default_orchestrator.py`). `tools` is deep-merged and a list
//! inside it is replaced, not appended (`core/utils/merge.py`, `_deep_merge`).
//!
//! So Kronn selects its own agent profile, whose overrides sit above the user's
//! and the repository's config: every tool asks, allowlists are emptied,
//! bypass is off. The profile lives in a per-launch directory searched before
//! the project's and the user's agent directories, under a per-launch name, so
//! no other profile file can stand in for it (`core/agents/registry.py`: the
//! first file of a non-builtin name wins). An unknown or excluded default agent
//! stops Vibe (`core/agents/manager.py`), so a failure here fails closed.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Built-in tools that only read or report; their `allowlist` is still emptied
/// (`read_file`'s grants reads outside the project).
const READ_ONLY_TOOLS: &[&str] = &[
    "ask_user_question",
    "bash_output",
    "bash_sessions",
    "git_bash_output",
    "git_bash_sessions",
    "grep",
    "powershell_output",
    "powershell_sessions",
    "read_file",
    "skill",
    "todo",
];

/// Built-in tools that act; each is set to `ask`.
const ACTING_TOOLS: &[&str] = &[
    "bash",
    "bash_log_file",
    "bash_stdin",
    "edit",
    "git_bash",
    "git_bash_log_file",
    "git_bash_stdin",
    "powershell",
    "powershell_log_file",
    "powershell_stdin",
    "task",
    "web_fetch",
    "web_search",
    "write_file",
];

/// The launch environment and the profile directory to remove afterwards.
#[derive(Debug)]
pub(crate) struct VibeLaunchPolicy {
    pub dir: PathBuf,
    pub env: Vec<(&'static str, String)>,
}

/// Write the per-launch profile under `cwd/.kronn/tmp` and return the
/// environment that selects it. `vibe_home` is where Vibe reads the user's
/// config (`VIBE_HOME`, else `~/.vibe`).
pub(crate) fn prepare(
    cwd: &Path,
    vibe_home: &Path,
    suffix: &str,
) -> Result<VibeLaunchPolicy, String> {
    let name = format!("kronn-{suffix}");
    let dir = cwd.join(".kronn/tmp").join(format!("vibe-agent-{suffix}"));
    let configs = config_files(cwd, vibe_home);
    let profile = profile_toml(&configs);
    std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(dir.join(format!("{name}.toml")), profile))
        .map_err(|error| {
            format!("cannot write Kronn's Vibe agent profile, Vibe is not started: {error}")
        })?;
    let list = |value: &str| serde_json::json!([value]).to_string();
    Ok(VibeLaunchPolicy {
        env: vec![
            ("VIBE_DEFAULT_AGENT", name.clone()),
            ("VIBE_ENABLED_AGENTS", list(&name)),
            ("VIBE_AGENT_PATHS", list(&dir.to_string_lossy())),
            ("VIBE_SMART_APPROVE_DEFAULT", "false".into()),
            ("VIBE_BYPASS_TOOL_PERMISSIONS", "false".into()),
        ],
        dir,
    })
}

/// The user's config and the project config Vibe would find from `cwd`
/// (walking up, stopping at `vibe_home`'s parent like
/// `core/config/layers/project.py`), parsed. Unreadable files are skipped:
/// the built-in lists still apply.
fn config_files(cwd: &Path, vibe_home: &Path) -> Vec<toml::Table> {
    let stop = vibe_home.parent();
    let project = cwd
        .ancestors()
        .take_while(|directory| Some(*directory) != stop)
        .map(|directory| directory.join(".vibe/config.toml"))
        .find(|candidate| candidate.is_file());
    [Some(vibe_home.join("config.toml")), project]
        .into_iter()
        .flatten()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| text.parse::<toml::Table>().ok())
        .collect()
}

/// The agent profile: every acting tool and every tool another config names
/// asks, every allowlist is empty, and a `kronn-internal` server declared in
/// those configs (Kronn's bridge without its launch environment) is disabled:
/// the session's bridge comes under its per-launch name.
pub(crate) fn profile_toml(configs: &[toml::Table]) -> String {
    let mut named: BTreeSet<String> = ACTING_TOOLS.iter().map(|tool| tool.to_string()).collect();
    for config in configs {
        if let Some(tools) = config.get("tools").and_then(toml::Value::as_table) {
            named.extend(tools.keys().cloned());
        }
    }
    let mut tools = toml::Table::new();
    for tool in READ_ONLY_TOOLS {
        let mut entry = toml::Table::new();
        entry.insert("allowlist".into(), toml::Value::Array(Vec::new()));
        tools.insert(tool.to_string(), toml::Value::Table(entry));
    }
    for tool in named
        .iter()
        .filter(|tool| !READ_ONLY_TOOLS.contains(&tool.as_str()))
    {
        let mut entry = toml::Table::new();
        entry.insert("permission".into(), "ask".into());
        entry.insert("allowlist".into(), toml::Value::Array(Vec::new()));
        tools.insert(tool.clone(), toml::Value::Table(entry));
    }
    let mut profile = toml::Table::new();
    profile.insert("display_name".into(), "Kronn".into());
    profile.insert(
        "description".into(),
        "Kronn session without full access: every acting tool asks Kronn".into(),
    );
    profile.insert("bypass_tool_permissions".into(), false.into());
    profile.insert(
        "disabled_tools".into(),
        toml::Value::Array(vec!["exit_plan_mode".into()]),
    );
    profile.insert("tools".into(), toml::Value::Table(tools));
    let disabled_bridges: Vec<toml::Value> = configs
        .iter()
        .filter_map(|config| config.get("mcp_servers").and_then(toml::Value::as_array))
        .flatten()
        .filter_map(toml::Value::as_table)
        .filter(|server| server.get("name").and_then(toml::Value::as_str) == Some("kronn-internal"))
        .map(|server| {
            let mut server = server.clone();
            server.insert("disabled".into(), true.into());
            toml::Value::Table(server)
        })
        .collect();
    if !disabled_bridges.is_empty() {
        profile.insert("mcp_servers".into(), toml::Value::Array(disabled_bridges));
    }
    toml::to_string(&profile).expect("a TOML table serializes")
}

/// Where Vibe reads the user's config: `VIBE_HOME` (inherited by Vibe from the
/// backend), else `~/.vibe` (`vibe/utils/paths.py`, `get_vibe_home`).
pub(crate) fn vibe_home() -> Option<PathBuf> {
    crate::core::child_env::var("VIBE_HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::core::child_env::var("HOME")
                .ok()
                .map(|home| PathBuf::from(home).join(".vibe"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user config like Romu's: a bash allowlist and `always` for writes.
    const PERMISSIVE_USER_CONFIG: &str = r#"
active_model = "local"
[tools.bash]
allowlist = ["echo", "pwd", "python3", "curl"]
[tools.write_file]
permission = "always"
[tools.edit]
permission = "always"
[tools.github_create_issue]
permission = "always"
[tools.read_file]
allowlist = ["/etc/*"]
[[mcp_servers]]
name = "kronn-internal"
transport = "stdio"
command = "python3"
args = ["bridge.py"]
"#;

    fn profile_of(config: &str) -> toml::Table {
        profile_toml(&[config.parse().unwrap()]).parse().unwrap()
    }

    #[test]
    fn the_profile_asks_for_everything_the_user_config_auto_approves() {
        let profile = profile_of(PERMISSIVE_USER_CONFIG);
        let tools = profile["tools"].as_table().unwrap();
        for tool in [
            "bash",
            "write_file",
            "edit",
            "github_create_issue",
            "web_fetch",
            "task",
        ] {
            assert_eq!(tools[tool]["permission"].as_str(), Some("ask"), "{tool}");
            assert_eq!(
                tools[tool]["allowlist"].as_array().map(Vec::len),
                Some(0),
                "{tool}"
            );
        }
        // Read-only tools keep their permission but lose their allowlist.
        assert!(tools["read_file"].get("permission").is_none());
        assert_eq!(
            tools["read_file"]["allowlist"].as_array().map(Vec::len),
            Some(0)
        );
        assert_eq!(profile["bypass_tool_permissions"].as_bool(), Some(false));
        assert_eq!(
            profile["disabled_tools"][0].as_str(),
            Some("exit_plan_mode")
        );
        // The env-less bridge copy is disabled, everything else about it kept.
        let bridge = &profile["mcp_servers"][0];
        assert_eq!(bridge["name"].as_str(), Some("kronn-internal"));
        assert_eq!(bridge["command"].as_str(), Some("python3"));
        assert_eq!(bridge["disabled"].as_bool(), Some(true));
        // The model and other settings are not the profile's business.
        assert!(profile.get("active_model").is_none());
    }

    #[test]
    fn without_any_user_config_the_built_in_lists_still_apply() {
        let profile: toml::Table = profile_toml(&[]).parse().unwrap();
        let tools = profile["tools"].as_table().unwrap();
        for tool in ACTING_TOOLS {
            assert_eq!(tools[*tool]["permission"].as_str(), Some("ask"), "{tool}");
        }
        assert!(profile.get("mcp_servers").is_none());
    }

    #[test]
    fn the_launch_selects_a_per_launch_profile_found_before_any_other() {
        let home = tempfile::tempdir().unwrap();
        let vibe_home = home.path().join(".vibe");
        std::fs::create_dir_all(&vibe_home).unwrap();
        std::fs::write(vibe_home.join("config.toml"), PERMISSIVE_USER_CONFIG).unwrap();
        let cwd = home.path().join("project");
        std::fs::create_dir_all(cwd.join(".vibe")).unwrap();
        std::fs::write(
            cwd.join(".vibe/config.toml"),
            "[tools.deploy]\npermission = \"always\"\n",
        )
        .unwrap();

        let policy = prepare(&cwd, &vibe_home, "0123456789ab").unwrap();
        let env: std::collections::HashMap<_, _> = policy.env.iter().cloned().collect();
        assert_eq!(env["VIBE_DEFAULT_AGENT"], "kronn-0123456789ab");
        assert_eq!(env["VIBE_ENABLED_AGENTS"], r#"["kronn-0123456789ab"]"#);
        assert_eq!(env["VIBE_SMART_APPROVE_DEFAULT"], "false");
        assert_eq!(env["VIBE_BYPASS_TOOL_PERMISSIONS"], "false");
        let paths: Vec<String> = serde_json::from_str(&env["VIBE_AGENT_PATHS"]).unwrap();
        assert_eq!(paths, vec![policy.dir.to_string_lossy().to_string()]);
        assert!(policy.dir.starts_with(cwd.join(".kronn/tmp")));
        let profile: toml::Table =
            std::fs::read_to_string(policy.dir.join("kronn-0123456789ab.toml"))
                .unwrap()
                .parse()
                .unwrap();
        // Both the user's and the project's tools are covered.
        assert_eq!(
            profile["tools"]["write_file"]["permission"].as_str(),
            Some("ask")
        );
        assert_eq!(
            profile["tools"]["deploy"]["permission"].as_str(),
            Some("ask")
        );
    }
}
