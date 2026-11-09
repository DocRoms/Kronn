# Cap on concurrent local task workers (KT-1632)

`max_concurrent_agents` counts agent **processes**; it does not bound the builds
and test runs behind them, so 2 to 4 task workers plus validations could load a
machine to 26–32. `workers.max_concurrent_task_workers` (Settings > Workers,
default 2, 1–64) is a fixed cap on the **local task workers** running at once.

## What counts

A *local task worker* is a `Running` row of `agent_dispatch_jobs` whose
discussion is a `task_executions.sub_discussion_id` (not a joined `cli` worker)
and whose effective agent is a local CLI: ClaudeCode, Codex, OpenCode,
GeminiCli, Kiro, Vibe, CopilotCli. Remote HTTP routes (LiteLlm, Nvidia, Custom)
and Ollama never count and are never held back. Reviews, cancels and ordinary
discussion turns do not go through this gate.

## How it is enforced

- `db::agent_dispatch::claim_with_limits` carries a reserved `__task_workers`
  key in its limits map (added by `agents::runner::dispatch_concurrency_limits`
  from the config). The claim is the single atomic admission point, so every
  path (first launch, rework re-dispatch, resume, restart recovery) obeys it.
- The queue **is** the durable `Pending` rows: it survives a restart (the
  recovery returns `Running` rows to `Pending`) and is served in launch order
  `(created_at, rowid)`. A claim is refused while an older eligible task worker
  waits, so the racing dispatcher scan cannot reorder it. An older worker held
  by its *own* per-agent cap does not hold the queue.
- A claim refused by the cap leaves `progress_phase = 'queued'` and
  `progress_detail = 'task_worker_limit:<running>/<cap>'` on the job (written
  only when it changes).

## Where it shows

- `TaskExecutionDetail.progress`: phase `queued`, `reason` =
  `task_worker_limit:<running>/<cap>`, `queue_position` (1-based).
- `task_exec_status` compact view: a `queued` sentence.
- The room's task-execution card: `orch-exec-queued` line.
