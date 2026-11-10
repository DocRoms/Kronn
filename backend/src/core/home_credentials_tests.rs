use super::*;
use crate::core::task_worker_env::ProbeCommand;
use crate::core::task_worker_sandbox::{CommandConfinement, Mechanism};
use crate::core::trust_sandbox::LevelConfinement;
use serial_test::serial;
use std::ffi::OsString;

/// A home holding every store the module knows: a directory with a file in it,
/// or a plain file, as the real tool would have left it.
fn full_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for rel in agent_rels().into_iter().chain(shared_rels()) {
        let path = home.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // A parent store already made `rel` a directory (`.claude`, `.cargo`).
        if path.is_dir() {
            continue;
        }
        if looks_like_file(rel) {
            std::fs::write(&path, "secret").unwrap();
        } else {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("secret.txt"), "secret").unwrap();
        }
    }
    home
}

fn agent_rels() -> Vec<&'static str> {
    AGENT_STORES.iter().map(|(_, store)| store.rel).collect()
}

fn shared_rels() -> Vec<&'static str> {
    SHARED_STORES.iter().map(|store| store.rel).collect()
}

fn looks_like_file(rel: &str) -> bool {
    [
        ".claude.json",
        ".credentials.json",
        "settings.json",
        "settings.local.json",
        "credentials",
        "credentials.toml",
        "config.json",
        "config",
        "settings.xml",
        "gradle.properties",
        "credentials.tfrc.json",
        ".vault-token",
        ".netrc",
        ".npmrc",
        ".yarnrc.yml",
        ".pypirc",
        ".git-credentials",
    ]
    .iter()
    .any(|file| rel == *file || rel.ends_with(&format!("/{file}")))
}

/// What a command reads from `rel` of the home: the file, or the file in it.
fn readable_file(home: &Path, rel: &str) -> PathBuf {
    let path = home.join(rel);
    if path.is_dir() {
        path.join("secret.txt")
    } else {
        path
    }
}

fn names(paths: &[PathBuf], home: &Path) -> Vec<String> {
    let real = real_path(home);
    paths
        .iter()
        .filter_map(|path| path.strip_prefix(&real).or(path.strip_prefix(home)).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn every_store_of_another_agent_is_hidden_and_the_running_ones_stays_open() {
    let home = full_home();
    let hidden = hidden_in(home.path(), Some(AgentFamily::Gemini), Reach::FullAccess);
    let hidden = names(&hidden, home.path());
    for (owner, store) in AGENT_STORES {
        let rel = store.rel;
        assert_eq!(
            hidden.iter().any(|path| path == rel),
            *owner != AgentFamily::Gemini,
            "{rel} of {owner:?}"
        );
    }
    assert!(!hidden.iter().any(|path| path == ".gemini"));
    assert!(hidden.iter().any(|path| path == ".codex"));
}

/// KT-1656 — the `env` block, `apiKeyHelper` and `mcpServers.*.env` of Claude's
/// settings are what leaked from `~/.gemini/settings.json`, in another shape.
#[test]
fn claude_settings_files_are_hidden_from_every_other_agent_and_from_a_workers_shell() {
    let home = full_home();
    let settings = [".claude/settings.json", ".claude/settings.local.json"];
    for other in [
        AgentFamily::Gemini,
        AgentFamily::Codex,
        AgentFamily::Copilot,
        AgentFamily::Kiro,
        AgentFamily::Vibe,
        AgentFamily::OpenCode,
        AgentFamily::Other,
    ] {
        for reach in [Reach::Sandboxed, Reach::FullAccess] {
            let hidden = names(&hidden_in(home.path(), Some(other), reach), home.path());
            for rel in settings {
                assert!(hidden.iter().any(|path| path == rel), "{rel} for {other:?}");
            }
        }
    }
    // A Claude worker's shell: nothing of Claude's is for it.
    let worker = names(&hidden_in(home.path(), None, Reach::Sandboxed), home.path());
    // The running Claude reads its own settings.
    let claude = names(
        &hidden_in(home.path(), Some(AgentFamily::Claude), Reach::FullAccess),
        home.path(),
    );
    for rel in settings {
        assert!(worker.iter().any(|path| path == rel), "{rel} for a worker");
        assert!(!claude.iter().any(|path| path == rel), "{rel} for Claude");
    }
}

#[test]
fn a_process_whose_own_login_its_commands_never_need_hides_all_of_them() {
    let home = full_home();
    let hidden = hidden_in(home.path(), None, Reach::Sandboxed);
    let hidden = names(&hidden, home.path());
    for rel in agent_rels() {
        assert!(hidden.iter().any(|path| path == rel), "{rel}");
    }
}

#[test]
fn the_shared_stores_are_for_the_sandboxed_launches_only() {
    let home = full_home();
    let sandboxed = names(
        &hidden_in(home.path(), Some(AgentFamily::Codex), Reach::Sandboxed),
        home.path(),
    );
    let open = names(
        &hidden_in(home.path(), Some(AgentFamily::Codex), Reach::FullAccess),
        home.path(),
    );
    for rel in shared_rels() {
        assert!(sandboxed.iter().any(|path| path == rel), "{rel}");
        assert!(!open.iter().any(|path| path == rel), "{rel} in full access");
    }
    // The ones the DoD names.
    for rel in [
        ".gemini",
        ".claude.json",
        ".config/gh",
        ".npmrc",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".aws",
        ".ssh",
    ] {
        let tier = if shared_rels().contains(&rel) {
            &sandboxed
        } else {
            &open
        };
        assert!(tier.iter().any(|path| path == rel), "{rel}");
    }
}

#[test]
#[cfg(unix)] // creates symlinks through std::os::unix
fn a_store_that_is_not_there_is_not_listed_and_a_link_is_listed_with_its_target() {
    let home = tempfile::tempdir().unwrap();
    assert!(hidden_in(home.path(), None, Reach::Sandboxed).is_empty());

    let dotfiles = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".config")).unwrap();
    std::os::unix::fs::symlink(dotfiles.path(), home.path().join(".config/gh")).unwrap();
    let hidden = hidden_in(home.path(), None, Reach::Sandboxed);
    assert!(hidden.contains(&home.path().join(".config/gh")));
    assert!(hidden.contains(&real_path(dotfiles.path())), "{hidden:?}");
}

async fn cat(file: &Path, confinement: Option<&LevelConfinement>) -> bool {
    let cwd = std::env::temp_dir();
    let probe = ProbeCommand {
        program: "cat".to_string(),
        args: vec![file.to_string_lossy().into_owned()],
        cwd: cwd.clone(),
        env: Vec::new(),
        timeout: std::time::Duration::from_secs(20),
        priority: crate::core::background_priority::Priority::Normal,
        clean_env: false,
    };
    let mut command = match confinement {
        Some(confinement) => {
            confinement.command(&probe, crate::core::child_env::ChildRoute::QuickExec)
        }
        None => {
            let mut command =
                crate::core::cmd::async_cmd("cat", crate::core::child_env::ChildRoute::QuickExec);
            command.args(&probe.args).current_dir(&cwd);
            command
        }
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());
    command
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Applied for real where the host allows it (macOS locally, Linux in CI); a
/// host that cannot apply a sandbox, or that is inside another one (macOS
/// refuses a nested Seatbelt), skips with a line on stderr.
fn real_confinement(hidden: Vec<PathBuf>) -> Option<LevelConfinement> {
    match Mechanism::usable_on_host() {
        Ok(mechanism) => Some(LevelConfinement::with(mechanism, hidden)),
        Err(reason) => {
            eprintln!("skipped: no sandbox can be applied here ({reason})");
            None
        }
    }
}

/// The DoD's check, per path: a confined command cannot read any store.
#[tokio::test]
#[serial]
async fn a_sandboxed_command_cannot_read_any_credential_store() {
    let home = full_home();
    let hidden = hidden_in(home.path(), None, Reach::Sandboxed);
    let Some(confinement) = real_confinement(hidden) else {
        return;
    };
    for rel in agent_rels().into_iter().chain(shared_rels()) {
        let file = readable_file(home.path(), rel);
        assert!(
            cat(&file, None).await,
            "control: {rel} is readable unconfined"
        );
        assert!(
            !cat(&file, Some(&confinement)).await,
            "{rel} was read from inside the sandbox"
        );
    }
}

#[tokio::test]
#[serial]
async fn a_full_access_agent_reads_its_own_login_and_the_owners_tools_but_not_another_agents() {
    let home = full_home();
    let hidden = hidden_in(home.path(), Some(AgentFamily::Gemini), Reach::FullAccess);
    let Some(confinement) = real_confinement(hidden) else {
        return;
    };
    let read = |rel: &'static str| {
        let file = readable_file(home.path(), rel);
        let confinement = &confinement;
        async move { cat(&file, Some(confinement)).await }
    };
    assert!(read(".gemini").await, "its own store");
    assert!(read(".ssh").await, "the owner's terminal tools");
    assert!(!read(".codex").await);
    assert!(!read(".claude.json").await);
}

/// The Claude task worker: `sandbox.filesystem.denyRead` for its shell and the
/// Read tool's own rules, for every store.
#[test]
#[serial]
fn a_claude_worker_is_told_every_store_in_its_settings() {
    let home = full_home();
    let previous = crate::core::child_env::var("HOME").ok();
    crate::core::child_env::set_var("HOME", home.path());
    let deny_read = crate::core::vault_guard::claude_sandbox_deny_read();
    let permissions = crate::core::vault_guard::claude_permission_deny();
    match previous {
        Some(value) => crate::core::child_env::set_var("HOME", value),
        None => crate::core::child_env::remove_var("HOME"),
    }

    let real = real_path(home.path());
    for rel in agent_rels().into_iter().chain(shared_rels()) {
        let path = real.join(rel);
        assert!(
            deny_read.contains(&path.to_string_lossy().into_owned()),
            "{rel} missing from denyRead"
        );
        let rule = if path.is_dir() {
            format!("Read(/{}/**)", path.display())
        } else {
            format!("Read(/{})", path.display())
        };
        assert!(permissions.contains(&rule), "{rule} missing");
    }
    // The shell of a Claude worker loses its own login too: the CLI reads it
    // from outside its sandbox.
    assert!(deny_read.contains(&real.join(".claude.json").to_string_lossy().into_owned()));
}

/// The launch of another agent: its own store open, the others' hidden, and the
/// mounts bubblewrap emits for files as well as for directories.
#[test]
fn bubblewrap_masks_a_file_with_dev_null_and_a_directory_with_a_tmpfs() {
    let home = full_home();
    let hidden = hidden_in(home.path(), None, Reach::Sandboxed);
    let args = crate::core::trust_sandbox::bubblewrap_hide_args(&hidden);
    let file = real_path(&home.path().join(".npmrc"))
        .to_string_lossy()
        .into_owned();
    let dir = real_path(&home.path().join(".gemini"))
        .to_string_lossy()
        .into_owned();
    let at = |needle: &str| args.iter().position(|arg| arg == needle).unwrap();
    assert_eq!(args[at(&file) - 2..at(&file)], ["--ro-bind", "/dev/null"]);
    assert_eq!(args[at(&dir) - 1], "--tmpfs");
    // The three emitters put them before the namespace flags.
    for rendered in [
        crate::core::trust_sandbox::bubblewrap_args(&hidden),
        crate::core::vault_guard::codex_bubblewrap_args(
            crate::core::vault_guard::CodexMode::ReadOnly,
            &crate::core::vault_guard::LaunchRoots::default(),
            &hidden,
        ),
        crate::core::task_worker_sandbox::bubblewrap_args_hiding(&[], None, &hidden),
    ] {
        assert!(rendered.contains(&file), "{rendered:?}");
        assert!(rendered.contains(&dir), "{rendered:?}");
        assert_eq!(rendered.last().map(String::as_str), Some("--"));
    }
}

/// One relocation: the variable, its value (under the scratch directory, a
/// list for `KUBECONFIG`), and the stores that must then be hidden there.
struct Moved {
    variable: &'static str,
    value: &'static [&'static str],
    stores: &'static [&'static str],
}

const MOVES: &[Moved] = &[
    Moved {
        variable: "CLAUDE_CONFIG_DIR",
        value: &["claude"],
        stores: &[
            "claude/.credentials.json",
            "claude/.claude.json",
            "claude/settings.json",
            "claude/settings.local.json",
        ],
    },
    Moved {
        variable: "CODEX_HOME",
        value: &["codex"],
        stores: &["codex"],
    },
    Moved {
        variable: "GEMINI_CLI_HOME",
        value: &["gemini-home"],
        stores: &["gemini-home/.gemini"],
    },
    Moved {
        variable: "XDG_CONFIG_HOME",
        value: &["xdg-config"],
        stores: &[
            "xdg-config/gh",
            "xdg-config/gcloud",
            "xdg-config/glab-cli",
            "xdg-config/opencode",
            "xdg-config/github-copilot",
            "xdg-config/git/credentials",
        ],
    },
    Moved {
        variable: "XDG_DATA_HOME",
        value: &["xdg-data"],
        stores: &["xdg-data/opencode", "xdg-data/kiro-cli"],
    },
    Moved {
        variable: "GNUPGHOME",
        value: &["gnupg"],
        stores: &["gnupg"],
    },
    Moved {
        variable: "AWS_SHARED_CREDENTIALS_FILE",
        value: &["aws-credentials"],
        stores: &["aws-credentials"],
    },
    Moved {
        variable: "AWS_CONFIG_FILE",
        value: &["aws-config"],
        stores: &["aws-config"],
    },
    Moved {
        variable: "KUBECONFIG",
        value: &["kube-a", "kube-b"],
        stores: &["kube-a", "kube-b"],
    },
    Moved {
        variable: "DOCKER_CONFIG",
        value: &["docker"],
        stores: &["docker/config.json"],
    },
    Moved {
        variable: "CARGO_HOME",
        value: &["cargo"],
        stores: &["cargo/credentials", "cargo/credentials.toml"],
    },
    Moved {
        variable: "NPM_CONFIG_USERCONFIG",
        value: &["npmrc"],
        stores: &["npmrc"],
    },
    Moved {
        variable: "GH_CONFIG_DIR",
        value: &["gh-dir"],
        stores: &["gh-dir"],
    },
    Moved {
        variable: "CLOUDSDK_CONFIG",
        value: &["gcloud-dir"],
        stores: &["gcloud-dir"],
    },
];

/// Make a store of `scratch`: a file holding `secret`.
fn plant(scratch: &Path, rel: &str) -> PathBuf {
    let path = scratch.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "secret").unwrap();
    path
}

fn env_of(scratch: &Path, moved: &Moved) -> impl Fn(&str) -> Option<OsString> + use<> {
    let variable = moved.variable;
    let value = std::env::join_paths(moved.value.iter().map(|rel| scratch.join(rel))).unwrap();
    move |name| (name == variable).then(|| value.clone())
}

#[test]
fn every_variable_that_moves_a_store_gets_the_new_place_hidden() {
    for moved in MOVES {
        // An empty home: only the relocated store exists.
        let home = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let planted: Vec<PathBuf> = moved
            .stores
            .iter()
            .map(|rel| plant(scratch.path(), rel))
            .collect();
        let env = env_of(scratch.path(), moved);
        let hidden = hidden_with(home.path(), None, Reach::Sandboxed, &env);
        for path in &planted {
            assert!(
                hidden.contains(path),
                "{}: {} is not hidden ({hidden:?})",
                moved.variable,
                path.display()
            );
        }
        // Without the variable, the same scratch store is not known.
        let without = hidden_with(home.path(), None, Reach::Sandboxed, &|_| None);
        assert!(without.is_empty(), "{}: {without:?}", moved.variable);
    }
}

#[test]
fn a_moved_store_keeps_its_default_place_hidden_too() {
    let home = full_home();
    let scratch = tempfile::tempdir().unwrap();
    plant(scratch.path(), "codex");
    let moved = &MOVES[1];
    assert_eq!(moved.variable, "CODEX_HOME");
    let env = env_of(scratch.path(), moved);
    let hidden = hidden_with(home.path(), None, Reach::Sandboxed, &env);
    let hidden = names(&hidden, home.path());
    assert!(hidden.iter().any(|path| path == ".codex"));
}

#[test]
fn the_running_agents_moved_store_stays_open_and_a_shared_one_is_for_sandboxed_launches() {
    let home = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let codex = plant(scratch.path(), "codex");
    let gnupg = plant(scratch.path(), "gnupg");
    let env = |name: &str| {
        let place = match name {
            "CODEX_HOME" => codex.clone(),
            "GNUPGHOME" => gnupg.clone(),
            _ => return None,
        };
        Some(place.into_os_string())
    };
    let codex_runs = Some(AgentFamily::Codex);
    let running = hidden_with(home.path(), codex_runs, Reach::FullAccess, &env);
    assert!(!running.contains(&codex), "the running agent's own store");
    assert!(!running.contains(&gnupg), "a shared store in full access");
    let gemini_runs = Some(AgentFamily::Gemini);
    let others = hidden_with(home.path(), gemini_runs, Reach::FullAccess, &env);
    assert!(others.contains(&codex));
    assert!(!others.contains(&gnupg));
    let sandboxed = hidden_with(home.path(), gemini_runs, Reach::Sandboxed, &env);
    assert!(sandboxed.contains(&codex) && sandboxed.contains(&gnupg));
}

#[test]
fn a_relative_value_is_ignored_and_a_variable_naming_the_home_does_not_hide_it() {
    let home = full_home();
    let relative = |name: &str| (name == "CODEX_HOME").then(|| OsString::from("codex"));
    let hidden = hidden_with(home.path(), None, Reach::Sandboxed, &relative);
    assert!(hidden.iter().all(|path| path.is_absolute()));

    // `GNUPGHOME=$HOME` must not hide the home: its stores are listed by name.
    let home_path = home.path().to_path_buf();
    let whole_home = |name: &str| {
        (name == "GNUPGHOME" || name == "CODEX_HOME").then(|| home_path.clone().into_os_string())
    };
    let hidden = hidden_with(home.path(), None, Reach::Sandboxed, &whole_home);
    assert!(!hidden.contains(&home.path().to_path_buf()));
    assert!(!hidden.contains(&real_path(home.path())));
    let hidden = names(&hidden, home.path());
    assert!(hidden.iter().any(|path| path == ".gnupg"));
    assert!(hidden.iter().any(|path| path == ".codex"));
}

/// A store moved by `CODEX_HOME`, applied for real: unreadable from inside.
#[tokio::test]
#[serial]
async fn a_sandboxed_command_cannot_read_a_store_moved_by_the_environment() {
    let home = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let secret = plant(scratch.path(), "codex/auth.json");
    let moved = &MOVES[1];
    let env = env_of(scratch.path(), moved);
    let hidden = hidden_with(home.path(), None, Reach::Sandboxed, &env);
    let Some(confinement) = real_confinement(hidden) else {
        return;
    };
    assert!(cat(&secret, None).await, "control: readable unconfined");
    assert!(
        !cat(&secret, Some(&confinement)).await,
        "the CODEX_HOME store was read from inside the sandbox"
    );
}
