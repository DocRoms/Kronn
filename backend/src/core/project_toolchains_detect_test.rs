//! KT-1747 — generic toolchain detection, from fixtures shaped like real
//! projects (the shapes only: no name of a repository appears in the code under
//! test).

use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn kinds(profile: &[PackageRoot]) -> Vec<String> {
    profile
        .iter()
        .map(|root| format!("{}@{}", root.kind.name(), root.label()))
        .collect()
}

#[test]
fn a_node_project_names_its_package_manager_from_its_lock_file() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "npm-app/package.json", "{}");
    write(repo, "npm-app/package-lock.json", "{}");
    write(repo, "yarn-app/package.json", "{}");
    write(repo, "yarn-app/yarn.lock", "");
    write(repo, "pnpm-app/package.json", "{}");
    write(repo, "pnpm-app/pnpm-lock.yaml", "");
    write(
        repo,
        "declared/package.json",
        r#"{"packageManager":"yarn@4.1.0"}"#,
    );
    write(repo, "bare/package.json", "{}");

    assert_eq!(
        kinds(&detect_toolchains(repo)),
        [
            "npm@bare",
            "yarn@declared",
            "npm@npm-app",
            "pnpm@pnpm-app",
            "yarn@yarn-app"
        ]
    );
}

#[test]
fn a_front_euronews_like_repository_is_a_yarn_root_with_a_php_application_and_make() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "package.json", "{}");
    write(repo, "yarn.lock", "");
    write(repo, "Makefile", "test:\n\ttrue\n");
    write(repo, "src/index.ts", "");
    write(repo, "application/composer.json", "{}");
    write(repo, "application/composer.lock", "{}");

    let profile = detect_toolchains(repo);

    assert_eq!(
        kinds(&profile),
        ["yarn@.", "composer@application", "make@."]
    );
    assert_eq!(profile[0].dependency_dir(), Some("node_modules"));
    assert_eq!(
        profile[1].dependency_path().as_deref(),
        Some("application/vendor")
    );
    assert_eq!(profile[2].dependency_dir(), None);
}

#[test]
fn a_docroms_web_like_repository_has_composer_and_a_nested_node_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "composer.json", "{}");
    write(repo, "composer.lock", "{}");
    write(repo, "assets/front/package.json", "{}");
    write(repo, "assets/front/package-lock.json", "{}");
    write(repo, "Makefile", "");

    let profile = detect_toolchains(repo);

    assert_eq!(
        kinds(&profile),
        ["composer@.", "npm@assets/front", "make@."]
    );
    assert_eq!(profile[0].dependency_dir(), Some("vendor"));
}

#[test]
fn a_front_api_like_repository_is_a_pnpm_root_with_a_nested_rust_crate() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "package.json", "{}");
    write(repo, "pnpm-lock.yaml", "");
    write(repo, "api/handlers/index.ts", "");
    write(
        repo,
        "redis-flush/Cargo.toml",
        "[package]\nname = \"flush\"\n",
    );
    write(repo, "redis-flush/src/main.rs", "fn main() {}\n");

    let profile = detect_toolchains(repo);

    assert_eq!(kinds(&profile), ["pnpm@.", "cargo@redis-flush"]);
    // The crate has no dependency directory to layer: its target is the lane's.
    assert_eq!(profile[1].dependency_dir(), None);
}

#[test]
fn an_app_ios_like_repository_is_cocoapods_xcode_and_make() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "Podfile", "");
    write(repo, "Podfile.lock", "");
    write(repo, "App.xcworkspace/contents.xcworkspacedata", "");
    write(repo, "App.xcodeproj/project.pbxproj", "");
    write(repo, "Makefile", "");
    write(repo, "Pods/Alamofire/a.swift", "");

    let profile = detect_toolchains(repo);

    assert_eq!(kinds(&profile), ["cocoapods@.", "xcode@.", "make@."]);
    assert_eq!(profile[0].dependency_dir(), Some("Pods"));
}

#[test]
fn kronn_itself_is_unchanged_a_cargo_backend_and_a_pnpm_frontend() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "package.json", "{}");
    write(repo, "pnpm-lock.yaml", "");
    write(repo, "Makefile", "");
    write(repo, "backend/Cargo.toml", "");
    write(repo, "desktop/src-tauri/Cargo.toml", "");
    write(repo, "frontend/package.json", "{}");
    write(repo, "frontend/pnpm-lock.yaml", "");
    write(repo, "frontend/node_modules/x/package.json", "{}");

    assert_eq!(
        kinds(&detect_toolchains(repo)),
        [
            "pnpm@.",
            "cargo@backend",
            "cargo@desktop/src-tauri",
            "pnpm@frontend",
            "make@."
        ]
    );
}

#[test]
fn dependency_and_build_trees_never_yield_a_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "node_modules/left-pad/package.json", "{}");
    write(repo, "vendor/acme/lib/composer.json", "{}");
    write(repo, "Pods/Foo/Podfile", "");
    write(repo, ".hidden/package.json", "{}");

    assert!(detect_toolchains(repo).is_empty());
}

#[test]
fn the_dependency_dirs_to_layer_are_the_detected_ones_unless_the_project_names_them() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "composer.json", "{}");
    write(repo, "assets/front/package.json", "{}");
    write(repo, "Makefile", "");
    let profile = detect_toolchains(repo);

    let (detected, unknown) = dependency_dirs(&profile, None);
    assert_eq!(detected, ["assets/front/node_modules", "vendor"]);
    assert!(unknown.is_empty());

    let wanted = vec!["./vendor/".to_string(), "../escape".to_string()];
    let (chosen, rejected) = dependency_dirs(&profile, Some(&wanted));
    assert_eq!(chosen, ["vendor"]);
    assert_eq!(rejected, ["../escape"]);

    let (none, _) = dependency_dirs(&profile, Some(&[]));
    assert!(none.is_empty());
}

#[test]
fn the_conventions_lines_describe_the_profile_and_stay_empty_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "composer.json", "{}");
    write(repo, "assets/front/package.json", "{}");
    write(repo, "assets/front/yarn.lock", "");
    write(repo, "Makefile", "");

    let lines = conventions_lines(&detect_toolchains(repo));

    assert_eq!(
        lines,
        [
            "composer at `.` (dependencies in `vendor/`)",
            "yarn at `assets/front` (dependencies in `assets/front/node_modules/`)",
            "make at `.`",
        ]
    );
    assert!(conventions_lines(&[]).is_empty());
}

// ── KT-1751: a package manager's download cache lives in the lane ─────────

#[test]
fn a_package_manager_that_caches_in_the_home_names_the_variable_that_moves_it() {
    assert_eq!(
        ToolchainKind::Node(NodeManager::Npm).cache_env(),
        Some(("npm_config_cache", "npm"))
    );
    assert_eq!(
        ToolchainKind::Node(NodeManager::Yarn).cache_env(),
        Some(("YARN_CACHE_FOLDER", "yarn"))
    );
    assert_eq!(
        ToolchainKind::Composer.cache_env(),
        Some(("COMPOSER_CACHE_DIR", "composer"))
    );
    // pnpm's store is the host's, read-only by design; Cargo's cache is the
    // lane's CARGO_HOME; the others cache nothing in a home.
    for kind in [
        ToolchainKind::Node(NodeManager::Pnpm),
        ToolchainKind::Cargo,
        ToolchainKind::Xcode,
        ToolchainKind::Make,
    ] {
        assert_eq!(kind.cache_env(), None, "{}", kind.name());
    }
}

#[test]
fn the_caches_of_a_project_follow_its_detected_toolchains_once_each() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(repo, "package.json", "{}");
    write(repo, "package-lock.json", "{}");
    write(repo, "admin/package.json", "{}");
    write(repo, "admin/package-lock.json", "{}");
    write(repo, "application/composer.json", "{}");
    write(repo, "Makefile", "test:\n\ttrue\n");

    assert_eq!(
        cache_env(&detect_toolchains(repo)),
        [
            ("npm_config_cache", "npm"),
            ("COMPOSER_CACHE_DIR", "composer")
        ]
    );
    // Kronn's own shape (Cargo + pnpm): nothing is added to its lanes.
    let kronn = tempfile::tempdir().unwrap();
    write(
        kronn.path(),
        "backend/Cargo.toml",
        "[package]\nname = \"x\"\n",
    );
    write(kronn.path(), "frontend/package.json", "{}");
    write(kronn.path(), "frontend/pnpm-lock.yaml", "");
    assert!(cache_env(&detect_toolchains(kronn.path())).is_empty());
}
