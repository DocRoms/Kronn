# TD-20260715-agent-queue-restart-loss

- **ID**: TD-20260715-agent-queue-restart-loss
- **Area**: Backend / Agents (spawn lifecycle, batch QP, ops)
- **Status**: RESOLVED in 0.9.2 — durable dispatch queue, boot re-enqueue,
  idempotent claim and tracked completion.
- **Problem (historical)**: Pending/in-flight agent work lived only in memory.
  Incident #1 (2026-07-15, disc `1306b6c4-9168-481d-b2eb-9f9a82fea378`):
  1. **Restart wipes the queue.** Batch-QP fan-out is a detached `tokio::spawn`
     (`backend/src/workflows/batch_step.rs:263`), and message-triggered agent runs
     are the same pattern (`backend/src/api/discussions/runtime.rs:24`,
     `spawn_agent_run_background` → detached `tokio::spawn`). A dev-watcher
     rebuild at 13:29 killed 23 queued triage responses + 1 pending discussion
     reply. Since 0.8.12 an `awaiting_agent` marker survives the restart and the
     boot reconcile appends an interruption notice, but the work is deliberately
     not re-spawned. [src: file: backend/src/db/discussions.rs:90-161]
- **Shipped mitigation (0.8.12)**:
  - batch/discussion deletion propagates cancellation before deleting rows;
  - queued and running batch children are distinct WS/UI states;
  - owed work is DB-backed and a boot reconcile makes interruption visible.
  These closed the original zombie and observability defects, but not lossless
  restart recovery. [src: file: backend/src/api/workflows.rs:2003-2071]
  [src: file: backend/src/workflows/batch_step.rs:209-217]
- **Resolution (0.9.2)**:
  - migration 083 persists one job per accepted turn/batch child, in the same
    transaction as its trigger message;
  - atomic claims serialize a discussion and enforce batch group concurrency;
  - interactive HTTP runs retain their live SSE while a detached completion
    monitor owns persistence, cancellation and the shared power lease;
  - agent replies record their exact dispatch job id and durable success bit,
    preventing a neighbouring turn's response from satisfying a recovered job;
  - boot resets interrupted `Running` jobs to `Pending`; recovered partial
    messages are explicitly excluded from completion detection;
  - QP chains advance their trigger transactionally, silent crashes retry once,
    and a global attempt ceiling dead-letters repeated restart failures;
  - MCP QP/batch launches and joined-peer replies use the same durable queue.
  [src: file: backend/src/db/agent_dispatch.rs]
  [src: file: backend/src/api/discussions/runtime.rs]
  [src: file: backend/src/db/sql/083_agent_dispatch_jobs.sql]
- **Impact**: backend/dev restarts no longer silently lose accepted agent work;
  queued turns resume automatically and already-persisted replies are not
  regenerated.
- **Where (pointers)**:
  - `backend/src/workflows/batch_step.rs:263` — batch fan-out `tokio::spawn`
    (fire-and-forget, no persisted queue).
  - `backend/src/api/discussions/runtime.rs:24` — `spawn_agent_run_background`
    (same pattern for message-triggered replies).
  - `backend/src/agents/runner.rs:1946` — `Spawning agent` (the observable spawn
    point); `runner.rs:1999` — existing step-level `cancel_token` to reuse.
  - `backend/src/db/workflows.rs` — batch run rows (`create_batch_run`), the
    natural anchor for a persisted pending-spawn set.
- **Implemented direction**:
  - **Ops mitigation first (no backend code)**: a `kronn serve` mode that runs a
    **copy** of the compiled binary (e.g. `~/.local/share/kronn/bin/kronn-stable`)
    without the watcher, so dev edits/rebuilds in the repo never restart the
    serving instance. Today `kronn start-dev` → `make dev-backend` →
    `cd backend && watchexec --restart … -- cargo run` (`Makefile:232-234`) is the only native
    path, i.e. the "production" instance is a hot-reload dev instance.
  - **T1 — persist + reconcile** is complete via `agent_dispatch_jobs`.
  - Graceful shutdown remains a complementary improvement, not a prerequisite
    for lossless queue recovery.
- **Next step**: keep the 0.8.12 interruption notice only as the fail-closed
  fallback for legacy/non-dispatch runs.

## Notes

- Full incident timeline + diagnosis: disc "INCIDENT #1 — file agent perdue au
  restart + batch zombies (2026-07-15)". Post-restart the pipeline itself was
  verified healthy end-to-end (PR-review run `deca31e6` and the relaunched
  triage batch `548479a9` both spawned and completed normally) — the loss is
  purely the absence of persistence/cancellation, not a spawn bug.
- The workflow-run layer already has `Interrupted` + boot reconcile and remains
  the pattern to generalize.
