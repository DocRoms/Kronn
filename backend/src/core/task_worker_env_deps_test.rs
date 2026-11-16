//! KT-1747 — a lane layers the dependency directory of every package root of the
//! project (any toolchain, any depth), read-only from the main checkout, unless
//! the project names its own list.

use super::*;
use crate::models::LaneWarmConfig;

struct Repo {
    _dir: tempfile::TempDir,
    main: PathBuf,
    worktree: PathBuf,
}

fn repo() -> Repo {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().canonicalize().unwrap();
    let worktree = main
        .join(".kronn")
        .join("worktrees")
        .join("task-kt-1747-abc12345");
    std::fs::create_dir_all(&worktree).unwrap();
    Repo {
        _dir: dir,
        main,
        worktree,
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

impl Repo {
    /// A PHP + Node project: Composer at the root, an npm package in `assets/front`.
    fn docroms_like(&self) {
        write(&self.worktree, "composer.json", "{}");
        write(&self.worktree, "assets/front/package.json", "{}");
        write(&self.main, "vendor/acme/lib/a.php", "<?php");
        write(
            &self.main,
            "assets/front/node_modules/left-pad/index.js",
            "x",
        );
    }

    fn lane(&self) -> WorkerLane {
        WorkerLane::for_worktree(&self.worktree).unwrap()
    }
}

fn state_of<'a>(provisioned: &'a Provisioned, path: &str) -> Option<&'a FrontendDeps> {
    provisioned
        .dependency_layers
        .iter()
        .find(|layer| layer.path == path)
        .map(|layer| &layer.state)
}

#[test]
fn every_detected_dependency_dir_is_layered_read_only_from_the_main_checkout() {
    let repo = repo();
    repo.docroms_like();
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert_eq!(provisioned.frontend_deps, FrontendDeps::NotApplicable);
    assert_eq!(
        state_of(&provisioned, "vendor"),
        Some(&FrontendDeps::Linked(repo.main.join("vendor")))
    );
    assert_eq!(
        state_of(&provisioned, "assets/front/node_modules"),
        Some(&FrontendDeps::Linked(
            repo.main.join("assets/front/node_modules")
        ))
    );
    // A real directory whose entries link to the main checkout's, plus the marker.
    let vendor = repo.worktree.join("vendor");
    assert!(vendor.is_dir() && !vendor.is_symlink());
    assert_eq!(
        std::fs::read_link(vendor.join("acme")).unwrap(),
        repo.main.join("vendor/acme")
    );
    assert!(vendor.join(".kronn-lane-layer").is_file());
    // Nothing was written into the main checkout.
    assert!(!repo.main.join("vendor/.kronn-lane-layer").exists());
    // The sources are readable by the sandbox and never writable.
    let reads = lane.read_roots(&SharedCaches::default());
    assert!(reads.contains(&repo.main.join("vendor")));
    assert!(reads.contains(&repo.main.join("assets/front/node_modules")));
    assert!(!lane
        .write_roots()
        .iter()
        .any(|root| root.starts_with(repo.main.join("vendor"))));
}

#[test]
fn a_project_names_the_dependency_dirs_to_layer_and_the_others_stay_untouched() {
    let repo = repo();
    repo.docroms_like();
    crate::core::task_worker_lane_settings::set_project(
        &repo.main,
        Some(&LaneWarmConfig {
            dependency_dirs: Some(vec!["./vendor/".to_string(), "../escape".to_string()]),
            ..LaneWarmConfig::default()
        }),
    );
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert!(matches!(
        state_of(&provisioned, "vendor"),
        Some(FrontendDeps::Linked(_))
    ));
    assert_eq!(state_of(&provisioned, "assets/front/node_modules"), None);
    assert!(!repo.worktree.join("assets/front/node_modules").exists());
    assert!(!repo.worktree.join("../escape").exists());
    assert!(provisioned
        .warnings
        .iter()
        .any(|warning| warning.contains("../escape")));
    crate::core::task_worker_lane_settings::set_project(&repo.main, None);
}

#[test]
fn a_dependency_dir_the_main_checkout_has_not_installed_is_reported_and_not_created() {
    let repo = repo();
    write(&repo.worktree, "composer.json", "{}");
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert_eq!(
        state_of(&provisioned, "vendor"),
        Some(&FrontendDeps::SourceMissing)
    );
    assert!(!repo.worktree.join("vendor").exists());
}

#[test]
fn a_dependency_dir_the_project_commits_is_never_replaced() {
    let repo = repo();
    write(&repo.worktree, "composer.json", "{}");
    write(&repo.worktree, "vendor/own/file.php", "mine");
    write(&repo.main, "vendor/acme/lib/a.php", "<?php");
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert_eq!(
        state_of(&provisioned, "vendor"),
        Some(&FrontendDeps::AlreadyPresent)
    );
    assert!(repo.worktree.join("vendor/own/file.php").is_file());
    assert!(!repo.worktree.join("vendor/acme").exists());
}

#[test]
fn kronns_own_frontend_keeps_its_dedicated_link_and_is_not_layered_twice() {
    let repo = repo();
    write(&repo.worktree, "frontend/package.json", "{}");
    write(
        &repo.main,
        "frontend/node_modules/.bin/vitest",
        "#!/bin/sh\n",
    );
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert!(matches!(provisioned.frontend_deps, FrontendDeps::Linked(_)));
    assert_eq!(state_of(&provisioned, "frontend/node_modules"), None);
}

#[test]
fn a_missing_dependency_dir_is_a_launch_warning_that_names_the_install_command() {
    let repo = repo();
    write(&repo.worktree, "composer.json", "{}");
    write(&repo.worktree, "assets/front/package.json", "{}");
    write(&repo.worktree, "assets/front/yarn.lock", "");
    write(&repo.worktree, "Podfile", "");
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    let warning_for = |dir: &str| {
        provisioned
            .warnings
            .iter()
            .find(|warning| warning.contains(dir))
            .cloned()
            .unwrap_or_default()
    };
    // Each layer the main checkout has not installed says what installs it, and
    // where: the lane never installs it itself.
    let vendor = warning_for("`vendor`");
    assert!(vendor.contains("composer install"), "{vendor}");
    assert!(vendor.contains("main checkout"), "{vendor}");
    let node = warning_for("assets/front/node_modules");
    assert!(node.contains("yarn install --frozen-lockfile"), "{node}");
    assert!(node.contains("assets/front"), "{node}");
    assert!(warning_for("`Pods`").contains("pod install"));
}

#[test]
fn kronns_own_frontend_missing_install_is_a_warning_too() {
    let repo = repo();
    write(&repo.worktree, "frontend/package.json", "{}");
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert_eq!(provisioned.frontend_deps, FrontendDeps::SourceMissing);
    assert!(
        provisioned
            .warnings
            .iter()
            .any(|warning| warning.contains("frontend/node_modules") && warning.contains("npm ci")),
        "{:?}",
        provisioned.warnings
    );
}

/// A Composer `vendor/` the way Composer writes it: the generated autoloader
/// maps the project's own namespace relative to `__DIR__`, and `bin/` holds a
/// symlink to a PHP script of a package.
#[cfg(unix)]
fn composer_project(repo: &Repo) {
    write(&repo.worktree, "composer.json", "{}");
    write(
        &repo.worktree,
        "src/Foo.php",
        "<?php function where() { return 'lane'; }",
    );
    write(
        &repo.main,
        "src/Foo.php",
        "<?php function where() { return 'main'; }",
    );
    write(
        &repo.main,
        "vendor/autoload.php",
        "<?php\nrequire __DIR__ . '/../src/Foo.php';\n",
    );
    write(
        &repo.main,
        "vendor/composer/autoload_static.php",
        "<?php // static map",
    );
    write(&repo.main, "vendor/acme/lib/a.php", "<?php");
    write(
        &repo.main,
        "vendor/phpunit/phpunit/phpunit",
        "#!/usr/bin/env php\n<?php echo 'phpunit';\n",
    );
    std::fs::create_dir_all(repo.main.join("vendor/bin")).unwrap();
    std::os::unix::fs::symlink(
        "../phpunit/phpunit/phpunit",
        repo.main.join("vendor/bin/phpunit"),
    )
    .unwrap();
    write(&repo.main, "vendor/bin/tool.sh", "#!/bin/sh\necho hi\n");
}

#[cfg(unix)]
#[test]
fn a_composer_vendor_layer_keeps_the_autoloader_and_bin_as_real_files_of_the_lane() {
    let repo = repo();
    composer_project(&repo);
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert!(matches!(
        state_of(&provisioned, "vendor"),
        Some(FrontendDeps::Linked(_))
    ));
    let vendor = repo.worktree.join("vendor");
    let is_link = |rel: &str| {
        std::fs::symlink_metadata(vendor.join(rel))
            .unwrap()
            .is_symlink()
    };
    // What decides which `src/` the project's classes load from is the lane's own.
    assert!(!is_link("autoload.php"));
    assert!(!is_link("composer"));
    assert!(!is_link("composer/autoload_static.php"));
    assert!(!is_link("bin"));
    assert!(!is_link("bin/tool.sh"));
    assert_eq!(
        std::fs::read_to_string(vendor.join("autoload.php")).unwrap(),
        std::fs::read_to_string(repo.main.join("vendor/autoload.php")).unwrap()
    );
    // A PHP bin is a proxy that tells the package where the LANE's autoloader is,
    // instead of a link PHP resolves back into the main checkout.
    assert!(!is_link("bin/phpunit"));
    let proxy = std::fs::read_to_string(vendor.join("bin/phpunit")).unwrap();
    assert!(proxy.contains("_composer_autoload_path"), "{proxy}");
    assert!(proxy.contains("/../autoload.php"), "{proxy}");
    assert!(proxy.contains("/../phpunit/phpunit/phpunit"), "{proxy}");
    // The packages themselves are still the main checkout's, linked.
    assert_eq!(
        std::fs::read_link(vendor.join("acme")).unwrap(),
        repo.main.join("vendor/acme")
    );
    assert_eq!(
        std::fs::read_link(vendor.join("phpunit")).unwrap(),
        repo.main.join("vendor/phpunit")
    );
    // Nothing was written into the main checkout.
    assert!(!repo.main.join("vendor/.kronn-lane-layer").exists());
}

#[cfg(unix)]
#[test]
fn a_php_class_of_the_project_resolves_from_the_lanes_own_src() {
    let probe = std::process::Command::new("php").arg("--version").output();
    if probe.map(|out| !out.status.success()).unwrap_or(true) {
        eprintln!("php is not installed: the file-level checks above carry this case");
        return;
    }
    let repo = repo();
    composer_project(&repo);
    repo.lane().provision(&SharedCaches::default());

    let out = std::process::Command::new("php")
        .current_dir(&repo.worktree)
        .args(["-r", "require 'vendor/autoload.php'; echo where();"])
        .output()
        .unwrap();

    assert_eq!(String::from_utf8_lossy(&out.stdout), "lane");
}

#[cfg(unix)]
#[test]
fn a_re_provisioned_lane_keeps_the_workers_own_autoloader_and_replaces_an_old_link() {
    let repo = repo();
    composer_project(&repo);
    let lane = repo.lane();
    lane.provision(&SharedCaches::default());
    let autoload = repo.worktree.join("vendor/autoload.php");
    std::fs::write(&autoload, "<?php // dumped by the worker").unwrap();

    lane.provision(&SharedCaches::default());

    assert_eq!(
        std::fs::read_to_string(&autoload).unwrap(),
        "<?php // dumped by the worker"
    );

    // A lane laid out by the previous version (a link into the main checkout)
    // is converted to the real copy.
    std::fs::remove_file(&autoload).unwrap();
    std::os::unix::fs::symlink(repo.main.join("vendor/autoload.php"), &autoload).unwrap();
    lane.provision(&SharedCaches::default());
    assert!(!std::fs::symlink_metadata(&autoload).unwrap().is_symlink());
}

#[cfg(unix)]
#[test]
fn a_node_workspace_package_in_a_layered_node_modules_resolves_to_the_lanes_own_copy() {
    let repo = repo();
    write(
        &repo.worktree,
        "package.json",
        "{\"workspaces\":[\"packages/*\"]}",
    );
    write(&repo.worktree, "packages/pkg/index.js", "lane");
    write(&repo.worktree, "packages/other/index.js", "lane");
    write(&repo.main, "packages/pkg/index.js", "main");
    write(&repo.main, "packages/other/index.js", "main");
    write(&repo.main, "node_modules/left-pad/index.js", "x");
    write(&repo.main, "node_modules/@types/node/index.d.ts", "x");
    std::fs::create_dir_all(repo.main.join("node_modules/@acme")).unwrap();
    std::os::unix::fs::symlink(
        "../../packages/pkg",
        repo.main.join("node_modules/@acme/pkg"),
    )
    .unwrap();
    std::os::unix::fs::symlink("../packages/other", repo.main.join("node_modules/other")).unwrap();
    let lane = repo.lane();

    let provisioned = lane.provision(&SharedCaches::default());

    assert!(matches!(
        state_of(&provisioned, "node_modules"),
        Some(FrontendDeps::Linked(_))
    ));
    let modules = repo.worktree.join("node_modules");
    let resolved = |rel: &str| modules.join(rel).canonicalize().unwrap();
    let packages = repo.worktree.canonicalize().unwrap().join("packages");
    assert_eq!(resolved("other"), packages.join("other"));
    assert_eq!(resolved("@acme/pkg"), packages.join("pkg"));
    assert_eq!(
        std::fs::read_to_string(modules.join("@acme/pkg/index.js")).unwrap(),
        "lane"
    );
    // A third-party package is still the main checkout's, linked as before.
    assert_eq!(
        std::fs::read_link(modules.join("left-pad")).unwrap(),
        repo.main.join("node_modules/left-pad")
    );
    // A scope with no workspace link stays one link.
    assert_eq!(
        std::fs::read_link(modules.join("@types")).unwrap(),
        repo.main.join("node_modules/@types")
    );
}

#[test]
fn a_package_manager_cache_is_the_lanes_own_and_inside_a_write_root() {
    let repo = repo();
    write(&repo.worktree, "package.json", "{}");
    write(&repo.worktree, "package-lock.json", "{}");
    write(&repo.worktree, "application/composer.json", "{}");
    let lane = repo.lane();
    let provisioned = lane.provision(&SharedCaches::default());

    let env = lane.env(&provisioned);
    let value = |name: &str| {
        env.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| PathBuf::from(value))
    };
    for (name, sub) in [
        ("npm_config_cache", "npm"),
        ("COMPOSER_CACHE_DIR", "composer"),
    ] {
        let path = value(name).unwrap_or_else(|| panic!("{name} is not set"));
        assert_eq!(path, lane.data_dir.join("cache").join(sub));
        assert!(
            lane.write_roots().iter().any(|root| path.starts_with(root)),
            "{name} -> {} is outside every write root",
            path.display()
        );
    }
    // A toolchain the project does not use is not given a cache.
    assert!(value("YARN_CACHE_FOLDER").is_none());
}

#[test]
fn a_project_with_no_such_package_manager_has_its_lane_environment_unchanged() {
    let repo = repo();
    write(
        &repo.worktree,
        "backend/Cargo.toml",
        "[package]\nname = \"x\"\n",
    );
    write(&repo.worktree, "frontend/package.json", "{}");
    write(&repo.worktree, "frontend/pnpm-lock.yaml", "");
    let lane = repo.lane();
    let provisioned = lane.provision(&SharedCaches::default());
    let env = lane.env(&provisioned);
    let names: Vec<&str> = env.iter().map(|(name, _)| *name).collect();
    for cache in [
        "npm_config_cache",
        "YARN_CACHE_FOLDER",
        "COMPOSER_CACHE_DIR",
    ] {
        assert!(!names.contains(&cache), "{cache} leaked into {names:?}");
    }
}

fn git_in(dir: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=kronn-test",
            "-c",
            "user.email=kronn-test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// KT-1747 review (KT-1595): provisioning runs again after an agent, so the
/// ignore rule of a dependency layer is written to the REGISTERED checkout's
/// exclude, never to the repository a rewritten `<lane>/.git` names.
#[cfg(unix)]
#[test]
fn a_dependency_layer_rule_never_follows_a_rewritten_git_pointer() {
    let repo = repo();
    write(&repo.main, ".gitignore", ".kronn/\n");
    write(&repo.main, "composer.json", "{}");
    git_in(&repo.main, &["init", "-q", "-b", "main"]);
    git_in(&repo.main, &["add", "-A"]);
    git_in(&repo.main, &["commit", "-q", "-m", "init"]);
    std::fs::remove_dir_all(&repo.worktree).unwrap();
    let lane_path = repo.worktree.to_str().unwrap();
    git_in(
        &repo.main,
        &["worktree", "add", "-q", "-b", "kt-1747", lane_path],
    );
    write(&repo.main, "vendor/acme/lib/a.php", "<?php");
    let victim = tempfile::tempdir().unwrap();
    let victim = victim.path().canonicalize().unwrap();
    git_in(&victim, &["init", "-q", "-b", "main"]);
    std::fs::write(
        repo.worktree.join(".git"),
        format!("gitdir: {}\n", victim.join(".git").display()),
    )
    .unwrap();

    let provisioned = repo.lane().provision(&SharedCaches::default());

    assert!(
        matches!(
            state_of(&provisioned, "vendor"),
            Some(FrontendDeps::Linked(_))
        ),
        "{provisioned:?}"
    );
    let stolen = std::fs::read_to_string(victim.join(".git/info/exclude")).unwrap_or_default();
    assert!(
        !stolen.contains("vendor"),
        "another repository's exclude was written: {stolen}"
    );
    let main_exclude = std::fs::read_to_string(repo.main.join(".git/info/exclude")).unwrap();
    assert!(main_exclude.contains("/vendor"), "{main_exclude}");
}

/// KT-1747 review — a hoisted workspace package has no install of its own: a
/// launch never asks to install it, only the workspace root.
#[test]
fn a_hoisted_workspace_package_is_never_asked_to_install() {
    let repo = repo();
    write(
        &repo.worktree,
        "package.json",
        r#"{"workspaces":["packages/*"],"packageManager":"yarn@4.1.0"}"#,
    );
    write(&repo.worktree, "yarn.lock", "");
    write(&repo.worktree, "packages/a/package.json", r#"{"name":"a"}"#);
    write(&repo.main, "node_modules/left-pad/index.js", "x");

    let provisioned = repo.lane().provision(&SharedCaches::default());

    assert!(
        !provisioned
            .warnings
            .iter()
            .any(|warning| warning.contains("packages/a") || warning.contains("npm ci")),
        "{:?}",
        provisioned.warnings
    );
}
