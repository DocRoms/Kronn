use super::*;
use crate::core::task_worker_env::ProbeCommand;
use crate::core::task_worker_sandbox::{CommandConfinement, Mechanism};
use crate::core::trust_sandbox::LevelConfinement;
use serial_test::serial;

/// A home holding every store the module knows: a directory with a file in it,
/// or a plain file, as the real tool would have left it.
fn full_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let paths = AGENT_STORES
        .iter()
        .map(|(_, rel)| *rel)
        .chain(SHARED_STORES.iter().copied());
    for rel in paths {
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

fn looks_like_file(rel: &str) -> bool {
    [
        ".claude.json",
        ".credentials.json",
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
    for (owner, rel) in AGENT_STORES {
        assert_eq!(
            hidden.iter().any(|path| path == rel),
            *owner != AgentFamily::Gemini,
            "{rel} of {owner:?}"
        );
    }
    assert!(!hidden.iter().any(|path| path == ".gemini"));
    assert!(hidden.iter().any(|path| path == ".codex"));
}

#[test]
fn a_process_whose_own_login_its_commands_never_need_hides_all_of_them() {
    let home = full_home();
    let hidden = hidden_in(home.path(), None, Reach::Sandboxed);
    let hidden = names(&hidden, home.path());
    for (_, rel) in AGENT_STORES {
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
    for rel in SHARED_STORES {
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
        let tier = if SHARED_STORES.contains(&rel) {
            &sandboxed
        } else {
            &open
        };
        assert!(tier.iter().any(|path| path == rel), "{rel}");
    }
}

#[test]
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
    let rels = AGENT_STORES
        .iter()
        .map(|(_, rel)| *rel)
        .chain(SHARED_STORES.iter().copied());
    for rel in rels {
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
    let rels = AGENT_STORES
        .iter()
        .map(|(_, rel)| *rel)
        .chain(SHARED_STORES.iter().copied());
    for rel in rels {
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
