use super::*;

const OWN: &str = "3e7f4d85-af84-4c3b-8a0d-4a5b6c7d8e9f";
const OTHER: &str = "4f805e96-b095-4d4c-9b1e-5b6c7d8e9fa0";

/// A root holding two run directories, the way `run_artifacts` lays them out.
fn root_with_two_runs() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("kronn-run-artifacts")
        .join("0123456789ab");
    for run in [OWN, OTHER] {
        std::fs::create_dir_all(root.join(run)).unwrap();
    }
    (temp, root)
}

fn claude_args(dir: &Path) -> Vec<String> {
    vec![
        "--add-dir".into(),
        dir.display().to_string(),
        "prompt".into(),
    ]
}

fn codex_args(dir: &Path) -> Vec<String> {
    vec![
        "-c".into(),
        format!(
            "permissions={{kronn_read_only_repos={{extends=\":workspace\",workspace_roots={{}},filesystem={{\":root\"=\"read\",{}=\"read\"}}}}}}",
            serde_json::to_string(dir).unwrap()
        ),
    ]
}

/// Whether a `Read(//dir/**)` rule covers `path`.
fn read_rule_covers(rules: &[String], path: &Path) -> bool {
    rules.iter().any(|rule| {
        rule.strip_prefix("Read(/")
            .and_then(|rest| rest.strip_suffix("/**)"))
            .is_some_and(|dir| path.starts_with(dir))
    })
}

#[test]
fn the_granted_run_directory_is_read_from_either_runtimes_arguments() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    assert_eq!(
        ArtifactsReach::new(root.clone(), &claude_args(&own)).own(),
        std::slice::from_ref(&own)
    );
    assert_eq!(
        ArtifactsReach::new(root.clone(), &codex_args(&own)).own(),
        std::slice::from_ref(&own)
    );
}

#[test]
fn only_a_direct_run_directory_of_the_root_counts_as_granted() {
    let (temp, root) = root_with_two_runs();
    let elsewhere = temp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(root.join(OWN).join("nested")).unwrap();
    for granted in [
        elsewhere,
        root.clone(),
        root.join(OWN).join("nested"),
        root.parent().unwrap().to_path_buf(),
    ] {
        assert!(
            ArtifactsReach::new(root.clone(), &claude_args(&granted))
                .own()
                .is_empty(),
            "{granted:?}"
        );
        assert!(
            ArtifactsReach::new(root.clone(), &codex_args(&granted))
                .own()
                .is_empty(),
            "{granted:?}"
        );
    }
}

#[test]
fn a_codex_entry_that_is_not_a_read_grant_is_not_one() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    let write = format!("{{{}=\"write\"}}", serde_json::to_string(&own).unwrap());
    assert!(ArtifactsReach::new(root, &["-c".into(), write])
        .own()
        .is_empty());
}

#[test]
fn seatbelt_denies_the_root_then_reopens_the_own_run_for_reads_only() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    let rules = ArtifactsReach::new(root.clone(), &claude_args(&own)).seatbelt_rules();
    let deny = rules.find("(deny file-read* file-write*").unwrap();
    let allow = rules.find("(allow file-read*\n").unwrap();
    assert!(deny < allow, "a later rule wins: {rules}");
    assert!(rules.contains(&format!("(subpath {})", seatbelt_literal(&root))));
    assert!(rules.contains(&format!("(subpath {})", seatbelt_literal(&own))));
    assert!(!rules.contains(OTHER), "{rules}");
    assert!(!rules.contains("(allow file-write*"), "{rules}");
}

#[test]
fn seatbelt_without_a_grant_only_denies() {
    let (_temp, root) = root_with_two_runs();
    let rules = ArtifactsReach::new(root.clone(), &[]).seatbelt_rules();
    assert!(rules.contains(&format!("(subpath {})", seatbelt_literal(&root))));
    assert!(!rules.contains("(allow"), "{rules}");
}

#[test]
fn bubblewrap_covers_the_root_and_binds_the_own_run_back_read_only() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    let mounts = ArtifactsReach::new(root.clone(), &claude_args(&own)).bubblewrap_mounts();
    let (root, own) = (root.display().to_string(), own.display().to_string());
    assert_eq!(
        mounts,
        ["--tmpfs", &root, "--ro-bind", &own, &own].map(String::from)
    );
    let missing = tempfile::tempdir().unwrap().path().join("not-there");
    assert!(ArtifactsReach::new(missing, &[])
        .bubblewrap_mounts()
        .is_empty());
}

#[test]
fn claude_reads_nothing_of_the_root_without_a_grant() {
    let (_temp, root) = root_with_two_runs();
    let rules = ArtifactsReach::new(root.clone(), &[]).claude_permission_deny();
    for run in [OWN, OTHER] {
        assert!(read_rule_covers(&rules, &root.join(run)), "{rules:?}");
    }
    assert!(rules.contains(&crate::core::vault_guard::claude_rule("Edit", &root)));
    assert!(rules.contains(&crate::core::vault_guard::claude_rule("Write", &root)));
}

#[test]
fn claude_with_a_grant_denies_each_other_run_by_its_literal_path() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    let reach = ArtifactsReach::new(root.clone(), &claude_args(&own));
    let rules = reach.claude_permission_deny();
    assert!(read_rule_covers(&rules, &root.join(OTHER)), "{rules:?}");
    assert!(!read_rule_covers(&rules, &own), "{rules:?}");
    assert!(
        !rules.contains(&crate::core::vault_guard::claude_rule("Read", &root)),
        "a deny of the whole root would hide the own run: {rules:?}"
    );
    // The sandbox closes the whole root and re-opens only the own run.
    assert!(reach
        .claude_sandbox_deny()
        .contains(&root.display().to_string()));
    assert_eq!(
        reach.claude_sandbox_allow_read(),
        [own.display().to_string()]
    );
}

#[test]
fn the_literal_denies_stay_bounded() {
    let (_temp, root) = root_with_two_runs();
    for at in 0..MAX_SIBLING_RULES + 20 {
        std::fs::create_dir_all(root.join(format!("run-{at:04}"))).unwrap();
    }
    let rules =
        ArtifactsReach::new(root.clone(), &claude_args(&root.join(OWN))).claude_permission_deny();
    let reads = rules
        .iter()
        .filter(|rule| rule.starts_with("Read("))
        .count();
    assert!(reads <= MAX_SIBLING_RULES * 2, "{reads}");
    assert!(!read_rule_covers(&rules, &root.join(OWN)));
}
