# Delegate a planning task to an agent

Kronn 0.11.0 can run a planning task through a durable worker lifecycle instead
of leaving delegation as an informal chat instruction. One execution owns one
task, one worker, one child discussion and one isolated Git worktree. The parent
discussion remains the control room: it follows progress, reviews the delivery
and decides whether the candidate may reach the target branch.

The architecture and state-machine contract live in
[`adr-002-orchestration-multi-agent.md`](../design/adr-002-orchestration-multi-agent.md).
This page is the operator workflow.

## Before launching

1. Link the task to the parent discussion and give it a concrete Definition of
   Done. The worker receives the task description and this checklist as its
   brief.
2. Attach the discussion to a registered project repository. Kronn refuses to
   provision an unmanaged or unresolved path.
3. Commit or intentionally preserve the parent workspace changes. The child is
   pinned to the selected base SHA; it never edits the parent checkout.
4. Confirm that the chosen agent is installed and authenticated. HTTP agents
   such as Ollama, LiteLLM and NVIDIA can also be selected when configured.

## Launch

Open the discussion plan, select an actionable task and choose **Run with an
agent**. The launch dialog makes the campaign policy explicit:

- agent, model and optional profile;
- project workspace and target branch;
- validation commands, one per line;
- maximum review rounds;
- allowed fallback agents;
- automatic continuation to the next ready task, off by default.

The 0.11.0 defaults are conservative: one active execution per campaign, one
CLI execution at a time, three review rounds, fast-forward-only integration and
preservation of failed or cancelled worktrees. A token budget is optional; no
value (`null`) means that Kronn enforces no campaign token ceiling, so operators
who require a hard cap must set one through the campaign API/MCP before launch.

Validation commands belong to the principal's launch policy. Kronn persists
them on both campaign runs and the implicit run created by a single-task
`task_exec_launch`; a worker manifest can report observations but cannot add,
remove or replace these gates.

Task orchestration is available by default in 0.11.0; there is no hidden feature
flag. Nothing starts automatically until an operator launches a task, and
`auto_continue` remains off unless the campaign explicitly enables it.

After launch, Kronn creates a child discussion and a managed worktree below its
managed worktree root. The execution card exposes the exact worker, model,
branch, worktree, elapsed time, tokens, cost and review state.

## Worker isolation

The managed worktree is only the worker's destination; Kronn also enforces the
execution boundary according to the worker transport:

- HTTP workers (Ollama, LiteLLM and NVIDIA) never receive a shell or a host
  filesystem path. They can inspect and mutate only through Kronn's bounded
  workspace tools (`search_text`, ranged reads, hash-guarded edits and mediated
  commits). This is the preferred path for cheap/local subtasks: a model cannot
  escape the worktree merely by emitting an absolute path or a destructive
  command.
- When the principal has already verified one exact file and inclusive range,
  it can launch a `prelocalized_edit` scope. Kronn then freezes the HTTP worker
  to one authoritative read and one CAS-bound `edit_lines`; a re-read, wider
  range or different path is refused before the tool executor. A successful
  edit exposes only commit, then delivery. The worker reports semantic evidence
  in Definition-of-Done order; Kronn injects opaque DoD ids, task identity, HEAD
  and committed file inventory from trusted state. This is the preferred
  contract for truly atomic local-model edits, not for discovery or cross-file
  reasoning.
- For a pure insertion, the principal uses
  `prelocalized_insert_after` with one verified `anchor_line` instead. The
  worker supplies only the new text; Kronn preserves the anchor bytes
  mechanically and refuses a stale receipt or a different target. Prefer this
  over asking a small model to reproduce an existing paragraph inside an
  `edit_lines` replacement.
- Claude Code workers run with Claude's OS-level sandbox enabled and required.
  The unsandboxed escape hatch and global permission bypass are disabled, and
  user/project settings are ignored so they cannot add writable directories.
  Normal discussions keep their existing permission policy.
- Spawned host CLI workers keep their native shell/editing capability, but use
  the same projected delivery contract as HTTP workers: they author semantic
  evidence only, while Kronn injects the task/Git/DoD mechanics. Joined CLI
  sessions remain on the full public DeliveryManifest contract.
- Codex workers explicitly use `workspace-write` while ignoring user config and
  exec-policy files that could widen writable roots. A discussion-level
  full-access preference never upgrades a spawned worker to
  `danger-full-access`.
- Other CLI adapters never inherit their global `yolo`, `trust all` or
  `allow-all-tools` switches in task-worker mode. If their non-interactive CLI
  cannot proceed without such a bypass, the task fails visibly and can be
  reassigned; Kronn does not trade confinement for apparent progress.

Sandbox startup failure is a launch failure, not a reason to run unsandboxed.
Likewise, delivery is accepted only through the execution-scoped capability and
against the exact managed branch/HEAD.

## Follow and communicate

Open the child discussion from the execution card to inspect the worker's live
messages and tool traces. Continue coordinating in the parent discussion:
Kronn routes worker delivery and attention messages back to the parent without
requiring every agent to sit in a blocking room wait.

The execution is durable across browser reloads and backend restarts. A restart
reconciles persisted execution, dispatch and worktree state; it does not infer
success from a stale `Running` row. When Kronn cannot prove the safe next step,
the execution becomes interrupted, blocked or escalated with a reason instead
of silently continuing.

## Review and integrate

When the worker delivers, the execution enters **Awaiting review**. Inspect the
delivery manifest and diff, then choose one of:

- **Approve** — record a result and non-empty evidence for every DoD item against
  the delivered attempt and exact HEAD. Kronn then builds and validates that
  candidate in an ephemeral integration worktree. The parent branch advances
  only by a guarded fast-forward after a clean-worktree check, a target-SHA
  compare-and-swap and a backup ref.
- **Request changes** — enter concrete feedback. The same worker, discussion,
  branch and worktree resume for the next bounded review round.

Review-budget exhaustion escalates to the parent; it never converts a rejected
candidate into an approval.

Worker-written DoD claims, a checked box in the live Planning task and a
principal approval without attempt-scoped evidence are all insufficient. This
keeps local workers useful without asking the model that produced the change to
be its own quality gate.

## Stop or reassign

**Stop** cancels live dispatch and preserves the child branch/worktree for
inspection. It does not force-delete local work.

**Reassign** is available only in resumable worker states. Choose another
allowed agent/model; Kronn keeps the task, child discussion and Git lineage so
the replacement can continue from the same evidence. Reassignment is the normal
response to provider quota/rate limits. Provider, tier, explicit model and
profile change together; Kronn refuses the reassignment if it cannot update the
durable child discussion that the runtime will actually execute.

## Resolve common failures

| Symptom | Safe response |
| --- | --- |
| Agent quota or authentication failure | Reassign to another allowed agent, or retry after quota renewal. |
| Validation failed | Open the child discussion, fix the reported command, deliver again. |
| Target branch moved | Refresh/relaunch against the new target; Kronn will not overwrite it. |
| Dirty parent worktree | Commit or move the changes intentionally, then retry integration. |
| Missing workspace | Restore/register the project workspace; do not edit execution rows manually. |
| Interrupted after restart | Read the persisted reason, verify the worker/worktree, then use the guarded resume/reassign action. |
| Review budget exhausted | Decide in the parent discussion whether to expand scope, create a follow-up task or stop. |

Do not repair orchestration by manually changing SQLite rows. The status,
dispatch, discussion and Git lineage form one audited state machine; a manual DB
change can make an unsafe transition look valid.

## Back up and roll back a 0.10.0 installation

Before first starting 0.11.0 against a persistent database, make a consistent
SQLite backup. Kronn automatically writes `<database>.db.backup` when pending
migrations exist, after checkpointing the WAL. For an operator-controlled copy,
use the Settings backup action or SQLite's online `.backup` command documented
in [`db-backup.md`](../operations/db-backup.md).

Database migrations are forward-only. Rolling the application binary back to
0.10.0 does not roll the database schema back. If application rollback is
required, stop Kronn and restore the pre-upgrade database **and** its matching
configuration backup before starting 0.10.0. Keep the failed upgraded database
for diagnosis.

Migration `127_task_orchestration` is frozen: it has run on persistent 0.11.0
databases and must never be edited in place. Every later orchestration schema
change uses migration 128 or above. The migration runner records names in
`_migrations`; an edited migration 127 would not replay and `IF NOT EXISTS`
would hide the drift.
