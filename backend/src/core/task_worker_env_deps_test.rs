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
