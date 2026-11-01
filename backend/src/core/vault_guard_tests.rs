use super::*;
use crate::core::task_worker_sandbox::Mechanism;
use serial_test::serial;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

fn vault() -> Vec<PathBuf> {
    vec![PathBuf::from("/data/kronn")]
}

#[test]
fn a_claude_launch_with_its_own_sandbox_is_never_wrapped_the_others_are() {
    let worker = strings(&[
        "--print",
        "--settings",
        r#"{"sandbox":{"enabled":true,"failIfUnavailable":true}}"#,
    ]);
    assert_eq!(
        launch_sandbox("claude", Some("@anthropic-ai/claude-code"), &worker),
        LaunchSandbox::Own
    );
    // Full access, an audit, a plain discussion: the CLI runs with the user's rights.
    for args in [
        strings(&["--print", "--dangerously-skip-permissions"]),
        strings(&["--print", "--allowedTools=mcp__kronn-internal"]),
        // The deny rules of the vault are not a sandbox.
        strings(&["--settings", r#"{"permissions":{"deny":["Read(//x/**)"]}}"#]),
    ] {
        assert_eq!(
            launch_sandbox("claude", None, &args),
            LaunchSandbox::None,
            "{args:?}"
        );
    }
}

#[test]
fn a_codex_launch_keeps_its_own_sandbox_unless_the_launch_turned_it_off() {
    for (args, mode) in [
        (
            strings(&["exec", "--skip-git-repo-check", "p"]),
            CodexMode::ReadOnly,
        ),
        (
            strings(&["exec", "--sandbox=workspace-write", "p"]),
            CodexMode::WorkspaceWrite,
        ),
        (
            strings(&["exec", "--sandbox", "workspace-write", "p"]),
            CodexMode::WorkspaceWrite,
        ),
        (
            strings(&["exec", "--sandbox", "read-only", "p"]),
            CodexMode::ReadOnly,
        ),
    ] {
        assert_eq!(
            launch_sandbox("codex", Some("@openai/codex"), &args),
            LaunchSandbox::Codex(mode),
            "{args:?}"
        );
    }
    for args in [
        strings(&["exec", "--sandbox=danger-full-access", "p"]),
        strings(&["exec", "--sandbox", "danger-full-access", "p"]),
        strings(&["exec", "-c", "sandbox_mode=\"danger-full-access\"", "p"]),
        strings(&["exec", "--dangerously-bypass-approvals-and-sandbox", "p"]),
    ] {
        assert_eq!(
            launch_sandbox("codex", None, &args),
            LaunchSandbox::None,
            "{args:?}"
        );
    }
}

#[test]
fn an_acp_runtime_is_always_wrapped() {
    for binary in ["opencode", "gemini", "copilot", "kiro-cli", "vibe-acp"] {
        assert_eq!(
            launch_sandbox(binary, None, &strings(&["acp"])),
            LaunchSandbox::None,
            "{binary}"
        );
    }
}

#[test]
fn the_seatbelt_wrap_closes_the_data_dir_and_nothing_else() {
    let confinement = LevelConfinement::with(Mechanism::Seatbelt, vault());
    let (program, args, outcome) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::None,
        &LaunchRoots::default(),
        "claude".into(),
        strings(&["--print", "hello"]),
    );
    assert_eq!(outcome, Confined::Wrapped);
    assert_eq!(program, "/usr/bin/sandbox-exec");
    assert_eq!(args[0], "-p");
    assert!(args[1].starts_with("(version 1)\n(allow default)\n"));
    assert!(args[1].contains("(deny file-read* file-write*\n  (subpath \"/data/kronn\")\n)"));
    // Allow by default: the network and the Kronn API port are not touched.
    assert!(!args[1].contains("network"));
    assert_eq!(&args[2..], ["claude", "--print", "hello"]);
}

#[test]
fn the_bubblewrap_wrap_hides_the_data_dir_keeps_the_network_and_lets_the_agent_lead_its_group() {
    let data = tempfile::tempdir().unwrap();
    let confinement =
        LevelConfinement::with(Mechanism::Bubblewrap, vec![data.path().to_path_buf()]);
    let (program, args, outcome) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::None,
        &LaunchRoots::default(),
        "codex".into(),
        strings(&["exec", "p"]),
    );
    assert_eq!(outcome, Confined::Wrapped);
    assert_eq!(program, "bwrap");
    let tmpfs = args.iter().position(|arg| arg == "--tmpfs").unwrap();
    // The hidden path is spelled the way the OS does (`/tmp` is `/private/tmp` on macOS).
    let spelled = data.path().canonicalize().unwrap();
    assert_eq!(args[tmpfs + 1], spelled.to_string_lossy());
    assert!(args.contains(&"--share-net".to_string()));
    assert!(args.contains(&"--die-with-parent".to_string()));
    // The agent is a process-group leader; `setsid` would refuse it.
    assert!(!args.contains(&"--new-session".to_string()));
    assert_eq!(&args[args.len() - 3..], ["codex", "exec", "p"]);
}

fn codex_roots() -> LaunchRoots {
    LaunchRoots {
        work_dir: PathBuf::from("/work/tree"),
        codex_home: Some(PathBuf::from("/home/u/.codex")),
        npm_cache: None,
        temp: vec![PathBuf::from("/tmp")],
    }
}

/// The text between a rule's opening line and its closing parenthesis.
fn rule<'a>(profile: &'a str, head: &str) -> Vec<&'a str> {
    profile
        .split(head)
        .skip(1)
        .map(|rest| rest.split("\n)\n").next().unwrap_or(""))
        .collect()
}

#[test]
fn a_replaced_codex_read_only_sandbox_writes_only_tmp_and_codex_home() {
    let profile = codex_seatbelt_profile(CodexMode::ReadOnly, &codex_roots(), &vault());
    assert!(profile.starts_with("(version 1)\n(allow default)\n(deny file-write*)\n"));
    let allowed = rule(&profile, "(allow file-write*\n").join("\n");
    assert!(allowed.contains("(subpath \"/tmp\")"), "{allowed}");
    assert!(
        allowed.contains("(subpath \"/home/u/.codex\")"),
        "{allowed}"
    );
    // Not the worktree, which only workspace-write may write.
    assert!(!allowed.contains("/work/tree"), "{allowed}");
    // The data directory is denied on top; the network is not mentioned.
    assert!(profile.ends_with("(deny file-read* file-write*\n  (subpath \"/data/kronn\")\n)\n"));
    assert!(!profile.contains("network"));
}

#[test]
fn a_replaced_codex_workspace_write_sandbox_adds_the_worktree_but_not_its_git_dir() {
    let profile = codex_seatbelt_profile(CodexMode::WorkspaceWrite, &codex_roots(), &vault());
    let allowed = rule(&profile, "(allow file-write*\n").join("\n");
    assert!(allowed.contains("(subpath \"/work/tree\")"), "{allowed}");
    // A later rule wins: the worktree's repository metadata is denied after it.
    let allow_at = profile.find("(subpath \"/work/tree\")").unwrap();
    let git_at = profile
        .find("(deny file-write*\n  (subpath \"/work/tree/.git\")")
        .expect("the .git dir stays read-only");
    assert!(allow_at < git_at);
    assert!(profile.ends_with("(subpath \"/data/kronn\")\n)\n"));
}

#[test]
fn the_bubblewrap_replacement_binds_the_disk_read_only_and_hides_the_data_dir_last() {
    let work = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    std::fs::create_dir(work.path().join(".git")).unwrap();
    let roots = LaunchRoots {
        work_dir: work.path().to_path_buf(),
        codex_home: Some(home.path().to_path_buf()),
        npm_cache: None,
        temp: Vec::new(),
    };
    let protected = vec![data.path().to_path_buf()];
    let read_only = codex_bubblewrap_args(CodexMode::ReadOnly, &roots, &protected);
    assert_eq!(&read_only[..3], ["--ro-bind", "/", "/"]);
    let binds = |args: &[String], dir: &Path| {
        args.windows(3).any(|w| {
            w[0] == "--bind" && w[1] == dir.to_string_lossy() && w[2] == dir.to_string_lossy()
        })
    };
    assert!(binds(&read_only, home.path()));
    assert!(
        !binds(&read_only, work.path()),
        "read-only writes no worktree"
    );
    let writing = codex_bubblewrap_args(CodexMode::WorkspaceWrite, &roots, &protected);
    assert!(binds(&writing, work.path()));
    let git = work.path().join(".git");
    assert!(writing.windows(3).any(|w| {
        w[0] == "--ro-bind" && w[1] == git.to_string_lossy() && w[2] == git.to_string_lossy()
    }));
    for args in [&read_only, &writing] {
        let tmpfs = args.iter().position(|arg| arg == "--tmpfs").unwrap();
        let spelled = data.path().canonicalize().unwrap();
        assert_eq!(args[tmpfs + 1], spelled.to_string_lossy());
        assert!(args.contains(&"--share-net".to_string()));
        assert!(!args.contains(&"--unshare-net".to_string()));
        // Own PID namespace and a fresh /proc: no walking around the tmpfs.
        assert!(args.contains(&"--unshare-pid".to_string()));
        let proc_mount = args.iter().position(|arg| arg == "--proc").unwrap();
        assert_eq!(args[proc_mount + 1], "/proc");
        // The tmpfs follows every bind: it wins over a root that contains it.
        assert!(args.iter().rposition(|a| a == "--bind").unwrap_or(0) < tmpfs);
    }
}

#[test]
fn codex_is_replaced_not_nested_and_keeps_its_arguments() {
    let confinement = LevelConfinement::with(Mechanism::Seatbelt, vault());
    let (program, args, outcome) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::Codex(CodexMode::WorkspaceWrite),
        &codex_roots(),
        "codex".into(),
        strings(&[
            "exec",
            "-c",
            "mcp_servers={}",
            "--sandbox=workspace-write",
            "--ignore-rules",
            "the prompt",
        ]),
    );
    assert_eq!(outcome, Confined::Replaced);
    assert_eq!(program, "/usr/bin/sandbox-exec");
    let at = args.iter().position(|arg| arg == "codex").unwrap();
    assert_eq!(
        &args[at..],
        [
            "codex",
            "exec",
            "--sandbox=danger-full-access",
            "-c",
            "mcp_servers={}",
            "--ignore-rules",
            "the prompt"
        ]
    );
    // The spelled-apart form is replaced too, never doubled.
    let (_, args, _) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::Codex(CodexMode::ReadOnly),
        &codex_roots(),
        "codex".into(),
        strings(&["exec", "--sandbox", "read-only", "p"]),
    );
    let at = args.iter().position(|arg| arg == "codex").unwrap();
    assert_eq!(
        &args[at..],
        ["codex", "exec", "--sandbox=danger-full-access", "p"]
    );
    // Not an `exec` launch: left alone rather than half-rewritten.
    let (_, args, outcome) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::Codex(CodexMode::ReadOnly),
        &codex_roots(),
        "codex".into(),
        strings(&["login"]),
    );
    assert_eq!(args, strings(&["login"]));
    assert!(matches!(outcome, Confined::Unchanged(_)));
}

#[test]
fn nothing_is_replaced_without_a_confinement() {
    let (_, args, outcome) = wrap_launch(
        None,
        LaunchSandbox::Codex(CodexMode::ReadOnly),
        &codex_roots(),
        "codex".into(),
        strings(&["exec", "p"]),
    );
    assert_eq!(args, strings(&["exec", "p"]), "Codex keeps its own sandbox");
    assert!(matches!(outcome, Confined::Unchanged(_)));
}

#[test]
fn nothing_is_wrapped_without_a_confinement_or_for_a_cli_with_its_own_sandbox() {
    let (program, args, outcome) = wrap_launch(
        None,
        LaunchSandbox::None,
        &LaunchRoots::default(),
        "claude".into(),
        strings(&["--print"]),
    );
    assert_eq!((program.as_str(), args), ("claude", strings(&["--print"])));
    assert!(matches!(outcome, Confined::Unchanged(_)));

    let confinement = LevelConfinement::with(Mechanism::Seatbelt, vault());
    let (program, args, outcome) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::Own,
        &LaunchRoots::default(),
        "claude".into(),
        strings(&["--print"]),
    );
    assert_eq!((program.as_str(), args), ("claude", strings(&["--print"])));
    assert!(matches!(outcome, Confined::Unchanged(_)));
}

#[test]
#[serial]
fn no_sandbox_on_the_host_or_the_setting_off_means_no_confinement_and_no_refusal() {
    set_test_wrap(true);
    crate::core::trust_sandbox::set_test_unavailable(Some("`bwrap` is not installed"));
    let missing = launch_confinement();
    crate::core::trust_sandbox::set_test_unavailable(None);

    crate::core::trust_sandbox::set_protect_vault(false);
    let off = launch_confinement();
    let off_settings = claude_vault_settings();
    crate::core::trust_sandbox::set_protect_vault(true);

    let on = launch_confinement();
    set_test_wrap(false);
    assert!(missing.is_none(), "the launch runs as it did");
    assert!(off.is_none());
    assert!(off_settings.is_none(), "no rule bridles the CLI either");
    assert!(on.is_some());
    assert!(
        launch_confinement().is_none(),
        "unit tests of other modules spawn unconfined"
    );
}

#[test]
#[serial]
fn the_confinement_hides_the_data_dir_of_this_install() {
    set_test_wrap(true);
    let confinement = launch_confinement().expect("the setting is on by default");
    set_test_wrap(false);
    let data = crate::core::config::config_dir().unwrap();
    assert!(confinement.protected().contains(&data));
}

#[test]
#[serial]
fn every_claude_launch_denies_the_data_dir_to_its_file_tools() {
    let data = crate::core::config::config_dir().unwrap();
    let settings: serde_json::Value =
        serde_json::from_str(&claude_vault_settings().expect("on by default")).unwrap();
    let deny: Vec<&str> = settings["permissions"]["deny"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|rule| rule.as_str())
        .collect();
    for tool in ["Read", "Edit", "Write"] {
        // Claude spells a path from the filesystem root with two slashes.
        let rule = format!("{tool}(/{}/**)", data.to_string_lossy());
        assert!(deny.contains(&rule.as_str()), "{rule} in {deny:?}");
    }
    assert!(
        settings.get("sandbox").is_none(),
        "the rules are not a sandbox: they would nest in nothing and bridle nothing"
    );
}

/// The wrap is more than argv: a real shell under it cannot read the data dir,
/// and still reads and writes everywhere else. Only where this process may
/// start a sandbox of its own.
#[cfg(target_os = "macos")]
#[test]
#[serial]
fn a_shell_under_the_wrap_loses_the_data_dir_and_keeps_the_rest() {
    let usable = Mechanism::Seatbelt.check_usable().is_ok();
    if !usable {
        eprintln!("skipped: a sandbox cannot be applied from here");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(data.path().join("encryption_key"), "secret").unwrap();
    std::fs::write(elsewhere.path().join("note"), "open").unwrap();
    let protected = vec![
        data.path().to_path_buf(),
        crate::core::task_worker_sandbox::real_path(data.path()),
    ];
    let confinement = LevelConfinement::with(Mechanism::Seatbelt, protected);
    let script = format!(
        "cat '{}' >/dev/null 2>&1 && echo LEAK; cat '{}'",
        data.path().join("encryption_key").display(),
        elsewhere.path().join("note").display()
    );
    let (program, args, _) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::None,
        &LaunchRoots::default(),
        "/bin/sh".into(),
        strings(&["-c", &script]),
    );
    let output = crate::core::cmd::sync_tool_cmd(program)
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("LEAK"), "{stdout}");
    assert!(stdout.contains("open"), "{stdout}");
}

/// The replacement of Codex's sandbox is more than argv: under its profile a
/// real shell writes what the mode allows and nothing else, and never reads the
/// data dir. Only where this process may start a sandbox of its own.
#[cfg(target_os = "macos")]
#[test]
#[serial]
fn a_shell_under_the_codex_replacement_keeps_the_mode_and_loses_the_data_dir() {
    if Mechanism::Seatbelt.check_usable().is_err() {
        eprintln!("skipped: a sandbox cannot be applied from here");
        return;
    }
    let data = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::create_dir(work.path().join(".git")).unwrap();
    std::fs::write(data.path().join("encryption_key"), "secret").unwrap();
    std::fs::write(elsewhere.path().join("note"), "open").unwrap();
    let roots = LaunchRoots {
        work_dir: work.path().to_path_buf(),
        codex_home: Some(home.path().to_path_buf()),
        npm_cache: None,
        temp: vec![temp.path().to_path_buf()],
    };
    let protected = vec![
        data.path().to_path_buf(),
        crate::core::task_worker_sandbox::real_path(data.path()),
    ];
    let git = work.path().join(".git");
    let tries = [work.path(), temp.path(), home.path(), git.as_path()];
    let mut script = String::new();
    for (at, dir) in tries.iter().enumerate() {
        script.push_str(&format!(
            "echo x > '{}/w' 2>/dev/null && echo WROTE-{at}; ",
            dir.display()
        ));
    }
    script.push_str(&format!(
        "cat '{}' 2>/dev/null && echo LEAK; cat '{}'",
        data.path().join("encryption_key").display(),
        elsewhere.path().join("note").display()
    ));
    for mode in [CodexMode::ReadOnly, CodexMode::WorkspaceWrite] {
        let output = crate::core::cmd::sync_tool_cmd(Mechanism::Seatbelt.launcher())
            .args(["-p", &codex_seatbelt_profile(mode, &roots, &protected)])
            .args(["/bin/sh", "-c", &script])
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(!out.contains("LEAK"), "{mode:?}: {out}");
        assert!(out.contains("open"), "{mode:?} reads the rest: {out}");
        assert!(out.contains("WROTE-1"), "{mode:?} writes tmp: {out}");
        assert!(out.contains("WROTE-2"), "{mode:?} writes its home: {out}");
        assert!(!out.contains("WROTE-3"), "{mode:?} and .git: {out}");
        assert_eq!(
            out.contains("WROTE-0"),
            mode == CodexMode::WorkspaceWrite,
            "{mode:?} and the worktree: {out}"
        );
    }
}

/// The paths a profile's `(deny file-read* …)` rule closes.
fn denied_reads(profile: &str) -> Vec<String> {
    rule(profile, "(deny file-read* file-write*\n")
        .iter()
        .flat_map(|body| body.lines())
        .filter_map(|line| line.trim().strip_prefix("(subpath \""))
        .filter_map(|line| line.strip_suffix("\")"))
        .map(str::to_owned)
        .collect()
}

/// KT-1495 — the data dir and the work dir are siblings under one temp root
/// (the live probe, and any install whose data dir is a neighbour of a
/// project): the profile closes the data dir and no ancestor of the work dir.
#[test]
fn a_work_dir_next_to_the_data_dir_is_not_closed_by_any_profile() {
    let root = PathBuf::from("/private/var/folders/xx/T");
    let data = root.join(".tmpXeimmm");
    let work = root.join(".tmpVWS1Jf");
    let protected = vec![data.clone()];
    let roots = LaunchRoots {
        work_dir: work.clone(),
        codex_home: None,
        npm_cache: None,
        temp: vec![root.clone()],
    };
    let profiles = [
        crate::core::trust_sandbox::seatbelt_profile(&protected),
        codex_seatbelt_profile(CodexMode::ReadOnly, &roots, &protected),
        codex_seatbelt_profile(CodexMode::WorkspaceWrite, &roots, &protected),
    ];
    for profile in profiles {
        let denied = denied_reads(&profile);
        assert_eq!(denied, [data.to_string_lossy()], "{profile}");
        assert!(denied.iter().all(|dir| !work.starts_with(dir)), "{profile}");
    }
    let args = codex_bubblewrap_args(CodexMode::ReadOnly, &roots, &protected);
    assert!(!args.iter().any(|arg| arg == work.to_str().unwrap()));
}

/// The same, with a real shell: a command started in the work dir reads its
/// files by absolute path (both spellings of the temp root), by relative path,
/// and lists the directory, while the sibling data dir stays closed.
#[cfg(target_os = "macos")]
#[test]
#[serial]
fn a_shell_in_a_work_dir_next_to_the_data_dir_reads_it_and_loses_the_data_dir() {
    if Mechanism::Seatbelt.check_usable().is_err() {
        eprintln!("skipped: a sandbox cannot be applied from here");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join(".tmpData");
    let work = root.path().join(".tmpWork");
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir(&work).unwrap();
    std::fs::write(data.join("probe-notes-a.txt"), "vault").unwrap();
    std::fs::write(work.join("control.txt"), "control").unwrap();
    let real_work = work.canonicalize().unwrap();
    let confinement = LevelConfinement::with(
        Mechanism::Seatbelt,
        vec![
            data.clone(),
            crate::core::task_worker_sandbox::real_path(&data),
        ],
    );
    let script = format!(
        "cat '{}' >/dev/null 2>&1 && echo LEAK; cat '{}'; cat '{}'; cat control.txt; ls . >/dev/null && echo LISTED",
        data.join("probe-notes-a.txt").display(),
        work.join("control.txt").display(),
        real_work.join("control.txt").display(),
    );
    let (program, args, _) = wrap_launch(
        Some(&confinement),
        LaunchSandbox::None,
        &LaunchRoots::default(),
        "/bin/sh".into(),
        strings(&["-c", &script]),
    );
    let output = crate::core::cmd::sync_tool_cmd(program)
        .args(args)
        .current_dir(&real_work)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("LEAK"), "{stdout}");
    assert_eq!(stdout.matches("control").count(), 3, "{stdout}");
    assert!(stdout.contains("LISTED"), "{stdout}");
}

/// A read-only launch carries its own `--settings`; the vault rules join that
/// document instead of a second flag Claude would not merge.
#[test]
#[serial]
fn the_vault_rules_fold_into_the_policy_settings_document() {
    let own = serde_json::json!({
        "permissions": { "deny": ["Edit(//repo/**)"] },
        "sandbox": { "enabled": true, "filesystem": { "denyWrite": ["/repo"] } }
    })
    .to_string();
    let mut args = strings(&["--print"]);
    args.extend(["--settings".into(), claude_vault_settings().unwrap()]);
    args.extend(["--settings".into(), own, "prompt".into()]);
    fold_vault_into_claude_settings(&mut args);
    let documents: Vec<&String> = args
        .windows(2)
        .filter(|pair| pair[0] == "--settings")
        .map(|pair| &pair[1])
        .collect();
    assert_eq!(documents.len(), 1, "{args:?}");
    let settings: serde_json::Value = serde_json::from_str(documents[0]).unwrap();
    let deny = settings["permissions"]["deny"].as_array().unwrap();
    assert_eq!(deny[0], "Edit(//repo/**)");
    for rule in claude_permission_deny() {
        assert!(deny.iter().any(|d| d == rule.as_str()), "{rule}");
    }
    assert_eq!(settings["sandbox"]["filesystem"]["denyWrite"][0], "/repo");
    assert_eq!(
        settings["sandbox"]["filesystem"]["denyRead"],
        serde_json::json!(claude_sandbox_deny_read())
    );
    // Folding twice adds nothing.
    let once = args.clone();
    fold_vault_into_claude_settings(&mut args);
    assert_eq!(args, once);

    // Without a policy document the standalone vault flag stays as it was.
    let mut alone = strings(&["--print", "--settings"]);
    alone.push(claude_vault_settings().unwrap());
    let before = alone.clone();
    fold_vault_into_claude_settings(&mut alone);
    assert_eq!(alone, before);
}
