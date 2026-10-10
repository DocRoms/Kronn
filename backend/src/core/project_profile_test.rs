use super::*;

const EXAMPLE: &str = r#"
schema_version = 1

[validation.targets.lint]
command = "make iso-lint"
description = "ESLint and stylelint in the iso container"

[validation.targets.test]
command = "make iso-test"

[forge]
base_branch = "develop"
merge_method = "rebase"
required_approvals = 1
pr_template = ".github/pull_request_template.md"

[forge.labels.to-test]
name = "to-test"
triggers = ["env-branch", "ci-test"]

[tracker]
kind = "jira"
project_key = "EN"
[tracker.statuses]
review = "To Review"
deployed = "Deployed"
[tracker.transitions.deploy]
from = "To Deploy"
to = "Deployed"

[delivery.workflows.staging]
workflow = "CD_Deploy-NONPROD"
inputs = { environment = "staging" }
"#;

fn git(dir: &Path, args: &[&str]) {
    let output = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repo_with_profile(profile: Option<&str>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["config", "user.email", "t@kronn.local"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("README.md"), "x").unwrap();
    if let Some(text) = profile {
        std::fs::create_dir_all(root.join("kronn")).unwrap();
        std::fs::write(root.join(PROFILE_PATH), text).unwrap();
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "init"]);
    dir
}

fn reads(paths: &[(&str, bool)]) -> BTreeMap<String, bool> {
    paths.iter().map(|(p, g)| (p.to_string(), *g)).collect()
}

#[test]
fn a_complete_profile_exposes_every_value_under_project() {
    let profile = parse_profile(EXAMPLE).expect("the documented example parses");
    let values = template_values(&profile);
    assert_eq!(
        values["project.validation.targets.lint.command"],
        "make iso-lint"
    );
    assert_eq!(values["project.forge.merge_method"], "rebase");
    assert_eq!(values["project.forge.required_approvals"], "1");
    assert_eq!(values["project.forge.labels.to-test.triggers.1"], "ci-test");
    assert_eq!(
        values["project.forge.labels.to-test.triggers"],
        r#"["env-branch","ci-test"]"#
    );
    assert_eq!(values["project.tracker.statuses.review"], "To Review");
    assert_eq!(values["project.tracker.transitions.deploy.to"], "Deployed");
    assert_eq!(
        values["project.delivery.workflows.staging.inputs.environment"],
        "staging"
    );
    assert!(!values.contains_key("project"));
}

#[test]
fn an_unknown_key_or_a_malformed_file_is_refused_with_its_location() {
    let unknown =
        parse_profile("schema_version = 1\n[forge]\nmerge_strategy = \"rebase\"\n").unwrap_err();
    assert!(unknown.contains("merge_strategy"), "{unknown}");
    assert!(unknown.contains(PROFILE_PATH), "{unknown}");
    let top = parse_profile("schema_version = 1\n[deploy]\nx = \"y\"\n").unwrap_err();
    assert!(top.contains("deploy"), "{top}");
    let malformed = parse_profile("schema_version = 1\n[forge\n").unwrap_err();
    assert!(malformed.contains("invalid"), "{malformed}");
    let missing = parse_profile("[forge]\nbase_branch = \"main\"\n").unwrap_err();
    assert!(missing.contains("schema_version"), "{missing}");
    let version = parse_profile("schema_version = 2\n").unwrap_err();
    assert!(version.contains("schema_version = 2"), "{version}");
    let method = parse_profile("schema_version = 1\n[forge]\nmerge_method = \"ff\"\n").unwrap_err();
    assert!(!method.contains("ff"), "{method}");
    assert!(method.contains("`rebase`"), "{method}");
    let key = parse_profile("schema_version = 1\n[tracker]\nkind = \"jira\"\n[tracker.statuses]\n\"To Review\" = \"x\"\n").unwrap_err();
    assert!(!key.contains("To Review"), "{key}");
    assert!(
        key.contains("project.tracker.statuses.<invalid key #1>"),
        "{key}"
    );
}

#[test]
fn a_secret_looking_key_or_value_is_refused() {
    for text in [
        "schema_version = 1\n[delivery.workflows.prod]\nworkflow = \"cd\"\ninputs = { api_token = \"x\" }\n",
        "schema_version = 1\n[tracker]\nkind = \"jira\"\n[tracker.statuses]\npassword = \"x\"\n",
        "schema_version = 1\n[forge.labels.client-secret]\nname = \"x\"\n",
    ] {
        let error = parse_profile(text).unwrap_err();
        assert!(error.contains("looks like a secret"), "{error}");
    }
    let value = parse_profile(
        "schema_version = 1\n[delivery.workflows.prod]\nworkflow = \"ghp_0123456789abcdefghijABCDEFGHIJ012345\"\n",
    )
    .unwrap_err();
    assert!(value.contains("looks like a secret"), "{value}");
}

#[test]
fn placeholders_and_escaping_paths_are_refused() {
    let template =
        parse_profile("schema_version = 1\n[forge]\nbase_branch = \"{{issue.title}}\"\n")
            .unwrap_err();
    assert!(template.contains("placeholder"), "{template}");
    for path in ["../x.md", "/etc/passwd", "a/../../b"] {
        let text = format!("schema_version = 1\n[forge]\npr_template = {path:?}\n");
        let error = parse_profile(&text).unwrap_err();
        assert!(error.contains("relative"), "{error}");
    }
}

#[test]
fn the_profile_is_read_from_the_default_branch_never_from_head_or_the_working_tree() {
    let repo = repo_with_profile(Some(EXAMPLE));
    let root = repo.path();
    git(root, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(
        root.join(PROFILE_PATH),
        EXAMPLE.replace("make iso-lint", "make from-feature"),
    )
    .unwrap();
    git(root, &["commit", "-qam", "feature edits the profile"]);
    std::fs::write(
        root.join(PROFILE_PATH),
        EXAMPLE.replace("make iso-lint", "make uncommitted"),
    )
    .unwrap();
    let ProfileSource::Loaded {
        git_ref, profile, ..
    } = load_from_default_branch(root).unwrap()
    else {
        panic!("profile expected");
    };
    assert_eq!(git_ref, "refs/heads/main");
    assert_eq!(
        template_values(&profile)["project.validation.targets.lint.command"],
        "make iso-lint"
    );
}

#[test]
fn origin_head_names_the_default_branch_and_its_remote_ref_stands_in() {
    let upstream = repo_with_profile(Some(EXAMPLE));
    git(upstream.path(), &["branch", "-m", "main", "develop"]);
    let clone = tempfile::tempdir().unwrap();
    git(
        clone.path(),
        &[
            "clone",
            "-q",
            &upstream.path().to_string_lossy(),
            &clone.path().join("c").to_string_lossy(),
        ],
    );
    let root = clone.path().join("c");
    git(&root, &["checkout", "-q", "-b", "work"]);
    git(&root, &["branch", "-D", "develop"]);
    assert_eq!(
        default_branch_ref(&root).as_deref(),
        Some("refs/remotes/origin/develop")
    );
    assert!(matches!(
        load_from_default_branch(&root).unwrap(),
        ProfileSource::Loaded { .. }
    ));
}

#[cfg(unix)]
#[test]
fn a_symlinked_profile_on_the_default_branch_is_refused() {
    let repo = repo_with_profile(None);
    let root = repo.path();
    std::fs::create_dir_all(root.join("kronn")).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", root.join(PROFILE_PATH)).unwrap();
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "link"]);
    let error = load_from_default_branch(root).unwrap_err();
    assert!(error.contains("not a regular file"), "{error}");
}

#[test]
fn a_project_without_a_profile_keeps_working_unless_a_step_requires_one() {
    let repo = repo_with_profile(None);
    assert_eq!(
        run_template_values(Some(repo.path()), &BTreeMap::new()).unwrap(),
        BTreeMap::new()
    );
    assert_eq!(
        run_template_values(
            Some(repo.path()),
            &reads(&[("project.forge.base_branch", true)])
        )
        .unwrap(),
        BTreeMap::new()
    );
    assert!(run_template_values(None, &BTreeMap::new())
        .unwrap()
        .is_empty());
    let error = run_template_values(
        Some(repo.path()),
        &reads(&[("project.forge.base_branch", false)]),
    )
    .unwrap_err();
    assert!(error.contains("{{project.forge.base_branch}}"), "{error}");
    assert!(error.contains("refs/heads/main"), "{error}");
}

#[test]
fn a_malformed_profile_fails_only_the_runs_that_read_it() {
    let repo = repo_with_profile(Some("schema_version = 1\nunknown = 1\n"));
    assert!(run_template_values(Some(repo.path()), &BTreeMap::new())
        .unwrap()
        .is_empty());
    let error = run_template_values(
        Some(repo.path()),
        &reads(&[("project.forge.base_branch", false)]),
    )
    .unwrap_err();
    assert!(error.contains("unknown"), "{error}");
    assert!(error.contains("refs/heads/main"), "{error}");
}

#[test]
fn a_key_the_profile_lacks_is_named() {
    let repo = repo_with_profile(Some(EXAMPLE));
    let error = run_template_values(
        Some(repo.path()),
        &reads(&[("project.tracker.statuses.qa", false)]),
    )
    .unwrap_err();
    assert!(error.contains("project.tracker.statuses.qa"), "{error}");
}

#[test]
fn step_reads_are_collected_with_their_fallbacks() {
    let mut step = WorkflowStep {
        name: "s".into(),
        ..Default::default()
    };
    step.exec_args = vec![
        "{{project.forge.base_branch}}".into(),
        "{{ project.tracker.kind ?? \"none\" }}".into(),
        "{{project.forge.base_branch|sh}} {{project}} {{issue.title}}".into(),
    ];
    let mut fallback = WorkflowStep {
        name: "rollback".into(),
        ..Default::default()
    };
    fallback.prompt_template = "{{project.tracker.kind}}".into();
    let found = steps_placeholders([&step, &fallback]);
    assert_eq!(
        found,
        reads(&[
            ("project.forge.base_branch", false),
            ("project.tracker.kind", false)
        ])
    );
}

/// Every file under `root` (`.git` included) with its bytes, plus git status.
fn tree_state(root: &Path) -> (BTreeMap<PathBuf, Vec<u8>>, String) {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(&path, out);
            } else {
                out.insert(path.clone(), std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, &mut files);
    let status = crate::core::cmd::git_cmd()
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--ignored"])
        .output()
        .unwrap();
    (files, String::from_utf8_lossy(&status.stdout).into_owned())
}

fn repo_needing_a_draft() -> tempfile::TempDir {
    let repo = repo_with_profile(None);
    let root = repo.path();
    std::fs::write(
        root.join("package.json"),
        r#"{"scripts": {"lint": "eslint .", "test": "vitest", "dev": "vite"}}"#,
    )
    .unwrap();
    std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
    std::fs::create_dir_all(root.join(".github")).unwrap();
    std::fs::write(root.join(".github/pull_request_template.md"), "## Why").unwrap();
    repo
}

#[test]
fn the_audit_drafts_a_valid_profile_without_touching_the_checkout() {
    let repo = repo_needing_a_draft();
    let root = repo.path();
    let before = tree_state(root);
    let draft = draft_if_missing(root).expect("a draft");
    assert_eq!(tree_state(root), before, "the checkout is byte-identical");
    let values = template_values(&parse_profile(&draft).expect("a valid profile"));
    assert_eq!(
        values["project.validation.targets.lint.command"],
        "pnpm lint"
    );
    assert_eq!(
        values["project.validation.targets.test.command"],
        "pnpm test"
    );
    assert!(!values.contains_key("project.validation.targets.dev.command"));
    assert_eq!(values["project.forge.base_branch"], "main");
    assert_eq!(
        values["project.forge.pr_template"],
        ".github/pull_request_template.md"
    );
}

#[tokio::test]
async fn the_audit_keeps_its_draft_with_the_run_and_leaves_the_checkout_alone() {
    let repo = repo_needing_a_draft();
    let root = repo.path().to_path_buf();
    let db = crate::db::Database::open_in_memory().unwrap();
    let project: crate::models::Project = serde_json::from_value(serde_json::json!({
        "id": "p-draft", "name": "p", "path": root.to_string_lossy(),
        "repo_url": null, "token_override": null,
        "ai_config": {"detected": false, "configs": []},
        "created_at": "2026-10-09T00:00:00Z", "updated_at": "2026-10-09T00:00:00Z",
    }))
    .unwrap();
    db.with_conn(move |conn| {
        crate::db::projects::insert_project(conn, &project)?;
        crate::db::audit_runs::insert_running(
            conn,
            "audit-1",
            "p-draft",
            "Full",
            "ClaudeCode",
            chrono::Utc::now(),
        )
    })
    .await
    .unwrap();
    let before = tree_state(&root);
    assert!(record_audit_draft(&db, "audit-1", root.clone())
        .await
        .unwrap());
    assert_eq!(tree_state(&root), before, "the checkout is byte-identical");
    assert!(!root.join("kronn").exists());
    let run = db
        .with_conn(|conn| crate::db::audit_runs::get_by_id(conn, "audit-1"))
        .await
        .unwrap()
        .unwrap();
    let draft = run
        .project_profile_draft
        .expect("readable from the audit run");
    assert!(parse_profile(&draft).is_ok(), "{draft}");
}

#[test]
fn the_audit_drafts_nothing_when_the_repository_has_a_profile_or_no_default_branch() {
    let repo = repo_with_profile(Some(EXAMPLE));
    std::fs::remove_file(repo.path().join(PROFILE_PATH)).unwrap();
    assert_eq!(draft_if_missing(repo.path()), None);
    let plain = tempfile::tempdir().unwrap();
    assert_eq!(draft_if_missing(plain.path()), None);
}

#[test]
fn a_default_branch_with_a_slash_keeps_its_full_name() {
    let upstream = repo_with_profile(None);
    git(upstream.path(), &["branch", "-m", "main", "release/stable"]);
    let clone = tempfile::tempdir().unwrap();
    let root = clone.path().join("c");
    git(
        clone.path(),
        &[
            "clone",
            "-q",
            &upstream.path().to_string_lossy(),
            &root.to_string_lossy(),
        ],
    );
    assert_eq!(
        default_branch_ref(&root).as_deref(),
        Some("refs/heads/release/stable")
    );
    let draft = draft_if_missing(&root).expect("a draft");
    let values = template_values(&parse_profile(&draft).unwrap());
    assert_eq!(values["project.forge.base_branch"], "release/stable");
}

const TOKEN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";

#[test]
fn a_refusal_never_copies_a_value_from_the_file() {
    for text in [
        format!("schema_version = 1\n[forge]\napi_token = \"{TOKEN}\"\n"),
        format!("schema_version = 1\n[forge]\nrequired_approvals = \"{TOKEN}\"\n"),
        format!("schema_version = 1\n[forge]\nmerge_method = \"{TOKEN}\"\n"),
        format!("schema_version = 1\n[forge]\nbase_branch = \"{TOKEN}\n"),
        format!("schema_version = 1\n[forge\nbase_branch = \"{TOKEN}\"\n"),
        format!("schema_version = 1\n[forge]\npr_template = \"/{TOKEN}\"\n"),
        format!("schema_version = 1\n{TOKEN} = 1\n"),
    ] {
        let error = parse_profile(&text).unwrap_err();
        assert!(!error.contains(TOKEN), "{error}");
        assert!(!error.contains("ghp_"), "{error}");
        assert!(error.contains(PROFILE_PATH), "{error}");
    }
    let unknown = parse_profile(&format!(
        "schema_version = 1\n[forge]\napi_token = \"{TOKEN}\"\n"
    ))
    .unwrap_err();
    assert!(unknown.contains("unknown key `api_token`"), "{unknown}");
    assert!(unknown.contains("line 3"), "{unknown}");
    let typed = parse_profile(&format!(
        "schema_version = 1\n[forge]\nrequired_approvals = \"{TOKEN}\"\n"
    ))
    .unwrap_err();
    assert!(typed.contains("wrong type"), "{typed}");
}

#[test]
fn an_invalid_profile_snapshot_carries_no_value_either() {
    let repo = repo_with_profile(Some(&format!(
        "schema_version = 1\n[forge]\nrequired_approvals = \"{TOKEN}\"\n"
    )));
    let snap = snapshot(Some(repo.path()));
    assert_eq!(snap.state, SnapshotState::Invalid);
    let json = serde_json::to_string(&snap).unwrap();
    assert!(!json.contains(TOKEN), "{json}");
    let error = values_for(&snap, &reads(&[("project.forge.base_branch", false)])).unwrap_err();
    assert!(!error.contains(TOKEN), "{error}");
}

#[test]
fn a_refused_dynamic_key_is_never_printed() {
    for text in [
        format!("schema_version = 1\n[tracker]\nkind = \"jira\"\n[tracker.statuses]\n\"{TOKEN}\" = \"To Review\"\n"),
        format!("schema_version = 1\n[validation.targets.\"{TOKEN}\"]\ncommand = \"make lint\"\n"),
        format!("schema_version = 1\n[forge.labels.\"{TOKEN}\"]\nname = \"to-test\"\n"),
        format!("schema_version = 1\n[delivery.workflows.\"{TOKEN}\"]\nworkflow = \"cd\"\n"),
        format!("schema_version = 1\n[delivery.workflows.staging]\nworkflow = \"cd\"\ninputs = {{ \"{TOKEN}\" = \"x\" }}\n"),
        format!("schema_version = 1\n[tracker]\nkind = \"jira\"\n[tracker.transitions.\"{TOKEN}\"]\nfrom = \"a\"\nto = \"b\"\n"),
    ] {
        let error = parse_profile(&text).unwrap_err();
        assert!(!error.contains(TOKEN) && !error.contains("ghp_"), "{error}");
        assert!(error.contains("<invalid key #1>"), "{error}");

        let repo = repo_with_profile(Some(&text));
        let snap = snapshot(Some(repo.path()));
        assert_eq!(snap.state, SnapshotState::Invalid);
        let json = serde_json::to_string(&snap).unwrap();
        assert!(!json.contains(TOKEN), "{json}");
        let failure =
            values_for(&snap, &reads(&[("project.forge.base_branch", false)])).unwrap_err();
        assert!(!failure.contains(TOKEN), "{failure}");
    }
}
