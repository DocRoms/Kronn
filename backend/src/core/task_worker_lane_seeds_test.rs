use super::*;
use crate::core::task_worker_lane_warm::clone_tree;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(["-c", "user.email=t@t.com", "-c", "user.name=T"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A main checkout whose `public/images` is generated and gitignored, with a
/// task worktree in the managed layout.
struct Fixture {
    _dir: tempfile::TempDir,
    main: PathBuf,
    worktree: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("kronn-seeds-")
        .tempdir()
        .unwrap();
    let main = dir.path().canonicalize().unwrap().join("main");
    std::fs::create_dir_all(&main).unwrap();
    run_git(&main, &["init", "-b", "main"]);
    write(
        &main.join(".gitignore"),
        "public/images/\npublic/build/\n.kronn/\n",
    );
    write(&main.join("public/index.txt"), "tracked");
    write(
        &main.join("templates/menu.twig"),
        "{% include '@web/images/icon.svg' %}",
    );
    run_git(&main, &["add", "."]);
    run_git(&main, &["commit", "-m", "init"]);
    write(&main.join("public/images/icon.svg"), "<svg/>");
    write(&main.join("public/images/vector/deep.svg"), "<svg deep/>");
    let worktree = main.join(".kronn/worktrees/task-kt-1");
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    run_git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "task-branch",
            worktree.to_str().unwrap(),
        ],
    );
    Fixture {
        _dir: dir,
        main,
        worktree,
    }
}

fn seeds(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| path.to_string()).collect()
}

fn only(record: &LaneSeedsRecord) -> &LaneSeedOutcome {
    assert_eq!(record.seeds.len(), 1, "{record:?}");
    &record.seeds[0]
}

fn skipped_reason(outcome: &LaneSeedOutcome) -> &str {
    match &outcome.status {
        SeedStatus::Skipped { reason } => reason,
        other => panic!("expected a skip, got {other:?}"),
    }
}

fn no_clone(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[test]
fn a_seed_path_is_a_normalized_relative_path() {
    assert_eq!(
        normalize_seed_path(" ./application/public//images/ ").unwrap(),
        "application/public/images"
    );
    for refused in [
        "",
        "   ",
        "/etc",
        "../outside",
        "a/../../b",
        ".",
        "./",
        ".git",
        "a/.git/hooks",
        ".kronn/worktrees",
        "a\\b",
        "a\0b",
    ] {
        assert!(
            normalize_seed_path(refused).is_err(),
            "`{refused:?}` must be refused"
        );
    }
}

#[test]
fn a_declared_list_is_deduplicated_and_never_nested() {
    assert_eq!(
        normalize_seed_list(&seeds(&["a/b", "./a/b", "c"])).unwrap(),
        vec!["a/b".to_string(), "c".to_string()]
    );
    assert!(normalize_seed_list(&seeds(&["a", "a/b"])).is_err());
    let many: Vec<String> = (0..=MAX_LANE_SEEDS).map(|i| format!("d{i}")).collect();
    assert!(normalize_seed_list(&many).is_err());
}

#[test]
fn a_seed_is_put_in_a_fresh_worktree_and_stays_out_of_git_status() {
    let fx = fixture();
    assert!(!fx.worktree.join("public/images").exists());

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );

    match &only(&record).status {
        SeedStatus::Seeded { refreshed, .. } => assert!(!refreshed),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(fx.worktree.join("public/images/vector/deep.svg")).unwrap(),
        "<svg deep/>"
    );
    assert_eq!(
        run_git(
            &fx.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        "",
        "the seed is gitignored: nothing for a delivery to pick up"
    );
    assert!(
        std::fs::read_dir(&fx.worktree).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".kronn-seed")),
        "no staging left behind"
    );
}

#[test]
fn a_file_system_that_cannot_clone_gets_a_copy() {
    let fx = fixture();

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &no_clone,
    );

    match &only(&record).status {
        SeedStatus::Seeded { method, .. } => assert_eq!(*method, SeedMethod::Copy),
        other => panic!("{other:?}"),
    }
    assert!(fx.worktree.join("public/images/icon.svg").is_file());
}

#[test]
fn an_unchanged_source_is_left_alone() {
    let fx = fixture();
    let declared = seeds(&["public/images"]);
    seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);
    // A marker of the worktree's own: the copy is not re-made.
    write(&fx.worktree.join("public/images/worker-note.txt"), "kept");

    let again = seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);

    assert_eq!(only(&again).status, SeedStatus::UpToDate);
    assert!(fx.worktree.join("public/images/worker-note.txt").exists());
}

#[test]
fn a_changed_source_refreshes_the_copy() {
    let fx = fixture();
    let declared = seeds(&["public/images"]);
    seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);
    write(&fx.main.join("public/images/new.svg"), "<svg new/>");
    std::fs::remove_file(fx.main.join("public/images/icon.svg")).unwrap();

    let again = seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);

    match &only(&again).status {
        SeedStatus::Seeded { refreshed, .. } => assert!(refreshed),
        other => panic!("{other:?}"),
    }
    assert!(fx.worktree.join("public/images/new.svg").is_file());
    assert!(
        !fx.worktree.join("public/images/icon.svg").exists(),
        "the refreshed copy mirrors the source"
    );
    assert_eq!(
        run_git(
            &fx.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        ),
        ""
    );
}

#[test]
fn a_path_the_worktree_already_has_is_never_overwritten() {
    let fx = fixture();
    write(&fx.worktree.join("public/images/mine.svg"), "worker");

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );

    assert!(skipped_reason(only(&record)).contains("did not put it there"));
    assert_eq!(
        std::fs::read_to_string(fx.worktree.join("public/images/mine.svg")).unwrap(),
        "worker"
    );
    assert!(!fx.worktree.join("public/images/icon.svg").exists());
}

#[test]
fn a_path_that_git_does_not_ignore_is_refused() {
    let fx = fixture();
    write(&fx.main.join("notes/todo.txt"), "not ignored");

    let record = seed_worktree(&fx.main, &fx.worktree, &seeds(&["notes"]), &clone_tree);

    assert!(skipped_reason(only(&record)).contains("not gitignored"));
    assert!(!fx.worktree.join("notes").exists());
}

#[test]
fn a_directory_that_is_only_partly_ignored_is_refused() {
    let fx = fixture();
    write(&fx.main.join(".gitignore"), "public/images/*.svg\n");
    write(&fx.main.join("public/images/photo.png"), "png");

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );

    assert!(skipped_reason(only(&record)).contains("not gitignored"));
    assert!(!fx.worktree.join("public/images").exists());
}

#[test]
fn a_path_with_tracked_files_is_refused() {
    let fx = fixture();

    let record = seed_worktree(&fx.main, &fx.worktree, &seeds(&["public"]), &clone_tree);

    assert!(skipped_reason(only(&record)).contains("tracked files"));
}

#[test]
fn a_source_the_main_checkout_lacks_is_a_skip_not_an_error() {
    let fx = fixture();

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/build"]),
        &clone_tree,
    );

    assert!(skipped_reason(only(&record)).contains("main checkout does not have it"));
}

#[cfg(unix)]
#[test]
fn a_symlinked_source_or_destination_parent_is_refused() {
    let fx = fixture();
    let elsewhere = fx.main.parent().unwrap().join("elsewhere");
    write(&elsewhere.join("secret.txt"), "outside");
    std::os::unix::fs::symlink(&elsewhere, fx.main.join("public/build")).unwrap();
    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/build"]),
        &clone_tree,
    );
    assert!(skipped_reason(only(&record)).contains("symlink"));

    // The worktree's `public` replaced by a link out of the tree.
    std::fs::remove_dir_all(fx.worktree.join("public")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, fx.worktree.join("public")).unwrap();
    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );
    assert!(skipped_reason(only(&record)).contains("symlink"));
    assert!(
        !elsewhere.join("images").exists(),
        "nothing was written through the link"
    );
}

#[test]
fn what_cannot_be_seeded_does_not_stop_the_others() {
    let fx = fixture();

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/build", "../escape", "public/images"]),
        &clone_tree,
    );

    assert_eq!(record.seeds.len(), 3);
    assert!(matches!(record.seeds[0].status, SeedStatus::Skipped { .. }));
    assert!(matches!(record.seeds[1].status, SeedStatus::Skipped { .. }));
    assert!(matches!(record.seeds[2].status, SeedStatus::Seeded { .. }));
    assert_eq!(record.skipped().count(), 2);
}

#[test]
fn no_declared_seed_touches_nothing() {
    let fx = fixture();
    assert!(seed_worktree(&fx.main, &fx.worktree, &[], &clone_tree)
        .seeds
        .is_empty());
}

#[test]
fn the_record_survives_the_journal() {
    let record = LaneSeedsRecord {
        seeds: vec![
            LaneSeedOutcome {
                path: "a".into(),
                status: SeedStatus::Seeded {
                    method: SeedMethod::Clone,
                    refreshed: false,
                    ms: 3,
                    dropped_links: vec![],
                },
            },
            LaneSeedOutcome::skipped("b", "why"),
            LaneSeedOutcome {
                path: "c".into(),
                status: SeedStatus::UpToDate,
            },
        ],
    };
    let back: LaneSeedsRecord =
        serde_json::from_value(serde_json::to_value(&record).unwrap()).unwrap();
    assert_eq!(back, record);
}

#[test]
fn suggestions_are_the_ignored_directories_tracked_code_mentions() {
    let fx = fixture();
    write(&fx.main.join("cache/blob.bin"), "x");
    write(&fx.main.join("node_modules/pkg/index.js"), "x");
    write(
        &fx.main.join(".gitignore"),
        "public/images/\ncache/\nnode_modules/\n",
    );
    write(
        &fx.main.join("templates/also.twig"),
        "node_modules/pkg and cache/",
    );
    run_git(&fx.main, &["add", "."]);
    run_git(&fx.main, &["commit", "-m", "mentions"]);

    let found = suggest_seeds(&fx.main).unwrap();
    let paths: Vec<&str> = found.iter().map(|c| c.path.as_str()).collect();

    assert!(paths.contains(&"public/images"), "{paths:?}");
    assert!(
        paths.contains(&"cache"),
        "mentioned by a tracked file: {paths:?}"
    );
    assert!(
        !paths.contains(&"node_modules"),
        "a dependency tree is never suggested: {paths:?}"
    );
    let images = found.iter().find(|c| c.path == "public/images").unwrap();
    assert_eq!(
        images.size_bytes,
        Some("<svg/>".len() as u64 + "<svg deep/>".len() as u64)
    );
}

#[test]
fn an_ignored_directory_nothing_mentions_is_not_suggested() {
    let fx = fixture();
    write(&fx.main.join("scratch/a.txt"), "x");
    write(&fx.main.join(".gitignore"), "public/images/\nscratch/\n");
    run_git(&fx.main, &["add", "."]);
    run_git(&fx.main, &["commit", "-m", "ignore scratch"]);

    let found = suggest_seeds(&fx.main).unwrap();

    assert!(found.iter().all(|c| c.path != "scratch"), "{found:?}");
}

// ── KT-1438 review round: links, ignore rules, fingerprint, delivery ────

#[cfg(unix)]
fn link(target: &str, at: &Path) {
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, at).unwrap();
}

/// A symlink of the source that points out of the seed is not handed to the
/// worker (absolute, or climbing past the seed's root); one that stays inside is.
#[cfg(unix)]
#[test]
fn a_symlink_pointing_out_of_the_seed_is_dropped_and_named() {
    for use_clone in [true, false] {
        let fx = fixture();
        let images = fx.main.join("public/images");
        link("/etc/hosts", &images.join("absolute"));
        link("../../../outside", &images.join("climbs"));
        link("vector/../icon.svg", &images.join("through-a-name"));
        link("../../.env", &images.join("vector/deep-climb"));
        link("icon.svg", &images.join("sibling"));
        link("../icon.svg", &images.join("vector/up-one"));
        let declared = seeds(&["public/images"]);

        let record = if use_clone {
            seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree)
        } else {
            seed_worktree(&fx.main, &fx.worktree, &declared, &no_clone)
        };

        let dropped = match &only(&record).status {
            SeedStatus::Seeded { dropped_links, .. } => dropped_links.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            dropped,
            vec![
                "absolute".to_string(),
                "climbs".to_string(),
                "through-a-name".to_string(),
                "vector/deep-climb".to_string(),
            ],
            "clone={use_clone}"
        );
        let copy = fx.worktree.join("public/images");
        for gone in ["absolute", "climbs", "through-a-name", "vector/deep-climb"] {
            assert!(
                std::fs::symlink_metadata(copy.join(gone)).is_err(),
                "{gone} must not be copied (clone={use_clone})"
            );
        }
        for kept in ["sibling", "vector/up-one"] {
            assert!(
                std::fs::symlink_metadata(copy.join(kept))
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{kept} stays inside the seed (clone={use_clone})"
            );
        }
        assert!(
            copy.join("icon.svg").is_file(),
            "the rest of the copy is intact"
        );
    }
}

/// The record names what was dropped, so the project's settings can show it.
#[cfg(unix)]
#[test]
fn the_dropped_links_survive_the_journal() {
    let fx = fixture();
    link("/etc/hosts", &fx.main.join("public/images/absolute"));
    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );
    let json = serde_json::to_string(&record).unwrap();
    assert!(json.contains("dropped_links"), "{json}");
    let back: LaneSeedsRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back, record);
}

/// A rewrite that keeps the size and restores the modification time (`cp -p`,
/// `rsync -t`) still changes the change time: the copy is refreshed.
#[cfg(unix)]
#[test]
fn a_same_size_rewrite_with_a_preserved_mtime_refreshes_the_copy() {
    let fx = fixture();
    let declared = seeds(&["public/images"]);
    seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);
    let file = fx.main.join("public/images/icon.svg");
    let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();

    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&file, "<svg!>").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), mtime);
    assert_eq!(
        std::fs::metadata(&file).unwrap().len(),
        "<svg/>".len() as u64
    );

    let again = seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);

    match &only(&again).status {
        SeedStatus::Seeded { refreshed, .. } => assert!(refreshed),
        other => panic!("the rewrite went unseen: {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(fx.worktree.join("public/images/icon.svg")).unwrap(),
        "<svg!>"
    );
}

/// Two trees with the same entry count and total size but other names do not
/// share a fingerprint.
#[test]
fn the_fingerprint_tells_a_renamed_entry_apart() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    write(&one.path().join("a.svg"), "12345");
    write(&two.path().join("b.svg"), "12345");
    assert_ne!(
        fingerprint(one.path()).unwrap().0,
        fingerprint(two.path()).unwrap().0
    );
    assert_eq!(
        fingerprint(one.path()).unwrap().0,
        fingerprint(one.path()).unwrap().0
    );
}

/// The worktree is the worker's to write: a `.gitignore` it edited after the
/// seeding is noticed on the next provisioning, and the copy then shown by
/// `git status` is taken back out.
#[test]
fn a_gitignore_the_worker_edited_is_seen_on_the_up_to_date_path() {
    let fx = fixture();
    let declared = seeds(&["public/images"]);
    seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);
    write(&fx.worktree.join(".gitignore"), "public/build/\n.kronn/\n");
    assert!(
        run_git(
            &fx.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .contains("public/images"),
        "the edit makes the copy visible to git"
    );

    let again = seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);

    let reason = skipped_reason(only(&again));
    assert!(reason.contains("no longer gitignored"), "{reason}");
    assert!(!fx.worktree.join("public/images").exists());
    assert_eq!(
        run_git(
            &fx.worktree,
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .trim(),
        "M .gitignore",
        "only the worker's own edit is left"
    );
}

/// A seed never enters a delivery: committed through the path a spawned worker
/// commits by (explicit files, no `-f`), the copy is not staged, whether it is
/// named, or reached through a directory that also holds tracked files.
#[test]
fn a_seed_never_enters_a_worker_commit() {
    let fx = fixture();
    run_git(&fx.main, &["config", "user.email", "t@t.com"]);
    run_git(&fx.main, &["config", "user.name", "T"]);
    seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );
    write(&fx.worktree.join("NOTES.md"), "worker notes");
    write(&fx.worktree.join("public/index.txt"), "tracked, edited");

    let notes = crate::api::agent_workspace_tools::git_commit_payload(
        &fx.worktree,
        &["NOTES.md".to_string()],
        "test: a worker commit",
    );
    assert!(notes.is_ok(), "{notes:?}");

    // Naming a seeded file, or the directory around it: refused, or committed
    // without the seed; never with it.
    for named in ["public/images/icon.svg", "public/images", "public"] {
        let _ = crate::api::agent_workspace_tools::git_commit_payload(
            &fx.worktree,
            &[named.to_string()],
            "test: a worker commit naming a seed",
        );
        let tracked = run_git(&fx.worktree, &["ls-tree", "-r", "--name-only", "HEAD"]);
        assert!(
            !tracked.contains("public/images"),
            "{named} staged the seed: {tracked}"
        );
        assert_eq!(
            run_git(&fx.worktree, &["ls-files", "--", "public/images"]),
            ""
        );
    }
    assert!(fx.worktree.join("public/images/icon.svg").is_file());
}

/// Make a path gitignored in the main checkout and in every worktree: the main
/// checkout's `info/exclude` is shared.
fn ignore_everywhere(fx: &Fixture, line: &str) {
    let exclude = fx.main.join(".git/info/exclude");
    let mut text = std::fs::read_to_string(&exclude).unwrap_or_default();
    text.push_str(line);
    text.push('\n');
    std::fs::write(exclude, text).unwrap();
}

/// A seed may be one generated file (a config, a key the project needs): it is
/// seeded, not skipped, by a clone and by a plain copy.
#[test]
fn a_single_file_seed_is_seeded() {
    for use_clone in [true, false] {
        let fx = fixture();
        ignore_everywhere(&fx, "config/generated.json");
        write(&fx.main.join("config/generated.json"), "version one");
        let declared = seeds(&["config/generated.json"]);

        let record = if use_clone {
            seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree)
        } else {
            seed_worktree(&fx.main, &fx.worktree, &declared, &no_clone)
        };

        match &only(&record).status {
            SeedStatus::Seeded { dropped_links, .. } => assert!(dropped_links.is_empty()),
            other => panic!("a file seed must be seeded (clone={use_clone}): {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(fx.worktree.join("config/generated.json")).unwrap(),
            "version one"
        );
        assert_eq!(
            run_git(
                &fx.worktree,
                &["status", "--porcelain", "--untracked-files=all"]
            )
            .trim(),
            "",
            "the seeded file stays out of git status"
        );

        let again = seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);
        assert_eq!(only(&again).status, SeedStatus::UpToDate, "{again:?}");

        write(&fx.main.join("config/generated.json"), "version two!");
        let refreshed = seed_worktree(&fx.main, &fx.worktree, &declared, &clone_tree);
        match &only(&refreshed).status {
            SeedStatus::Seeded { refreshed, .. } => assert!(refreshed),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(fx.worktree.join("config/generated.json")).unwrap(),
            "version two!"
        );
    }
}

/// A seed that is itself a link is refused (its target is not part of the copy),
/// and `drop_escaping_links` looks at a file without listing it.
#[cfg(unix)]
#[test]
fn a_single_link_seed_is_refused() {
    let fx = fixture();
    ignore_everywhere(&fx, "config/link.json");
    write(&fx.main.join("config/real.json"), "real");
    link("real.json", &fx.main.join("config/link.json"));

    let record = seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["config/link.json"]),
        &clone_tree,
    );

    let reason = skipped_reason(only(&record));
    assert!(reason.contains("symlink"), "{reason}");
    assert!(std::fs::symlink_metadata(fx.worktree.join("config/link.json")).is_err());

    // The helper on its own: a file is kept, a link as the root is an error.
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("file"), "x");
    assert!(drop_escaping_links(&dir.path().join("file"))
        .unwrap()
        .is_empty());
    link("file", &dir.path().join("root-link"));
    assert!(drop_escaping_links(&dir.path().join("root-link")).is_err());
}

/// The copy under construction (or one a crash left) is ignored by git before it
/// exists, so a worker's `git add -A` cannot commit it.
#[test]
fn a_leftover_transient_copy_is_ignored_by_git() {
    let fx = fixture();
    seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );
    for kind in [STAGING_KIND, TRASH_KIND] {
        let name = format!(".kronn-{kind}-0123456789abcdef");
        write(&fx.worktree.join(&name).join("left.txt"), "left by a crash");
        let status = run_git(
            &fx.worktree,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        assert_eq!(status.trim(), "", "{name} shows in git status: {status}");
        run_git(&fx.worktree, &["add", "-A"]);
        assert_eq!(
            run_git(&fx.worktree, &["ls-files", "--", &name]),
            "",
            "{name} was staged"
        );
    }

    // The next seeding sweeps them.
    seed_worktree(
        &fx.main,
        &fx.worktree,
        &seeds(&["public/images"]),
        &clone_tree,
    );
    let left: Vec<String> = std::fs::read_dir(&fx.worktree)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".kronn-seed"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn a_seed_path_that_looks_secret_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(&repo.join(".env"), "TOKEN=1");
    write(&repo.join("config/server.key"), "k");
    write(&repo.join("assets/logo.svg"), "<svg/>");
    write(&repo.join("assets/.env.example"), "TOKEN=");

    let found = secret_findings(
        repo,
        &seeds(&[".env", "config/server.key", "assets", "missing"]),
    )
    .unwrap();

    assert_eq!(
        found,
        vec![
            LaneSeedSecretFinding {
                path: ".env".into(),
                files: vec![".env".into()],
                truncated: false
            },
            LaneSeedSecretFinding {
                path: "config/server.key".into(),
                files: vec!["config/server.key".into()],
                truncated: false
            },
        ],
        "a clean tree and a versioned template are no finding"
    );
}

/// A seeded directory holding a `.env` or a key gets a warning naming the files,
/// although its own path is harmless.
#[test]
fn a_secret_inside_a_seeded_directory_is_listed() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(&repo.join("generated/ok.txt"), "fine");
    write(&repo.join("generated/deep/.env.production"), "TOKEN=1");
    write(&repo.join("generated/deep/er/id_ed25519"), "key");
    write(&repo.join("generated/cert.pem"), "pem");

    let found = secret_findings(repo, &seeds(&["generated"])).unwrap();

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, "generated");
    assert_eq!(
        found[0].files,
        vec![
            "generated/cert.pem".to_string(),
            "generated/deep/.env.production".to_string(),
            "generated/deep/er/id_ed25519".to_string(),
        ]
    );
    assert!(!found[0].truncated);
}

#[test]
fn the_list_of_found_secrets_is_capped_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..(MAX_SECRET_FILES_LISTED + 5) {
        write(&dir.path().join(format!("gen/k{index}.key")), "k");
    }

    let found = secret_findings(dir.path(), &seeds(&["gen"])).unwrap();

    assert_eq!(found[0].files.len(), MAX_SECRET_FILES_LISTED);
    assert!(found[0].truncated);
}

/// The walk never follows a link out of the seed, and a link to a secret is one.
#[cfg(unix)]
#[test]
fn the_secret_scan_does_not_follow_links_and_flags_a_link_to_a_secret() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    write(&repo.join("outside/.env"), "TOKEN=1");
    write(&repo.join("gen/a.txt"), "a");
    link("../outside", &repo.join("gen/to-dir"));
    link("../outside/.env", &repo.join("gen/to-env"));

    let found = secret_findings(repo, &seeds(&["gen"])).unwrap();

    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].files, vec!["gen/to-env".to_string()]);
}

/// A tree that cannot be read is an error, never "nothing found".
#[cfg(unix)]
#[test]
fn an_unreadable_tree_fails_the_secret_scan_closed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("gen/inner/a.txt"), "a");
    let inner = dir.path().join("gen/inner");
    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A superuser reads it anyway: then there is nothing to fail on.
    let unreadable = std::fs::read_dir(&inner).is_err();

    let result = secret_findings(dir.path(), &seeds(&["gen"]));

    std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o755)).unwrap();
    if unreadable {
        assert!(result.is_err(), "{result:?}");
    }
}
