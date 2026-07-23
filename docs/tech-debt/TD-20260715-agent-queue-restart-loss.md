# TD-20260715-agent-queue-restart-loss

- **ID**: TD-20260715-agent-queue-restart-loss
- **Area**: Backend / Agents (spawn lifecycle, batch QP, ops)
- **Status**: PARTIAL — lifecycle visibility and cancellation shipped in
  0.8.12; persistent re-enqueue remains open.
- **Problem (fact)**: Pending/in-flight **agent work still lives in memory**.
  Incident #1 (2026-07-15, disc `1306b6c4-9168-481d-b2eb-9f9a82fea378`):
  1. **Restart wipes the queue.** Batch-QP fan-out is a detached `tokio::spawn`
     (`backend/src/workflows/batch_step.rs:263`), and message-triggered agent runs
     are the same pattern (`backend/src/api/discussions/runtime.rs:24`,
     `spawn_agent_run_background` → detached `tokio::spawn`). A `cargo watch`
     rebuild at 13:29 killed 23 queued triage responses + 1 pending discussion
     reply. Since 0.8.12 an `awaiting_agent` marker survives the restart and the
     boot reconcile appends an interruption notice, but the work is deliberately
     not re-spawned. [src: file: backend/src/db/discussions.rs:90-161]
- **Shipped mitigation (0.8.12)**:
  - batch/discussion deletion propagates cancellation before deleting rows;
  - queued and running batch children are distinct WS/UI states;
  - owed work is DB-backed and a boot reconcile makes interruption visible.
  These close the original zombie and observability defects, but not lossless
  restart recovery. [src: file: backend/src/api/workflows.rs:2003-2071]
  [src: file: backend/src/workflows/batch_step.rs:209-217]
- **Why we can't fix now (constraint)**: Persistence + reconcile of the agent
  queue is a lifecycle feature (DB table for pending spawns, boot re-enqueue,
  idempotence guarantees), cancellation needs plumbing from the delete paths
  down to child processes, and the observability gap spans API + UI. Sequenced
  after the 0.9 campaign passes; too broad for a hotfix.
- **Impact**: correctness and operator friction: interrupted work is now visible
  and recoverable manually, but a restart still requires a relaunch.
- **Where (pointers)**:
  - `backend/src/workflows/batch_step.rs:263` — batch fan-out `tokio::spawn`
    (fire-and-forget, no persisted queue).
  - `backend/src/api/discussions/runtime.rs:24` — `spawn_agent_run_background`
    (same pattern for message-triggered replies).
  - `backend/src/agents/runner.rs:1946` — `Spawning agent` (the observable spawn
    point); `runner.rs:1999` — existing step-level `cancel_token` to reuse.
  - `backend/src/db/workflows.rs` — batch run rows (`create_batch_run`), the
    natural anchor for a persisted pending-spawn set.
- **Suggested direction (non-binding)**:
  - **Ops mitigation first (no backend code)**: a `kronn serve` mode that runs a
    **copy** of the compiled binary (e.g. `~/.local/share/kronn/bin/kronn-stable`)
    without `cargo watch`, so dev edits/rebuilds in the repo never restart the
    serving instance. Today `kronn start-dev` → `make dev-backend` →
    `cd backend && cargo watch -x run` (`Makefile:232-234`) is the only native
    path, i.e. the "production" instance is a hot-reload dev instance.
  - **T1 — persist + reconcile**: persist pending agent work (batch children not
    yet answered, message-triggered replies) and re-enqueue at boot, generalizing
    the existing `Interrupted`-runs reconcile pattern.
  - Graceful shutdown complements T1: on SIGTERM stop accepting spawns, flush
    state, then exit — makes even voluntary restarts lossless.
- **Next step**: design the persisted spawn record and idempotent boot claim;
  retain the 0.8.12 interruption notice as the fail-closed fallback.

## Notes

- Full incident timeline + diagnosis: disc "INCIDENT #1 — file agent perdue au
  restart + batch zombies (2026-07-15)". Post-restart the pipeline itself was
  verified healthy end-to-end (PR-review run `deca31e6` and the relaunched
  triage batch `548479a9` both spawned and completed normally) — the loss is
  purely the absence of persistence/cancellation, not a spawn bug.
- The workflow-run layer already has `Interrupted` + boot reconcile and remains
  the pattern to generalize.
