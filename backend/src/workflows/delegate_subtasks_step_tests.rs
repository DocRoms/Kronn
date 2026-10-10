//! KT-909 — the delegation loop against the real task-execution saga, with
//! fake workers driven from `idle` and a scripted reviewer. Each test counts
//! the model calls the step made: reviews only, never an orchestrator turn.

use super::*;
use crate::api::orchestration::tests::{
    exec_of, git, git_rev, init_repo, projected_manifest_for_execution, test_actor, test_project,
};
use crate::api::orchestration::{
    deliver_native_worker_manifest, DeliverOutcome, NativeExecutionCaller,
};
use crate::models::{
    CreatePlanningDodItem, CreatePlanningTaskRequest, ModelTier, PlanningTaskStatus,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

const RUN_ID: &str = "run-kt909-0001";
const STEP: &str = "implement";
const MAX_IDLES: u32 = 60;

#[derive(Clone, Copy, PartialEq)]
enum Worker {
    Deliver,
    /// The dispatch dies: the watchdog escalates the execution.
    Crash,
    Silent,
}

type Verdicts = Box<dyn FnMut(&ReviewRequest) -> String + Send>;
type Files = Box<dyn Fn(&str, u32) -> (String, String) + Send + Sync>;

struct FakeWorld {
    db: Arc<Database>,
    verdicts: Mutex<Verdicts>,
    files: Files,
    workers: Mutex<HashMap<String, Worker>>,
    reviews: Mutex<Vec<ReviewRequest>>,
    /// Task references seen `Working` at each idle, in plan order.
    working: Mutex<Vec<Vec<String>>>,
    idles: AtomicU32,
    clock: Mutex<Instant>,
    /// The reviewer never answers: only the step deadline ends its call.
    suspended: AtomicBool,
    /// The reviewer answers an hour later.
    late: AtomicBool,
    /// Reviews the provider refuses for a quota before answering.
    quota_refusals: AtomicU32,
}

impl FakeWorld {
    fn new(db: Arc<Database>, verdicts: Verdicts) -> Self {
        Self {
            db,
            verdicts: Mutex::new(verdicts),
            files: Box::new(|task, attempt| {
                (format!("{task}.txt"), format!("{task} attempt {attempt}\n"))
            }),
            workers: Mutex::new(HashMap::new()),
            reviews: Mutex::new(Vec::new()),
            working: Mutex::new(Vec::new()),
            idles: AtomicU32::new(0),
            clock: Mutex::new(Instant::now()),
            suspended: AtomicBool::new(false),
            late: AtomicBool::new(false),
            quota_refusals: AtomicU32::new(0),
        }
    }

    fn reviews(&self) -> Vec<ReviewRequest> {
        self.reviews.lock().unwrap().clone()
    }

    async fn executions(&self) -> Vec<(TaskExecution, String)> {
        self.db
            .with_read_conn(|conn| {
                let mut stmt =
                    conn.prepare("SELECT id FROM task_executions ORDER BY created_at, rowid")?;
                let ids = stmt
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids.iter()
                    .map(|id| {
                        let execution = crate::db::orchestration::get_task_execution(conn, id)?
                            .context("execution")?;
                        let task = crate::db::planning::get_task(conn, &execution.task_id)?
                            .context("task")?;
                        Ok((execution, task.summary.reference))
                    })
                    .collect()
            })
            .await
            .unwrap()
    }

    async fn work(&self, execution: &TaskExecution, task: &str) {
        let (id, attempt) = (execution.id.clone(), execution.attempt_no);
        let delivered = self
            .db
            .with_read_conn(move |conn| {
                Ok(crate::db::worker_deliveries::get_delivery(conn, &id, attempt)?.is_some())
            })
            .await
            .unwrap();
        if delivered {
            return;
        }
        let mode = *self
            .workers
            .lock()
            .unwrap()
            .get(task)
            .unwrap_or(&Worker::Deliver);
        match mode {
            Worker::Silent => {}
            Worker::Crash => {
                let id = execution.id.clone();
                self.db
                    .with_conn(move |conn| {
                        crate::db::orchestration::transition_execution(
                            conn,
                            &id,
                            TaskExecutionStatus::Escalated,
                            &test_actor(),
                            json!({ "reason": "worker dispatch failed" }),
                        )?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            }
            Worker::Deliver => self.deliver(execution, task).await,
        }
    }

    async fn deliver(&self, execution: &TaskExecution, task: &str) {
        let id = execution.id.clone();
        let dispatch = execution.dispatch_job_id.clone().expect("worker dispatch");
        let lookup = dispatch.clone();
        let (worktree, trigger) = self
            .db
            .with_read_conn(move |conn| {
                let worktree =
                    crate::db::discussion_workspaces::get_managed_for_execution(conn, &id)?
                        .and_then(|workspace| workspace.canonical_path)
                        .context("worktree")?;
                let trigger = crate::db::agent_dispatch::get(conn, &lookup)?
                    .context("dispatch")?
                    .trigger_message_id;
                Ok((worktree, trigger))
            })
            .await
            .unwrap();
        let (path, content) = (self.files)(task, execution.attempt_no);
        std::fs::write(Path::new(&worktree).join(&path), content).unwrap();
        for args in [vec!["add", "."], vec!["commit", "-m", "worker change"]] {
            assert!(git(Path::new(&worktree), &args).status.success());
        }
        let manifest = projected_manifest_for_execution(&self.db, &execution.id).await;
        let child = execution.sub_discussion_id.clone().expect("worker room");
        let outcome = deliver_native_worker_manifest(
            &self.db,
            &execution.id,
            NativeExecutionCaller {
                discussion_id: &child,
                agent_type: &AgentType::ClaudeCode,
                source_message_id: Some(&trigger),
                alias: "Fake worker",
                actor_session_id: Some("fake-worker"),
            },
            &dispatch,
            &manifest,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, DeliverOutcome::Delivered { .. }),
            "{outcome:?}"
        );
    }
}

#[async_trait::async_trait]
impl DelegationWorld for FakeWorld {
    async fn review(&self, request: ReviewRequest) -> ReviewReply {
        let refused = {
            let mut left = self.quota_refusals.load(Ordering::SeqCst);
            loop {
                if left == 0 {
                    break false;
                }
                match self.quota_refusals.compare_exchange_weak(
                    left,
                    left - 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                ) {
                    Ok(_) => break true,
                    Err(seen) => left = seen,
                }
            }
        };
        if refused {
            self.reviews.lock().unwrap().push(request);
            return ReviewReply {
                quota_wait: Some(QuotaWait {
                    id: Some("quota-1".into()),
                    reset_at: None,
                    wake_at: None,
                    attempt: 1,
                    parked: None,
                    detail: None,
                }),
                ..Default::default()
            };
        }
        let text = (self.verdicts.lock().unwrap())(&request);
        self.reviews.lock().unwrap().push(request);
        if self.suspended.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.late.load(Ordering::SeqCst) {
            *self.clock.lock().unwrap() += Duration::from_secs(3600);
        }
        ReviewReply {
            text,
            tokens: Some(100),
            cost_usd: Some(0.01),
            ..Default::default()
        }
    }

    async fn idle(&self) {
        let idles = self.idles.fetch_add(1, Ordering::SeqCst);
        assert!(idles < MAX_IDLES, "the step never settled");
        *self.clock.lock().unwrap() += Duration::from_secs(60);
        let executions = self.executions().await;
        self.working.lock().unwrap().push(
            executions
                .iter()
                .filter(|(execution, _)| execution.status == TaskExecutionStatus::Working)
                .map(|(_, task)| task.clone())
                .collect(),
        );
        for (execution, task) in executions {
            if execution.status == TaskExecutionStatus::Working {
                self.work(&execution, &task).await;
            }
        }
    }

    fn now(&self) -> Instant {
        *self.clock.lock().unwrap()
    }

    async fn wait_until(&self, deadline: Instant) {
        let mut clock = self.clock.lock().unwrap();
        *clock = (*clock).max(deadline);
    }
}

/// The DoD ids the prompt lists, as a reviewer would read them.
fn dod_ids(prompt: &str) -> Vec<String> {
    prompt
        .lines()
        .filter_map(|line| line.strip_prefix("- `"))
        .filter_map(|line| line.split_once("` — ").map(|(id, _)| id.to_string()))
        .collect()
}

fn approve(request: &ReviewRequest) -> String {
    let verifications: Vec<Value> = dod_ids(&request.prompt)
        .into_iter()
        .map(|id| json!({ "dod_id": id, "met": true, "evidence": "the diff writes the file" }))
        .collect();
    assert!(!verifications.is_empty(), "the prompt lists the DoD ids");
    let verdict =
        json!({ "decision": "approve", "comment": "ok", "dod_verifications": verifications });
    format!("Looks right.\n```json\n{verdict}\n```")
}

fn verdict(decision: &str, comment: &str) -> String {
    json!({ "decision": decision, "comment": comment, "reason": comment,
            "findings": [{ "path": "x.txt", "line": 1, "issue": comment }] })
    .to_string()
}

struct Fixture {
    _repo: tempfile::TempDir,
    repo: PathBuf,
    state: AppState,
    parent: String,
    subtasks: Vec<String>,
}

/// A project repo, a parent task and `count` Todo subtasks tagged `worker:haiku`;
/// `blocked_by` pairs are (blocked index, blocker index).
async fn fixture(count: usize, blocked_by: &[(usize, usize)]) -> Fixture {
    let repo = init_repo();
    let path = repo.path().to_path_buf();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let project = test_project("proj-1", &path.to_string_lossy());
    let blocked_by = blocked_by.to_vec();
    let (parent, subtasks) = db
        .with_conn(move |conn| {
            crate::db::projects::insert_project(conn, &project)?;
            let workflow: crate::models::Workflow = serde_json::from_value(json!({
                "id": "wf-kt909", "name": "wf-kt909", "project_id": "proj-1",
                "trigger": {"type": "Manual"},
                "steps": [{"name": STEP, "step_type": {"type": "Gate"}}],
                "actions": [],
                "safety": {"sandbox": false, "max_files": null, "max_lines": null, "require_approval": false},
                "workspace_config": null, "concurrency_limit": null, "enabled": true,
                "created_at": chrono::Utc::now(), "updated_at": chrono::Utc::now(),
            }))?;
            crate::db::workflows::insert_workflow(conn, &workflow)?;
            let run: crate::models::WorkflowRun = serde_json::from_value(json!({
                "id": RUN_ID, "workflow_id": "wf-kt909", "status": "Running",
                "trigger_context": null, "step_results": [], "tokens_used": 0,
                "workspace_path": null, "started_at": chrono::Utc::now(), "finished_at": null,
            }))?;
            crate::db::workflows::insert_run(conn, &run)?;
            let task = |title: &str, parent: Option<String>| CreatePlanningTaskRequest {
                title: title.into(),
                discussion_id: None,
                idempotency_key: None,
                description: format!("Implement {title}."),
                status: PlanningTaskStatus::Todo,
                priority: Default::default(),
                parent_id: parent,
                project_ids: vec!["proj-1".into()],
                tags: vec!["worker:haiku".into()],
                definition_of_done: vec![CreatePlanningDodItem {
                    id: None,
                    sentence: format!("{title} is written to its file"),
                    completed: false,
                }],
                links: vec![],
                actor: test_actor(),
            };
            let parent = crate::db::planning::create_task(conn, &task("Parent", None))?;
            let mut subtasks = Vec::new();
            for index in 0..count {
                let child = crate::db::planning::create_task(
                    conn,
                    &task(&format!("Part {index}"), Some(parent.summary.id.clone())),
                )?;
                subtasks.push(child.summary.reference);
            }
            for (blocked, blocker) in blocked_by {
                let blocker_id = crate::db::planning::get_task(conn, &subtasks[blocker])?
                    .context("blocker")?
                    .summary
                    .id;
                crate::db::planning::add_blocker(
                    conn,
                    &subtasks[blocked],
                    &crate::models::AddPlanningBlockerRequest {
                        blocker_task_id: blocker_id,
                        actor: test_actor(),
                    },
                )?;
            }
            Ok((parent.summary.reference, subtasks))
        })
        .await
        .unwrap();
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        db,
        4,
    );
    Fixture {
        _repo: repo,
        repo: path,
        state,
        parent,
        subtasks,
    }
}

fn config(concurrency: u32, rounds: u32) -> DelegateSubtasksConfig {
    DelegateSubtasksConfig {
        parent_task: "{{steps.guard.data.taskId}}".into(),
        worker_map: [(
            "haiku".to_string(),
            DelegateWorker {
                agent: AgentType::ClaudeCode,
                tier: Some(ModelTier::Economy),
                model: None,
            },
        )]
        .into(),
        concurrency: Some(concurrency),
        max_review_rounds: Some(rounds),
        ..Default::default()
    }
}

async fn delegate(
    fixture: &Fixture,
    config: &DelegateSubtasksConfig,
    world: &FakeWorld,
    budget: Option<SharedBudget>,
) -> DelegationReport {
    let repo = fixture.repo.to_string_lossy().to_string();
    let delegation = Delegation {
        state: &fixture.state,
        run_id: RUN_ID,
        step_name: STEP,
        workflow_project_id: Some("proj-1"),
        reviewer_agent: AgentType::ClaudeCode,
        config,
        parent_task: fixture.parent.clone(),
        target_branch: None,
        work_dir: &repo,
        reviewer_guidance: "",
        budget,
    };
    run_delegation(&delegation, world).await
}

async fn count(db: &Database, sql: &str, param: &str) -> i64 {
    let (sql, param) = (sql.to_string(), param.to_string());
    db.with_read_conn(move |conn| Ok(conn.query_row(&sql, [param], |row| row.get(0))?))
        .await
        .unwrap()
}

fn step_with_routes() -> WorkflowStep {
    serde_json::from_value(json!({
        "name": STEP,
        "step_type": { "type": "DelegateSubtasks" },
        "on_result": [{ "contains": "ESCALATED", "action": { "type": "Goto", "step_name": "arbitrate" } }]
    }))
    .unwrap()
}

#[tokio::test]
async fn a_three_subtask_plan_reaches_integration_with_one_review_per_delivery_and_no_orchestrator()
{
    // Part 2 waits on Part 0: plan order and blockers decide the waves.
    let fixture = fixture(3, &[(2, 0)]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let report = delegate(&fixture, &config(2, 3), &world, None).await;

    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    let [part0, part1, part2] = [0, 1, 2].map(|index| fixture.subtasks[index].clone());
    let waves = world.working.lock().unwrap().clone();
    assert_eq!(
        waves.first(),
        Some(&vec![part0.clone(), part1.clone()]),
        "the cap admits two workers, in plan order; the blocked part waits"
    );
    assert!(waves
        .iter()
        .take_while(|wave| !wave.contains(&part2))
        .all(|wave| wave.len() <= 2));
    assert!(waves.iter().any(|wave| wave.contains(&part2)));

    // One fresh review per delivery, each with the DoD, the diff and the report.
    let reviews = world.reviews();
    assert_eq!(reviews.len(), 3);
    for request in &reviews {
        let task = &request.task_reference;
        assert!(request.prompt.contains("is written to its file"), "DoD");
        assert!(
            request.prompt.contains(&format!("+{task} attempt 0")),
            "diff"
        );
        assert!(request.prompt.contains("did the thing"), "worker report");
    }
    let mut reviewed: Vec<_> = reviews.iter().map(|r| r.task_reference.clone()).collect();
    reviewed.sort();
    let mut expected = fixture.subtasks.clone();
    expected.sort();
    assert_eq!(
        reviewed, expected,
        "exactly one review session per delivery"
    );

    // Every part landed on the run's branch, each with its integrated SHA.
    let main = git_rev(&fixture.repo, "main");
    for part in &report.subtasks {
        assert_eq!(part.status, "Done");
        let sha = part.integrated_sha.as_deref().expect("integrated sha");
        assert!(
            git(&fixture.repo, &["merge-base", "--is-ancestor", sha, &main])
                .status
                .success()
        );
        assert_eq!((part.review_rounds, part.attempts), (0, 1));
        assert_eq!(part.cost_usd, Some(0.01), "the review's cost");
        assert!(fixture.repo.join(format!("{}.txt", part.task)).exists());
    }

    // The token saving: the step made 3 review calls and no orchestrator turn.
    let db = &fixture.state.db;
    let owner = report.discussion_id.clone().unwrap();
    assert_eq!(report.review_calls, 3);
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = ?1",
            &owner
        )
        .await,
        0,
        "no agent turn was dispatched in the principal room"
    );
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id <> ?1",
            &owner
        )
        .await,
        3,
        "one worker dispatch per subtask"
    );

    let outcome = finish(&step_with_routes(), Instant::now(), report);
    assert_eq!(outcome.result.status, RunStatus::Success);
    assert!(outcome.result.output.trim_end().ends_with("[SIGNAL: OK]"));
    let envelope = crate::workflows::template::extract_step_envelope(&outcome.result.output)
        .expect("envelope");
    let data: Value = serde_json::from_str(&envelope.data_json).unwrap();
    let first = &data["subtasks"][0];
    for field in ["status", "integrated_sha", "review_rounds", "cost_usd"] {
        assert!(!first[field].is_null(), "output lists {field}");
    }
}

#[tokio::test]
async fn request_changes_returns_to_the_worker_then_the_round_cap_escalates_routably() {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(
        fixture.state.db.clone(),
        Box::new(|_| verdict("request_changes", "the guard is missing")),
    );
    let report = delegate(&fixture, &config(1, 1), &world, None).await;

    assert_eq!(
        report.signal,
        DelegationSignal::Escalated,
        "{}",
        report.summary
    );
    assert_eq!(world.reviews().len(), 2, "one review per delivery");
    let executions = world.executions().await;
    let (execution, _) = &executions[0];
    assert_eq!(execution.status, TaskExecutionStatus::Escalated);
    assert_eq!(execution.review_rounds, 2);
    let child = execution.sub_discussion_id.clone().unwrap();
    assert!(
        count(
            &fixture.state.db,
            "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1 \
             AND content LIKE '%the guard is missing%'",
            &child,
        )
        .await
            >= 1,
        "the findings reached the worker"
    );
    let reviewed_twice = world.reviews();
    assert!(reviewed_twice[1].prompt.contains("attempt 1"));

    let outcome = finish(&step_with_routes(), Instant::now(), report);
    assert_eq!(outcome.result.status, RunStatus::Failed);
    assert!(matches!(
        outcome.condition_action,
        Some(ConditionAction::Goto { ref step_name, .. }) if step_name == "arbitrate"
    ));
}

#[tokio::test]
async fn a_reviewer_escalation_stops_the_step_and_holds_the_execution() {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(
        fixture.state.db.clone(),
        Box::new(|_| verdict("escalate", "the DoD contradicts the parent")),
    );
    let report = delegate(&fixture, &config(1, 3), &world, None).await;
    assert_eq!(report.signal, DelegationSignal::Escalated);
    assert!(report.summary.contains("contradicts"));
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(execution.status, TaskExecutionStatus::Escalated);
    assert_eq!(world.reviews().len(), 1);
}

#[tokio::test]
async fn a_repeated_integration_conflict_is_reported() {
    let fixture = fixture(2, &[]).await;
    let mut world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world.files = Box::new(|task, attempt| ("shared.txt".into(), format!("{task} {attempt}\n")));
    let report = delegate(&fixture, &config(2, 3), &world, None).await;

    assert_eq!(
        report.signal,
        DelegationSignal::Conflict,
        "{}",
        report.summary
    );
    let conflicted = report
        .subtasks
        .iter()
        .find(|part| part.integration_conflicts >= 2)
        .expect("a subtask conflicted twice");
    assert!(conflicted.integrated_sha.is_none());
    assert!(report
        .subtasks
        .iter()
        .any(|part| part.status == "Done" && part.integrated_sha.is_some()));
    assert_eq!(
        world.reviews().len(),
        3,
        "first deliveries, then the rework"
    );
}

#[tokio::test]
async fn a_worker_failure_and_a_timeout_stop_the_step_without_cancelling_work() {
    let fixture = fixture(2, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world
        .workers
        .lock()
        .unwrap()
        .insert(fixture.subtasks[0].clone(), Worker::Crash);
    world
        .workers
        .lock()
        .unwrap()
        .insert(fixture.subtasks[1].clone(), Worker::Silent);
    let report = delegate(&fixture, &config(2, 3), &world, None).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Escalated,
        "{}",
        report.summary
    );
    assert!(report.summary.contains("worker dispatch failed"));

    let fixture = fixture_with_silent_worker().await;
    let (fixture, world) = fixture;
    let mut short = config(1, 3);
    short.timeout_secs = Some(300);
    let report = delegate(&fixture, &short, &world, None).await;
    assert_eq!(report.signal, DelegationSignal::Timeout);
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(
        execution.status,
        TaskExecutionStatus::Working,
        "a stop never cancels a running worker"
    );
    assert!(world.reviews().is_empty());
}

async fn fixture_with_silent_worker() -> (Fixture, FakeWorld) {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world
        .workers
        .lock()
        .unwrap()
        .insert(fixture.subtasks[0].clone(), Worker::Silent);
    (fixture, world)
}

#[tokio::test]
async fn a_resumed_step_relaunches_nothing_and_reviews_only_what_is_pending() {
    let fixture = fixture(2, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    // A one-call budget: the first delivery is reviewed, the second waits.
    let spent = SharedBudget::root(1);
    let report = delegate(&fixture, &config(2, 3), &world, Some(spent.clone())).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Blocked,
        "{}",
        report.summary
    );
    assert!(report.summary.contains("budget"));
    assert_eq!((world.reviews().len(), spent.llm_calls()), (1, 1));
    let before = world.executions().await;
    assert_eq!(before.len(), 2);
    let done_id = before
        .iter()
        .find(|(execution, _)| execution.status == TaskExecutionStatus::Done)
        .map(|(execution, _)| execution.id.clone())
        .expect("the reviewed part is integrated");
    let done_sha = exec_of(&fixture.state.db, &done_id).await.integrated_sha;

    // The restarted step finds the room and the campaign, launches nothing new,
    // and spends one review on the delivery still waiting.
    // Boot recovery leaves the pending review to the step: no paid principal turn.
    let pending = before
        .iter()
        .find(|(execution, _)| execution.status == TaskExecutionStatus::AwaitingReview)
        .map(|(execution, _)| execution.id.clone())
        .expect("one delivery awaits review");
    crate::api::orchestration::wake_recovered_principal(&fixture.state.db, &pending)
        .await
        .unwrap();
    let owner = report.discussion_id.clone().unwrap();
    assert_eq!(
        count(
            &fixture.state.db,
            "SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = ?1",
            &owner
        )
        .await,
        0
    );

    let resumed = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let report = delegate(
        &fixture,
        &config(2, 3),
        &resumed,
        Some(SharedBudget::root(5)),
    )
    .await;
    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    assert_eq!(resumed.reviews().len(), 1);
    let after = resumed.executions().await;
    assert_eq!(after.len(), 2, "no execution was relaunched");
    assert_eq!(
        exec_of(&fixture.state.db, &done_id).await.integrated_sha,
        done_sha
    );
    assert_eq!(
        count(
            &fixture.state.db,
            "SELECT COUNT(*) FROM discussions WHERE workflow_run_id = ?1",
            RUN_ID
        )
        .await,
        1,
        "the resumed step reuses its room"
    );
}

#[tokio::test]
async fn a_verdict_recorded_before_a_restart_is_applied_without_a_new_review() {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let report = delegate(&fixture, &config(1, 3), &world, Some(SharedBudget::root(0))).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Blocked,
        "{}",
        report.summary
    );
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(execution.status, TaskExecutionStatus::AwaitingReview);

    // The crash window: the verdict is durable, its decision is not applied yet.
    let message = review_message_id(&execution.id, execution.attempt_no);
    let owner = report.discussion_id.unwrap();
    fixture
        .state
        .db
        .with_conn(move |conn| {
            let dod_id = conn.query_row(
                "SELECT d.id FROM planning_task_dod_items d \
                 JOIN task_executions e ON e.task_id = d.task_id LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )?;
            let verdict = json!({ "decision": "approve", "dod_verifications": [
                { "dod_id": dod_id, "met": true, "evidence": "read before the crash" }
            ]});
            let mut note =
                crate::api::orchestration::orchestrator_message(message, verdict.to_string());
            note.role = MessageRole::Agent;
            crate::db::discussions::insert_message(conn, &owner, &note)?;
            Ok(())
        })
        .await
        .unwrap();
    let resumed = FakeWorld::new(fixture.state.db.clone(), Box::new(|_| unreachable!()));
    let report = delegate(
        &fixture,
        &config(1, 3),
        &resumed,
        Some(SharedBudget::root(0)),
    )
    .await;
    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    assert_eq!(report.review_calls, 0);
}

#[tokio::test]
async fn a_reassignment_moves_the_subtask_once_then_escalates() {
    let fixture = fixture(1, &[]).await;
    let mut calls = 0;
    let world = FakeWorld::new(
        fixture.state.db.clone(),
        Box::new(move |request: &ReviewRequest| {
            calls += 1;
            match calls {
                1 => json!({ "decision": "reassign", "worker": "sonnet", "reason": "too subtle" })
                    .to_string(),
                _ => approve(request),
            }
        }),
    );
    let mut two_workers = config(1, 3);
    two_workers.worker_map.insert(
        "sonnet".into(),
        DelegateWorker {
            agent: AgentType::ClaudeCode,
            tier: Some(ModelTier::Default),
            model: Some("sonnet".into()),
        },
    );
    let report = delegate(&fixture, &two_workers, &world, None).await;
    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    assert_eq!(
        world.reviews().len(),
        2,
        "the reassigned worker's delivery is reviewed"
    );
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(execution.worker_model.as_deref(), Some("sonnet"));

    let fixture = self::fixture(1, &[]).await;
    let world = FakeWorld::new(
        fixture.state.db.clone(),
        Box::new(|_| json!({ "decision": "reassign", "worker": "sonnet" }).to_string()),
    );
    let report = delegate(&fixture, &two_workers, &world, None).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Escalated,
        "{}",
        report.summary
    );
    assert!(report.summary.contains("second reassignment"));
}

#[tokio::test]
async fn a_subtask_without_a_worker_stops_before_anything_launches() {
    let fixture = fixture(2, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let mut unmapped = config(1, 3);
    unmapped.worker_map.clear();
    let report = delegate(&fixture, &unmapped, &world, None).await;
    assert_eq!(report.signal, DelegationSignal::Failed);
    assert!(report.summary.contains("no worker for"));
    assert!(world.executions().await.is_empty());
}

#[test]
fn the_authoring_schema_example_is_a_valid_step() {
    let schema: Value =
        serde_json::from_str(include_str!("../api/workflow_step_schema.json")).unwrap();
    let example = schema["fields_by_type"]["DelegateSubtasks"]["example"].clone();
    let step: WorkflowStep = serde_json::from_value(example).unwrap();
    let config = step.delegate_subtasks.expect("config");
    assert_eq!(step.step_type, crate::models::StepType::DelegateSubtasks);
    assert_eq!(config.worker_map.len(), 2);
    assert!(config.worker_map.values().all(|worker| !matches!(
        worker_selection(worker).target.kind,
        crate::models::MessageTargetKind::Cli
    )));
}

#[test]
fn the_verdict_is_the_last_json_object_with_a_decision() {
    let text = "Example: {\"decision\": \"escalate\"} is not mine.\n\
                {\"decision\": \"approve\", \"comment\": \"fine\"}\n{\"other\": 1}";
    let verdict = parse_verdict(text).expect("verdict");
    assert_eq!(verdict.decision, "approve");
    assert!(parse_verdict("no json here").is_none());
}

async fn review_messages(db: &Database) -> i64 {
    count(
        db,
        "SELECT COUNT(*) FROM messages WHERE id LIKE ?1",
        "wf-review:%",
    )
    .await
}

#[tokio::test]
async fn a_review_still_running_at_the_deadline_times_out_without_a_late_integration() {
    let fixture = fixture(1, &[]).await;
    let main = git_rev(&fixture.repo, "main");
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world.suspended.store(true, Ordering::SeqCst);
    let mut bounded = config(1, 3);
    bounded.timeout_secs = Some(300);
    let report = delegate(&fixture, &bounded, &world, None).await;

    assert_eq!(
        report.signal,
        DelegationSignal::Timeout,
        "{}",
        report.summary
    );
    assert_eq!(report.review_calls, 1, "the cut session still counts");
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(execution.status, TaskExecutionStatus::AwaitingReview);
    assert_eq!(
        git_rev(&fixture.repo, "main"),
        main,
        "nothing integrated late"
    );
    assert_eq!(review_messages(&fixture.state.db).await, 0);
}

#[tokio::test]
async fn a_verdict_returned_after_the_deadline_is_kept_and_applied_only_on_resume() {
    let fixture = fixture(1, &[]).await;
    let main = git_rev(&fixture.repo, "main");
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world.late.store(true, Ordering::SeqCst);
    let mut bounded = config(1, 3);
    bounded.timeout_secs = Some(300);
    let report = delegate(&fixture, &bounded, &world, None).await;

    assert_eq!(
        report.signal,
        DelegationSignal::Timeout,
        "{}",
        report.summary
    );
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(execution.status, TaskExecutionStatus::AwaitingReview);
    assert_eq!(
        git_rev(&fixture.repo, "main"),
        main,
        "the late approval is not applied"
    );
    assert_eq!(
        review_messages(&fixture.state.db).await,
        1,
        "the verdict is durable"
    );

    let resumed = FakeWorld::new(fixture.state.db.clone(), Box::new(|_| unreachable!()));
    let report = delegate(&fixture, &bounded, &resumed, None).await;
    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    assert_eq!(report.review_calls, 0);
}

#[tokio::test]
async fn a_pending_reassignment_is_applied_once_on_resume() {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let mut two_workers = config(1, 3);
    two_workers.worker_map.insert(
        "sonnet".into(),
        DelegateWorker {
            agent: AgentType::ClaudeCode,
            tier: Some(ModelTier::Default),
            model: Some("sonnet".into()),
        },
    );
    let report = delegate(&fixture, &two_workers, &world, Some(SharedBudget::root(0))).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Blocked,
        "{}",
        report.summary
    );
    let (execution, _) = world.executions().await.remove(0);
    assert_eq!(execution.status, TaskExecutionStatus::AwaitingReview);

    // The crash window: verdict and marker are durable, the reassignment is not.
    let owner = report.discussion_id.unwrap();
    let (verdict_id, marker_id) = (
        review_message_id(&execution.id, execution.attempt_no),
        reassign_marker_id(&execution.id, execution.attempt_no),
    );
    let parent = execution.parent_discussion_id.clone();
    fixture
        .state
        .db
        .with_conn(move |conn| {
            let mut verdict = crate::api::orchestration::orchestrator_message(
                verdict_id,
                json!({ "decision": "reassign", "worker": "sonnet" }).to_string(),
            );
            verdict.role = MessageRole::Agent;
            crate::db::discussions::insert_message(conn, &owner, &verdict)?;
            let marker = orchestrator_note(marker_id, "pending".into());
            crate::db::discussions::insert_message(conn, &parent, &marker)?;
            Ok(())
        })
        .await
        .unwrap();
    let assignments =
        "SELECT COUNT(*) FROM task_execution_assignment_events WHERE task_execution_id = ?1";
    let before = count(&fixture.state.db, assignments, &execution.id).await;

    let resumed = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let report = delegate(&fixture, &two_workers, &resumed, None).await;
    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    assert_eq!(
        count(&fixture.state.db, assignments, &execution.id).await - before,
        1,
        "exactly one reassignment"
    );
    assert_eq!(
        resumed.reviews().len(),
        1,
        "only the reassigned worker's new delivery is reviewed"
    );
    let done = exec_of(&fixture.state.db, &execution.id).await;
    assert_eq!(done.worker_model.as_deref(), Some("sonnet"));
}

async fn tree_llm_calls(db: &Database) -> u32 {
    db.with_read_conn(|conn| crate::db::workflows::tree_llm_calls(conn, RUN_ID))
        .await
        .unwrap()
}

#[tokio::test]
async fn each_review_is_recorded_durably_so_a_fresh_budget_keeps_the_cap() {
    let fixture = fixture(2, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let budget = SharedBudget::root(1).recorded_on(RUN_ID);
    let report = delegate(&fixture, &config(2, 3), &world, Some(budget)).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Blocked,
        "{}",
        report.summary
    );
    assert_eq!(world.reviews().len(), 1);
    assert_eq!(
        tree_llm_calls(&fixture.state.db).await,
        1,
        "recorded before the step returned"
    );

    // A restarted runner rebuilds its budget from the durable count only.
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        fixture.state.db.clone(),
        4,
    );
    let restarted = Fixture { state, ..fixture };
    let fresh = SharedBudget::root(1).recorded_on(RUN_ID);
    fresh.add_llm_calls(tree_llm_calls(&restarted.state.db).await);
    let resumed = FakeWorld::new(restarted.state.db.clone(), Box::new(|_| unreachable!()));
    let report = delegate(&restarted, &config(2, 3), &resumed, Some(fresh)).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Blocked,
        "{}",
        report.summary
    );
    assert_eq!(report.review_calls, 0, "the cap holds across the restart");
}

#[tokio::test]
async fn a_refused_count_write_stops_terminally_before_the_next_review() {
    let fixture = fixture(2, &[]).await;
    fixture
        .state
        .db
        .with_conn(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER refuse_second_call BEFORE UPDATE OF tree_llm_calls ON workflow_runs \
                 WHEN NEW.tree_llm_calls >= 2 BEGIN SELECT RAISE(ABORT, 'count refused'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    let budget = SharedBudget::root(5).recorded_on(RUN_ID);
    let report = delegate(&fixture, &config(2, 3), &world, Some(budget)).await;

    assert_eq!(
        world.reviews().len(),
        1,
        "the second reviewer is never called"
    );
    assert!(report
        .terminal_stop
        .as_deref()
        .is_some_and(|reason| reason.contains("count refused")));
    let outcome = finish(&step_with_routes(), Instant::now(), report);
    assert_eq!(outcome.result.status, RunStatus::Failed);
    assert!(outcome.result.terminal_stop.is_some());
}

/// A runner restarted on the same database, its budget rebuilt from the durable count.
async fn restarted_with_budget(fixture: Fixture, cap: u32) -> (Fixture, SharedBudget) {
    let state = AppState::new_defaults(
        Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        fixture.state.db.clone(),
        4,
    );
    let restarted = Fixture { state, ..fixture };
    let fresh = SharedBudget::root(cap).recorded_on(RUN_ID);
    fresh.add_llm_calls(tree_llm_calls(&restarted.state.db).await);
    (restarted, fresh)
}

#[tokio::test]
async fn a_quota_refused_review_is_not_counted_so_the_resume_can_still_review() {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world.quota_refusals.store(1, Ordering::SeqCst);
    let budget = SharedBudget::root(1).recorded_on(RUN_ID);
    let report = delegate(&fixture, &config(1, 3), &world, Some(budget.clone())).await;
    assert!(report.quota_wait.is_some(), "{}", report.summary);
    assert_eq!(world.reviews().len(), 1);
    assert_eq!(budget.llm_calls(), 0, "the refused call spent nothing");
    assert_eq!(tree_llm_calls(&fixture.state.db).await, 0);
    let outcome = finish(&step_with_routes(), Instant::now(), report);
    assert!(outcome.result.quota_wait.is_some());

    // After the release, a restarted runner still has the budget for one review.
    let (restarted, fresh) = restarted_with_budget(fixture, 1).await;
    assert!(
        fresh.llm_calls() < fresh.max_llm_calls(),
        "the runner guard lets it run"
    );
    let resumed = FakeWorld::new(restarted.state.db.clone(), Box::new(approve));
    let report = delegate(&restarted, &config(1, 3), &resumed, Some(fresh.clone())).await;
    assert_eq!(report.signal, DelegationSignal::Ok, "{}", report.summary);
    assert_eq!(resumed.reviews().len(), 1);
    assert_eq!(fresh.llm_calls(), 1, "the real review counts once");
    assert_eq!(tree_llm_calls(&restarted.state.db).await, 1);
}

#[tokio::test]
async fn an_interrupted_review_stays_counted_across_a_restart() {
    let fixture = fixture(1, &[]).await;
    let world = FakeWorld::new(fixture.state.db.clone(), Box::new(approve));
    world.suspended.store(true, Ordering::SeqCst);
    let mut bounded = config(1, 3);
    bounded.timeout_secs = Some(300);
    let budget = SharedBudget::root(1).recorded_on(RUN_ID);
    let report = delegate(&fixture, &bounded, &world, Some(budget.clone())).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Timeout,
        "{}",
        report.summary
    );
    assert_eq!(budget.llm_calls(), 1);
    assert_eq!(tree_llm_calls(&fixture.state.db).await, 1);

    let (restarted, fresh) = restarted_with_budget(fixture, 1).await;
    let resumed = FakeWorld::new(restarted.state.db.clone(), Box::new(|_| unreachable!()));
    let report = delegate(&restarted, &config(1, 3), &resumed, Some(fresh)).await;
    assert_eq!(
        report.signal,
        DelegationSignal::Blocked,
        "{}",
        report.summary
    );
    assert_eq!(report.review_calls, 0, "the cut call is not bought again");
}
