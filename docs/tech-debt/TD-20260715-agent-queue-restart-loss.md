# TD-20260715-agent-queue-restart-loss

- **ID**: TD-20260715-agent-queue-restart-loss
- **Area**: Backend / Agents (spawn lifecycle, batch QP, ops)
- **Problem (fact)**: All pending/in-flight **agent work lives in memory only** — a
  backend restart loses it without trace, and deleting a batch does not stop its
  agents. Incident #1 (2026-07-15, disc `1306b6c4-9168-481d-b2eb-9f9a82fea378`):
  1. **Restart wipes the queue.** Batch-QP fan-out is a detached `tokio::spawn`
     (`backend/src/workflows/batch_step.rs:263`), and message-triggered agent runs
     are the same pattern (`backend/src/api/discussions/runtime.rs:24`,
     `spawn_agent_run_background` → detached `tokio::spawn`). A `cargo watch`
     rebuild at 13:29 killed 23 queued triage responses + 1 pending discussion
     reply — no error, no `Interrupted` marker, nothing to reconcile at boot.
     The boot-reconcile pass that exists for workflow **runs** (`Interrupted`
     status) does not cover these agent spawns.
  2. **Deleting a batch leaks its agents.** The agents of a deleted batch kept
     running to completion (~12:17-12:21), writing into deleted discs →
     serial `FOREIGN KEY constraint failed` + `batch progress bump blocked —
     batch no longer Running` (run `cce0ba09`), while occupying concurrency
     slots that starved legitimate work. The per-step `cancel_token` exists
     (`backend/src/agents/runner.rs:1999`) but batch/disc deletion does not
     propagate any cancellation to child agent processes.
  3. **In-flight agent work is invisible.** During an Agent step,
     `workflow_run_status` reports `current_step: null`, omits the running step
     from `steps[]` and shows `tokens_used: 0` — a healthy 11-min agent run is
     indistinguishable from a zombie. Same blindness in the UI: 23 identical
     loaders, queued vs running indistinguishable, no runner presence.
- **Why we can't fix now (constraint)**: Persistence + reconcile of the agent
  queue is a lifecycle feature (DB table for pending spawns, boot re-enqueue,
  idempotence guarantees), cancellation needs plumbing from the delete paths
  down to child processes, and the observability gap spans API + UI. Sequenced
  after the 0.9 campaign passes; too broad for a hotfix.
- **Impact**: correctness (user work silently lost on every rebuild/restart) ·
  resource waste (zombie agents burn concurrency slots + tokens into deleted
  discs) · observability (no way to tell queued/running/dead apart).
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
  - **T2 — cancel on delete**: batch/disc deletion propagates cancellation to
    child agents (kill the process, free the slot, mark the child cancelled).
  - **T3 — visibility**: `workflow_run_status` must expose the in-flight Agent
    step (name, elapsed, streaming tokens); UI distinguishes "queued (n/23)"
    from "running" and shows runner presence.
  - Graceful shutdown complements T1: on SIGTERM stop accepting spawns, flush
    state, then exit — makes even voluntary restarts lossless.
- **Next step**: create ticket (T1/T2 P1, T3 P2 — tickets drafted in release
  room `3f603a34`, msgs 499-500).

## Notes

- Full incident timeline + diagnosis: disc "INCIDENT #1 — file agent perdue au
  restart + batch zombies (2026-07-15)". Post-restart the pipeline itself was
  verified healthy end-to-end (PR-review run `deca31e6` and the relaunched
  triage batch `548479a9` both spawned and completed normally) — the loss is
  purely the absence of persistence/cancellation, not a spawn bug.
- Related: TD-20260630-workflow-run-restart-resilience (same root cause one
  layer up — workflow runs; its `Interrupted` + boot-reconcile now exists and
  is the pattern to generalize).
