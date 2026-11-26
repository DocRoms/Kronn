use super::*;
use std::path::{Path, PathBuf};

const MIN: u64 = 60;

fn secs(minutes: u64) -> Duration {
    Duration::from_secs(minutes * MIN)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = crate::core::cmd::git_cmd()
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("kronn-kt1408-")
        .tempdir()
        .unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "worker@example.com"]);
    git(dir.path(), &["config", "user.name", "Worker"]);
    std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "init"]);
    dir
}

#[test]
fn a_120_minute_limit_warns_at_90_and_again_at_110() {
    let limit = secs(120);
    assert_eq!(limit_stage(limit, secs(89)), None);
    assert_eq!(limit_stage(limit, secs(90)), Some(LimitStage::Near));
    assert_eq!(limit_stage(limit, secs(109)), Some(LimitStage::Near));
    assert_eq!(limit_stage(limit, secs(110)), Some(LimitStage::Imminent));
    assert_eq!(limit_stage(limit, secs(125)), Some(LimitStage::Imminent));
}

#[test]
fn a_short_limit_is_not_warned_from_its_first_minute() {
    let limit = secs(20);
    assert_eq!(limit_stage(limit, secs(14)), None);
    assert_eq!(limit_stage(limit, secs(15)), Some(LimitStage::Near));
    assert_eq!(limit_stage(limit, secs(19)), Some(LimitStage::Imminent));
    assert_eq!(limit_stage(Duration::ZERO, secs(1)), None);
}

#[test]
fn the_warning_tells_the_worker_to_commit_and_deliver() {
    assert!(time_limit_notice(secs(120), secs(30)).is_none());

    let near = time_limit_notice(secs(120), secs(91)).unwrap();
    assert_eq!(near["stage"], "near");
    assert_eq!(near["remaining_min"], 29);
    let text = near["notice"].as_str().unwrap();
    assert!(text.contains("task_exec_commit"), "{text}");
    assert!(text.contains("task_exec_deliver"), "{text}");
    assert!(text.contains("91 of 120 min"), "{text}");

    let imminent = time_limit_notice(secs(120), secs(111)).unwrap();
    assert_eq!(imminent["stage"], "imminent");
    let text = imminent["notice"].as_str().unwrap();
    assert!(text.contains("FINAL WARNING"), "{text}");
    assert!(text.contains("NOW"), "{text}");
}

#[test]
fn the_budget_follows_the_provider() {
    assert_eq!(
        global_limit(&AgentType::ClaudeCode, 120, 240),
        Duration::from_secs(120 * 60)
    );
    assert_eq!(
        global_limit(&AgentType::Ollama, 120, 240),
        Duration::from_secs(240 * 60)
    );
}

#[test]
fn a_clean_worktree_needs_no_checkpoint() {
    let repo = repo();
    let outcome = checkpoint_worktree(repo.path(), "KT-1", secs(120), secs(120), None).unwrap();
    assert_eq!(outcome, None);
}

#[test]
fn a_checkpoint_carries_no_sign_off_in_a_project_that_does_not_sign() {
    let repo = repo();
    std::fs::write(repo.path().join("a.txt"), "one\ntwo\n").unwrap();
    checkpoint_worktree(repo.path(), "KT-1750", secs(120), secs(120), None)
        .unwrap()
        .expect("a dirty worktree is checkpointed");
    let message = git(repo.path(), &["log", "-1", "--format=%B"]);
    assert!(!message.contains("Signed-off-by"), "{message}");
}

#[test]
fn a_dirty_worktree_is_checkpointed_as_a_signed_wip_commit() {
    let repo = repo();
    // A project that signs off (KT-1750: the sign-off follows the project).
    git(repo.path(), &["config", "kronn.signoff", "true"]);
    std::fs::write(repo.path().join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(repo.path().join("new.txt"), "fresh\n").unwrap();
    let before = git(repo.path(), &["rev-parse", "HEAD"]);

    let checkpoint = checkpoint_worktree(repo.path(), "KT-1404", secs(120), secs(120), None)
        .unwrap()
        .expect("a dirty worktree is checkpointed");

    let head = git(repo.path(), &["rev-parse", "HEAD"]);
    assert_ne!(head, before, "the checkpoint is a new commit on the branch");
    assert!(head.starts_with(checkpoint.commit.as_deref().unwrap()));
    assert_eq!(checkpoint.patch, None);
    let mut files = checkpoint.files.clone();
    files.sort();
    assert_eq!(files, ["a.txt", "new.txt"]);
    assert!(
        git(repo.path(), &["status", "--porcelain"]).is_empty(),
        "nothing is left uncommitted"
    );

    let message = git(repo.path(), &["log", "-1", "--format=%B"]);
    assert!(message.starts_with(CHECKPOINT_SUBJECT_PREFIX), "{message}");
    assert!(message.contains("KT-1404"), "{message}");
    assert!(message.contains("120-minute"), "{message}");
    assert!(message.contains("Kronn-Checkpoint: execution-time-limit"));
    assert!(
        message.contains("Signed-off-by: Worker <worker@example.com>"),
        "DCO sign-off: {message}"
    );
}

/// A linked worktree, with a dirty `a.txt`, of a repository whose main checkout
/// has a `pre-commit` hook that refuses everything (KT-1745: that hook is the
/// owner's, in the main checkout's common git directory).
#[cfg(unix)]
fn refusing_main_checkout_with_worktree() -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let main = repo();
    let hook = main.path().join(".git/hooks/pre-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\necho 'lint failed' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let parent = tempfile::Builder::new()
        .prefix("kronn-kt1745-wt-")
        .tempdir()
        .unwrap();
    let worktree = parent.path().canonicalize().unwrap().join("work");
    git(
        main.path(),
        &["worktree", "add", "-b", "work", worktree.to_str().unwrap()],
    );
    std::fs::write(worktree.join("a.txt"), "one\ntwo\n").unwrap();
    (main, parent, worktree)
}

#[cfg(unix)] // runs a `#!/bin/sh` pre-commit hook
#[test]
fn a_refused_commit_leaves_a_patch_in_the_evidence() {
    let (_main, _parent, worktree) = refusing_main_checkout_with_worktree();
    let before = git(&worktree, &["rev-parse", "HEAD"]);

    let checkpoint = checkpoint_worktree_with_hooks(
        &worktree,
        "KT-1",
        secs(120),
        secs(120),
        None,
        WorkerCommitHooks::Sandboxed,
    )
    .unwrap()
    .expect("a dirty worktree is reported");

    assert_eq!(checkpoint.commit, None);
    let error = checkpoint.commit_error.expect("the refusal is reported");
    assert!(
        error.contains("HookRefused")
            && (error.contains("lint failed") || error.contains("not run:")),
        "{error}"
    );
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), before);
    let patch = checkpoint
        .patch
        .expect("the tracked changes are kept as a patch");
    assert!(patch.contains("+two"), "{patch}");
    assert!(!checkpoint.patch_truncated);
}

/// KT-1745: the default policy runs no hook, so a hook cannot stop the
/// time-limit checkpoint from saving the worker's work.
#[cfg(unix)] // runs a `#!/bin/sh` pre-commit hook
#[test]
fn the_default_policy_checkpoints_past_a_refusing_hook() {
    let (_main, _parent, worktree) = refusing_main_checkout_with_worktree();

    let checkpoint = checkpoint_worktree(&worktree, "KT-1", secs(120), secs(120), None)
        .unwrap()
        .expect("a dirty worktree is checkpointed");

    assert!(checkpoint.commit.is_some(), "{:?}", checkpoint.commit_error);
    assert_eq!(checkpoint.patch, None);
}

#[test]
fn an_unfinished_merge_is_not_committed_over() {
    let repo = repo();
    // The state `git merge --no-commit` leaves behind: a MERGE_HEAD in the git dir.
    let head = git(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::write(repo.path().join(".git/MERGE_HEAD"), format!("{head}\n")).unwrap();
    std::fs::write(repo.path().join("c.txt"), "work\n").unwrap();

    let checkpoint = checkpoint_worktree(repo.path(), "KT-1", secs(120), secs(120), None)
        .unwrap()
        .unwrap();

    assert_eq!(checkpoint.commit, None);
    assert!(checkpoint
        .commit_error
        .as_deref()
        .is_some_and(|error| error.contains("merge")));
}

fn seeded_execution() -> (Connection, String) {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn.execute(
        "INSERT INTO discussions (id, title, created_at, updated_at) \
         VALUES ('d-parent', 'D', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at) \
         VALUES ('t-1408', 1408, 'T', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    let execution = crate::db::orchestration::launch_single_task(
        &conn,
        &crate::models::LaunchSingleTaskInput::new("t-1408", "d-parent"),
        &crate::models::PlanningActor {
            kind: crate::models::PlanningActorKind::Backend,
            id: Some("test".into()),
            session_id: None,
            source_message_id: None,
            token_declared: false,
        },
    )
    .unwrap()
    .execution;
    (conn, execution.id)
}

fn stop_event(conn: &Connection, exec: &str, checkpoint: Option<&Checkpoint>) {
    record_stop_event(conn, exec, "dispatch-1", secs(120), secs(120), checkpoint).unwrap();
}

#[test]
fn the_handoff_names_each_generation_and_its_checkpoint() {
    let (conn, exec) = seeded_execution();

    assert_eq!(handoff_note(&conn, &exec, false), None, "no stop, no note");

    stop_event(
        &conn,
        &exec,
        Some(&Checkpoint {
            files: vec!["a.txt".into(), "b.txt".into()],
            commit: Some("abc1234".into()),
            patch: None,
            patch_truncated: false,
            commit_error: None,
        }),
    );
    stop_event(&conn, &exec, None);

    let en = handoff_note(&conn, &exec, false).unwrap();
    assert!(
        en.contains("generation 1: 120 min of the 120-min limit"),
        "{en}"
    );
    assert!(en.contains("WIP commit `abc1234`"), "{en}");
    assert!(en.contains("2 uncommitted file(s)"), "{en}");
    assert!(en.contains("generation 2: 120 min"), "{en}");
    assert!(en.contains("the worktree was clean"), "{en}");

    let fr = handoff_note(&conn, &exec, true).unwrap();
    assert!(fr.contains("génération 1 : 120 min sur 120 min"), "{fr}");
    assert!(fr.contains("commit WIP `abc1234`"), "{fr}");
}

#[test]
fn a_patch_only_stop_is_named_as_such() {
    let (conn, exec) = seeded_execution();
    stop_event(
        &conn,
        &exec,
        Some(&Checkpoint {
            files: vec!["a.txt".into()],
            commit: None,
            patch: Some("diff".into()),
            patch_truncated: false,
            commit_error: Some("hook refused".into()),
        }),
    );
    let note = handoff_note(&conn, &exec, false).unwrap();
    assert!(note.contains("no commit could be made"), "{note}");
    assert!(note.contains("a patch is kept"), "{note}");
    assert!(note.contains(TIME_LIMIT_EVENT), "{note}");
}

/// A claimed dispatch `id` of the room `d-child`, as the run loop holds it.
fn claimed_dispatch(conn: &Connection, id: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO discussions (id, title, created_at, updated_at) \
         VALUES ('d-child', 'Child', ?1, ?1)",
        [&now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages \
         (id, discussion_id, role, content, timestamp, sort_order, received_at) \
         VALUES (?1, 'd-child', 'User', 'go', ?2, 1, ?2)",
        rusqlite::params![format!("u-{id}"), now],
    )
    .unwrap();
    crate::db::agent_dispatch::enqueue_for_latest_user(
        conn,
        crate::db::agent_dispatch::NewLatestUserDispatch {
            id,
            discussion_id: "d-child",
            dedupe_key: &format!("message:u-{id}"),
            agent_override: Some(&AgentType::Ollama),
            chain_prompt_ids: &[],
            batch_item: None,
            group_id: None,
            group_concurrency_limit: None,
        },
    )
    .unwrap();
    crate::db::agent_dispatch::claim(conn, id)
        .unwrap()
        .expect("the dispatch is claimable");
}

/// KT-2037 — the stop the limit recorded for a dispatch is read back with what
/// it left: elapsed time, limit, uncommitted files and the WIP commit.
#[test]
fn a_limit_stop_is_read_back_for_its_dispatch() {
    let (conn, exec) = seeded_execution();
    claimed_dispatch(&conn, "dispatch-1");
    assert_eq!(
        limit_stop_of_dispatch(&conn, "dispatch-1").unwrap(),
        None,
        "no stop recorded yet"
    );

    stop_event(
        &conn,
        &exec,
        Some(&Checkpoint {
            files: vec!["a.txt".into(), "b.txt".into()],
            commit: Some("abc1234".into()),
            patch: None,
            patch_truncated: false,
            commit_error: None,
        }),
    );

    assert_eq!(
        limit_stop_of_dispatch(&conn, "dispatch-1").unwrap(),
        Some(LimitStop {
            elapsed: secs(120),
            limit: secs(120),
            files: 2,
            checkpoint_commit: Some("abc1234".into()),
        })
    );
    assert_eq!(
        limit_stop_of_dispatch(&conn, "another-dispatch").unwrap(),
        None
    );
}

/// KT-2037 — a dispatch claimed again after a stop (a requeue) is a new run:
/// the earlier stop is not read as this run's.
#[test]
fn a_stop_of_an_earlier_claim_is_not_this_runs_stop() {
    let (conn, exec) = seeded_execution();
    claimed_dispatch(&conn, "dispatch-1");
    stop_event(&conn, &exec, None);
    assert!(limit_stop_of_dispatch(&conn, "dispatch-1")
        .unwrap()
        .is_some());

    let later = (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
    conn.execute(
        "UPDATE agent_dispatch_jobs SET claimed_at = ?1 WHERE id = 'dispatch-1'",
        [&later],
    )
    .unwrap();

    assert_eq!(limit_stop_of_dispatch(&conn, "dispatch-1").unwrap(), None);
}

/// KT-2037 — the compact status shows 160 characters of `last_error`: the cause
/// and the two durations fit in them, and the worker is not blamed.
#[test]
fn the_stop_diagnostic_names_the_limit_within_160_characters() {
    let stop = |files, commit: Option<&str>| LimitStop {
        elapsed: secs(240),
        limit: secs(240),
        files,
        checkpoint_commit: commit.map(str::to_string),
    };
    let clean = stop_diagnostic(&stop(0, None));
    let head: String = clean.chars().take(160).collect();
    assert!(head.contains("failure_kind=execution_time_limit"), "{head}");
    assert!(head.contains("240 of 240 min"), "{head}");
    assert!(clean.contains("same limit"), "{clean}");
    // An Ollama worker's limit is another field: no field is named here.
    assert!(
        !clean.contains("Maximum agent execution duration"),
        "{clean}"
    );
    assert!(clean.contains("the worktree was clean"), "{clean}");
    assert!(!clean.contains("worker_reported_failure"), "{clean}");

    let saved = stop_diagnostic(&stop(3, Some("abc1234")));
    assert!(
        saved.contains("3 uncommitted file(s) are saved in WIP commit `abc1234`"),
        "{saved}"
    );
    let left = stop_diagnostic(&stop(3, None));
    assert!(
        left.contains("3 uncommitted file(s) stayed in the worktree"),
        "{left}"
    );
}

/// KT-2037 — a worker far from the limit still reads its clock and the time the
/// run ends; the warnings keep their notice.
#[test]
fn the_time_limit_clock_is_returned_while_the_limit_is_far() {
    use chrono::TimeZone;
    let started = chrono::Utc.with_ymd_and_hms(2026, 1, 1, 20, 22, 0).unwrap();

    let far = time_limit_clock(secs(120), secs(30), started);
    assert_eq!(far["stage"], "ok");
    assert_eq!(far["elapsed_min"], 30);
    assert_eq!(far["limit_min"], 120);
    assert_eq!(far["remaining_min"], 90);
    assert_eq!(far["ends_at"], "22:22 UTC");
    assert!(far.get("notice").is_none(), "{far}");

    let near = time_limit_clock(secs(120), secs(91), started);
    assert_eq!(near["stage"], "near");
    assert!(near["notice"]
        .as_str()
        .unwrap()
        .contains("task_exec_commit"));
    assert_eq!(near["ends_at"], "22:22 UTC");

    let imminent = time_limit_clock(secs(120), secs(111), started);
    assert_eq!(imminent["stage"], "imminent");
    assert!(imminent["notice"]
        .as_str()
        .unwrap()
        .contains("FINAL WARNING"));
    assert_eq!(imminent["ends_at"], "22:22 UTC");
}

/// KT-2037 — a resumed worker is told its run has a limit too and not to start
/// the long command again.
#[test]
fn the_handoff_says_the_same_limit_applies_again() {
    let (conn, exec) = seeded_execution();
    stop_event(&conn, &exec, None);

    let en = handoff_note(&conn, &exec, false).unwrap();
    assert!(en.contains("Do not start again the long command"), "{en}");
    assert!(
        !en.contains("This run"),
        "no budget without a typed worker: {en}"
    );
    assert!(en.contains("run the targeted tests only"), "{en}");

    let fr = handoff_note(&conn, &exec, true).unwrap();
    assert!(fr.contains("Ne relance pas la commande longue"), "{fr}");
    assert!(fr.contains("lance seulement les tests ciblés"), "{fr}");

    // Once the execution has a typed worker, the handoff states that agent's budget.
    conn.execute(
        "UPDATE task_executions SET worker_agent_type = 'ClaudeCode' WHERE id = ?1",
        [&exec],
    )
    .unwrap();
    let typed = handoff_note(&conn, &exec, false).unwrap();
    assert!(typed.contains("This run may last "), "{typed}");
    assert!(
        typed.contains("min: do not start again the long command"),
        "{typed}"
    );
}

/// KT-2037 — the budget stated to a worker is the one of the agent that runs
/// now: a worker moved from Ollama to a hosted agent is not promised Ollama's.
#[test]
fn the_run_budget_follows_the_agent_the_execution_was_moved_to() {
    let (conn, exec) = seeded_execution();
    assert_eq!(run_budget_minutes(&conn, &exec, (30, 240)), None);

    let set_agent = |agent: &str| {
        conn.execute(
            "UPDATE task_executions SET worker_agent_type = ?2 WHERE id = ?1",
            [exec.as_str(), agent],
        )
        .unwrap();
    };
    set_agent("Ollama");
    assert_eq!(run_budget_minutes(&conn, &exec, (30, 240)), Some(240));
    set_agent("ClaudeCode");
    assert_eq!(run_budget_minutes(&conn, &exec, (30, 240)), Some(30));
    assert_eq!(
        run_budget_minutes(&conn, "no-such-execution", (30, 240)),
        None
    );

    // A joined CLI worker is not run by Kronn: no limit applies, none is stated.
    conn.execute(
        "INSERT INTO discussion_sessions \
             (id, disc_id, agent_type, session_id, role, status, joined_at) \
         VALUES (7, 'd-parent', 'ClaudeCode', 'sess-w', 'peer', 'active', \
                 '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE task_executions SET worker_target_kind = 'cli', worker_cli_session_id = 7 \
         WHERE id = ?1",
        [&exec],
    )
    .unwrap();
    assert_eq!(run_budget_minutes(&conn, &exec, (30, 240)), None);
    assert_eq!(run_budget_line(&conn, &exec, false), None);
}

/// KT-2037 — the brief states the budget in minutes to every worker Kronn can
/// stop, and points only a spawned CLI at `task_exec_status`.
#[test]
fn the_brief_section_states_the_budget() {
    for fr in [false, true] {
        let cli = brief_section(secs(240), false, true, fr);
        let http = brief_section(secs(240), false, false, fr);
        assert!(cli.contains("240 min"), "{cli}");
        assert!(http.contains("240 min"), "{http}");
        assert!(cli.contains("`time_limit`"), "{cli}");
        assert!(!http.contains("time_limit"), "{http}");
        assert!(cli.ends_with("\n\n") && http.ends_with("\n\n"));
    }
    assert!(brief_section(secs(240), false, true, false).starts_with("## Time limit\n"));
    assert!(brief_section(secs(240), false, true, true).starts_with("## Limite de temps\n"));
    // An Ollama worker is pointed at the local-agent field, a hosted one at the other.
    let local = brief_section(secs(240), true, false, false);
    assert!(
        local.contains("Maximum local-agent duration (Ollama)"),
        "{local}"
    );
    let hosted = brief_section(secs(240), false, false, false);
    assert!(
        hosted.contains("Maximum agent execution duration"),
        "{hosted}"
    );
    assert!(brief_section(secs(240), true, false, true).contains("agents locaux (Ollama)"));
}
