# TD-20260630-workflow-run-restart-resilience

- **ID**: TD-20260630-workflow-run-restart-resilience
- **Area**: Backend / Workflows (runner, run lifecycle)
- **Problem (fact)**: A workflow run that is in-flight when the **backend process
  restarts** is lost: the run (and its children) are marked `Failed`, every
  not-yet-processed item is silently dropped, and there's **no resume**. There's
  also **no distinction** between "killed by a restart" and a genuine
  step/logic failure — both surface identically as `status: Failed`. Observed
  on the PR-Review cron (`f120ca51-…`): the 18:00 sweep reviewed 4 PRs cleanly,
  then a backend restart (~18:15) killed it mid-5th-child (PR 1834, during the
  `reason` agent step) → run marked `Failed`, the remaining ~22 PRs never
  processed. Same shape hit the 12:08 sweep. In a `cargo-watch watch -x run`
  dev setup this is frequent: **any** source rebuild during a sweep restarts the
  process and aborts the run.
- **Why we can't fix now (constraint)**: Resumability is a real lifecycle
  feature, not a one-line guard. The runner (`backend/src/workflows/runner.rs`)
  drives an in-memory `WorkflowRun` and persists progress snapshots, but on
  boot there's no "find runs left `Running`/`Pending` and resume them" pass, and
  no checkpoint of "which foreach items are done" that a resume could pick up
  from (the foreach loop in `sub_workflow_step.rs` holds its cursor in stack
  state, not in a resumable store). Distinguishing restart-kill from real
  failure needs a new terminal status (e.g. `Interrupted`) plumbed through the
  run model, the SSE/poll surface, the UI badges, and the cron's "did the last
  run succeed?" logic. Too broad to bundle with unrelated fixes.
- **Impact**: correctness/operability (long cron sweeps silently lose most of
  their work on any restart) · observability (Failed badge cries wolf — a
  restart looks like a bug) · dev-loop friction (cargo-watch rebuilds nuke
  running sweeps).
- **Where (pointers)**:
  - `backend/src/workflows/runner.rs` — `execute_run` (in-memory run drive;
    progress persisted via `RunProgressSnapshot`/`update_run_progress` but never
    re-hydrated on boot).
  - `backend/src/workflows/sub_workflow_step.rs` — `execute_foreach` (≈336-589):
    the foreach cursor lives in stack state; a resume would need the
    per-item done-set persisted (the `.kronn/current_task.json` + child run rows
    are the raw material).
  - `backend/src/db/workflows.rs` — run rows + `has_running_run`; boot path that
    would scan for orphaned `Running`/`Pending` runs.
  - `backend/src/models/workflows.rs` — `RunStatus` enum (no `Interrupted`
    variant today).
  - Backend startup wiring (`lib.rs` / main) — where a "reap or resume orphaned
    runs" pass would live.
- **Suggested direction (non-binding)**:
  - **Cheapest first**: on boot, scan for runs stuck in `Running`/`Pending` and
    mark them a distinct **`Interrupted`** status (not `Failed`), so the UI and
    the cron's last-run check don't treat a restart as a real failure.
  - **Then resumability**: persist the foreach done-set per parent run (which
    item indices completed) so a resumed parent skips them; on boot, re-enqueue
    `Interrupted` runs from their last checkpoint. Linear (non-foreach) runs can
    resume from the last persisted `step_results` index.
  - **Ops mitigation (no code)**: run the backend without `cargo-watch` (stable
    `cargo run` / release build) for reliable cron sweeps; the PR-Review sweep
    is also naturally self-healing across runs now that `skip_check` works
    (`per_page=100` — see TD-20260630-apicall-link-header-pagination), so the
    next cron re-reviews only the PRs the killed run didn't reach.
- **Next step**: create ticket.

## Notes

- Surfaced 2026-06-30: the PR-Review cron showed `Failed` after a backend
  restart killed it mid-sweep (4/27 PRs done). Not a workflow-logic bug — the
  foreach already continues past a *child* failure (`succeeded > 0` → parent
  `Success`); the `Failed` came purely from the process dying in flight. Worth
  noting the silver lining: once `skip_check` dedup works, an interrupted sweep
  is largely recovered by the *next* cron (it skips already-reviewed heads),
  so this is operability/observability polish rather than data loss for the
  PR-Review case specifically — but a general resume story matters for
  workflows whose steps have side effects that aren't idempotent across runs.
