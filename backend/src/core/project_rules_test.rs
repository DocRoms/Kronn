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
fn a_module_matches_itself_and_its_submodules_only() {
    assert!(names_module("lodash", "lodash", Syntax::Js));
    assert!(names_module("lodash/fp", "lodash", Syntax::Js));
    assert!(!names_module("lodash.debounce", "lodash", Syntax::Js));
    assert!(!names_module("lodash-es", "lodash", Syntax::Js));
    assert!(names_module(
        "requests.adapters",
        "requests",
        Syntax::Python
    ));
    assert!(!names_module(
        "requests_toolbelt",
        "requests",
        Syntax::Python
    ));
    assert!(names_module("::serde::Serialize", "serde", Syntax::Rust));
    assert!(!names_module("serde_json", "serde", Syntax::Rust));
    assert!(names_module(
        "\\App\\Legacy\\Mailer",
        "App\\Legacy",
        Syntax::Php
    ));
}

#[test]
fn comments_and_prose_are_not_imports() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "src/a.ts",
        "// we use moment here\n/* import moment from 'moment' */\n * require('moment')\nconst s = 'moment';\nlog(\"from moment\");\n",
    );
    write(
        t.path(),
        "src/b.py",
        "# import moment\nprint('use moment')\n",
    );
    write(
        t.path(),
        "src/c.rs",
        "// use moment;\nlet use_moment = 1;\n",
    );
    let rules = [ban("x", ProjectRulePatternKind::Import, "moment")];
    assert!(detect_ban_violations(t.path(), &rules).is_empty());
}

#[test]
fn import_syntax_is_read_per_language() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "a.py",
        "from requests.adapters import X\nimport os, requests as r\n",
    );
    write(
        t.path(),
        "b.rs",
        "pub use serde::Serialize;\nextern crate serde;\n",
    );
    write(
        t.path(),
        "c.go",
        "import (\n\t\"fmt\"\n\tlog \"github.com/x/logrus\"\n)\n",
    );
    write(t.path(), "d.java", "import static com.legacy.Util.run;\n");
    write(t.path(), "e.c", "#include <legacy/api.h>\n");
    write(
        t.path(),
        "f.vue",
        "<script>\nimport { x } from \"legacy-ui/button\";\n</script>\n",
    );
    let rules = [
        ban("a", ProjectRulePatternKind::Import, "requests"),
        ban("b", ProjectRulePatternKind::Import, "serde"),
        ban("c", ProjectRulePatternKind::Import, "github.com/x/logrus"),
        ban("d", ProjectRulePatternKind::Import, "com.legacy"),
        ban("e", ProjectRulePatternKind::Import, "legacy"),
        ban("f", ProjectRulePatternKind::Import, "legacy-ui"),
    ];
    let mut ev = evidence(&detect_ban_violations(t.path(), &rules))
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    ev.sort();
    assert_eq!(
        ev,
        vec!["a.py:1", "a.py:2", "b.rs:1", "b.rs:2", "c.go:3", "d.java:1", "e.c:1", "f.vue:2"]
    );
}

#[test]
fn cargo_dotted_keys_and_pyproject_are_read() {
    let t = tempfile::tempdir().unwrap();
    write(
        t.path(),
        "Cargo.toml",
        "[package]\nname = \"x\"\n\n[dependencies]\nserde.workspace = true\nopenssl.version = \"0.10\"\n",
    );
    write(
        t.path(),
        "py/pyproject.toml",
        "[project]\nname = \"x\"\ndependencies = [\n  \"requests>=2\",\n]\n[project.optional-dependencies]\ndev = [\"PyYAML\"]\n",
    );
    write(
        t.path(),
        "poetry/pyproject.toml",
        "[tool.poetry]\nname = \"y\"\n\n[tool.poetry.dependencies]\npython = \"^3.11\"\nDjango = \"^5\"\n\n[tool.poetry.group.dev.dependencies]\npytest = \"*\"\n",
    );
    let rules = [
        ban("a", ProjectRulePatternKind::Dependency, "serde"),
        ban("b", ProjectRulePatternKind::Dependency, "openssl"),
        ban("c", ProjectRulePatternKind::Dependency, "requests"),
        ban("d", ProjectRulePatternKind::Dependency, "pyyaml"),
        ban("e", ProjectRulePatternKind::Dependency, "django"),
        ban("f", ProjectRulePatternKind::Dependency, "pytest"),
        ban("g", ProjectRulePatternKind::Dependency, "flask"),
    ];
    let mut ev = evidence(&detect_ban_violations(t.path(), &rules))
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    ev.sort();
    assert_eq!(
        ev,
        vec![
            "Cargo.toml:5",
            "Cargo.toml:6",
            "poetry/pyproject.toml:6",
            "poetry/pyproject.toml:9",
            "py/pyproject.toml:4",
            "py/pyproject.toml:7",
        ]
    );
}

#[test]
fn glob_matching_semantics() {
    assert!(glob_matches("src/**/*.min.js", "src/a/b/x.min.js"));
    assert!(glob_matches("src/**/*.min.js", "src/x.min.js"));
    assert!(!glob_matches("src/*.js", "src/a/x.js"));
    assert!(glob_matches("src/**", "src/a/b"));
    assert!(glob_matches("a?c", "abc"));
    assert!(!glob_matches("a?c", "a/c"));
    assert!(!glob_matches("*.min.js", "x.js"));
    assert!(glob_hit("*.min.js", "deep/dir/app.min.js"));
}

#[test]
fn a_pathological_glob_stays_linear() {
    // The backtracking matcher took >20M calls for 12 stars on a 48-char path.
    let path = format!("{}/{}", "a".repeat(30), "b".repeat(200));
    let pattern = format!("{}Q", "*a".repeat(40));
    let start = std::time::Instant::now();
    for _ in 0..1_000 {
        assert!(!glob_matches(&pattern, &path));
        assert!(!glob_matches("**********************Q", &path));
    }
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}
