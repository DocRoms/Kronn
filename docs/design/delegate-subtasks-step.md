# Design note — the `DelegateSubtasks` workflow step (KT-909)

> Status: **design only (0.14.3)**. Implementation is planned for 0.15.x.
> Grounded in the code of `feat/0.14.3`; every claim cites `file:line`.

## 1. Problem

A plan is already structured before implementation: the subtasks exist, each
with its DoD, its order and its worker. Yet the only way to run them is an
orchestrator agent that chains the `task_exec_*` MCP tools and keeps its whole
context alive while it waits. On EW-7633 that orchestrator cost 10.16 $ out of
11.42 $, against 1.26 $ for the ten delegated subtasks (figures in KT-909). The
only judgement in that loop is reading a delivery against its DoD.

No workflow step can delegate: `StepType` has no variant that drives a task
execution. `[src: file: backend/src/models/workflows.rs:890-963]`

## 2. What already exists (and is reused as is)

- **Durable executions.** `TaskExecution` carries the worker identity, the
  attempt number, `review_rounds` / `max_review_rounds`, the integrated SHA and
  an `idempotency_key`. `[src: file: backend/src/models/orchestration.rs:764-832]`
- **Campaign runs already chain plan tasks.** A campaign reads the principal
  discussion's plan, keeps only Active, Todo, unblocked, single-project tasks
  within the concurrency, budget and escalation limits, and, with
  `auto_continue`, launches the next launchable task after each integration
  under the idempotency key `campaign-auto:<run>:<task>`.
  `[src: file: backend/src/db/orchestration.rs:979-1039]`
  `[src: file: backend/src/api/orchestration.rs:2996-3060]`
- **Worker resolution.** Explicit override, then the campaign default worker,
  then the principal discussion's identity.
  `[src: file: backend/src/db/orchestration.rs:842-866]`
- **Review contract.** `ReviewDecisionV1` with a verdict `approve |
  request_changes`, the reviewed HEAD, findings and DoD verifications.
  `decide_review` bumps the round on `request_changes`, sends the findings to
  the worker, and escalates (`Escalated`, principal solicited) when the budget
  is spent. `[src: file: backend/src/models/orchestration.rs:1793-1845]`
  `[src: file: backend/src/api/orchestration.rs:5255-5258]`
  `[src: file: backend/src/api/orchestration.rs:5274-5282]`
  `[src: file: backend/src/api/orchestration.rs:5580-5583]`
- **Reassignment** is its own operation (`reassign_execution`), and a human
  decision has `human_review`. `[src: file: backend/src/api/orchestration.rs:9351]`
  `[src: file: backend/src/api/orchestration.rs:12210]`
- **Integration saga.** `run_integration` integrates an `Approved` execution
  into `run.target_branch`, through the single worktree where that branch is
  checked out, refusing a dirty checkout, and sends a conflict back to the
  worker. `[src: file: backend/src/api/orchestration.rs:2215-2420]`
  `[src: file: backend/src/core/worktree.rs:1298-1318]`
- **Workflow side.** `on_result` routes on a `[SIGNAL: X]` line among the last
  five of the step output; an Agent step can join a room through `room_id`
  (KT-793); `SubWorkflow` foreach records durable per-item markers.
  `[src: file: backend/src/workflows/steps.rs:1882-1899]`
  `[src: file: backend/src/models/workflows.rs:695]`
  `[src: file: backend/src/workflows/sub_workflow_step.rs:631-648]`

So the step does not need a new orchestration engine: it needs a **mechanical
principal** for a campaign, plus a **reviewer** that is not a long-lived room
agent.

## 3. The audit point: discussion, target workspace, integration target

The 0.14.3 audit says an orchestration run requires a discussion and a target
workspace, and that integration targets a discussion workspace, not the run's
branch. Checked against the code:

- `OrchestrationRun.discussion_id` is mandatory; `target_workspace_id` is
  `Option` and `run_integration` never reads it.
  `[src: file: backend/src/models/orchestration.rs:721-728]`
- Integration targets `run.target_branch`, through the unique worktree that has
  it checked out; the run needs a `project_id`.
  `[src: file: backend/src/api/orchestration.rs:2281-2338]`
- Every `task_exec_*` route requires the caller to be an active member of the
  principal discussion (`principal_is_authorized`), and the review obligation
  and the escalation are messages dispatched to the principal agent of that
  discussion. `[src: file: backend/src/api/orchestration.rs:9753]`
  `[src: file: backend/src/api/orchestration.rs:1404-1418]`
- A workflow run works in its own worktree on branch
  `kronn/<workflow>/<run8>`. `[src: file: backend/src/workflows/workspace.rs:593-597]`

So the real constraints are: a discussion that owns the campaign, a pinned
target branch with exactly one clean checkout, and an authorized principal.
The "target workspace" part of the finding is inaccurate today.

**Proposal.**

1. **Owner discussion.** The step takes a `room_id` (templated, like the Agent
   step). Without it, the step creates one technical discussion per run
   (`<workflow> · run <run8>`), linked to the run, and links the parent task to
   it. The campaign is created on that discussion; the plan relations are the
   parent task's subtasks (the step adds them to the plan if missing, Active,
   in their rank order, blockers kept).
2. **Integration target = the run's branch.** The campaign's `target_branch` is
   the workflow run's branch, and the run's worktree is its single checkout, so
   `integration_target_worktree` already finds it and approved deliveries land
   on the run's branch, which later steps (tests, PR) consume. The step
   commits or refuses before starting if the run worktree is dirty (the saga
   would hold every integration otherwise). A step that targets another
   branch must name it explicitly (`target_branch`), with the same
   one-checkout rule.
3. **Mechanical principal.** The step acts as a backend principal: it calls
   the provisioning, review and integration functions directly, not the HTTP
   routes, with an actor that is recorded in the journal (`workflow-step:<run>:
   <step>`). `principal_is_authorized` stays unchanged for agents; the step
   never goes through it. The campaign's escalation solicitation is replaced by
   the step's own stop (section 5), so nothing waits for a room agent.

## 4. Step contract

```text
StepType::DelegateSubtasks
  delegate.parent_task        templated task ref, e.g. {{steps.guard.data.taskId}}
  delegate.room_id            optional owner discussion (else one per run)
  delegate.target_branch      optional; default = the run's branch
  delegate.worker_map         tag → worker, e.g. {"worker:haiku": {agent, tier, model}}
  delegate.default_worker     used when no tag matches
  delegate.concurrency        max concurrent executions (campaign field)
  delegate.max_review_rounds  per subtask (campaign field, default 3)
  delegate.validations        ValidationSpec list run before integration
  delegate.reviewer           {agent, model, tier, effort, profile_id?}
  delegate.timeout_secs       whole-step bound
```

Behaviour:

1. Resolve the parent task, its subtasks, their blockers and their worker
   (first `worker:*` tag found in `worker_map`, else `default_worker`). A
   subtask with no resolvable worker stops the step before anything launches.
2. Create (or, on resume, find) the campaign with `auto_continue: true`, then
   launch the launchable subtasks with an idempotency key
   `wf:<run>:<step>:<task>` and a per-task worker override.
3. Wait for executions without an agent: event-driven on the execution state
   (the same wait as `task_exec_status`'s `wait_for`, but in-process and
   unbounded within `timeout_secs`).
   `[src: file: backend/src/api/orchestration.rs:10690-10700]`
4. On each `AwaitingReview`: start **one fresh reviewer session** with a
   compact context (task title, DoD, delivered diff against `base_sha`, the
   worker's manifest and the validation results), no room history and no MCP
   orchestration tools. Its output is a typed verdict:
   `approve | request_changes(findings) | reassign(worker?) | escalate(reason)`.
5. Apply it: `approve` and `request_changes` become a `ReviewDecisionV1`
   through `decide_review`'s logic; `reassign` goes through
   `reassign_execution`; `escalate` stops the step (section 5). Approved
   executions integrate through the existing saga, validations included.
6. Campaign `auto_continue` launches the next ready subtask after each
   integration; the step ends when the campaign is `Completed`, or stops.

## 5. Bounds, statuses and routing

The step output ends with one signal, so `on_result` can route it:

| Signal | When |
| --- | --- |
| `[SIGNAL: OK]` | every subtask integrated |
| `[SIGNAL: ESCALATED]` | a reviewer chose `escalate`, or `request_changes` past `max_review_rounds` (`Escalated`) |
| `[SIGNAL: CONFLICT]` | an integration conflict came back more than once for one subtask |
| `[SIGNAL: BLOCKED]` | no launchable subtask left while some are not done (blockers, Later, scope) |
| `[SIGNAL: TIMEOUT]` | `timeout_secs` reached |

Executions still running when the step stops are left as they are (resumable),
never cancelled implicitly. Structured data (`data.subtasks`):

```json
[{"task": "KT-12", "status": "Done", "integrated_sha": "…",
  "review_rounds": 1, "attempts": 2, "cost_usd": 0.042,
  "tokens": 18342, "worker": "claude/haiku"}]
```

`cost_usd` and `tokens` sum the messages of the execution's sub-discussion and
of its reviewer sessions (`Message.cost_usd`); `TaskExecution` has no cost
field today. `[src: file: backend/src/models/discussions.rs:360]`

## 6. Resume and cleanup

- The executions, the campaign and their journal are the durable state; the
  idempotency keys make a replayed launch return the existing execution.
- A reviewer session is keyed by `(execution, attempt_no)`: a restart during a
  review starts a new session for the same attempt only if no decision was
  recorded for it.
- A run that restarts `Interrupted` re-enters the step, which re-reads the
  campaign instead of relaunching.
- Two worktree lifecycles coexist: the workflow run's and each execution's
  managed worktree. Execution worktrees are already removed by the saga after
  integration; the workflow janitor (KT-985) must exclude a run whose campaign
  still has active executions.

## 7. Tests the implementation must bring (from the KT-909 DoD)

1. A 3-subtask plan reaches integration with fake workers and no orchestrator
   agent.
2. Each delivery triggers exactly one fresh reviewer session whose context
   holds the DoD, the diff and the worker's report.
3. `request_changes` returns to the worker with the findings; the round cap,
   then `escalate`, stop the step with a routable signal.
4. Order and blockers are respected; an integration conflict is reported.
5. The output lists, per subtask, status, integrated SHA, rounds and cost.

## 8. Open decisions

1. **Owner discussion**: a technical discussion per run, or a mandatory
   `room_id`? A per-run discussion keeps the step self-contained but adds
   discussions to purge with the run (KT-984).
2. **Parent task or discussion plan**: drive the parent task's subtasks (the
   step writes them into the plan), or require the plan to be prepared by an
   earlier step?
3. **Reviewer transport**: a one-shot agent call (cheapest, no room) or a
   sub-discussion per review (inspectable in the UI, more rows)?
4. **`reassign` and `escalate` in the review contract**: extend
   `ReviewVerdict` (shared with agent principals) or keep them as step-only
   verdicts mapped onto `reassign_execution` and a stop?
5. **Dirty run worktree before delegation**: commit automatically, or refuse
   and let the workflow add a commit step?
6. **CLI workers**: V1 limited to native and HTTP workers (a CLI worker needs a
   joined session to accept its offer), or support them?
7. **Escalation path**: stop the step only, or also notify the room / the
   campaign's `escalation_notify_url`?
8. **Budget**: reuse the campaign `token_budget`, the workflow run's shared
   budget, or both (the stricter wins)?
