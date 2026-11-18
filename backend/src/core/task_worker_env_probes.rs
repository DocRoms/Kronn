//! KT-1747 — what the launch-time smoke check probes, from the project's own
//! toolchain roots (any depth, any layout) instead of Kronn's.

use super::*;

pub(super) fn check_rust(
    lane: &WorkerLane,
    env: &[(String, OsString)],
    runner: &dyn ProbeRunner,
) -> Capability {
    // Every Cargo root of the project, wherever it sits (a crate in a
    // sub-directory of a PHP or Node repository included).
    let roots = crate::core::project_toolchains::rust_roots(&lane.worktree);
    let Some(first) = roots.first() else {
        return Capability::no("not applicable: no Cargo.toml in the worktree");
    };
    if let Err(error) = probe_writable(&lane.cargo_target_dir) {
        return Capability::no(format!(
            "the lane target directory {} is not writable: {error}",
            lane.cargo_target_dir.display()
        ));
    }
    let probe = |dir: &Path, args: &[&str], timeout: Duration| ProbeCommand {
        program: "cargo".into(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: dir.to_path_buf(),
        env: env.to_vec(),
        timeout,
        priority: Priority::Normal,
        clean_env: false,
    };
    // KT-1747 review: nextest is probed only for a project that configures it;
    // one that tests with `cargo test` must not be marked red on a host
    // without nextest (the offline fetch below proves cargo itself runs).
    let uses_nextest = roots
        .iter()
        .any(|root| configures_nextest(&root.manifest_dir))
        || configures_nextest(&lane.worktree);
    if uses_nextest {
        let nextest = runner.run(&probe(
            &first.manifest_dir,
            &["nextest", "--version"],
            NEXTEST_TIMEOUT,
        ));
        if !nextest.success {
            return Capability::no(format!(
                "`cargo nextest --version` failed: {}",
                nextest.detail
            ));
        }
    }
    let runner_name = if uses_nextest {
        "cargo nextest"
    } else {
        "cargo test"
    };
    // A cold `cargo check` would compile the whole dependency graph; resolving
    // it offline from the cache proves the part a sandbox actually breaks.
    // One root that cannot is named; the others stay usable.
    let mut failed = Vec::new();
    let mut usable = 0usize;
    for root in &roots {
        let fetch = runner.run(&probe(
            &root.manifest_dir,
            &["fetch", "--offline", "--locked"],
            OFFLINE_FETCH_TIMEOUT,
        ));
        if fetch.success {
            usable += 1;
        } else {
            failed.push(format!("`{}`: {}", root.label(), fetch.detail));
        }
    }
    if usable == 0 {
        return Capability::no(format!(
            "offline dependency resolution failed (`cargo fetch --offline --locked`) in {}",
            failed.join("; ")
        ));
    }
    let mut reason = format!(
        "{runner_name} and the offline dependency cache work; lane target {}",
        lane.cargo_target_dir.display()
    );
    if !failed.is_empty() {
        reason.push_str(&format!(
            "; offline resolution failed in {}",
            failed.join("; ")
        ));
    }
    Capability::yes(reason)
}

/// Does `dir` configure cargo-nextest (`.config/nextest.toml`, nextest's own
/// configuration file)?
fn configures_nextest(dir: &Path) -> bool {
    dir.join(".config").join("nextest.toml").is_file()
}

/// Does the `package.json` in `dir` name vitest as a dependency?
fn declares_vitest(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("package.json")).is_ok_and(|text| text.contains("\"vitest\""))
}

pub fn check_frontend(
    lane: &WorkerLane,
    provisioned: &Provisioned,
    env: &[(String, OsString)],
    runner: &dyn ProbeRunner,
) -> Capability {
    use crate::core::project_toolchains::{detect_toolchains, ToolchainKind};
    // Every Node root of the project, at any depth (a root app, a package under
    // `assets/`), not only `frontend/`.
    let roots: Vec<_> = detect_toolchains(&lane.worktree)
        .into_iter()
        .filter(|root| matches!(root.kind, ToolchainKind::Node(_)))
        .collect();
    if roots.is_empty() {
        return Capability::no("not applicable: the worktree has no package.json");
    }
    let frontend_path = format!("{FRONTEND_DIR}/{NODE_MODULES}");
    let state_of = |path: &str| -> Option<&FrontendDeps> {
        if path == frontend_path {
            Some(&provisioned.frontend_deps)
        } else {
            provisioned
                .dependency_layers
                .iter()
                .find(|layer| layer.path == path)
                .map(|layer| &layer.state)
        }
    };
    let mut blockers: Vec<String> = Vec::new();
    let mut missing_vitest: Vec<String> = Vec::new();
    for root in &roots {
        let dir = lane.worktree.join(&root.rel_dir);
        let path = root
            .dependency_path()
            .unwrap_or_else(|| NODE_MODULES.into());
        // A workspace package's dependencies are hoisted to its workspace
        // root: its own missing directory is not a blocker, and its vitest
        // may be the root's.
        let workspace_dir = root
            .workspace
            .as_ref()
            .map(|workspace| lane.worktree.join(workspace));
        match state_of(&path) {
            Some(FrontendDeps::SourceMissing) if workspace_dir.is_some() => {}
            Some(FrontendDeps::SourceMissing) => {
                blockers.push(format!(
                    "the main checkout has no {path} to reuse and a worker never installs from \
                     the network"
                ));
                continue;
            }
            Some(FrontendDeps::Failed(error)) => {
                blockers.push(format!("{path} could not be linked: {error}"));
                continue;
            }
            _ => {}
        }
        let bin_vitest = |base: &Path| base.join(NODE_MODULES).join(".bin").join("vitest");
        let vitest = std::iter::once(bin_vitest(&dir))
            .chain(workspace_dir.as_deref().map(bin_vitest))
            .find(|candidate| candidate.exists())
            .unwrap_or_else(|| bin_vitest(&dir));
        if !vitest.exists() {
            if declares_vitest(&dir) {
                missing_vitest.push(format!("{path}/.bin/vitest is missing"));
            }
            continue;
        }
        return probe_vitest(&vitest, &dir, &path, env, runner);
    }
    if let Some(reason) = blockers.into_iter().chain(missing_vitest).next() {
        return Capability::no(reason);
    }
    // Node roots that do not use vitest run their own tests: nothing to probe,
    // and nothing to tell the worker is unavailable.
    Capability::no("not applicable: no Node root of the worktree uses vitest")
}

/// The two vitest probes of one Node root.
fn probe_vitest(
    vitest: &Path,
    dir: &Path,
    path: &str,
    env: &[(String, OsString)],
    runner: &dyn ProbeRunner,
) -> Capability {
    let outcome = runner.run(&ProbeCommand {
        program: vitest.to_string_lossy().into_owned(),
        args: vec!["--version".into()],
        cwd: dir.to_path_buf(),
        env: env.to_vec(),
        timeout: VITEST_TIMEOUT,
        priority: Priority::Normal,
        clean_env: false,
    });
    if !outcome.success {
        return Capability::no(format!("`vitest --version` failed: {}", outcome.detail));
    }
    let version = outcome.detail;
    // `--version` never loads the config, which is where Vite writes
    // (`node_modules/.vite-temp`) and where a lane failed with EPERM
    // (KT-1266): a run that selects no test file loads it and exits.
    let config = runner.run(&ProbeCommand {
        program: vitest.to_string_lossy().into_owned(),
        args: vec![
            "run".into(),
            "--passWithNoTests".into(),
            VITEST_CONFIG_PROBE_FILTER.into(),
        ],
        cwd: dir.to_path_buf(),
        env: env.to_vec(),
        timeout: VITEST_CONFIG_TIMEOUT,
        priority: Priority::Normal,
        clean_env: false,
    });
    if config.success {
        Capability::yes(format!(
            "vitest {version} runs from {path} and loads its config in the lane"
        ))
    } else {
        Capability::no(format!(
            "vitest cannot load its config in the lane (`vitest run --passWithNoTests`): {}",
            config.detail
        ))
    }
}
