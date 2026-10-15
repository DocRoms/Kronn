//! KT-1199 — the run state file: deterministic, secret-free, trusted only when
//! committed, a changed produced file never trusted.
use super::*;

fn state() -> RunState {
    let mut steps = BTreeMap::new();
    let mut put = |index: u32, status: StepStatus, target: &str, digest: Option<&str>| {
        steps.insert(
            step_key(index),
            StepState {
                status,
                target: target.to_string(),
                digest: digest.map(str::to_string),
                by: None,
            },
        );
    };
    // Inserted out of order on purpose: the file sorts them.
    put(
        10,
        StepStatus::Pending,
        "docs/inconsistencies-security.md",
        None,
    );
    put(2, StepStatus::Done, "docs/glossary.md", Some("aa"));
    put(1, StepStatus::Unchanged, "docs/AGENTS.md", Some("bb"));
    put(3, StepStatus::Failed, "docs/repo-map.md", None);
    RunState {
        schema: RUN_STATE_SCHEMA,
        run_id: "run-1".into(),
        kind: "Full".into(),
        status: "Interrupted".into(),
        started_at: "2026-10-11T08:00:00+00:00".into(),
        ended_at: Some("2026-10-11T08:20:00+00:00".into()),
        updated_at: "2026-10-11T08:20:00+00:00".into(),
        started_by: "romu".into(),
        resumed_from: None,
        kronn_version: "0.15.1".into(),
        template_version: Some("0.15.0".into()),
        briefing_digest: Some("cc".into()),
        total_steps: 4,
        baselines: Some(Baselines::from_text(
            "# Decisions\n\n## ADR-0001\nAccepted\n",
            "TD-1: Decision record: ADR-0001\n",
        )),
        steps,
    }
}

fn git(root: &Path, args: &[&str]) {
    let out = crate::core::cmd::git_cmd()
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--quiet"]);
    std::fs::create_dir_all(dir.path().join("docs")).unwrap();
    dir
}

#[test]
fn the_file_is_deterministic_sorted_and_one_line_per_step() {
    let text = state().render().unwrap();
    assert_eq!(text, state().render().unwrap(), "same state, same bytes");
    let keys: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.trim_start().split('"').nth(1).unwrap())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "top-level keys sorted: {text}");
    // One line per step, in step order: a conflict stays on its own line.
    let step_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("    \"0")).collect();
    assert_eq!(step_lines.len(), 4, "{text}");
    assert!(step_lines[0].starts_with("    \"001\""));
    assert!(step_lines[3].starts_with("    \"010\""));
    assert!(step_lines[1].contains("\"status\":\"done\""));
    // Round trip.
    assert_eq!(RunState::parse(&text).unwrap(), state());
}

#[test]
fn the_file_carries_no_cost_and_no_secret() {
    let mut run = state();
    run.baselines = Some(Baselines::from_text(
        "# Decisions\nAPI_KEY=sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\n",
        "",
    ));
    let text = run.render().unwrap();
    assert!(!text.contains("sk-ant-api03"), "{text}");
    for forbidden in ["cost", "token", "usd", "price"] {
        assert!(
            !text.to_lowercase().contains(forbidden),
            "{forbidden}: {text}"
        );
    }
}

#[test]
fn baselines_round_trip_their_text() {
    let decisions = "# Decisions\n\n## ADR-0001\nAccepted\n";
    let baselines = Baselines::from_text(decisions, "");
    assert_eq!(baselines.decisions_text(), decisions);
    assert!(baselines.records.is_empty());
}

#[test]
fn only_a_committed_file_is_trusted() {
    let dir = repo();
    let root = dir.path();
    assert_eq!(probe(root), RepoProbe::Absent);

    write(root, &state()).unwrap();
    match probe(root) {
        RepoProbe::Found {
            committed,
            state: found,
        } => {
            assert!(!committed, "an uncommitted file is not trusted");
            assert_eq!(*found, state());
        }
        other => panic!("{other:?}"),
    }

    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "audit run"]);
    assert!(matches!(
        probe(root),
        RepoProbe::Found {
            committed: true,
            ..
        }
    ));

    // An edit after the commit (an agent, a human) is no longer the
    // committed state.
    let mut edited = state();
    edited.status = "Completed".into();
    write(root, &edited).unwrap();
    assert!(matches!(
        probe(root),
        RepoProbe::Found {
            committed: false,
            ..
        }
    ));
}

#[test]
fn a_conflict_marked_or_foreign_file_is_unreadable() {
    let dir = repo();
    let root = dir.path();
    std::fs::write(
        state_path(root),
        "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> theirs\n",
    )
    .unwrap();
    assert_eq!(probe(root), RepoProbe::Unreadable);
    let mut newer = state();
    newer.schema = RUN_STATE_SCHEMA + 1;
    std::fs::write(state_path(root), serde_json::to_string(&newer).unwrap()).unwrap();
    assert_eq!(
        probe(root),
        RepoProbe::Unreadable,
        "a newer schema is not guessed"
    );
}

#[test]
fn a_step_whose_file_changed_is_never_trusted() {
    let dir = repo();
    let root = dir.path();
    std::fs::write(root.join("docs/AGENTS.md"), "# Team\n").unwrap();
    std::fs::write(root.join("docs/glossary.md"), "# Glossary\n").unwrap();
    let mut run = state();
    run.steps.get_mut("001").unwrap().digest = file_digest(root, "docs/AGENTS.md");
    run.steps.get_mut("002").unwrap().digest = file_digest(root, "docs/glossary.md");

    let verdicts = |run: &RunState| -> Vec<(u32, StepVerdict)> {
        assess(root, run)
            .into_iter()
            .map(|a| (a.index, a.verdict))
            .collect()
    };
    assert_eq!(
        verdicts(&run),
        vec![
            (1, StepVerdict::Trusted),
            (2, StepVerdict::Trusted),
            (3, StepVerdict::Failed),
            (10, StepVerdict::Pending),
        ]
    );

    std::fs::write(root.join("docs/glossary.md"), "# Glossary\nedited\n").unwrap();
    assert_eq!(verdicts(&run)[1], (2, StepVerdict::Recheck));
    std::fs::remove_file(root.join("docs/AGENTS.md")).unwrap();
    assert_eq!(
        verdicts(&run)[0],
        (1, StepVerdict::Recheck),
        "a vanished file"
    );
}

#[test]
fn a_target_outside_the_project_has_no_digest() {
    let dir = repo();
    assert_eq!(file_digest(dir.path(), "../outside.md"), None);
    assert_eq!(file_digest(dir.path(), "/etc/hosts"), None);
    assert_eq!(file_digest(dir.path(), "Final review"), None);
}
