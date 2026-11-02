use super::*;
use crate::core::task_worker_sandbox::Mechanism;
use serial_test::serial;
use std::path::Path;

fn probe(program: &str, args: &[&str], cwd: &Path) -> ProbeCommand {
    ProbeCommand {
        program: program.to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        env: Vec::new(),
        timeout: std::time::Duration::from_secs(20),
    }
}

#[test]
fn the_seatbelt_profile_allows_by_default_and_closes_only_the_data_dir() {
    let profile = seatbelt_profile(&[PathBuf::from("/data/kronn")]);
    assert!(profile.starts_with("(version 1)\n(allow default)\n"));
    assert!(profile.contains("(deny file-read* file-write*\n  (subpath \"/data/kronn\")\n)"));
    // The deny comes after the allow: the last matching rule wins.
    assert!(profile.find("(allow default)") < profile.find("(deny file-read*"));
    // Nothing else is closed: not the network, not the Kronn API port, not Docker.
    assert_eq!(profile.matches("(deny ").count(), 1);
    assert!(!profile.contains("network"), "{profile}");
}

#[test]
fn the_profile_escapes_the_path() {
    let profile = seatbelt_profile(&[PathBuf::from("/d/\"x\"")]);
    assert!(profile.contains("\"/d/\\\"x\\\"\""));
}

/// A data directory reached through a symlink (a linked `$HOME`, a
/// `KRONN_DATA_DIR` through a link): `/` is bind-mounted, so a tmpfs over the
/// alias alone leaves the real directory readable. The tmpfs goes over the
/// resolved path, once, whichever form is given.
#[cfg(unix)]
#[test]
fn bubblewrap_hides_the_resolved_data_dir_when_it_is_given_through_a_symlink() {
    let base = tempfile::tempdir().unwrap();
    let real = base.path().join("real-data");
    std::fs::create_dir(&real).unwrap();
    let link = base.path().join("linked-data");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let resolved = real_path(&real).to_string_lossy().into_owned();

    for given in [vec![link.clone()], vec![link.clone(), real.clone()]] {
        assert_eq!(bubblewrap_mount_points(&given), vec![resolved.clone()]);
        let args = bubblewrap_args(&given);
        let mounts: Vec<&String> = args
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "--tmpfs")
            .map(|(at, _)| &args[at + 1])
            .collect();
        assert_eq!(mounts, vec![&resolved], "{args:?}");
    }

    // The worker sandbox's own `hiding` goes through the same resolution.
    let worker_args = crate::core::task_worker_sandbox::bubblewrap_args_hiding(&[], None, &[link]);
    let at = worker_args.iter().position(|a| *a == resolved).unwrap();
    assert_eq!(worker_args[at - 1], "--tmpfs", "{worker_args:?}");
    assert_eq!(worker_args.iter().filter(|a| **a == resolved).count(), 1);
}

/// Gemini review: the worker sandbox emits the tmpfs mounts while it builds the
/// list, over every bind (the Docker socket included), before the namespace
/// flags and the `--` sentinel, whether or not the network is unshared. Nothing
/// is spliced into a finished list, so an option never splits and a path or
/// command named like a flag cannot move the mounts.
#[cfg(unix)]
#[test]
fn the_worker_sandbox_hides_the_data_dir_inside_the_options_before_the_command() {
    use crate::core::task_worker_sandbox::{
        bubblewrap_args_hiding, bubblewrap_args_with, confine, SandboxPolicy,
    };
    let data = tempfile::tempdir().unwrap();
    let resolved = real_path(data.path()).to_string_lossy().into_owned();
    let hidden = [data.path().to_path_buf()];
    let mounts = |args: &[String]| -> Vec<usize> {
        args.iter()
            .enumerate()
            .filter(|(at, arg)| *arg == "--tmpfs" && args.get(at + 1) == Some(&resolved))
            .map(|(at, _)| at)
            .collect()
    };

    for (writable, socket) in [
        (vec![], None),
        (vec![PathBuf::from("/work")], None),
        (
            vec![PathBuf::from("/work")],
            Some(Path::new("/var/run/docker.sock")),
        ),
    ] {
        let hiding = bubblewrap_args_hiding(&writable, socket, &hidden);
        let found = mounts(&hiding);
        assert_eq!(found.len(), 1, "{hiding:?}");
        // Taking the two mount tokens out gives back the list without hiding,
        // byte for byte: no option was split.
        let mut rest = hiding.clone();
        rest.drain(found[0]..found[0] + 2);
        assert_eq!(rest, bubblewrap_args_with(&writable, socket), "{hiding:?}");
        // On top of every bind, before the namespace flags and the sentinel.
        assert!(hiding
            .iter()
            .rposition(|a| a == "--bind")
            .is_none_or(|bind| bind + 2 < found[0]));
        assert!(found[0] < hiding.iter().position(|a| a == "--unshare-net").unwrap());
        assert_eq!(hiding.last().map(String::as_str), Some("--"));
        assert_eq!(hiding.iter().filter(|a| *a == "--").count(), 1);
    }

    // Through `confine`: the command comes last, after the only `--`, even when
    // it and its arguments are named like options.
    let policy = SandboxPolicy {
        write_roots: vec![PathBuf::from("/work")],
        temp_dir: PathBuf::from("/work/tmp"),
        docker_socket: None,
        hidden: hidden.to_vec(),
    };
    let command = probe(
        "--unshare-net",
        &["--", "--tmpfs", "--unshare-net"],
        Path::new("/work"),
    );
    let confined = confine(Mechanism::Bubblewrap, &policy, &command);
    let sentinel = confined.args.iter().position(|a| a == "--").unwrap();
    assert_eq!(
        &confined.args[sentinel + 1..],
        ["--unshare-net", "--", "--tmpfs", "--unshare-net"]
    );
    assert_eq!(mounts(&confined.args[..sentinel]).len(), 1);
    assert_eq!(mounts(&confined.args[sentinel..]).len(), 0);
}

#[test]
fn bubblewrap_replaces_the_data_dir_by_an_empty_tmpfs_and_keeps_the_host_network() {
    let dir = tempfile::tempdir().unwrap();
    let args = bubblewrap_args(&[dir.path().to_path_buf(), PathBuf::from("/does/not/exist")]);
    let at = |needle: &str| args.iter().position(|a| a == needle);
    let tmpfs = at("--tmpfs").expect("a tmpfs hides the data dir");
    assert_eq!(args[tmpfs + 1], real_path(dir.path()).to_string_lossy());
    assert_eq!(args.iter().filter(|a| *a == "--tmpfs").count(), 1);
    assert!(at("--share-net").is_some(), "the host network is kept");
    assert!(
        at("--unshare-net").is_none() && at("--unshare-all").is_none(),
        "the network is never cut"
    );
    assert!(at("--die-with-parent").is_some() && at("--new-session").is_some());
    // A private PID namespace and a fresh /proc: the host's /proc is no way
    // around the tmpfs (`/proc/<pid>/root/<data dir>`).
    assert!(at("--unshare-pid").is_some());
    let proc_mount = at("--proc").expect("a fresh /proc is mounted");
    assert_eq!(args[proc_mount + 1], "/proc");
    assert!(
        proc_mount > at("--bind").unwrap() && proc_mount < tmpfs,
        "the fresh /proc replaces the bound one, before the tmpfs"
    );
    assert_eq!(args.last().map(String::as_str), Some("--"));
    assert!(
        at("--bind").unwrap() < tmpfs,
        "the tmpfs is mounted over the bind"
    );
}

#[test]
fn a_wrapped_command_is_the_launcher_then_the_profile_then_the_program() {
    let confinement = LevelConfinement::with(Mechanism::Seatbelt, vec![PathBuf::from("/d")]);
    let (program, args) = confinement.wrap("gh", &["pr".into(), "list".into()]);
    assert_eq!(program, "/usr/bin/sandbox-exec");
    assert_eq!(args[0], "-p");
    assert_eq!(args[1], seatbelt_profile(&[PathBuf::from("/d")]));
    assert_eq!(&args[2..], ["gh", "pr", "list"]);

    let confinement = LevelConfinement::with(Mechanism::Bubblewrap, vec![]);
    let (program, args) = confinement.wrap("gh", &["pr".into()]);
    assert_eq!(program, "bwrap");
    assert_eq!(&args[args.len() - 3..], ["--", "gh", "pr"]);
}

#[test]
fn the_protected_dirs_hold_the_data_dir_as_given_and_resolved() {
    let real = tempfile::tempdir().unwrap();
    let link = real.path().parent().unwrap().join(format!(
        "kronn-link-{}",
        real.path().file_name().unwrap().to_string_lossy()
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(real.path(), &link).unwrap();
        let dirs = protected_from(vec![link.clone()]);
        assert!(dirs.contains(&link));
        assert!(dirs.contains(&real_path(real.path())));
        std::fs::remove_file(&link).unwrap();
    }
}

#[test]
fn a_docker_argument_that_names_the_data_dir_or_a_parent_is_caught() {
    let protected = vec![PathBuf::from("/home/me/.config/kronn")];
    let cwd = std::env::temp_dir();
    let hit = |arg: &str| argv_reaches_protected(&[arg.to_string()], &protected, &cwd).is_some();
    assert!(hit("/home/me/.config/kronn:/data"));
    assert!(hit("/home/me/.config/kronn/kronn.db:/db"));
    assert!(hit("type=bind,source=/home/me/.config,target=/c"));
    assert!(hit("/home/me/.config:/c"));
    assert!(hit("/:/host"));
    assert!(!hit("/home/me/project:/work"));
    assert!(!hit("/home/me/.config/kronn-other:/x"));
    assert!(!hit("--rm"));
    assert!(!hit("alpine:3"));
    // The short flag glued to its value, which Docker accepts.
    assert!(hit("-v/home/me/.config/kronn:/data"));
    assert!(hit("-v/:/host"));
    assert!(hit("-v\"/home/me/.config:/c\""));
    assert!(!hit("-v/home/me/project:/work"));
    assert!(!hit("-it"));
}

/// Docker (pflag) reads a group of short flags letter by letter, the last one
/// taking the glued value: every suffix after the leading letters is a source.
#[test]
fn a_docker_volume_glued_to_a_group_of_short_flags_is_caught() {
    let protected = vec![PathBuf::from("/home/me/.config/kronn")];
    let cwd = std::env::temp_dir();
    let hit = |arg: &str| argv_reaches_protected(&[arg.to_string()], &protected, &cwd).is_some();
    assert!(hit("-itv/home/me/.config/kronn:/c"));
    assert!(hit("-dv/home/me/.config/kronn:/c"));
    assert!(hit("-itdv/home/me/.config:/c"));
    assert!(hit("-itv/:/host"));
    assert!(hit("-itv\"/home/me/.config/kronn:/c\""));
    assert!(!hit("-itv/home/me/project:/work"));
    assert!(!hit("-itd"));
    assert!(!hit("-itv"));
    // `~` and `$HOME` expand the way the shell would, behind a group too.
    let home = Path::new("/home/me");
    let hit_home = |arg: &str| {
        argv_reaches_protected_from(&[arg.to_string()], &protected, &cwd, Some(home)).is_some()
    };
    assert!(hit_home("-dv~/.config/kronn:/c"));
    assert!(hit_home("-itv~/.config:/c"));
    assert!(hit_home("-itv$HOME/.config/kronn:/c"));
    assert!(!hit_home("-itv~/project:/c"));
}

#[cfg(unix)]
#[test]
fn a_docker_volume_that_reaches_the_data_dir_through_a_symlink_is_caught() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let work = root.path().join("worktree");
    std::fs::create_dir(&work).unwrap();
    // A worktree link to the data dir, and one to the directory holding it.
    std::os::unix::fs::symlink(&data, work.join("vault")).unwrap();
    std::os::unix::fs::symlink(root.path(), work.join("everything")).unwrap();
    let protected = protected_from(vec![data.clone()]);
    let hit = |arg: String| argv_reaches_protected(&[arg], &protected, &work).is_some();
    assert!(hit(format!("{}:/x", work.join("vault").display())));
    assert!(hit(format!(
        "type=bind,source={},target=/x",
        work.join("everything").display()
    )));
    assert!(hit(format!("{}:/x", work.join("vault/kronn.db").display())));
    assert!(!hit(format!("{}:/x", work.display())));
}

/// The forms a shell expands before Docker sees them, and the ones Docker
/// resolves against the working directory: `~`, `$HOME`, `${HOME}`, `./link`.
#[cfg(unix)]
#[test]
fn a_docker_volume_written_with_a_tilde_a_variable_or_a_relative_link_is_caught() {
    let root = tempfile::tempdir().unwrap();
    let home = real_path(root.path());
    let data = home.join(".config/kronn");
    std::fs::create_dir_all(&data).unwrap();
    let work = home.join("worktree");
    std::fs::create_dir(&work).unwrap();
    std::os::unix::fs::symlink(&data, work.join("link-to-data")).unwrap();
    std::os::unix::fs::symlink(home.join(".config"), work.join("link-to-config")).unwrap();
    let protected = protected_from(vec![data.clone()]);
    let other_cwd = std::env::temp_dir();
    let hit = |arg: &str, cwd: &Path| {
        argv_reaches_protected_from(&[arg.to_string()], &protected, cwd, Some(&home)).is_some()
    };

    // The very example of the doc, and its siblings.
    assert!(hit("~/.config:/x", &work));
    assert!(hit("~/.config/kronn:/x", &work));
    assert!(hit("~:/x", &work));
    assert!(hit("$HOME/.config:/x", &work));
    assert!(hit("${HOME}/.config/kronn:/x", &work));
    assert!(hit("type=bind,source=~/.config/kronn,target=/x", &work));
    assert!(hit("\"~/.config:/x\"", &work));
    // A relative source is the validation's, not the backend's.
    assert!(hit("./link-to-data:/x", &work));
    assert!(hit("./link-to-config/kronn:/x", &work));
    assert!(
        hit("link-to-data:/x", &work),
        "an existing bare name is a path"
    );
    assert!(hit("type=bind,source=./link-to-data,target=/x", &work));
    assert!(hit("../.config:/x", &work));
    assert!(hit(".:/x", &home.join(".config")));
    assert!(
        !hit("./link-to-data:/x", &other_cwd),
        "the link lives in the worktree"
    );
    // The short flag glued to its value (`-v~/…`), and the long forms.
    assert!(hit("-v~/.config/kronn:/x", &work));
    assert!(hit("-v$HOME/.config:/x", &work));
    assert!(hit("-v./link-to-data:/x", &work));
    assert!(hit("--volume=~/.config/kronn:/x", &work));
    assert!(hit("--volume=./link-to-data:/x", &work));
    assert!(hit(
        "--mount=type=bind,source=~/.config/kronn,target=/x",
        &work
    ));
    assert!(hit("--mount=type=bind,src=./link-to-data,dst=/x", &work));
    assert!(!hit("-v~/project:/x", &work));
    assert!(!hit("--volume=~/project:/x", &work));
    // What stays allowed.
    assert!(!hit("~/project:/x", &work));
    assert!(!hit("$HOME/worktree:/x", &work));
    assert!(!hit("./src:/x", &work));
    assert!(!hit("alpine:3", &work));
    assert!(!hit("$UNDEFINED_KRONN_TEST_VAR/x:/x", &work));
}

// ── the real sandbox ────────────────────────────────────────────────────────
// Applied for real where the host allows it (macOS locally, Linux in CI); a
// host that cannot apply a sandbox, or that is inside another one (macOS
// refuses a nested Seatbelt), skips with a line on stderr.

fn real_confinement(protected: Vec<PathBuf>) -> Option<LevelConfinement> {
    match Mechanism::usable_on_host() {
        Ok(mechanism) => Some(LevelConfinement::with(mechanism, protected)),
        Err(reason) => {
            eprintln!("skipped: no sandbox can be applied here ({reason})");
            None
        }
    }
}

async fn succeeds(confinement: Option<&LevelConfinement>, program: &str, args: &[&str]) -> bool {
    let cwd = std::env::temp_dir();
    let probe = probe(program, args, &cwd);
    let mut command = match confinement {
        Some(confinement) => confinement.command(&probe, ChildRoute::QuickExec),
        None => {
            let mut command = crate::core::cmd::async_cmd(program, ChildRoute::QuickExec);
            command.args(&probe.args).current_dir(&cwd);
            command
        }
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());
    command.status().await.map(|s| s.success()).unwrap_or(false)
}

/// A data dir shaped like Kronn's, with the files a command must not read.
fn fake_data_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "encryption_key",
        "kronn.db",
        "recovery.key",
        "human-admin-secret",
        "config.toml",
        "trust-proof.json",
    ] {
        std::fs::write(dir.path().join(name), "secret").unwrap();
    }
    std::fs::create_dir(dir.path().join("backups")).unwrap();
    std::fs::write(dir.path().join("backups/kronn.db.bak"), "secret").unwrap();
    dir
}

#[tokio::test]
#[serial]
async fn a_level_approved_line_cannot_read_the_encryption_key_or_the_database() {
    let data = fake_data_dir();
    let Some(confinement) = real_confinement(protected_from(vec![data.path().to_path_buf()]))
    else {
        return;
    };
    for name in [
        "encryption_key",
        "kronn.db",
        "recovery.key",
        "human-admin-secret",
        "config.toml",
        "trust-proof.json",
        "backups/kronn.db.bak",
    ] {
        let file = data.path().join(name);
        let file = file.to_string_lossy();
        assert!(
            succeeds(None, "cat", &[&file]).await,
            "control: {name} is readable without the sandbox"
        );
        assert!(
            !succeeds(Some(&confinement), "cat", &[&file]).await,
            "{name} was read from inside the sandbox"
        );
    }
}

/// Bubblewrap only: the data dir is not reachable through the host's `/proc`
/// (`/proc/<our pid>/root/...` crosses a mount namespace for the same uid).
/// Self-skips where bubblewrap is not usable.
#[cfg(target_os = "linux")]
#[tokio::test]
#[serial]
async fn a_level_approved_line_cannot_walk_around_the_tmpfs_through_proc() {
    let data = fake_data_dir();
    if !matches!(Mechanism::usable_on_host(), Ok(Mechanism::Bubblewrap)) {
        eprintln!("skipped: no bubblewrap can be applied here");
        return;
    }
    let confinement = LevelConfinement::with(
        Mechanism::Bubblewrap,
        protected_from(vec![data.path().to_path_buf()]),
    );
    let through_proc = format!(
        "/proc/{}/root{}/encryption_key",
        std::process::id(),
        data.path().display()
    );
    assert!(
        succeeds(None, "cat", &[&through_proc]).await,
        "control: the key is readable through /proc without the sandbox"
    );
    assert!(
        !succeeds(Some(&confinement), "cat", &[&through_proc]).await,
        "the key was read through /proc from inside the sandbox"
    );
}

#[tokio::test]
#[serial]
async fn a_level_approved_line_cannot_write_into_the_data_dir() {
    let data = fake_data_dir();
    let Some(confinement) = real_confinement(protected_from(vec![data.path().to_path_buf()]))
    else {
        return;
    };
    let target = data.path().join("planted");
    let script = format!("echo x > '{}'", target.display());
    assert!(!succeeds(Some(&confinement), "bash", &["-c", &script]).await);
    assert!(!target.exists(), "the sandbox let a command plant a file");
    let overwrite = format!(
        "echo x > '{}'",
        data.path().join("encryption_key").display()
    );
    assert!(!succeeds(Some(&confinement), "bash", &["-c", &overwrite]).await);
    assert_eq!(
        std::fs::read_to_string(data.path().join("encryption_key")).unwrap(),
        "secret"
    );
}

#[tokio::test]
#[serial]
async fn a_level_approved_line_reaches_the_network_and_the_kronn_api_port_but_not_the_key() {
    // A listener stands for the Kronn API: the agents' MCP bridge and the
    // owner's Quick Exec scripts go through it, so the sandbox leaves it open
    // (the human-only routes are guarded by the TOTP proof, not the network).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let data = fake_data_dir();
    let Some(confinement) = real_confinement(protected_from(vec![data.path().to_path_buf()]))
    else {
        return;
    };
    let connect = format!("exec 3<>/dev/tcp/127.0.0.1/{port}");
    assert!(
        succeeds(None, "bash", &["-c", &connect]).await,
        "control: the port answers outside the sandbox"
    );
    assert!(
        succeeds(Some(&confinement), "bash", &["-c", &connect]).await,
        "the sandbox cut the network or the Kronn API port"
    );
    let key = data.path().join("encryption_key");
    assert!(
        !succeeds(Some(&confinement), "cat", &[&key.to_string_lossy()]).await,
        "the encryption key was read from inside the sandbox"
    );
    drop(listener);
}

#[tokio::test]
#[serial]
async fn the_sandbox_leaves_the_rest_of_the_machine_to_the_command() {
    let data = fake_data_dir();
    let Some(confinement) = real_confinement(protected_from(vec![data.path().to_path_buf()]))
    else {
        return;
    };
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("README"), "hello").unwrap();
    let file = project.path().join("README");
    assert!(succeeds(Some(&confinement), "cat", &[&file.to_string_lossy()]).await);
    let out = project.path().join("out");
    let script = format!("echo x > '{}'", out.display());
    assert!(succeeds(Some(&confinement), "bash", &["-c", &script]).await);
    assert!(out.exists(), "other repositories stay writable");
}

#[test]
#[serial]
fn a_host_without_a_sandbox_reports_the_protection_unavailable_and_runs_unconfined() {
    set_protect_vault(true);
    set_test_unavailable(Some("`bwrap` is not installed"));
    match vault_protection() {
        VaultProtection::Unavailable(reason) => assert!(reason.contains("bwrap"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let missing = vault_protection().missing().unwrap();
    assert!(
        missing.contains("Kronn vault protection is unavailable on this machine"),
        "{missing}"
    );
    assert!(
        missing.contains("no sandbox-exec or bubblewrap"),
        "{missing}"
    );
    // The level is not refused: the command simply has no sandbox to run under.
    assert!(confinement().is_none());
    set_test_unavailable(None);
    assert!(vault_protection().is_active());
    assert!(confinement().is_some());
}

#[test]
#[serial]
fn the_setting_turns_the_protection_off_and_it_is_on_by_default() {
    set_test_unavailable(None);
    assert!(protect_vault_setting(), "on by default");
    assert!(vault_protection().is_active());
    set_protect_vault(false);
    assert_eq!(vault_protection(), VaultProtection::Disabled);
    assert!(
        confinement().is_none(),
        "off: level-granted commands run unconfined, as the owner chose"
    );
    assert!(vault_protection()
        .missing()
        .unwrap()
        .contains("protect_vault_from_agents"));
    set_protect_vault(true);
    assert!(confinement().is_some());
}

#[test]
fn only_macos_and_linux_have_a_mechanism() {
    // Any other host (Windows) has none, and is refused like a missing bwrap.
    assert_eq!(
        Mechanism::of_host().is_some(),
        cfg!(any(target_os = "macos", target_os = "linux"))
    );
}
