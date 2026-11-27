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
rejected; the delivery, worktree and branch are intact. The execution stays
`AwaitingReview` (`recovery_action` `await_review`, `recovery_reason`
`review_wait_timeout`, `pending = 0`) so `task_exec_review` still accepts an
approval or a request for changes. The review and total-duration deadlines are
disarmed: the alert is posted once. Because the row is not `Escalated`, an
overdue review does not put the campaign in `AwaitingHuman` and does not block
scheduling.

A total-duration deadline that expires while the row is `AwaitingReview` takes
the same road (`recovery_reason` `total_duration_timeout`): one alert, the
execution stays `AwaitingReview`, and the provider run is not cancelled so a
`request_changes` can still wake the worker. Any other state keeps the generic
`Escalated` / `await_human` handling.

## Validation and Exec time limit (KT-2129)

A task validation (prepare and run), an integration gate and a workflow Exec
step share one ceiling for the time a single command may run.

| Level | Where | Default |
|---|---|---|
| Global | Settings > Workers, `[workers] validation_max_timeout_secs` in `config.toml` | 1800 s (30 min) |
| Project | the project's lane warm-up card, `workspace.lane_warm.validation_max_timeout_secs` | the global value |

The project value wins over the global one. Both are bounded to 60..14400 s
(4 h); a value outside is refused when saved, and a hand-edited file is clamped
to the bounds. A spec that gives no timeout still runs for 600 s (or the ceiling
when lower).

- `timeout_secs` of a validation above the ceiling is refused at launch, swap and
  campaign creation, naming the ceiling and where to raise it. Those intakes do
  not always know the project, so they accept the highest ceiling any scope
  holds; the run clamps to the project's own and says so in its output.
- `exec_timeout_secs` of an Exec step above the ceiling of the workflow's project
  (else the global one) is refused when the workflow is saved. Import and
  definition checks that carry no project use the highest ceiling.
- A validation stopped at its limit starts its output with
  `[kronn: validation timed out after Ns, ceiling Ms]`. The integration notice
  then reads "stopped by the time limit" instead of "the process did not
  complete", names the ceiling and the setting, and tells the principal that
  `task_exec_resume` reruns the same validation on the same candidate. The
  delivery is still sent back to the worker as before.
- A workflow Exec step is also held to the ceiling when it runs (the project of
  its tree, else the global one): a step stored higher is cut at the ceiling and
  its output starts with a `clamped to` line naming the ceiling and the setting.
- Saved Quick Execs and CollectApiData sources keep their own 1..1800 s bound and
  do not follow this setting.

The silent-worker clock above never counts a validation Kronn runs: it expires
`Working` rows only, and a validation runs in `Integrating`, `Validating` or
`Applying`. A command the worker itself runs (a CLI's shell tool) is seen only
through its dispatch's `tool_activity` phase, for up to
`KRONN_TASK_TOOL_IN_FLIGHT_CAP_SECS` (2 h); a worker without that phase (a joined
CLI) is not seen.
