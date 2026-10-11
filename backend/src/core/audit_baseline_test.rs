use super::*;
use std::fs;

fn proj() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn run(root: &Path) -> Vec<DetectedSignal> {
    run_baseline(root, &Inventory::collect(root))
}

fn of<'a>(signals: &'a [DetectedSignal], id: &str) -> Vec<&'a DetectedSignal> {
    signals.iter().filter(|s| s.detector_id == id).collect()
}

const GOOD_DOCKERFILE: &str = "\
FROM node:22.4-slim AS build
RUN apt-get update && apt-get install -y --no-install-recommends git \\
    && rm -rf /var/lib/apt/lists/*
FROM gcr.io/distroless/nodejs22-debian12@sha256:abc
COPY --from=build /app /app
HEALTHCHECK CMD [\"/app/health\"]
USER 10001
";

#[test]
fn a_clean_dockerfile_raises_no_dockerfile_signal() {
    let t = proj();
    write(t.path(), "Dockerfile", GOOD_DOCKERFILE);
    let s = run(t.path());
    assert!(
        s.iter().all(|s| !s.detector_id.starts_with("dockerfile-")),
        "{s:?}"
    );
}

#[test]
fn instructions_join_continuations_and_skip_comments() {
    let instrs = dockerfile_instructions("# c\nFROM a:1\nRUN x \\\n  # inner\n  && y\nUSER app\n");
    assert_eq!(instrs.len(), 3);
    assert_eq!(instrs[1].keyword, "RUN");
    assert_eq!(instrs[1].line, 3);
    assert_eq!(instrs[1].args, "x && y");
}

// ── dockerfile-root-user ──
#[test]
fn root_user_fires_without_user_and_on_user_root() {
    let t = proj();
    write(t.path(), "Dockerfile", "FROM node:22\nRUN true\n");
    write(
        t.path(),
        "svc/Dockerfile",
        "FROM node:22\nUSER app\nFROM node:22\nUSER root\n",
    );
    let s = run(t.path());
    let hits = of(&s, "dockerfile-root-user");
    let ev: Vec<&str> = hits.iter().map(|h| h.evidence.as_str()).collect();
    assert_eq!(ev, vec!["Dockerfile:1", "svc/Dockerfile:4"], "{hits:?}");
    assert!(hits.iter().all(|h| h.severity == Severity::High));
}

#[test]
fn root_user_judges_the_final_stage_and_trusts_nonroot_images() {
    let t = proj();
    // A USER in the builder does not protect the runtime stage.
    write(
        t.path(),
        "Dockerfile",
        "FROM rust:1.80 AS b\nUSER app\nFROM debian:12\n",
    );
    write(
        t.path(),
        "d/Dockerfile",
        "FROM gcr.io/distroless/static:nonroot\n",
    );
    let s = run(t.path());
    let hits = of(&s, "dockerfile-root-user");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "Dockerfile:3");
}

// ── dockerfile-no-healthcheck ──
#[test]
fn no_healthcheck_fires_unless_one_is_declared() {
    let t = proj();
    write(
        t.path(),
        "Dockerfile",
        "FROM node:22\nHEALTHCHECK NONE\nUSER app\n",
    );
    write(
        t.path(),
        "ok/Dockerfile",
        "FROM node:22\nHEALTHCHECK CMD curl -f http://x/\n",
    );
    let s = run(t.path());
    let hits = of(&s, "dockerfile-no-healthcheck");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "Dockerfile:1");
}

// ── dockerfile-apt-hygiene ──
#[test]
fn apt_hygiene_names_each_missing_flag() {
    let t = proj();
    write(
        t.path(),
        "Dockerfile",
        "FROM debian:12\nRUN apt-get update && apt-get install -y curl\nRUN --mount=type=cache,target=/var/lib/apt apt-get install -y --no-install-recommends git\n",
    );
    let s = run(t.path());
    let hits = of(&s, "dockerfile-apt-hygiene");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "Dockerfile:2");
    assert!(hits[0].title.contains("--no-install-recommends"));
    assert!(hits[0].title.contains("/var/lib/apt/lists"));
}

// ── dockerfile-unpinned-base ──
#[test]
fn unpinned_base_flags_latest_and_untagged_but_not_stages_or_digests() {
    let t = proj();
    write(
        t.path(),
        "Dockerfile",
        "ARG V=1\nFROM node AS build\nFROM build\nFROM nginx:latest\nFROM --platform=$P alpine:3.20\nFROM ${BASE}\nFROM scratch\nFROM x/y@sha256:00\nFROM localhost:5000/app\n",
    );
    let s = run(t.path());
    let ev: Vec<String> = of(&s, "dockerfile-unpinned-base")
        .iter()
        .map(|h| h.evidence.clone())
        .collect();
    assert_eq!(ev, vec!["Dockerfile:2", "Dockerfile:4", "Dockerfile:9"]);
}

// ── dockerfile-add-remote ──
#[test]
fn add_remote_fires_on_urls_without_checksum() {
    let t = proj();
    write(
        t.path(),
        "Dockerfile",
        "FROM a:1\nADD https://x.io/t.tgz /opt/\nADD --checksum=sha256:00 https://x.io/t.tgz /opt/\nADD ./local /opt/\n",
    );
    let s = run(t.path());
    let hits = of(&s, "dockerfile-add-remote");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "Dockerfile:2");
}

// ── dockerfile-secret-arg ──
#[test]
fn secret_arg_reports_the_name_never_the_value() {
    let t = proj();
    write(
        t.path(),
        "Dockerfile",
        "FROM a:1\nARG NPM_TOKEN=hunter2-value\nARG DB_PASSWORD_FILE=/run/s\nARG NODE_VERSION=22\n",
    );
    let s = run(t.path());
    let hits = of(&s, "dockerfile-secret-arg");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "Dockerfile:2");
    assert!(hits[0].title.contains("NPM_TOKEN"));
    assert!(
        !hits[0].title.contains("hunter2"),
        "value leaked: {}",
        hits[0].title
    );
}

// ── compose-unpinned-image ──
#[test]
fn compose_flags_latest_and_untagged_images() {
    let t = proj();
    write(
        t.path(),
        "docker-compose.prod.yml",
        "services:\n  db:\n    image: postgres\n  web:\n    image: \"nginx:latest\" # edge\n  app:\n    image: ${REGISTRY}/app:${TAG}\n  cache:\n    image: redis:7.2\n",
    );
    let s = run(t.path());
    let ev: Vec<String> = of(&s, "compose-unpinned-image")
        .iter()
        .map(|h| h.evidence.clone())
        .collect();
    assert_eq!(
        ev,
        vec!["docker-compose.prod.yml:3", "docker-compose.prod.yml:5"]
    );
}

// ── ci-ssh-hostkey-off ──
#[test]
fn ssh_hostkey_off_fires_in_ci_files_only() {
    let t = proj();
    write(
        t.path(),
        ".github/workflows/deploy.yml",
        "jobs:\n  d:\n    steps:\n      - run: ssh -o StrictHostKeyChecking=no u@h\n      - run: ssh -o StrictHostKeyChecking=accept-new u@h\n",
    );
    write(
        t.path(),
        ".gitlab-ci.yml",
        "x:\n  script: scp -o 'StrictHostKeyChecking no' a b\n",
    );
    write(t.path(), "scripts/dev.yml", "StrictHostKeyChecking=no\n");
    let s = run(t.path());
    let ev: Vec<String> = of(&s, "ci-ssh-hostkey-off")
        .iter()
        .map(|h| h.evidence.clone())
        .collect();
    assert_eq!(
        ev,
        vec![".github/workflows/deploy.yml:4", ".gitlab-ci.yml:2"]
    );
}

// ── ci-pull-request-target ──
#[test]
fn pull_request_target_is_high_when_the_head_is_checked_out() {
    let t = proj();
    write(
        t.path(),
        ".github/workflows/a.yml",
        "on:\n  pull_request_target:\njobs:\n  x:\n    steps:\n      - uses: actions/checkout@v4\n        with:\n          ref: ${{ github.event.pull_request.head.sha }}\n",
    );
    write(
        t.path(),
        ".github/workflows/b.yml",
        "# pull_request_target is banned\non: [pull_request_target]\n",
    );
    write(t.path(), ".github/workflows/c.yml", "on: [pull_request]\n");
    let s = run(t.path());
    let hits = of(&s, "ci-pull-request-target");
    assert_eq!(hits.len(), 2, "{hits:?}");
    let a = hits
        .iter()
        .find(|h| h.evidence == ".github/workflows/a.yml:2")
        .unwrap();
    assert_eq!(a.severity, Severity::High);
    let b = hits
        .iter()
        .find(|h| h.evidence == ".github/workflows/b.yml:2")
        .unwrap();
    assert_eq!(b.severity, Severity::Medium);
}

// ── ci-action-moving-ref ──
#[test]
fn action_moving_ref_flags_branches_and_missing_refs() {
    let t = proj();
    write(
        t.path(),
        ".github/workflows/ci.yml",
        "steps:\n  - uses: actions/checkout@v4\n  - uses: org/tool@master\n  - uses: org/other\n  - uses: ./local-action\n  - uses: docker://alpine:3\n  - uses: org/pinned@0123456789abcdef # main\n",
    );
    let s = run(t.path());
    let ev: Vec<String> = of(&s, "ci-action-moving-ref")
        .iter()
        .map(|h| h.evidence.clone())
        .collect();
    assert_eq!(
        ev,
        vec![".github/workflows/ci.yml:3", ".github/workflows/ci.yml:4"]
    );
}

// ── tracked-dotenv ──
#[test]
fn tracked_dotenv_flags_real_env_files_not_templates() {
    let listing = b".env\0.env.local\0app/prod.env\0.env.dist\0.env.example\0config/.env.local.dist\0example.env\0.envrc\0README.md\0";
    let ev: Vec<String> = detect_tracked_dotenv(listing)
        .into_iter()
        .map(|s| s.evidence)
        .collect();
    assert_eq!(ev, vec![".env", ".env.local", "app/prod.env"]);
}

// ── img-missing-alt ──
#[test]
fn img_missing_alt_needs_more_than_three_templates() {
    let t = proj();
    write(t.path(), "a.html", "<img src=\"x.png\">\n");
    assert!(
        of(&run(t.path()), "img-missing-alt").is_empty(),
        "gated below 4 templates"
    );
    for f in ["b.html", "c.html", "d.html"] {
        write(t.path(), f, "<p>x</p>\n");
    }
    let s = run(t.path());
    let hits = of(&s, "img-missing-alt");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "a.html:1");
}

#[test]
fn img_alt_line_check_skips_alt_spreads_svg_image_and_split_tags() {
    assert!(line_has_img_without_alt("<img src=a>"));
    assert!(!line_has_img_without_alt("<img src=a alt=\"\">"));
    assert!(!line_has_img_without_alt("<img src={a} alt={t} />"));
    assert!(!line_has_img_without_alt("<img {...props} />"));
    assert!(!line_has_img_without_alt("<image href=x />"));
    assert!(!line_has_img_without_alt("<img"));
}

// ── inline-block-oversize ──
#[test]
fn inline_block_oversize_counts_body_lines_in_server_templates() {
    let t = proj();
    let long = format!("<script>\n{}</script>\n", "x();\n".repeat(51));
    let short = format!("<style>\n{}</style>\n", "a{}\n".repeat(50));
    write(
        t.path(),
        "views/page.html.twig",
        &format!("<p/>\n{long}{short}<script src=\"a.js\"></script>\n"),
    );
    write(t.path(), "src/App.vue", &long);
    let s = run(t.path());
    let hits = of(&s, "inline-block-oversize");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].evidence, "views/page.html.twig:2");
    assert!(hits[0].title.contains("51 lines"));
}

// ── oss-community-standards ──
#[test]
fn oss_standards_are_skipped_without_oss_intent() {
    let t = proj();
    write(t.path(), "README.md", "# Internal tool\n");
    assert!(of(&run(t.path()), "oss-community-standards").is_empty());
}

#[test]
fn oss_standards_list_each_missing_item_once_intent_is_shown() {
    let t = proj();
    write(
        t.path(),
        "README.md",
        "# Lib\n\n## Install\n\nContributions welcome.\n",
    );
    let s = run(t.path());
    let titles: Vec<&str> = of(&s, "oss-community-standards")
        .iter()
        .map(|h| h.title.as_str())
        .collect();
    let expected: Vec<&str> = OSS_ITEMS
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, (_, title, _))| *title)
        .collect();
    assert_eq!(titles, expected);
    for title in titles {
        assert!(oss_anchor(title).is_some());
    }
}

#[test]
fn oss_standards_are_quiet_on_a_complete_project() {
    let t = proj();
    write(
        t.path(),
        "LICENSE",
        "MIT License\n\nPermission is hereby granted, free of charge",
    );
    write(
        t.path(),
        "README.md",
        "# Lib\n\n[![ci](b.svg)](x)\n\nA small library.\n",
    );
    write(t.path(), ".github/ISSUE_TEMPLATE/bug.yml", "name: bug");
    write(t.path(), ".github/pull_request_template.md", "x");
    write(t.path(), "CONTRIBUTING.md", "x");
    write(t.path(), "docs/CODE_OF_CONDUCT.md", "x");
    assert!(of(&run(t.path()), "oss-community-standards").is_empty());
    write(t.path(), "LICENSE", "All rights reserved.");
    let s = run(t.path());
    let hits = of(&s, "oss-community-standards");
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].title, OSS_ITEMS[1].1);
}

// ── bounds and safety ──
#[cfg(unix)]
#[test]
fn symlinks_are_never_followed() {
    let outside = proj();
    write(outside.path(), "Dockerfile", "FROM node\n");
    write(
        outside.path(),
        "ci/deploy.yml",
        "StrictHostKeyChecking=no\n",
    );
    let t = proj();
    std::os::unix::fs::symlink(
        outside.path().join("Dockerfile"),
        t.path().join("Dockerfile"),
    )
    .unwrap();
    fs::create_dir_all(t.path().join(".github")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("ci"),
        t.path().join(".github/workflows"),
    )
    .unwrap();
    std::os::unix::fs::symlink(outside.path().join("Dockerfile"), t.path().join("LICENSE"))
        .unwrap();
    let s = run(t.path());
    assert!(s.is_empty(), "{s:?}");
}

#[test]
fn oversized_files_are_skipped() {
    let t = proj();
    let mut body = String::from("FROM node\n");
    body.push_str(&"# pad\n".repeat((MAX_READ_BYTES as usize / 6) + 1));
    write(t.path(), "Dockerfile", &body);
    assert!(run(t.path()).is_empty());
}

#[test]
fn signals_are_capped_with_a_remainder() {
    let t = proj();
    let body: String = (0..20).map(|i| format!("FROM img{i}\n")).collect();
    write(t.path(), "Dockerfile", &body);
    let s = run(t.path());
    let hits = of(&s, "dockerfile-unpinned-base");
    assert_eq!(hits.len(), MAX_PER_DETECTOR + 1);
    assert!(hits.last().unwrap().evidence.starts_with("+5 "));
}

#[test]
fn line_evidence_is_a_valid_file_citation() {
    let t = proj();
    write(t.path(), "deploy/Dockerfile", "FROM node\n");
    write(
        t.path(),
        "docker-compose.yml",
        "services:\n  a:\n    image: redis\n",
    );
    let s = run(t.path());
    assert!(!s.is_empty());
    for signal in s.iter().filter(|s| !s.evidence.starts_with('+')) {
        let check = crate::core::anti_halluc::verify_source_marker(
            &format!("file: {}", signal.evidence),
            Some(t.path()),
        );
        assert_eq!(
            check.status,
            crate::core::anti_halluc::SourceStatus::Verified,
            "{signal:?}: {check:?}"
        );
    }
}

#[test]
fn every_baseline_detector_fires_on_one_fixture() {
    let t = proj();
    write(
        t.path(),
        "Dockerfile",
        "FROM node\nARG API_TOKEN\nADD https://x.io/a /a\nRUN apt-get install -y curl\n",
    );
    write(
        t.path(),
        "compose.yaml",
        "services:\n  a:\n    image: redis:latest\n",
    );
    write(
        t.path(),
        ".github/workflows/ci.yml",
        "on: pull_request_target\njobs:\n  a:\n    steps:\n      - uses: org/a@main\n      - run: ssh -o StrictHostKeyChecking=no h\n",
    );
    let long = format!("<script>\n{}</script>\n<img src=x>\n", "x;\n".repeat(60));
    for f in ["a.html", "b.html", "c.html", "d.html"] {
        write(t.path(), f, &long);
    }
    write(
        t.path(),
        "README.md",
        "# X\n\nOpen source, contributions welcome.\n",
    );
    let s = run(t.path());
    let fired: std::collections::BTreeSet<&str> = s.iter().map(|s| s.detector_id).collect();
    let listing_only = ["tracked-dotenv"];
    for id in crate::core::audit_detectors::DETECTOR_IDS
        .iter()
        .filter(|id| {
            ![
                "zero-tests",
                "blank-noopener",
                "csp-unsafe",
                "missing-community-files",
                "tracked-repo-artifacts",
            ]
            .contains(id)
                && !listing_only.contains(id)
        })
    {
        assert!(fired.contains(id), "{id} never fired: {fired:?}");
    }
    assert!(!detect_tracked_dotenv(b".env\0").is_empty());
}

#[test]
fn new_signals_are_disposed_by_path_or_keyword() {
    use crate::core::audit_detectors::undisposed_signals;
    let dotenv = detect_tracked_dotenv(b"config/.env.prod\0");
    assert_eq!(undisposed_signals(&dotenv, "nothing here").len(), 1);
    assert!(undisposed_signals(&dotenv, "TD cites `config/.env.prod`").is_empty());

    let t = proj();
    write(t.path(), "README.md", "# X\n\nContributions welcome.\n");
    let oss = detect_oss_community_standards(t.path());
    assert!(!oss.is_empty());
    let undisposed = undisposed_signals(&oss, "README is fine");
    assert!(
        undisposed.iter().any(|s| s.title.contains("CONTRIBUTING")),
        "{undisposed:?}"
    );
    let all = "LICENSE, ISSUE_TEMPLATE, pull_request_template, CONTRIBUTING.md, CODE_OF_CONDUCT.md";
    assert!(undisposed_signals(&oss, all).is_empty());

    let t = proj();
    write(t.path(), "Dockerfile", "FROM node:22\nUSER app\n");
    let s = run(t.path());
    assert_eq!(undisposed_signals(&s, "nothing").len(), 1, "{s:?}");
    assert!(undisposed_signals(&s, "Dockerfile:1 lacks a HEALTHCHECK").is_empty());
}
