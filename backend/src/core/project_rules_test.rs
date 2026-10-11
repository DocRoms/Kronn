use super::*;
use std::fs;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(p, body).unwrap();
}

fn ban(text: &str, kind: ProjectRulePatternKind, value: &str) -> ProjectRule {
    ProjectRule {
        kind: ProjectRuleKind::Ban,
        text: text.into(),
        pattern: Some(ProjectRulePattern {
            kind,
            value: value.into(),
        }),
    }
}

fn evidence(signals: &[DetectedSignal]) -> Vec<&str> {
    signals.iter().map(|s| s.evidence.as_str()).collect()
}

#[test]
fn no_checked_ban_means_no_signal() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "package.json",
        r#"{"dependencies":{"lodash":"1"}}"#,
    );
    let rule = ProjectRule {
        kind: ProjectRuleKind::Rule,
        text: "always lodash".into(),
        pattern: Some(ProjectRulePattern {
            kind: ProjectRulePatternKind::Dependency,
            value: "lodash".into(),
        }),
    };
    let plain_ban = ProjectRule {
        kind: ProjectRuleKind::Ban,
        text: "no new dependency without approval".into(),
        pattern: None,
    };
    assert!(detect_ban_violations(t.path(), &[rule, plain_ban]).is_empty());
}

#[test]
fn a_forbidden_dependency_fires_in_each_manifest_kind() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "package.json",
        "{\n  \"name\": \"app\",\n  \"devDependencies\": {\n    \"lodash\": \"^4\"\n  }\n}\n",
    );
    write(
        t.path(),
        "api/composer.json",
        "{\n  \"require\": {\n    \"Vendor/Legacy\": \"*\"\n  }\n}\n",
    );
    write(
        t.path(),
        "core/Cargo.toml",
        "[package]\nname = \"core\"\n\n[dependencies]\nserde = \"1\"\nopenssl_sys = \"0.9\"\n\n[dev-dependencies.mockall]\nversion = \"0.12\"\n",
    );
    write(
        t.path(),
        "requirements-dev.txt",
        "# tools\nblack==24.1\nPyYAML>=6 ; python_version > '3'\n",
    );
    let rules = [
        ban("never lodash", ProjectRulePatternKind::Dependency, "lodash"),
        ban(
            "no legacy",
            ProjectRulePatternKind::Dependency,
            "vendor/legacy",
        ),
        ban(
            "no openssl",
            ProjectRulePatternKind::Dependency,
            "openssl-sys",
        ),
        ban("no mockall", ProjectRulePatternKind::Dependency, "mockall"),
        ban("no yaml", ProjectRulePatternKind::Dependency, "pyyaml"),
    ];
    let signals = detect_ban_violations(t.path(), &rules);
    let mut ev = evidence(&signals);
    ev.sort();
    assert_eq!(
        ev,
        vec![
            "api/composer.json:3",
            "core/Cargo.toml:6",
            "core/Cargo.toml:8",
            "package.json:4",
            "requirements-dev.txt:3",
        ]
    );
    assert!(signals.iter().all(|s| s.detector_id == DETECTOR_ID));
    assert!(signals[0].title.starts_with("Ban violated: "));
}

#[test]
fn a_dependency_not_declared_does_not_fire() {
    let t = tempfile::tempdir().unwrap();
    // Named in scripts and in the package name, never declared as a dependency.
    write(
        t.path(),
        "package.json",
        r#"{"name":"lodash-free","scripts":{"lodash":"echo"},"dependencies":{"lodash-es":"4"}}"#,
    );
    write(
        t.path(),
        "Cargo.toml",
        "[package]\nname = \"lodash\"\n[features]\nlodash = []\n",
    );
    write(t.path(), "requirements.txt", "lodashy==1\n");
    let rules = [ban("x", ProjectRulePatternKind::Dependency, "lodash")];
    assert!(detect_ban_violations(t.path(), &rules).is_empty());
}

#[test]
fn a_forbidden_import_fires_on_import_lines_only() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/a.ts",
        "import _ from 'lodash';\nconst x = require(\"lodash/fp\");\n// lodash is gone\nconst lodashy = 1;\nimport y from 'lodash-es';\n",
    );
    write(t.path(), "src/b.php", "<?php\nuse App\\Legacy\\Mailer;\n");
    write(t.path(), "README.md", "import lodash\n");
    let rules = [
        ban("never lodash", ProjectRulePatternKind::Import, "lodash"),
        ban(
            "never the legacy mailer",
            ProjectRulePatternKind::Import,
            "App\\Legacy\\Mailer",
        ),
    ];
    let mut ev = evidence(&detect_ban_violations(t.path(), &rules))
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    ev.sort();
    assert_eq!(ev, vec!["src/a.ts:1", "src/a.ts:2", "src/b.php:2"]);
}

#[test]
fn a_forbidden_import_absent_does_not_fire() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/a.ts", "import x from 'lodash-es';\n");
    let rules = [ban("x", ProjectRulePatternKind::Import, "lodash")];
    assert!(detect_ban_violations(t.path(), &rules).is_empty());
}

#[test]
fn a_forbidden_glob_fires_by_path_and_by_name() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/legacy/old.js", "x");
    write(t.path(), "public/app.min.js", "x");
    write(t.path(), "src/app.js", "x");
    let rules = [
        ban(
            "no legacy dir",
            ProjectRulePatternKind::FileGlob,
            "src/legacy/**",
        ),
        ban("no minified", ProjectRulePatternKind::FileGlob, "*.min.js"),
    ];
    let signals = detect_ban_violations(t.path(), &rules);
    let mut ev = evidence(&signals);
    ev.sort();
    assert_eq!(ev, vec!["public/app.min.js", "src/legacy/old.js"]);
}

#[test]
fn a_glob_matching_nothing_does_not_fire() {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "src/app.js", "x");
    let rules = [ban("x", ProjectRulePatternKind::FileGlob, "*.min.js")];
    assert!(detect_ban_violations(t.path(), &rules).is_empty());
}

#[cfg(unix)]
#[test]
fn symlinks_are_never_followed() {
    let t = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(
        outside.path(),
        "package.json",
        r#"{"dependencies":{"lodash":"1"}}"#,
    );
    write(outside.path(), "x.ts", "import 'lodash';\n");
    std::os::unix::fs::symlink(outside.path(), t.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("package.json"),
        t.path().join("package.json"),
    )
    .unwrap();
    let rules = [
        ban("a", ProjectRulePatternKind::Dependency, "lodash"),
        ban("b", ProjectRulePatternKind::Import, "lodash"),
    ];
    assert!(detect_ban_violations(t.path(), &rules).is_empty());
}

#[test]
fn hits_are_capped_with_a_remainder_signal() {
    let t = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(t.path(), &format!("src/f{i}.ts"), "import 'lodash';\n");
    }
    let rules = [ban("x", ProjectRulePatternKind::Import, "lodash")];
    let signals = detect_ban_violations(t.path(), &rules);
    assert_eq!(
        signals.len(),
        crate::core::audit_detectors::MAX_PER_DETECTOR + 1
    );
    assert!(signals.last().unwrap().evidence.starts_with('+'));
}

#[test]
fn ban_signals_reach_step_8_and_need_a_disposition() {
    use crate::core::audit_detectors::{scan_project_with_rules, undisposed_signals};
    let t = tempfile::tempdir().unwrap();
    write(t.path(), "public/app.min.js", "x");
    write(t.path(), "src/a.ts", "import _ from 'lodash';\n");
    let rules = [
        ban("no minified", ProjectRulePatternKind::FileGlob, "*.min.js"),
        ban("never lodash", ProjectRulePatternKind::Import, "lodash"),
    ];
    let scan = scan_project_with_rules(t.path(), &rules);
    let bans: Vec<_> = scan
        .signals
        .into_iter()
        .filter(|s| s.detector_id == DETECTOR_ID)
        .collect();
    assert_eq!(bans.len(), 2);
    assert_eq!(undisposed_signals(&bans, "nothing").len(), 2);
    let disposed = "TD-1: `src/a.ts:1` imports lodash; TD-2: `public/app.min.js` committed";
    assert!(undisposed_signals(&bans, disposed).is_empty());
}

#[test]
fn word_match_needs_a_boundary() {
    assert!(word_match("import 'lodash';", "lodash"));
    assert!(word_match("from \"lodash/fp\"", "lodash"));
    assert!(!word_match("import 'lodash-es'", "lodash"));
    assert!(!word_match("const mylodash = 1", "lodash"));
    assert!(word_match("é lodash", "lodash"));
    assert!(!word_match("anything", ""));
}
