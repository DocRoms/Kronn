//! KT-1656 — the tier of home credential stores a launch loses follows its real
//! trust level, and does not depend on the "protect the vault" setting.

use super::*;
use crate::core::task_worker_sandbox::real_path;
use serial_test::serial;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

const EVERY_FAMILY: [AgentFamily; 7] = [
    AgentFamily::Claude,
    AgentFamily::Codex,
    AgentFamily::Gemini,
    AgentFamily::Copilot,
    AgentFamily::Kiro,
    AgentFamily::Vibe,
    AgentFamily::OpenCode,
];

const SHARED: [&str; 4] = [".ssh", ".aws", ".config/gh", ".npmrc"];

/// Runs `run` with `home` as the process's `HOME`.
fn at_home<T>(home: &Path, run: impl FnOnce() -> T) -> T {
    let previous = crate::core::child_env::var("HOME").ok();
    crate::core::child_env::set_var("HOME", home);
    let result = run();
    match previous {
        Some(value) => crate::core::child_env::set_var("HOME", value),
        None => crate::core::child_env::remove_var("HOME"),
    }
    result
}

/// A home with the login of every agent and some shared tools' files.
fn credential_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    for rel in [
        ".gemini/settings.json",
        ".codex/auth.json",
        ".claude.json",
        ".claude/.credentials.json",
        ".claude/settings.json",
        ".copilot/config.json",
        ".kiro/credentials",
        ".vibe/config.toml",
        ".local/share/opencode/auth.json",
        ".ssh/id_ed25519",
        ".aws/credentials",
        ".config/gh/hosts.yml",
        ".npmrc",
    ] {
        let path = home.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "secret").unwrap();
    }
    home
}

fn hides(confinement: &LevelConfinement, home: &Path, rel: &str) -> bool {
    confinement.protected().contains(&home.join(rel))
}

/// What the agent's own process needs to read.
fn own_store(family: AgentFamily) -> &'static str {
    match family {
        AgentFamily::Claude => ".claude/settings.json",
        AgentFamily::Codex => ".codex",
        AgentFamily::Gemini => ".gemini",
        AgentFamily::Copilot => ".copilot",
        AgentFamily::Kiro => ".kiro",
        AgentFamily::Vibe => ".vibe",
        AgentFamily::OpenCode => ".local/share/opencode",
        other => unreachable!("{other:?} owns no store"),
    }
}

#[test]
fn only_a_full_access_launch_without_a_sandbox_of_its_own_keeps_the_shared_stores() {
    use home_credentials::Reach::{FullAccess, Sandboxed};
    for (sandbox, full_access, expected) in [
        (LaunchSandbox::None, true, FullAccess),
        // A restricted launch, whatever the agent: the kind of sandbox is not
        // the trust level.
        (LaunchSandbox::None, false, Sandboxed),
        (LaunchSandbox::Own, true, Sandboxed),
        (LaunchSandbox::Own, false, Sandboxed),
        (LaunchSandbox::Codex(CodexMode::ReadOnly), true, Sandboxed),
        (
            LaunchSandbox::Codex(CodexMode::WorkspaceWrite),
            false,
            Sandboxed,
        ),
    ] {
        assert_eq!(
            launch_reach(sandbox, full_access),
            expected,
            "{sandbox:?} full_access={full_access}"
        );
    }
}

#[test]
#[serial]
fn a_restricted_launch_of_any_agent_hides_the_shared_stores_a_full_access_one_keeps_them() {
    let home = credential_home();
    set_test_wrap(true);
    at_home(home.path(), || {
        let home = real_path(home.path());
        for family in EVERY_FAMILY {
            let restricted = launch_confinement_for(family, LaunchSandbox::None, false)
                .unwrap_or_else(|| panic!("{family:?}: no confinement"));
            let open = launch_confinement_for(family, LaunchSandbox::None, true)
                .unwrap_or_else(|| panic!("{family:?}: no confinement"));
            for rel in SHARED {
                assert!(
                    hides(&restricted, &home, rel),
                    "{family:?} restricted: {rel}"
                );
                assert!(!hides(&open, &home, rel), "{family:?} full access: {rel}");
            }
            // The other agents' stores are hidden in both, the agent's own in none.
            for other in EVERY_FAMILY {
                let rel = own_store(other);
                for (confinement, level) in [(&restricted, "restricted"), (&open, "full access")] {
                    assert_eq!(
                        hides(confinement, &home, rel),
                        other != family,
                        "{family:?} {level}: {rel} of {other:?}"
                    );
                }
            }
        }
        // A sandbox of the launch's own is restricted, whatever it is told.
        for sandbox in [
            LaunchSandbox::Own,
            LaunchSandbox::Codex(CodexMode::ReadOnly),
            LaunchSandbox::Codex(CodexMode::WorkspaceWrite),
        ] {
            let confinement = launch_confinement_for(AgentFamily::Codex, sandbox, true).unwrap();
            for rel in SHARED {
                assert!(hides(&confinement, &home, rel), "{sandbox:?}: {rel}");
            }
        }
    });
    set_test_wrap(false);
}

#[test]
#[serial]
fn the_stores_are_hidden_with_the_vault_setting_off_and_no_kronn_data_with_them() {
    let home = credential_home();
    set_test_wrap(true);
    crate::core::trust_sandbox::set_protect_vault(false);
    let results: Vec<_> = at_home(home.path(), || {
        EVERY_FAMILY
            .into_iter()
            .map(|family| {
                (
                    family,
                    launch_confinement_for(family, LaunchSandbox::None, false),
                    launch_confinement_for(family, LaunchSandbox::None, true),
                )
            })
            .collect()
    });
    // Nothing of the home to hide: the launch is not wrapped for nothing.
    let bare = tempfile::tempdir().unwrap();
    let nothing = at_home(bare.path(), || {
        // A host whose environment moves a store somewhere real has something to hide.
        home_credentials::hidden(
            Some(AgentFamily::Gemini),
            home_credentials::Reach::Sandboxed,
        )
        .is_empty()
        .then(|| launch_confinement_for(AgentFamily::Gemini, LaunchSandbox::None, false))
        .flatten()
    });
    crate::core::trust_sandbox::set_protect_vault(true);
    set_test_wrap(false);

    let home = real_path(home.path());
    let data = crate::core::config::config_dir().unwrap();
    for (family, restricted, open) in results {
        let restricted = restricted.unwrap_or_else(|| panic!("{family:?}: not confined"));
        let open = open.unwrap_or_else(|| panic!("{family:?}: not confined"));
        for other in EVERY_FAMILY.into_iter().filter(|other| *other != family) {
            let rel = own_store(other);
            assert!(
                hides(&restricted, &home, rel),
                "{family:?} restricted: {rel}"
            );
            assert!(hides(&open, &home, rel), "{family:?} full access: {rel}");
        }
        assert!(!hides(&restricted, &home, own_store(family)));
        assert!(hides(&restricted, &home, ".ssh"), "{family:?}");
        assert!(!hides(&open, &home, ".ssh"), "{family:?}");
        assert!(
            !restricted.protected().contains(&data) && !open.protected().contains(&data),
            "the setting is off: no Kronn data is hidden"
        );
    }
    assert!(nothing.is_none(), "{nothing:?}");
}

#[test]
#[serial]
fn with_the_vault_setting_on_the_data_dir_is_hidden_along_with_the_stores() {
    let home = credential_home();
    set_test_wrap(true);
    let (confinement, real_home) = at_home(home.path(), || {
        (
            launch_confinement_for(AgentFamily::Gemini, LaunchSandbox::None, false).unwrap(),
            real_path(home.path()),
        )
    });
    set_test_wrap(false);
    assert!(hides(&confinement, &real_home, ".codex"));
    let data = crate::core::config::config_dir().unwrap();
    assert!(confinement.protected().contains(&data));
}

/// The wrap carries every store: the argv a restricted Gemini, Kiro, Copilot or
/// Vibe launch really gets names `~/.ssh`, which a full-access one does not.
#[test]
#[serial]
fn the_wrapped_argv_of_a_restricted_launch_names_the_shared_stores() {
    let home = credential_home();
    set_test_wrap(true);
    at_home(home.path(), || {
        let ssh = real_path(home.path()).join(".ssh");
        let ssh = ssh.to_string_lossy().into_owned();
        for family in [
            AgentFamily::Gemini,
            AgentFamily::Kiro,
            AgentFamily::Copilot,
            AgentFamily::Vibe,
        ] {
            let argv = |full_access| {
                let (_, args, outcome) = wrap_launch(
                    launch_confinement_for(family, LaunchSandbox::None, full_access).as_ref(),
                    LaunchSandbox::None,
                    &LaunchRoots::default(),
                    "agent".into(),
                    strings(&["--flag"]),
                );
                assert_eq!(outcome, Confined::Wrapped, "{family:?}");
                args.join("\n")
            };
            assert!(argv(false).contains(&ssh), "{family:?} restricted");
            assert!(!argv(true).contains(&ssh), "{family:?} full access");
        }
    });
    set_test_wrap(false);
}
