use super::*;

fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_COMMITTER_DATE", "2024-03-02T10:00:00Z")
        .env("GIT_AUTHOR_DATE", "2024-03-02T10:00:00Z")
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

#[test]
fn classify_names_each_kind_from_the_path() {
    assert_eq!(classify("README.md", false), BriefingDocKind::Readme);
    assert_eq!(
        classify("CONTRIBUTING", false),
        BriefingDocKind::Contributing
    );
    assert_eq!(classify("docs/adr", true), BriefingDocKind::Adr);
    assert_eq!(classify("docs/decisions", true), BriefingDocKind::Adr);
    assert_eq!(classify("api/openapi.yaml", false), BriefingDocKind::Api);
    assert_eq!(classify("apidoc", true), BriefingDocKind::Api);
    assert_eq!(classify("wiki", true), BriefingDocKind::Wiki);
    assert_eq!(classify("docs", true), BriefingDocKind::Docs);
    assert_eq!(classify("CHANGELOG.md", false), BriefingDocKind::Markdown);
    // A file named like an ADR folder is not one.
    assert_eq!(classify("adr", false), BriefingDocKind::Markdown);
}

#[test]
fn detect_finds_the_well_known_places_and_skips_redirectors_and_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for f in [
        "README.md",
        "CONTRIBUTING.md",
        "NOTES.md",
        "CLAUDE.md",
        "main.rs",
    ] {
        std::fs::write(root.join(f), "x").unwrap();
    }
    std::fs::create_dir_all(root.join("docs/adr")).unwrap();
    std::fs::create_dir_all(root.join("wiki")).unwrap();
    std::fs::create_dir_all(root.join("api")).unwrap();
    std::fs::write(root.join("api/swagger.json"), "{}").unwrap();
    let outside = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.path(), root.join("doc")).unwrap();

    let paths: Vec<(String, BriefingDocKind)> =
        detect(root).into_iter().map(|(p, k, _)| (p, k)).collect();
    assert_eq!(
        paths,
        vec![
            ("README.md".into(), BriefingDocKind::Readme),
            ("docs".into(), BriefingDocKind::Docs),
            ("wiki".into(), BriefingDocKind::Wiki),
            ("docs/adr".into(), BriefingDocKind::Adr),
            ("api/swagger.json".into(), BriefingDocKind::Api),
            ("CONTRIBUTING.md".into(), BriefingDocKind::Contributing),
            ("NOTES.md".into(), BriefingDocKind::Markdown),
        ]
    );
}

#[test]
fn a_typed_path_is_normalised_and_must_stay_inside_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir_all(root.join("docs/guide")).unwrap();
    std::fs::write(root.join("docs/guide/intro.md"), "x").unwrap();
    std::fs::write(dir.path().join("secret.md"), "x").unwrap();

    assert_eq!(
        normalize_path(&root, " ./docs//guide/ ").unwrap(),
        ("docs/guide".into(), true)
    );
    assert_eq!(
        normalize_path(&root, "docs/guide/intro.md").unwrap(),
        ("docs/guide/intro.md".into(), false)
    );
    for bad in [
        "../secret.md",
        "docs/../../secret.md",
        "/etc/passwd",
        "",
        "docs\\guide",
        "docs/`x`",
        "docs/missing.md",
    ] {
        assert!(
            normalize_path(&root, bad).is_err(),
            "{bad:?} must be refused"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("secret.md"), root.join("leak.md")).unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("up")).unwrap();
        assert!(normalize_path(&root, "leak.md").is_err());
        assert!(normalize_path(&root, "up/secret.md").is_err());
        // A symlink that stays inside is an ordinary location.
        std::os::unix::fs::symlink(root.join("docs"), root.join("handbook")).unwrap();
        assert!(normalize_path(&root, "handbook").is_ok());
    }
    let long = "a".repeat(PATH_MAX_CHARS + 1);
    assert!(normalize_path(&root, &long).is_err());
}

#[tokio::test]
async fn the_last_commit_date_comes_from_git_and_is_none_outside_history() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join("README.md"), "x").unwrap();
    git(root, &["add", "README.md"]);
    git(root, &["commit", "-q", "-m", "init"]);
    std::fs::write(root.join("NEW.md"), "x").unwrap();
    // A pathspec-looking name is taken literally.
    std::fs::write(root.join(":(glob)*"), "x").unwrap();

    let dates = last_commit_dates(
        root,
        &["README.md".into(), "NEW.md".into(), ":(glob)*".into()],
    )
    .await;
    assert_eq!(dates, vec![Some("2024-03-02".into()), None, None]);

    let plain = tempfile::tempdir().unwrap();
    assert_eq!(last_commit_date(plain.path(), "README.md").await, None);
}
