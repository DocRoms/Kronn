use super::*;

const OWN: &str = "3e7f4d85-af84-4c3b-8a0d-4a5b6c7d8e9f";
const OTHER: &str = "4f805e96-b095-4d4c-9b1e-5b6c7d8e9fa0";
const LINKED: &str = "5a916fa7-c1a6-4e5d-8c2f-6c7d8e9fa0b1";

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

fn grant(dir: &Path) -> Vec<PathBuf> {
    vec![dir.to_path_buf()]
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
fn the_granted_run_directory_is_the_canonical_own_run() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    assert_eq!(
        ArtifactsReach::new(root.clone(), &grant(&own)).own(),
        std::slice::from_ref(&own)
    );
    // A root, and a grant, spelled through a link to the root: the rules still
    // name the canonical run, never the spelling that went through the link.
    #[cfg(unix)]
    {
        let linked_root = root.parent().unwrap().join("via-link");
        std::os::unix::fs::symlink(&root, &linked_root).unwrap();
        for granted in [linked_root.join(OWN), own.clone()] {
            assert_eq!(
                ArtifactsReach::new(linked_root.clone(), &grant(&granted)).own(),
                std::slice::from_ref(&own),
                "{granted:?}"
            );
        }
    }
}

#[test]
fn only_a_direct_run_directory_of_the_root_counts_as_granted() {
    let (temp, root) = root_with_two_runs();
    let elsewhere = temp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(root.join(OWN).join("nested")).unwrap();
    // A directory of the root that is not named like a run.
    std::fs::create_dir_all(root.join("scratch")).unwrap();
    for granted in [
        elsewhere,
        root.clone(),
        root.join(OWN).join("nested"),
        root.join("scratch"),
        root.join("missing"),
        root.parent().unwrap().to_path_buf(),
        PathBuf::from("relative/dir"),
    ] {
        assert!(
            ArtifactsReach::new(root.clone(), &grant(&granted))
                .own()
                .is_empty(),
            "{granted:?}"
        );
    }
}

#[test]
fn a_grant_with_a_parent_component_is_refused_even_when_it_resolves_to_a_run() {
    let (_temp, root) = root_with_two_runs();
    for granted in [
        root.join(OTHER).join("..").join(OWN),
        root.join("a").join("..").join(OWN),
        root.join(OWN).join("..").join(OWN),
        root.join("..").join(root.file_name().unwrap()).join(OWN),
    ] {
        assert!(
            ArtifactsReach::new(root.clone(), &grant(&granted))
                .own()
                .is_empty(),
            "{granted:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_link_inside_the_root_is_never_a_grant() {
    let (_temp, root) = root_with_two_runs();
    // A run-shaped name that is a link to another run, and a link to the own run.
    std::os::unix::fs::symlink(root.join(OTHER), root.join(LINKED)).unwrap();
    std::os::unix::fs::symlink(root.join(OWN), root.join("alias")).unwrap();
    for granted in [root.join(LINKED), root.join("alias")] {
        assert!(
            ArtifactsReach::new(root.clone(), &grant(&granted))
                .own()
                .is_empty(),
            "{granted:?}"
        );
    }
    // The victim is not opened by the link either: no rule names the link's
    // target as granted.
    let reach = ArtifactsReach::new(root.clone(), &grant(&root.join(LINKED)));
    assert!(!reach.seatbelt_rules().contains("(allow file-read*"));
    assert!(reach.bubblewrap_mounts().iter().all(|mount| mount != OTHER));
}

#[test]
fn an_uppercase_spelling_never_becomes_a_grant() {
    let (_temp, root) = root_with_two_runs();
    let upper = root.join(OWN.to_uppercase());
    let own = ArtifactsReach::new(root.clone(), &grant(&upper));
    // On a case-sensitive volume nothing is there; on a case-insensitive one it
    // is the own run itself, and only its on-disk (lowercase) spelling is kept.
    for dir in own.own() {
        assert_eq!(dir, &root.join(OWN), "{dir:?}");
    }
    let victim = ArtifactsReach::new(root.clone(), &grant(&root.join(OTHER.to_uppercase())));
    for dir in victim.own() {
        assert_eq!(dir, &root.join(OTHER), "{dir:?}");
        assert!(
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == name.to_lowercase()),
            "{dir:?}"
        );
    }
}

#[test]
fn seatbelt_denies_the_root_then_reopens_the_own_run_for_reads_only() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    let rules = ArtifactsReach::new(root.clone(), &grant(&own)).seatbelt_rules();
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
    let mounts = ArtifactsReach::new(root.clone(), &grant(&own)).bubblewrap_mounts();
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
    let reach = ArtifactsReach::new(root.clone(), &[]).claude_reach();
    for run in [OWN, OTHER] {
        assert!(
            read_rule_covers(&reach.permission_deny, &root.join(run)),
            "{reach:?}"
        );
    }
    assert!(reach
        .permission_deny
        .contains(&crate::core::vault_guard::claude_rule("Edit", &root)));
    assert!(reach
        .permission_deny
        .contains(&crate::core::vault_guard::claude_rule("Write", &root)));
    assert!(reach.sandbox_allow_read.is_empty());
}

#[test]
fn claude_with_a_grant_denies_each_other_run_by_its_literal_path() {
    let (_temp, root) = root_with_two_runs();
    let own = root.join(OWN);
    let reach = ArtifactsReach::new(root.clone(), &grant(&own));
    let claude = reach.claude_reach();
    assert!(
        read_rule_covers(&claude.permission_deny, &root.join(OTHER)),
        "{claude:?}"
    );
    assert!(
        !read_rule_covers(&claude.permission_deny, &own),
        "{claude:?}"
    );
    assert!(
        !claude
            .permission_deny
            .contains(&crate::core::vault_guard::claude_rule("Read", &root)),
        "a deny of the whole root would hide the own run: {claude:?}"
    );
    // The sandbox closes the whole root and re-opens only the own run.
    assert!(reach
        .claude_sandbox_deny()
        .contains(&root.display().to_string()));
    assert_eq!(claude.sandbox_allow_read, [own.display().to_string()]);
}

/// A step over the cap is not granted at all: the Read tool closes the whole
/// root and the sandbox re-opens nothing, so no run depends on the working
/// directory limit alone.
#[test]
fn a_launch_with_more_runs_than_the_rules_can_name_fails_closed() {
    let (_temp, root) = root_with_two_runs();
    for at in 0..MAX_SIBLING_RULES + 20 {
        std::fs::create_dir_all(root.join(format!("run-{at:04}"))).unwrap();
    }
    let reach = ArtifactsReach::new(root.clone(), &grant(&root.join(OWN)));
    assert_eq!(reach.own().len(), 1, "the grant itself is valid");
    let claude = reach.claude_reach();
    for run in [OWN, OTHER] {
        assert!(
            read_rule_covers(&claude.permission_deny, &root.join(run)),
            "{run}"
        );
    }
    assert!(claude.sandbox_allow_read.is_empty(), "{claude:?}");
    assert!(
        claude.permission_deny.len() < 10,
        "no per-run rule is emitted past the cap"
    );
}

#[test]
fn at_the_cap_every_other_run_is_still_named() {
    let (_temp, root) = root_with_two_runs();
    // OTHER is already there: fill up to exactly the cap of other entries.
    for at in 0..MAX_SIBLING_RULES - 1 {
        std::fs::create_dir_all(root.join(format!("run-{at:04}"))).unwrap();
    }
    let claude = ArtifactsReach::new(root.clone(), &grant(&root.join(OWN))).claude_reach();
    assert!(!claude.sandbox_allow_read.is_empty());
    for at in 0..MAX_SIBLING_RULES - 1 {
        assert!(read_rule_covers(
            &claude.permission_deny,
            &root.join(format!("run-{at:04}"))
        ));
    }
    assert!(read_rule_covers(&claude.permission_deny, &root.join(OTHER)));
}
