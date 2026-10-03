//! KT-964 — under Docker, project MCP files never carry a secret value.
//!
//! The container mounts every repository and runs agents with the user's UID,
//! so a token written into one project's `.mcp.json` was readable by an agent
//! working on any other project. Under Docker, Claude Code's `.mcp.json` now
//! holds `${KRONN_MCP_…}` references (verified: it resolves them from its own
//! environment), and the values live only in this process's memory, keyed by
//! project directory, to be given to an agent started inside that directory.
//! CLIs that do not resolve references (Vibe passes the raw string; Kiro and
//! Gemini are unverified) get no secret-bearing MCP under Docker at all.
//!
//! An agent can still read the values it is given (`env`): this narrows what a
//! prompt-injected agent can reach to its own project's MCPs. Keeping tokens
//! out of the agent's reach entirely is the MCP gateway's job (KT-968).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

/// Secret values per project directory, never written to disk.
static VALUES: LazyLock<RwLock<HashMap<PathBuf, BTreeMap<String, String>>>> =
    LazyLock::new(Default::default);

/// Whether project MCP files carry references instead of values: under Docker,
/// unless `KRONN_MCP_SECRET_REFERENCES` says otherwise (`1` / `0`).
pub fn enabled() -> bool {
    match std::env::var("KRONN_MCP_SECRET_REFERENCES").as_deref() {
        Ok("1") => true,
        Ok("0") => false,
        _ => crate::core::env::is_docker(),
    }
}

/// The environment variable an agent receives for one key of one MCP config.
/// Scoped by config so two MCPs using the same key never collide.
pub fn reference_name(config_id: &str, key: &str) -> String {
    let scope: String = config_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect::<String>()
        .to_ascii_uppercase();
    let key: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("KRONN_MCP_{scope}_{key}")
}

/// Whether an env value is exactly one reference written by [`as_references`],
/// which carries no secret.
pub fn is_reference(value: &str) -> bool {
    value
        .strip_prefix("${KRONN_MCP_")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
}

/// An MCP entry's env as references, and the values those references stand for.
pub fn as_references(
    config_id: &str,
    env: &HashMap<String, String>,
) -> (HashMap<String, String>, Vec<(String, String)>) {
    let mut references = HashMap::with_capacity(env.len());
    let mut values = Vec::with_capacity(env.len());
    for (key, value) in env {
        let name = reference_name(config_id, key);
        references.insert(key.clone(), format!("${{{name}}}"));
        values.push((name, value.clone()));
    }
    (references, values)
}

/// Projects are recorded under their host path, while an agent may start under
/// the `/host-home` mount of the same directory: both map to the host path.
fn canonical(dir: &Path) -> PathBuf {
    let dir = PathBuf::from(crate::core::scanner::restore_host_path(dir));
    std::fs::canonicalize(&dir).unwrap_or(dir)
}

/// Record the values an agent started in `dir` (or below it) must receive.
/// Replaces what was known for that directory: a removed MCP stops being given.
pub fn remember(dir: &str, values: Vec<(String, String)>) {
    if let Ok(mut map) = VALUES.write() {
        let dir = canonical(Path::new(dir));
        if values.is_empty() {
            map.remove(&dir);
        } else {
            map.insert(dir, values.into_iter().collect());
        }
    }
}

/// The values for an agent started in `work_dir`: those of the closest
/// recorded directory containing it (a project, or one of its worktrees),
/// never another project's.
pub fn values_for(work_dir: &Path) -> Vec<(String, String)> {
    let work_dir = canonical(work_dir);
    let Ok(map) = VALUES.read() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(dir, _)| work_dir.starts_with(dir))
        .max_by_key(|(dir, _)| dir.components().count())
        .map(|(_, values)| values.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// Give an agent command the MCP values of the directory it starts in.
pub fn apply_to(command: &mut tokio::process::Command, work_dir: &Path) {
    if !enabled() {
        return;
    }
    for (name, value) in values_for(work_dir) {
        command.env(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_scoped_by_config_and_never_carry_the_value() {
        let env = HashMap::from([(
            "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
            "ghp_secret".to_string(),
        )]);
        let (references, values) = as_references("3f2a-91bc-7d", &env);
        let reference = &references["GITHUB_PERSONAL_ACCESS_TOKEN"];
        assert_eq!(
            reference,
            "${KRONN_MCP_3F2A91BC7D_GITHUB_PERSONAL_ACCESS_TOKEN}"
        );
        assert!(!reference.contains("ghp_secret"));
        assert_eq!(
            values,
            vec![(
                "KRONN_MCP_3F2A91BC7D_GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                "ghp_secret".into()
            )]
        );
        assert_ne!(
            reference_name("config-a", "TOKEN"),
            reference_name("config-b", "TOKEN")
        );
        assert_eq!(reference_name("c", "my-key.v2"), "KRONN_MCP_C_MY_KEY_V2");
    }

    #[test]
    fn an_agent_gets_only_its_own_projects_values_including_in_a_worktree() {
        let root = tempfile::tempdir().unwrap();
        let alpha = root.path().join("alpha");
        let beta = root.path().join("beta");
        let worktree = alpha.join(".kronn/worktrees/task-1");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(&beta).unwrap();
        remember(
            &alpha.to_string_lossy(),
            vec![("KRONN_MCP_A_TOKEN".into(), "alpha".into())],
        );
        remember(
            &beta.to_string_lossy(),
            vec![("KRONN_MCP_B_TOKEN".into(), "beta".into())],
        );

        assert_eq!(
            values_for(&alpha),
            vec![("KRONN_MCP_A_TOKEN".into(), "alpha".into())]
        );
        assert_eq!(
            values_for(&worktree),
            vec![("KRONN_MCP_A_TOKEN".into(), "alpha".into())]
        );
        assert_eq!(
            values_for(&beta),
            vec![("KRONN_MCP_B_TOKEN".into(), "beta".into())]
        );
        assert!(
            values_for(root.path()).is_empty(),
            "a parent directory gets nothing"
        );

        remember(&alpha.to_string_lossy(), Vec::new());
        assert!(
            values_for(&worktree).is_empty(),
            "a removed MCP stops being given"
        );
    }

    #[test]
    fn only_an_exact_reference_counts_as_one() {
        assert!(is_reference("${KRONN_MCP_DA784B86E470_RECETTE_TOKEN}"));
        for value in [
            "ghp_secret",
            "${KRONN_MCP_}",
            "${KRONN_MCP_A}x",
            "x${KRONN_MCP_A}",
            "${KRONN_MCP_a}",
            "${KRONN_MCP_A} ${KRONN_MCP_B}",
            "${OTHER_TOKEN}",
        ] {
            assert!(!is_reference(value), "{value}");
        }
    }

    #[test]
    #[serial_test::serial]
    fn an_agent_started_under_the_host_home_mount_gets_its_project_values() {
        let home = tempfile::tempdir().unwrap();
        let project = home.path().join("Repositories").join("alpha");
        std::fs::create_dir_all(&project).unwrap();
        std::env::set_var("KRONN_HOST_HOME", home.path());
        remember(
            &project.to_string_lossy(),
            vec![("KRONN_MCP_A_TOKEN".into(), "alpha".into())],
        );
        let given = values_for(Path::new("/host-home/Repositories/alpha"));
        remember(&project.to_string_lossy(), Vec::new());
        std::env::remove_var("KRONN_HOST_HOME");
        assert_eq!(given, vec![("KRONN_MCP_A_TOKEN".into(), "alpha".into())]);
    }

    #[test]
    #[serial_test::serial]
    fn an_agent_command_receives_the_values_only_when_references_are_on() {
        let project = tempfile::tempdir().unwrap();
        remember(
            &project.path().to_string_lossy(),
            vec![("KRONN_MCP_P_TOKEN".into(), "value".into())],
        );
        let given = |mode: &str| {
            std::env::set_var("KRONN_MCP_SECRET_REFERENCES", mode);
            let mut command = tokio::process::Command::new("true");
            apply_to(&mut command, project.path());
            std::env::remove_var("KRONN_MCP_SECRET_REFERENCES");
            command
                .as_std()
                .get_envs()
                .any(|(name, value)| name == "KRONN_MCP_P_TOKEN" && value == Some("value".as_ref()))
        };
        assert!(given("1"));
        assert!(!given("0"), "natively nothing is injected");
        remember(&project.path().to_string_lossy(), Vec::new());
    }
}
