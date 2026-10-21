//! KT-1199 — the audit run, portable by the repository: what Kronn writes at
//! each step, what a clone shows after a fetch, what a colleague resumes and
//! validates, and what an agent editing the file cannot change.
use super::*;
use crate::core::audit_run_state::StepAssessment;
use crate::models::AiAuditStatus;
use std::sync::Arc;
use tokio::sync::RwLock;

fn test_state() -> AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = Arc::new(RwLock::new(crate::core::config::default_config()));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

fn git(root: &std::path::Path, args: &[&str]) -> String {
    let out = crate::core::cmd::git_cmd()
        .args([
            "-c",
            "user.name=Kronn Test",
            "-c",
            "user.email=kronn@test.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit_all(root: &std::path::Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "--allow-empty", "-m", message]);
}

/// The docs a Full run of 3 steps produced.
const TARGETS: [&str; 3] = ["docs/AGENTS.md", "docs/glossary.md", "docs/repo-map.md"];

fn write_docs(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("docs/tech-debt")).unwrap();
    for (i, target) in TARGETS.iter().enumerate() {
        std::fs::write(
            root.join(target),
            format!("# Doc {i}\nFilled by the audit.\n"),
        )
        .unwrap();
    }
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "--quiet", "-b", "main"]);
    write_docs(dir.path());
    dir
}

async fn register(state: &AppState, root: &std::path::Path) {
    let path = root.to_str().unwrap().to_string();
    state
        .db
        .with_conn(move |conn| {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('p1', 'P', ?1, ?2, ?2)",
                rusqlite::params![path, now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

fn planned() -> Vec<(u32, String)> {
    TARGETS
        .iter()
        .enumerate()
        .map(|(i, t)| (i as u32 + 1, t.to_string()))
        .collect()
}

fn writer(root: &std::path::Path, run_id: &str, resumed_from: Option<&str>) -> RunStateWriter {
    RunStateWriter::new(
        root.to_path_buf(),
        run_id.to_string(),
        RunStateMeta {
            started_by: "Romu".into(),
            resumed_from: resumed_from.map(str::to_string),
            template_version: Some("0.15.0".into()),
            briefing_digest: None,
        },
        planned(),
    )
}

fn finish_step(conn: &rusqlite::Connection, run: &str, step: u32, ok: bool) {
    let target = TARGETS[step as usize - 1];
    crate::db::audit_runs::insert_audit_step_start(conn, run, step, target, chrono::Utc::now())
        .unwrap();
    crate::db::audit_runs::finalize_audit_step(
        conn,
        run,
        step,
        chrono::Utc::now(),
        10,
        &crate::db::audit_runs::StepTokens::UNKNOWN,
        None,
        ok,
        (!ok).then_some("gate failed"),
        false,
    )
    .unwrap();
}

fn start_run(conn: &rusqlite::Connection, run: &str) {
    crate::db::audit_runs::insert_running(
        conn,
        run,
        "p1",
        "Full",
        "ClaudeCode",
        chrono::Utc::now(),
    )
    .unwrap();
}

fn read_state(root: &std::path::Path) -> RunState {
    RunState::parse(&std::fs::read_to_string(repo_state::state_path(root)).unwrap()).unwrap()
}

/// A run state as another machine would have committed it.
fn committed_run(
    root: &std::path::Path,
    run_id: &str,
    status: &str,
    done: &[u32],
    updated_at: &str,
) -> RunState {
    let mut steps = BTreeMap::new();
    for (index, target) in planned() {
        let finished = done.contains(&index);
        steps.insert(
            repo_state::step_key(index),
            StepState {
                status: if finished {
                    StepStatus::Done
                } else {
                    StepStatus::Pending
                },
                digest: finished
                    .then(|| repo_state::file_digest(root, &target))
                    .flatten(),
                target,
                by: None,
            },
        );
    }
    RunState {
        schema: repo_state::RUN_STATE_SCHEMA,
        run_id: run_id.into(),
        kind: "Full".into(),
        status: status.into(),
        started_at: "2026-10-11T08:00:00+00:00".into(),
        ended_at: (status != "Running").then(|| updated_at.to_string()),
        updated_at: updated_at.into(),
        started_by: "Alice".into(),
        resumed_from: None,
        template_version: None,
        briefing_digest: None,
        total_steps: 3,
        baselines: Some(Baselines::from_text(
            "# Decisions\n\n## ADR-0001\nAccepted\n",
            "TD-1: ADR-0001\n",
        )),
        steps,
    }
}

fn settle_sheet(root: &std::path::Path, id: &str, severity: &str, status: &str) {
    std::fs::write(
        root.join(format!("docs/tech-debt/{id}.md")),
        format!(
            "---\nname: {id}\nmetadata:\n  type: tech-debt\n---\n\n# {id}\n\n- **Severity**: {severity}\n- **Status**: {status}\n"
        ),
    )
    .unwrap();
}

// ── DoD 1 — the file Kronn writes ────────────────────────────────────

#[tokio::test]
async fn the_writer_projects_each_step_deterministically_without_costs() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let w = Arc::new(writer(&root, "run-a", None));
    let (wr, rr) = (w.clone(), root.clone());
    state
        .db
        .with_conn(move |conn| {
            start_run(conn, "run-a");
            crate::db::audit_runs::record_adr_baseline(
                conn,
                "run-a",
                None,
                crate::core::adr::StartOfRun {
                    decisions: "# Decisions\n\n## ADR-0001\nAccepted\n".into(),
                    records: "TD-1: ADR-0001\n".into(),
                },
            )?;
            wr.sync(conn, false);
            let first = std::fs::read_to_string(repo_state::state_path(&rr))?;
            // The run exists from its first second, every step pending.
            let parsed =
                RunState::parse(&first).ok_or_else(|| anyhow::anyhow!("unparsable run state"))?;
            assert_eq!(parsed.status, "Running");
            assert!(parsed
                .steps
                .values()
                .all(|s| s.status == StepStatus::Pending));
            assert_eq!(parsed.started_by, "Romu");
            assert!(parsed.baselines.is_some());

            finish_step(conn, "run-a", 1, true);
            finish_step(conn, "run-a", 2, false);
            wr.sync(conn, false);
            let second = std::fs::read_to_string(repo_state::state_path(&rr))?;
            // No change, same bytes: git sees no diff.
            wr.sync(conn, false);
            assert_eq!(
                second,
                std::fs::read_to_string(repo_state::state_path(&rr))?
            );
            Ok(())
        })
        .await
        .unwrap();

    let text = std::fs::read_to_string(repo_state::state_path(&root)).unwrap();
    let parsed = RunState::parse(&text).unwrap();
    assert_eq!(parsed.step(1).unwrap().status, StepStatus::Done);
    assert_eq!(
        parsed.step(1).unwrap().digest,
        repo_state::file_digest(&root, "docs/AGENTS.md")
    );
    assert_eq!(parsed.step(2).unwrap().status, StepStatus::Failed);
    assert_eq!(parsed.step(2).unwrap().digest, None);
    assert_eq!(parsed.step(3).unwrap().status, StepStatus::Pending);
    assert_eq!(
        parsed.baselines.as_ref().unwrap().decisions_text(),
        "# Decisions\n\n## ADR-0001\nAccepted\n"
    );
    let lowered = text.to_lowercase();
    for forbidden in ["cost", "token", "usd", "secret", "password"] {
        assert!(!lowered.contains(forbidden), "{forbidden}: {text}");
    }
    assert_eq!(
        text.lines().filter(|l| l.starts_with("    \"00")).count(),
        3,
        "one line per step: {text}"
    );
    // The file sits in the docs folder, which travels with the repository:
    // the local `.kronn/` exclusion does not cover it.
    git(&root, &["add", "-A"]);
    let staged = git(&root, &["status", "--porcelain"]);
    assert!(staged.contains("docs/.kronn-audit-run.json"), "{staged}");
}

// ── DoD 5 — Kronn's memory is authoritative during a run ─────────────

#[tokio::test]
async fn an_agent_editing_the_state_file_mid_run_changes_nothing() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let w = Arc::new(writer(&root, "run-a", None));
    let (wr, rr) = (w.clone(), root.clone());
    state
        .db
        .with_conn(move |conn| {
            start_run(conn, "run-a");
            crate::db::audit_runs::record_adr_baseline(
                conn,
                "run-a",
                None,
                crate::core::adr::StartOfRun {
                    decisions: "# Decisions\nreal baseline\n".into(),
                    records: String::new(),
                },
            )?;
            finish_step(conn, "run-a", 1, true);
            wr.sync(conn, false);
            let authoritative = std::fs::read_to_string(repo_state::state_path(&rr))?;

            // The agent rewrites the file: forged baseline, steps marked done.
            let mut forged = RunState::parse(&authoritative)
                .ok_or_else(|| anyhow::anyhow!("unparsable run state"))?;
            forged.baselines = Some(Baselines::from_text(
                "# Decisions\nforged ADR accepted\n",
                "",
            ));
            for step in forged.steps.values_mut() {
                step.status = StepStatus::Done;
                step.digest = Some("0".repeat(64));
            }
            repo_state::write(&rr, &forged).map_err(anyhow::Error::msg)?;
            // …and a previous step's document is edited behind Kronn's back.
            std::fs::write(rr.join("docs/AGENTS.md"), "# Tampered\n")?;

            finish_step(conn, "run-a", 2, true);
            wr.sync(conn, false);
            let after = RunState::parse(&std::fs::read_to_string(repo_state::state_path(&rr))?)
                .ok_or_else(|| anyhow::anyhow!("unparsable run state"))?;
            // The DB baseline, not the file's, is what the file says again.
            assert_eq!(
                after
                    .baselines
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("absent"))?
                    .decisions_text(),
                "# Decisions\nreal baseline\n"
            );
            assert_eq!(
                after
                    .step(3)
                    .ok_or_else(|| anyhow::anyhow!("absent"))?
                    .status,
                StepStatus::Pending
            );
            assert_eq!(
                RunState::parse(&authoritative)
                    .ok_or_else(|| anyhow::anyhow!("unparsable run state"))?
                    .step(1)
                    .ok_or_else(|| anyhow::anyhow!("absent"))?
                    .digest,
                after
                    .step(1)
                    .ok_or_else(|| anyhow::anyhow!("absent"))?
                    .digest,
                "the digest of step 1 is the one it had when it finished"
            );
            // The authoritative baseline is still the DB's.
            let db_baseline = crate::db::audit_runs::adr_baseline(conn, "run-a")?
                .ok_or_else(|| anyhow::anyhow!("absent"))?;
            assert_eq!(db_baseline.decisions, "# Decisions\nreal baseline\n");
            Ok(())
        })
        .await
        .unwrap();

    // The tampered document no longer matches what step 1 produced.
    let after = read_state(&root);
    let verdicts: Vec<StepAssessment> = repo_state::assess(&root, &after);
    assert_eq!(verdicts[0].verdict, StepVerdict::Recheck, "{verdicts:?}");
    assert_eq!(verdicts[1].verdict, StepVerdict::Trusted);
}

// ── DoD 2 — the UI follows the repository ────────────────────────────

#[tokio::test]
async fn a_run_started_elsewhere_shows_from_the_repository_and_follows_a_rebase() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    settle_sheet(&root, "TD-20261011-a", "High", "Confirmed by user");
    settle_sheet(&root, "TD-20261011-b", "Medium", "Verified in source");

    // Nothing in the repository: nothing to show.
    assert!(repo_view(&root, None).is_none());

    // A colleague ran steps 1-2 and committed.
    let first = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &first).unwrap();
    commit_all(&root, "audit: steps 1-2");

    let view = repo_view(&root, None).expect("the repository's run");
    assert!(view.newer_than_local && view.committed);
    assert_eq!(view.started_by, "Alice");
    assert_eq!(view.done_steps, vec![1, 2]);
    assert_eq!(view.resume_steps, vec![3]);
    assert!(view.recheck_steps.is_empty());
    assert!(view.resumable);
    assert_eq!((view.td_total, view.td_settled), (2, 1), "2 TD, 1 settled");

    // A fetch brings the colleague's next commit (a rebase does the same
    // to the working tree): the view follows the file.
    let second = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2, 3],
        "2026-10-11T09:00:00+00:00",
    );
    repo_state::write(&root, &second).unwrap();
    commit_all(&root, "audit: all steps");
    let view = repo_view(&root, None).unwrap();
    assert_eq!(view.done_steps, vec![1, 2, 3]);
    assert!(view.resume_steps.is_empty());

    // The same run recorded here, or a newer local run, stays authoritative.
    let local_state = |id: &str, status: &str, at: &str| {
        let at = chrono::DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut run = crate::models::AuditRun {
            id: id.into(),
            project_id: "p1".into(),
            kind: "Full".into(),
            agent_type: "ClaudeCode".into(),
            started_at: at,
            ended_at: Some(at),
            duration_ms: None,
            status: status.into(),
            last_completed_step: 0,
            validation_discussion_id: None,
            step_outcomes_json: None,
            td_critical: 0,
            td_high: 0,
            td_medium: 0,
            td_low: 0,
            td_total: 0,
            td_resolved_since_last: 0,
            td_new_since_last: 0,
            td_carried_over: 0,
            health_score: None,
            report_path: None,
            recommendations_json: None,
            head_sha: None,
            branch: None,
            source_fingerprint: None,
            model: None,
            project_profile_draft: None,
            cancelled_at_step: None,
            resume_drift: None,
        };
        run.ended_at = Some(at);
        run
    };
    let same = local_state("run-alice", "Interrupted", "2026-10-11T07:00:00+00:00");
    assert!(!repo_view(&root, Some(&same)).unwrap().newer_than_local);
    let newer_local = local_state("run-bob", "Completed", "2026-10-12T00:00:00+00:00");
    assert!(
        !repo_view(&root, Some(&newer_local))
            .unwrap()
            .newer_than_local
    );
    let older_local = local_state("run-bob", "Completed", "2026-10-10T00:00:00+00:00");
    assert!(
        repo_view(&root, Some(&older_local))
            .unwrap()
            .newer_than_local
    );
    let live_local = local_state("run-bob", "Running", "2026-10-10T00:00:00+00:00");
    assert!(
        !repo_view(&root, Some(&live_local))
            .unwrap()
            .newer_than_local
    );
}

#[tokio::test]
async fn a_changed_step_file_is_to_recheck_and_an_uncommitted_file_is_flagged() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let run = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2, 3],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &run).unwrap();
    // Written, not committed yet.
    let view = repo_view(&root, None).unwrap();
    assert!(!view.committed && !view.resumable, "{view:?}");

    commit_all(&root, "audit");
    std::fs::write(
        root.join("docs/glossary.md"),
        "# Glossary\nEdited by a human.\n",
    )
    .unwrap();
    let view = repo_view(&root, None).unwrap();
    assert_eq!(view.recheck_steps, vec![2], "« à revérifier »");
    assert_eq!(view.done_steps, vec![1, 3]);
    assert!(view.resumable, "the changed step is replayed");
}

// ── DoD 3 — a colleague resumes ──────────────────────────────────────

#[tokio::test]
async fn a_colleague_resumes_trusting_matching_steps_and_replaying_the_others() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let run = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &run).unwrap();

    // Uncommitted: refused, nothing inherited.
    let (rr, rid) = (root.clone(), "run-alice".to_string());
    let refused = state
        .db
        .with_conn(move |conn| Ok(adopt_repo_run(conn, "p1", &rr, &rid)))
        .await
        .unwrap();
    assert!(refused.unwrap_err().contains("not committed"));

    commit_all(&root, "audit: steps 1-2");
    // Step 2's document is edited after the commit: it is replayed.
    std::fs::write(root.join("docs/glossary.md"), "# Glossary\nEdited.\n").unwrap();

    let (rr, rid) = (root.clone(), "run-alice".to_string());
    let inherited = state
        .db
        .with_conn(move |conn| Ok(adopt_repo_run(conn, "p1", &rr, &rid)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inherited, 1, "only step 1 still matches");

    let (row, steps, baseline) = state
        .db
        .with_conn(|conn| {
            Ok((
                crate::db::audit_runs::get_by_id(conn, "run-alice")?
                    .ok_or_else(|| anyhow::anyhow!("absent"))?,
                crate::db::audit_runs::list_audit_steps(conn, "run-alice")?,
                crate::db::audit_runs::adr_baseline(conn, "run-alice")?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(row.status, "Interrupted");
    assert_eq!(row.kind, "Full");
    assert_eq!(row.last_completed_step, 1);
    let skipped = crate::api::audit::full::already_succeeded_step_indices(&steps);
    assert_eq!(skipped, std::collections::HashSet::from([1]));
    // The baseline is the committed run's, not what decisions.md says now.
    assert_eq!(
        baseline.unwrap().decisions,
        "# Decisions\n\n## ADR-0001\nAccepted\n"
    );
    // The adopted run resumes like any interrupted run.
    assert!(crate::api::audit::full::resolve_resume_row("run-alice", Some(&row), "p1").is_ok());
    let resumable = state
        .db
        .with_conn(|conn| crate::db::audit_runs::latest_resumable(conn, "p1"))
        .await
        .unwrap();
    assert_eq!(resumable.map(|run| run.id), Some("run-alice".to_string()));

    // Adopting again is a no-op on the row this instance now has.
    let (rr, rid) = (root.clone(), "run-alice".to_string());
    let again = state
        .db
        .with_conn(move |conn| Ok(adopt_repo_run(conn, "p1", &rr, &rid)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, 0);
}

#[tokio::test]
async fn a_completed_or_superseded_repository_run_is_not_resumed() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let finished = committed_run(
        &root,
        "run-alice",
        "Completed",
        &[1, 2, 3],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &finished).unwrap();
    commit_all(&root, "audit done");
    let (rr, rid) = (root.clone(), "run-alice".to_string());
    let refused = state
        .db
        .with_conn(move |conn| Ok(adopt_repo_run(conn, "p1", &rr, &rid)))
        .await
        .unwrap();
    assert!(refused.unwrap_err().contains("not resumable"));

    let unknown = state
        .db
        .with_conn({
            let rr = root.clone();
            move |conn| Ok(adopt_repo_run(conn, "p1", &rr, "run-unknown"))
        })
        .await
        .unwrap();
    assert!(unknown.unwrap_err().contains("not found"));
}

// ── DoD 4 — validation from the repository ───────────────────────────

fn validated_at(root: &std::path::Path) -> Option<String> {
    crate::core::kronn_state::read(root).and_then(|s| s.validated_at)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validating_from_the_repository_without_a_local_run_and_propagating_by_git() {
    // The colleague's committed audit, pushed to a shared remote.
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--quiet", "--bare", "-b", "main"]);
    let seed = repo();
    let seed_root = seed.path().to_path_buf();
    settle_sheet(&seed_root, "TD-20261011-a", "Critical", "Confirmed by user");
    settle_sheet(&seed_root, "TD-20261011-b", "High", "Verified in source");
    let run = committed_run(
        &seed_root,
        "run-alice",
        "Completed",
        &[1, 2, 3],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&seed_root, &run).unwrap();
    commit_all(&seed_root, "audit completed");
    git(
        &seed_root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git(&seed_root, &["push", "--quiet", "origin", "main"]);

    // Two clones, two Kronn instances, neither ran the audit.
    let clone_b = tempfile::tempdir().unwrap();
    let clone_a = tempfile::tempdir().unwrap();
    for dir in [&clone_b, &clone_a] {
        git(
            dir.path(),
            &["clone", "--quiet", remote.path().to_str().unwrap(), "."],
        );
    }
    let state_b = test_state();
    register(&state_b, clone_b.path()).await;
    let state_a = test_state();
    register(&state_a, clone_a.path()).await;

    // B: a High sheet is still open — the repository state cannot validate.
    let shown = crate::api::audit::validate::get_validation_state(
        axum::extract::State(state_b.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(
        shown.source,
        crate::api::audit::validate::ValidationSource::Repo
    );
    assert_eq!(shown.run_id.as_deref(), Some("run-alice"));
    assert_eq!(shown.started_by.as_deref(), Some("Alice"));
    assert_eq!(
        shown.blocked,
        Some(crate::api::audit::validate::ValidationBlock::RepoTdOpen)
    );
    assert!(!shown.can_validate);
    let refused = crate::api::audit::validate_audit(
        axum::extract::State(state_b.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0;
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("Critical and High"));
    assert_eq!(validated_at(clone_b.path()), None);

    // B settles it and validates: no local run, the same gates.
    settle_sheet(clone_b.path(), "TD-20261011-b", "High", "Accepted decision");
    let shown = crate::api::audit::validate::get_validation_state(
        axum::extract::State(state_b.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0
    .data
    .unwrap();
    assert!(shown.can_validate, "{shown:?}");
    let validated = crate::api::audit::validate_audit(
        axum::extract::State(state_b.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0;
    assert!(validated.success, "{:?}", validated.error);
    assert_eq!(validated.data, Some(AiAuditStatus::Validated));
    assert!(validated_at(clone_b.path()).is_some());
    // The run's digests follow the documents the human validated.
    let follow = read_state(clone_b.path());
    assert!(repo_state::assess(clone_b.path(), &follow)
        .iter()
        .all(|s| s.verdict == StepVerdict::Trusted));

    // B commits and pushes; A fetches and sees Validated.
    assert_eq!(validated_at(clone_a.path()), None);
    commit_all(clone_b.path(), "audit validated");
    git(clone_b.path(), &["push", "--quiet", "origin", "main"]);
    git(
        clone_a.path(),
        &["pull", "--quiet", "--ff-only", "origin", "main"],
    );
    assert!(
        validated_at(clone_a.path()).is_some(),
        "validated_at propagated by git"
    );
    assert_eq!(
        crate::core::scanner::detect_audit_status(&clone_a.path().display().to_string()),
        AiAuditStatus::Validated
    );
    let seen_by_a = crate::api::audit::validate::get_validation_state(
        axum::extract::State(state_a.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0
    .data
    .unwrap();
    assert_eq!(
        seen_by_a.source,
        crate::api::audit::validate::ValidationSource::Repo
    );
    assert!(seen_by_a.validated_at.is_some());
    assert_eq!(seen_by_a.validated_by.as_deref(), Some("anonymous"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_repository_state_refuses_what_a_local_run_would() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    settle_sheet(&root, "TD-20261011-a", "High", "Confirmed by user");
    let validate = || async {
        crate::api::audit::validate_audit(
            axum::extract::State(state.clone()),
            axum::extract::Path("p1".to_string()),
        )
        .await
        .0
    };

    // An interrupted run is no run to validate.
    let interrupted = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &interrupted).unwrap();
    commit_all(&root, "partial");
    assert!(validate()
        .await
        .error
        .unwrap()
        .contains("only a Completed run"));

    // A completed one whose file was not committed is not trusted.
    let finished = committed_run(
        &root,
        "run-alice",
        "Completed",
        &[1, 2, 3],
        "2026-10-11T09:00:00+00:00",
    );
    repo_state::write(&root, &finished).unwrap();
    assert!(validate().await.error.unwrap().contains("not committed"));

    // Committed, but a step's document changed since: « à revérifier ».
    commit_all(&root, "completed");
    std::fs::write(root.join("docs/repo-map.md"), "# Repo map\nEdited since.\n").unwrap();
    let refused = validate().await;
    assert!(!refused.success);
    assert!(refused.error.unwrap().contains("re-verify"));
    assert_eq!(validated_at(&root), None);
}

#[tokio::test]
async fn nothing_in_the_repository_leaves_the_local_gate_untouched() {
    let dir = repo();
    let state = test_state();
    register(&state, dir.path()).await;
    let refused = crate::api::audit::validate_audit(
        axum::extract::State(state.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0;
    assert!(refused.error.unwrap().contains("No audit run recorded"));
}

// ── Review of KT-1199: the rule, the resume offer, the trusted rebuild ──

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clone_validates_only_from_a_committed_completed_state_that_passes() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    settle_sheet(&root, "TD-20261011-a", "High", "Confirmed by user");
    let shown = || async {
        crate::api::audit::validate::get_validation_state(
            axum::extract::State(state.clone()),
            axum::extract::Path("p1".to_string()),
        )
        .await
        .0
        .data
        .unwrap()
    };
    let validate = || async {
        crate::api::audit::validate_audit(
            axum::extract::State(state.clone()),
            axum::extract::Path("p1".to_string()),
        )
        .await
        .0
    };

    // A clone WITHOUT a committed state: read-only, and it says what lifts it.
    let without = shown().await;
    assert_eq!(
        without.blocked,
        Some(crate::api::audit::validate::ValidationBlock::NoLocalRun)
    );
    assert!(!without.can_validate);
    let refused = validate().await;
    assert!(!refused.success);
    let reason = refused.error.unwrap();
    assert!(
        reason.contains("read-only until the repository carries a committed, Completed run state"),
        "{reason}"
    );
    assert_eq!(validated_at(&root), None);

    // The same clone once the repository carries a committed, Completed
    // state that passes the gates: allowed, from the repository.
    let finished = committed_run(
        &root,
        "run-alice",
        "Completed",
        &[1, 2, 3],
        "2026-10-11T09:00:00+00:00",
    );
    repo_state::write(&root, &finished).unwrap();
    commit_all(&root, "audit completed");
    let with = shown().await;
    assert_eq!(
        with.source,
        crate::api::audit::validate::ValidationSource::Repo
    );
    assert_eq!(with.blocked, None);
    assert!(with.can_validate, "{with:?}");
    let validated = validate().await;
    assert!(validated.success, "{:?}", validated.error);
    assert_eq!(validated.data, Some(AiAuditStatus::Validated));
    assert!(validated_at(&root).is_some());
}

async fn resumable(state: &AppState) -> Option<serde_json::Value> {
    let found = crate::api::audit::audit_latest_resumable(
        axum::extract::State(state.clone()),
        axum::extract::Path("p1".to_string()),
    )
    .await
    .0;
    assert!(found.success, "{:?}", found.error);
    found
        .data
        .map(|row| serde_json::to_value(row).unwrap())
        .filter(|value| !value.is_null())
}

#[tokio::test]
async fn audit_resumable_offers_the_run_a_colleague_committed_and_the_launch_adopts_it() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let interrupted = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &interrupted).unwrap();

    // Not committed: nothing is offered.
    assert!(resumable(&state).await.is_none());

    commit_all(&root, "audit: steps 1-2");
    let offered = resumable(&state)
        .await
        .expect("the repository run is offered");
    assert_eq!(offered["id"], "run-alice");
    assert_eq!(offered["kind"], "Full");
    assert_eq!(offered["from_repository"], true);
    assert_eq!(offered["last_completed_step"], 2);
    assert_eq!(offered["steps_to_redo"], serde_json::json!([3]));

    // A step document edited since is offered back to be replayed.
    std::fs::write(root.join("docs/glossary.md"), "# Glossary\nEdited.\n").unwrap();
    let edited = resumable(&state).await.unwrap();
    assert_eq!(edited["steps_to_redo"], serde_json::json!([2, 3]));
    assert_eq!(edited["last_completed_step"], 1);

    // The launch sends this id: it adopts the run, which is then a local
    // resumable run, no longer shown as the repository's.
    let (rr, rid) = (root.clone(), "run-alice".to_string());
    state
        .db
        .with_conn(move |conn| Ok(adopt_repo_run(conn, "p1", &rr, &rid)))
        .await
        .unwrap()
        .unwrap();
    let adopted = resumable(&state).await.unwrap();
    assert_eq!(adopted["id"], "run-alice");
    assert_eq!(adopted["from_repository"], false);
}

#[tokio::test]
async fn a_newer_local_run_or_a_finished_repository_run_is_never_offered() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let completed = committed_run(
        &root,
        "run-alice",
        "Completed",
        &[1, 2, 3],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &completed).unwrap();
    commit_all(&root, "audit completed");
    assert!(resumable(&state).await.is_none(), "nothing left to resume");

    let interrupted = committed_run(
        &root,
        "run-alice",
        "Interrupted",
        &[1, 2],
        "2026-10-11T08:30:00+00:00",
    );
    repo_state::write(&root, &interrupted).unwrap();
    commit_all(&root, "audit interrupted");
    assert!(resumable(&state).await.is_some());

    // This instance ran a later audit: the file is the older attempt.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO audit_runs (id, project_id, kind, agent_type, started_at, ended_at, status)
                 VALUES ('local-new', 'p1', 'Full', 'ClaudeCode',
                         '2026-10-12T08:00:00+00:00', '2026-10-12T09:00:00+00:00', 'Completed')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(resumable(&state).await.is_none());
}

#[test]
fn the_digests_after_a_validation_come_from_the_committed_copy_not_the_working_tree() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let run = committed_run(
        &root,
        "run-alice",
        "Completed",
        &[1, 2, 3],
        "2026-10-11T09:00:00+00:00",
    );
    repo_state::write(&root, &run).unwrap();
    commit_all(&root, "audit completed");

    // An agent rewrites the file after the commit, then the validation
    // settles a step's document.
    let mut forged = run.clone();
    forged.run_id = "forged".into();
    forged.started_by = "Mallory".into();
    repo_state::write(&root, &forged).unwrap();
    std::fs::write(root.join("docs/glossary.md"), "# Glossary\nSettled.\n").unwrap();

    refresh_validated_digests(&root, "run-alice", None);
    let after = read_state(&root);
    assert_eq!(after.run_id, "run-alice");
    assert_eq!(after.started_by, "Alice");
    assert!(repo_state::assess(&root, &after)
        .iter()
        .all(|s| s.verdict == StepVerdict::Trusted));
}

#[tokio::test]
async fn after_a_validation_the_digests_are_rebuilt_from_the_rows_not_from_the_file() {
    let dir = repo();
    let root = dir.path().to_path_buf();
    let state = test_state();
    register(&state, &root).await;
    let writer = writer(&root, "run-local", None);
    state
        .db
        .with_conn(move |conn| {
            start_run(conn, "run-local");
            for step in 1..=3 {
                finish_step(conn, "run-local", step, true);
            }
            conn.execute(
                "UPDATE audit_runs SET status = 'Completed', ended_at = ?1 WHERE id = 'run-local'",
                [chrono::Utc::now().to_rfc3339()],
            )?;
            writer.sync(conn, true);
            Ok(())
        })
        .await
        .unwrap();

    // An agent marks step 3 failed in the (uncommitted) file; the
    // validation then settles that step's document.
    let mut forged = read_state(&root);
    let step = forged.steps.get_mut("003").unwrap();
    step.status = StepStatus::Failed;
    step.digest = None;
    repo_state::write(&root, &forged).unwrap();
    std::fs::write(root.join("docs/repo-map.md"), "# Repo map\nSettled.\n").unwrap();

    let rows = state
        .db
        .with_conn(|conn| {
            Ok(LocalRunRows {
                run: crate::db::audit_runs::get_by_id(conn, "run-local")?
                    .ok_or_else(|| anyhow::anyhow!("absent"))?,
                steps: crate::db::audit_runs::list_audit_steps(conn, "run-local")?,
                baseline: crate::db::audit_runs::adr_baseline(conn, "run-local")?,
            })
        })
        .await
        .unwrap();
    refresh_validated_digests(&root, "run-local", Some(rows));
    let after = read_state(&root);
    let step = after.step(3).unwrap();
    assert_eq!(step.status, StepStatus::Done, "the DB row decides");
    assert_eq!(
        step.digest,
        repo_state::file_digest(&root, "docs/repo-map.md")
    );
}
