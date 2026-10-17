use super::*;
use crate::models::{DependencyManagerUpdate, DependencyUpdatePackage};
use chrono::TimeZone;

fn summary() -> DependencyUpdateSummary {
    DependencyUpdateSummary {
        managers: vec![
            DependencyManagerUpdate {
                manager: "npm".into(),
                manifest: "frontend/package.json".into(),
                status: DependencyCheckStatus::UpdatesAvailable,
                outdated: 2,
                major: 1,
                packages: vec![
                    DependencyUpdatePackage {
                        name: "react".into(),
                        current: "18.2.0".into(),
                        latest: "19.1.0".into(),
                        major: true,
                        declared_only: false,
                        lockfile: None,
                    },
                    DependencyUpdatePackage {
                        name: "@types/node".into(),
                        current: "20.1.0".into(),
                        latest: "20.4.0".into(),
                        major: false,
                        declared_only: false,
                        lockfile: None,
                    },
                ],
                reason: None,
            },
            DependencyManagerUpdate {
                manager: "cargo".into(),
                manifest: "backend/Cargo.toml".into(),
                status: DependencyCheckStatus::UpToDate,
                outdated: 0,
                major: 0,
                packages: vec![],
                reason: None,
            },
            DependencyManagerUpdate {
                manager: "composer".into(),
                manifest: "composer.json".into(),
                status: DependencyCheckStatus::Unavailable,
                outdated: 0,
                major: 0,
                packages: vec![],
                reason: None,
            },
        ],
        total_outdated: 2,
        total_major: 1,
        checked_at: chrono::Utc
            .with_ymd_and_hms(2026, 10, 4, 23, 30, 0)
            .unwrap(),
        cached: false,
        monitoring_interval_days: Some(7),
        next_check_at: None,
    }
}

#[test]
fn snapshot_keeps_rows_dates_and_unchecked_manifests_apart() {
    let snap = DependencySnapshot::from_summary(&summary());
    assert_eq!(snap.checked_on, "2026-10-04");
    assert_eq!(snap.packages.len(), 2);
    assert_eq!(
        snap.up_to_date,
        vec![("cargo".into(), "backend/Cargo.toml".into())]
    );
    assert_eq!(snap.unchecked.len(), 1);
    assert_eq!(snap.unchecked[0].manager, "composer");
    assert_eq!(
        snap.citation(&snap.packages[1]),
        "[src: deps: npm/@types/node@2026-10-04]"
    );
}

#[test]
fn a_dep_citation_is_verified_only_when_the_snapshot_holds_it() {
    let snap = DependencySnapshot::from_summary(&summary());
    let verified = |raw: &str| verify_dep_marker(raw, Some(&snap)).map(|(s, _)| s);

    assert_eq!(
        verified("deps: npm/react@2026-10-04"),
        Some(SourceStatus::Verified)
    );
    assert_eq!(
        verified("deps: NPM/@types/node@2026-10-04"),
        Some(SourceStatus::Verified)
    );
    assert_eq!(
        verified("deps: npm/vue@2026-10-04"),
        Some(SourceStatus::NotFound)
    );
    assert_eq!(
        verified("deps: cargo/react@2026-10-04"),
        Some(SourceStatus::NotFound)
    );
    assert_eq!(
        verified("deps: npm/react@2026-10-05"),
        Some(SourceStatus::NotFound)
    );
    assert_eq!(verified("deps: npm/react"), Some(SourceStatus::EmptyRef));
    assert_eq!(verified("file: src/lib.rs:1"), None);
    assert_eq!(verified("url: https://example.org"), None);

    let (status, detail) = verify_dep_marker("deps: npm/react@2026-10-04", None).unwrap();
    assert_eq!(status, SourceStatus::NotFound);
    assert!(detail.contains("no dependency watch"), "{detail}");
}

/// KT-1295 — the citation the snapshot emits must verify against that snapshot
/// for every manager label the producer can carry, incl. `JS / TS` (a `/` in the
/// label) and a scoped package (a `/` in the name).
#[test]
fn every_citation_the_snapshot_emits_verifies_against_it() {
    let labels = crate::core::dependency_updates::manager_labels();
    assert!(labels.contains(&"JS / TS"), "{labels:?}");
    let packages = ["react", "argparse", "@types/node", "@scope/pkg"];
    let snap = DependencySnapshot {
        checked_on: "2026-10-16".into(),
        packages: labels
            .iter()
            .flat_map(|label| {
                packages.iter().map(move |name| WatchedPackage {
                    manager: (*label).into(),
                    manifest: "manifest".into(),
                    name: (*name).into(),
                    current: "1.0.0".into(),
                    latest: "2.0.0".into(),
                    major: true,
                    declared_only: false,
                    lockfile: None,
                })
            })
            .collect(),
        unchecked: vec![],
        up_to_date: vec![],
    };
    assert_eq!(snap.packages.len(), labels.len() * packages.len());
    for package in &snap.packages {
        let citation = snap.citation(package);
        let marker = citation
            .trim_start_matches("[src: ")
            .trim_end_matches(']')
            .to_string();
        let (status, detail) = verify_dep_marker(&marker, Some(&snap)).unwrap();
        assert_eq!(status, SourceStatus::Verified, "{citation}: {detail}");
        // The manager label is case-insensitive (the package names here are lowercase).
        let lowered = marker.to_lowercase();
        let (status, detail) = verify_dep_marker(&lowered, Some(&snap)).unwrap();
        assert_eq!(status, SourceStatus::Verified, "{lowered}: {detail}");
    }

    // A manager the watch does not hold, or a package under another manager's
    // label, is still refused.
    let verdict = |raw: &str| verify_dep_marker(raw, Some(&snap)).map(|(s, _)| s);
    assert_eq!(
        verdict("deps: JS / TS/vue@2026-10-16"),
        Some(SourceStatus::NotFound)
    );
    assert_eq!(
        verdict("deps: TS/react@2026-10-16"),
        Some(SourceStatus::NotFound)
    );
    assert_eq!(
        verdict("deps: JS / TS@2026-10-16"),
        Some(SourceStatus::NotFound)
    );
    assert_eq!(verdict("deps: JS / TS/react"), Some(SourceStatus::EmptyRef));
}

#[test]
fn ci_runtimes_are_read_from_pin_files_and_workflows_with_their_line() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join(".nvmrc"), "\n# pin\nv18.17.0\n").unwrap();
    std::fs::write(
        root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.80.0\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".tool-versions"),
        "nodejs 20.11.0\npython 3.12.1\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(
        root.join(".github/workflows/ci.yml"),
        "jobs:\n  test:\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: '18' # LTS\n      - uses: actions/setup-python@v5\n        with:\n          python-version: \"3.11\"\n      - uses: dtolnay/rust-toolchain@master\n        with:\n          toolchain: 1.79\n      - with:\n          node-version: ${{ matrix.node }}\n          node-version-file: .nvmrc\n",
    )
    .unwrap();
    std::fs::write(root.join(".github/workflows/notes.txt"), "node-version: 4").unwrap();

    let found = ci_runtime_versions(root);
    let rows: Vec<(String, String, String, usize)> = found
        .into_iter()
        .map(|r| (r.runtime, r.version, r.file, r.line))
        .collect();
    let row =
        |rt: &str, v: &str, f: &str, l: usize| (rt.to_string(), v.to_string(), f.to_string(), l);
    assert_eq!(
        rows,
        vec![
            row("node", "18.17.0", ".nvmrc", 3),
            row("rust", "1.80.0", "rust-toolchain.toml", 2),
            row("node", "20.11.0", ".tool-versions", 1),
            row("python", "3.12.1", ".tool-versions", 2),
            row("node", "18", ".github/workflows/ci.yml", 6),
            row("python", "3.11", ".github/workflows/ci.yml", 9),
            row("rust", "1.79", ".github/workflows/ci.yml", 12),
        ]
    );
}

#[test]
fn an_empty_repository_pins_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(ci_runtime_versions(dir.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_pin_file_is_not_read() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "s3cr3t\n").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join(".nvmrc")).unwrap();
    assert!(ci_runtime_versions(dir.path()).is_empty());
}

#[test]
fn cascade_prefers_context7_then_web_then_the_watch() {
    let pick = |context7, web, watch| {
        select_version_source(VersionSourceInputs {
            context7,
            web,
            watch,
        })
    };
    assert_eq!(pick(true, true, true), VersionSource::Context7);
    assert_eq!(pick(true, false, false), VersionSource::Context7);
    assert_eq!(pick(false, true, true), VersionSource::Web);
    assert_eq!(pick(false, true, false), VersionSource::Web);
    assert_eq!(pick(false, false, true), VersionSource::DependencyWatch);
    assert_eq!(pick(false, false, false), VersionSource::None);
}

#[test]
fn context7_is_detected_only_when_enabled_and_mountable() {
    let stdio =
        r#"{"mcpServers":{"Context7":{"command":"npx","args":["-y","@upstash/context7-mcp"]}}}"#;
    let remote =
        r#"{"mcpServers":{"context7":{"type":"http","url":"https://mcp.context7.com/mcp"}}}"#;
    let disabled = r#"{"mcpServers":{"context7":{"command":"npx","disabled":true}}}"#;
    let other = r#"{"mcpServers":{"Git":{"command":"uvx"}}}"#;
    assert!(context7_configured(stdio, false));
    assert!(context7_configured(stdio, true));
    assert!(context7_configured(remote, false));
    assert!(!context7_configured(remote, true));
    assert!(!context7_configured(disabled, false));
    assert!(!context7_configured(other, false));
    assert!(!context7_configured("not json", false));
    assert!(!context7_configured("{}", false));
}

#[test]
fn block_states_the_source_and_lists_citable_rows() {
    let snap = DependencySnapshot::from_summary(&summary());
    let runtimes = vec![CiRuntime {
        runtime: "node".into(),
        version: "18".into(),
        file: ".github/workflows/ci.yml".into(),
        line: 6,
        stage: None,
    }];
    let block = render_version_block(
        VersionSource::DependencyWatch,
        Some(&snap),
        &runtimes,
        "2026-10-11",
    );
    assert!(block.contains("**Source used: dependency watch dated 2026-10-04.**"));
    assert!(block.contains("[src: deps: npm/react@2026-10-04]"));
    assert!(block.contains("| react | 18.2.0 | installed | 19.1.0 | major |"));
    assert!(block.contains("[src: file: .github/workflows/ci.yml:6]"));
    assert!(block.contains("not verified: no source"));
    assert!(block.contains("composer.json (composer): not checked"));
    assert!(block.contains("do NOT judge a version"));

    let live = render_version_block(VersionSource::Context7, Some(&snap), &[], "2026-10-11");
    assert!(
        live.contains("**Source used: context7 (fallback: dependency watch dated 2026-10-04).**")
    );
    assert!(live.contains("context7"));
    assert!(!live.contains("do NOT judge a version"));

    let web = render_version_block(VersionSource::Web, None, &[], "2026-10-11");
    assert!(web.contains("**Source used: web (fallback: no dependency watch stored).**"));
    assert!(web.contains("No dependency watch result is stored"));

    let none = render_version_block(VersionSource::None, None, &[], "2026-10-11");
    assert!(none.contains("**Source used: no dependency watch stored.**"));
}

#[test]
fn a_large_watch_is_capped_with_a_remainder_count() {
    let mut s = summary();
    s.managers[0].packages = (0..70)
        .map(|i| DependencyUpdatePackage {
            name: format!("pkg-{i}"),
            current: "1.0.0".into(),
            latest: "2.0.0".into(),
            major: true,
            declared_only: false,
            lockfile: None,
        })
        .collect();
    let snap = DependencySnapshot::from_summary(&s);
    let block = render_version_block(
        VersionSource::DependencyWatch,
        Some(&snap),
        &[],
        "2026-10-11",
    );
    assert!(block.contains("pkg-59"));
    assert!(!block.contains("pkg-60 "));
    assert!(block.contains("…and 10 more row(s) not shown."));
}

#[test]
fn the_stack_table_block_feeds_the_currency_cells_and_asks_for_no_td() {
    // KT-819 DoD 8 — the project index reads the same dated rows, with its own
    // instructions: no `Dependencies` coverage row and no TD exists in step 1.
    let snap = DependencySnapshot::from_summary(&summary());
    let block = render_version_block_for(
        VersionPurpose::StackTable,
        VersionSource::DependencyWatch,
        Some(&snap),
        &[],
        "2026-10-11",
    );
    assert!(block.contains("`Latest stable` and `Support end` cells"));
    assert!(block.contains("[src: deps: npm/react@2026-10-04]"));
    assert!(block.contains("never filled from memory"));
    assert!(!block.contains("coverage row"), "{block}");
    assert!(!block.contains("becomes a TD"), "{block}");
    // The judging steps keep their wording.
    let judged = render_version_block(
        VersionSource::DependencyWatch,
        Some(&snap),
        &[],
        "2026-10-11",
    );
    assert!(judged.contains("`Dependencies` coverage row"));
    assert!(judged.contains("becomes a TD"));
}

/// A front-apollo-shaped tree (KT-1279): the reusable workflow's input is
/// `nodeversion`, Jenkins pins Node in an agent label, the Dockerfile in FROM.
fn front_apollo_like(root: &Path) {
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(
        root.join(".github/workflows/CI_Quality-test.yaml"),
        "name: Quality\njobs:\n  quality:\n    uses: org/shared/.github/workflows/quality.yaml@main\n    with:\n      nodeversion: 20\n      workdir: .\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Jenkinsfile"),
        "pipeline {\n  agent { label 'slavenodejs18' }\n  tools {\n    nodejs 'NodeJS 22.3'\n  }\n  stages { }\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("Dockerfile"),
        "ARG NODE_VERSION=21\nFROM --platform=linux/amd64 docker.io/library/node:20.11-alpine AS build\nFROM nginx:1.25\nFROM node:${NODE_VERSION}\n",
    )
    .unwrap();
}

#[test]
fn reusable_workflow_inputs_jenkinsfile_and_dockerfile_pins_are_found() {
    let dir = tempfile::tempdir().unwrap();
    front_apollo_like(dir.path());

    let rows: Vec<(String, String, String, usize)> = ci_runtime_versions(dir.path())
        .into_iter()
        .map(|r| (r.runtime, r.version, r.file, r.line))
        .collect();
    let row = |v: &str, f: &str, l: usize| ("node".to_string(), v.to_string(), f.to_string(), l);
    assert_eq!(
        rows,
        vec![
            row("20", ".github/workflows/CI_Quality-test.yaml", 6),
            row("18", "Jenkinsfile", 2),
            row("22.3", "Jenkinsfile", 4),
            row("21", "Dockerfile", 1),
            row("20.11", "Dockerfile", 2),
        ]
    );
}

#[test]
fn a_node_version_input_is_matched_in_any_spelling_and_by_its_default() {
    let mut out = Vec::new();
    scan_workflow(
        "on:\n  workflow_call:\n    inputs:\n      node_version:\n        type: string\n        default: '18'\n      NodeVersion: 16\n      node-version-file: .nvmrc\n      nodeversionX: 4\n",
        "w.yml",
        &mut out,
    );
    let rows: Vec<_> = out.iter().map(|r| (r.version.as_str(), r.line)).collect();
    assert_eq!(rows, vec![("18", 6), ("16", 7)]);
}

#[test]
fn jenkins_labels_and_tools_only_match_node_versions() {
    let mut out = Vec::new();
    scan_jenkinsfile(
        "agent { label 'maven3' }\nagent { label \"node20\" }\nnodejs(nodeJSInstallationName: 'Node 18')\n// nodejs '14'\nlabel 'nodeless'\n",
        "Jenkinsfile",
        &mut out,
    );
    let rows: Vec<_> = out.iter().map(|r| (r.version.as_str(), r.line)).collect();
    assert_eq!(rows, vec![("20", 2), ("18", 3)]);
}

#[test]
fn a_node_agent_label_needs_node_directly_followed_by_a_major() {
    for (label, expected) in [
        ("slavenodejs18", Some("18")),
        ("node22-arm", Some("22")),
        ("nodejs20.4", Some("20.4")),
        ("NodeJS16", Some("16")),
        ("build-node-1", None),
        ("docker-node-02", None),
        ("k8s-node-3", None),
        ("node3", None),
        ("node-20", None),
        ("node2024", None),
        ("node22x", None),
        ("nodejs-builder", None),
        ("maven3", None),
    ] {
        assert_eq!(
            node_label_version(label).as_deref(),
            expected,
            "label {label}"
        );
    }
    let mut out = Vec::new();
    scan_jenkinsfile(
        "agent { label 'build-node-1' }\nagent { label 'k8s-node-3' }\nagent { label 'docker-node-02' }\n",
        "Jenkinsfile",
        &mut out,
    );
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn a_dockerfile_only_pins_a_node_that_starts_with_a_digit() {
    let mut out = Vec::new();
    scan_dockerfile(
        "FROM node:lts-alpine\nFROM node:alpine\nFROM node:current\nFROM node\nARG NODE_VERSION=$X\nARG NODE_VERSION=lts\nARG NODE_VERSION=\"22.1\"\nFROM node:22.1-slim\n",
        "Dockerfile",
        &mut out,
    );
    let rows: Vec<_> = out.iter().map(|r| (r.version.as_str(), r.line)).collect();
    assert_eq!(rows, vec![("22.1", 7), ("22.1", 8)]);
}

#[test]
fn a_multi_stage_dockerfile_keeps_a_row_per_stage_and_marks_the_runtime_one() {
    let mut out = Vec::new();
    scan_dockerfile(
        "FROM node:18 AS deps\nFROM node:20-alpine AS build\nFROM node:22-slim\nCMD [\"node\"]\n",
        "Dockerfile",
        &mut out,
    );
    let rows: Vec<_> = out
        .iter()
        .map(|r| (r.version.as_str(), r.line, r.stage))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("18", 1, Some(Stage::Build)),
            ("20", 2, Some(Stage::Build)),
            ("22", 3, Some(Stage::Runtime)),
        ]
    );

    // A non-node final stage leaves every node stage a build stage.
    let mut out = Vec::new();
    scan_dockerfile(
        "FROM node:20 AS build\nFROM nginx:1.25\n",
        "Dockerfile",
        &mut out,
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].stage, Some(Stage::Build));

    // A single stage is not annotated.
    let mut out = Vec::new();
    scan_dockerfile("FROM node:20\n", "Dockerfile", &mut out);
    assert_eq!(out[0].stage, None);

    let block = render_version_block_with_ci(
        VersionPurpose::Judgement,
        VersionSource::None,
        None,
        &[CiRuntime {
            runtime: "node".into(),
            version: "22".into(),
            file: "Dockerfile".into(),
            line: 3,
            stage: Some(Stage::Runtime),
        }],
        &[],
        "2026-10-11",
    );
    assert!(
        block.contains("| node (final stage: what runs) | 22 |"),
        "{block}"
    );
}

#[test]
fn a_declared_only_row_is_never_rendered_as_the_version_in_place() {
    let summary = DependencyUpdateSummary {
        managers: vec![DependencyManagerUpdate {
            manager: "npm".into(),
            manifest: "package.json".into(),
            status: DependencyCheckStatus::UpdatesAvailable,
            outdated: 1,
            major: 1,
            packages: vec![DependencyUpdatePackage {
                name: "joi".into(),
                current: String::new(),
                latest: "18.0.0".into(),
                major: true,
                declared_only: true,
                lockfile: None,
            }],
            reason: None,
        }],
        total_outdated: 1,
        total_major: 1,
        checked_at: chrono::Utc.with_ymd_and_hms(2026, 10, 4, 10, 0, 0).unwrap(),
        cached: false,
        monitoring_interval_days: None,
        next_check_at: None,
    };
    let snap = DependencySnapshot::from_summary(&summary);
    let block = render_version_block(
        VersionSource::DependencyWatch,
        Some(&snap),
        &[],
        "2026-10-11",
    );
    assert!(
        block.contains("| joi | not installed (declared range only; installed version unknown) | none | 18.0.0 | major |"),
        "{block}"
    );
    assert!(!block.contains("| joi |  |"), "{block}");
}

#[test]
fn ci_files_present_lists_what_exists_whether_parsed_or_not() {
    let dir = tempfile::tempdir().unwrap();
    front_apollo_like(dir.path());
    std::fs::write(dir.path().join(".gitlab-ci.yml"), "stages: []\n").unwrap();
    assert_eq!(
        ci_files_present(dir.path()),
        vec![
            ".github/workflows/CI_Quality-test.yaml",
            ".gitlab-ci.yml",
            "Jenkinsfile",
            "Dockerfile",
        ]
    );
    assert!(ci_files_present(tempfile::tempdir().unwrap().path()).is_empty());
}

#[test]
fn an_empty_pin_table_says_none_parsed_and_lists_the_ci_files() {
    let files = vec![
        ".github/workflows/ci.yml".to_string(),
        "Jenkinsfile".to_string(),
    ];
    let block = render_version_block_with_ci(
        VersionPurpose::Judgement,
        VersionSource::DependencyWatch,
        None,
        &[],
        &files,
        "2026-10-16",
    );
    assert!(block.contains("None parsed by Kronn"), "{block}");
    assert!(!block.contains("None found"), "{block}");
    assert!(
        block.contains("CI files present: `.github/workflows/ci.yml`, `Jenkinsfile`"),
        "{block}"
    );

    let bare = render_version_block(VersionSource::DependencyWatch, None, &[], "2026-10-16");
    assert!(bare.contains("None parsed by Kronn"), "{bare}");
    assert!(bare.contains("no CI file was found either"), "{bare}");
}

#[test]
fn a_not_installed_manifest_is_listed_as_unchecked() {
    let mut s = summary();
    s.managers[2].status = DependencyCheckStatus::NotInstalled;
    s.managers[2].manifest = "package.json".into();
    let snap = DependencySnapshot::from_summary(&s);
    assert!(snap
        .unchecked
        .iter()
        .any(|m| m.manifest == "package.json" && m.status == DependencyCheckStatus::NotInstalled));
    assert!(!snap
        .up_to_date
        .iter()
        .any(|(_, manifest)| manifest == "package.json"));
}

/// KT-1325 — the version source (lockfile vs installed) travels from the
/// snapshot to the prompt table and the citation verdict.
#[test]
fn a_lockfile_version_names_its_source_in_the_table_and_the_citation_verdict() {
    let mut s = summary();
    s.managers[0].packages[0].lockfile = Some("package-lock.json".into());
    let snap = DependencySnapshot::from_summary(&s);
    assert_eq!(
        snap.packages[0].lockfile.as_deref(),
        Some("package-lock.json")
    );
    assert_eq!(snap.packages[1].lockfile, None);

    let block = render_version_block(
        VersionSource::DependencyWatch,
        Some(&snap),
        &[],
        "2026-10-11",
    );
    assert!(
        block.contains("| react | 18.2.0 | from package-lock.json | 19.1.0 | major |"),
        "{block}"
    );
    assert!(
        block.contains("| @types/node | 20.1.0 | installed | 20.4.0 | minor/patch |"),
        "{block}"
    );

    let (status, detail) = verify_dep_marker("deps: npm/react@2026-10-04", Some(&snap)).unwrap();
    assert_eq!(status, SourceStatus::Verified);
    assert!(
        detail.contains("18.2.0 (from package-lock.json)"),
        "{detail}"
    );
    let (_, installed) =
        verify_dep_marker("deps: npm/@types/node@2026-10-04", Some(&snap)).unwrap();
    assert!(installed.contains("20.1.0 (installed)"), "{installed}");
}

/// KT-1325 — a manifest the watch could not check says why, in the snapshot
/// and in the prompt block.
#[test]
fn an_unchecked_manifest_carries_its_reason_into_the_prompt_block() {
    let mut s = summary();
    s.managers[2].status = DependencyCheckStatus::Error;
    s.managers[2].reason = Some("`composer` failed (exit 1): No composer.lock file present".into());
    let snap = DependencySnapshot::from_summary(&s);
    assert_eq!(snap.unchecked.len(), 1);
    assert_eq!(
        snap.unchecked[0].reason.as_deref(),
        Some("`composer` failed (exit 1): No composer.lock file present")
    );
    let block = render_version_block(
        VersionSource::DependencyWatch,
        Some(&snap),
        &[],
        "2026-10-11",
    );
    assert!(
        block.contains("composer.json (composer): not checked (Error) — `composer` failed (exit 1): No composer.lock file present"),
        "{block}"
    );
}
