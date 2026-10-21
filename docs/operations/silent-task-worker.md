# Silent task worker — detection and recovery (KT-1247)

A task worker that stops producing activity is detected, parked as resumable
and surfaced to the principal. Nothing is killed while the worker is active and
no delivery is ever lost.

## Clocks

| Clock | Applies while | Default | Per-campaign override | Global override (env) |
|---|---|---|---|---|
| Activity | `Working` | 20 min | `orchestration_run_resilience_policy.activity_timeout_secs` | `KRONN_TASK_ACTIVITY_TIMEOUT_SECS` |
| Review | `AwaitingReview` | 4 h | `orchestration_run_resilience_policy.review_timeout_secs` | `KRONN_TASK_REVIEW_TIMEOUT_SECS` |

The campaign policy row always wins over the default. A non-positive or
unparsable env value falls back to the built-in default. The defaults live in
`backend/src/db/orchestration.rs` (`DEFAULT_ACTIVITY_TIMEOUT_SECS`,
`DEFAULT_REVIEW_TIMEOUT_SECS`).

## What counts as activity

Any of these pushes the activity deadline of the `Working` execution:

- a provider/tool boundary recorded by `agent_dispatch::mark_progress`;
- any tool call a worker makes from its execution room (`git_commit` included);
- a message the worker posts in its room (a delivery is one);
- a status transition.

## When the activity clock expires

The watchdog (`start_resilience_watchdog`) applies `apply_execution_timeout`:

1. the execution moves `Working → Interrupted` (`interrupted_from_status =
   Working`) with `recovery_action = resume_worker`, `pending = true`;
2. the worker's dispatch is settled as failed (`dispatch_stalled`) so the later
   resume enqueues a fresh dispatch;
3. the worktree and branch are kept untouched;
4. the principal room receives an orchestration message naming the three
   options: `task_exec_resume` (same worker), `task_exec_reassign` (another
   worker) or `task_exec_reassign` to the principal's own CLI session (take
   over);
5. only then is the provider process stopped.

A row that is no longer `Working` (a delivery moved it to `AwaitingReview`, it
is already parked, ...) is never interrupted: its stale activity deadline is
disarmed and nothing else happens.

## When the review clock expires

The principal is alerted in the parent room (`task_exec_review` / reassign /
cancel are named). No review row is written and nothing is approved or
rejected; the delivery, worktree and branch are intact. The execution keeps its
existing human gate (`Escalated`, `await_human`) from `review_wait_timeout`.
