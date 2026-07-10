# TD — Overlapping non-isolated runs: main-tree cross-contamination + checkpoint reset

**Date:** 2026-07-09
**Origin:** 0.8.11 hardening pass — codebase-wide concurrency review (finding F5).
**Severity:** RISK (requires a specific, now-rarer setup — see mitigations shipped).
**Status:** OPEN — needs a design decision, not a hot-fix.

## The problem

When worktree creation fails (and `require_isolation` is off), a run falls back
to executing **in the project's main checkout** (`runner.rs` worktree-failure
fallback). Two overlapping runs in that mode share mutable state:

1. **`.kronn/` machine files** — `manifest.json` (read by round-2 triage
   hydration), `tasks.json`, `decision_ids.txt`, `files_touched.txt` are all
   fixed paths under `work_dir`. Run A's triage can hydrate `unchanged` items
   from run B's manifest; B's `tasks.json` overwrites A's, so A's foreach fans
   out B's tasks.

2. **Gate-checkpoint `git reset --hard`** — gate checkpointing only activates
   in non-isolated mode (`runner.rs` skips it when `workspace_path.is_some()`).
   A run paused at its gate holds a checkpoint on the **shared** checkout; when
   the operator later picks *Request Changes*, `reset_to_checkpoint(proj.path)`
   hard-resets the main tree — wiping any uncommitted work another run (or the
   human) did there since the checkpoint, potentially **hours later**. The
   `StagedChangesPresent` guard exists only at *commit* time, not at reset time.

## Why it's rarer after 0.8.11

- The cron double-fire (two concurrent runs of the same workflow ~1/31
  occurrences) is fixed — the most likely source of surprise overlap is gone.
- The engine's concurrency check is now atomic with the insert, so
  `concurrency_limit: 1` is actually enforced against racing triggers.
- Cancelled is sticky and Pending/WaitingApproval runs are cancellable, so an
  operator can reliably kill one of two overlapping runs.

Remaining exposure: two *different* workflows bound to the same project, both
falling back to main-tree mode (or one manual + one cron), plus a human working
in the checkout while a gate-paused run holds a checkpoint.

## Options (pick one)

| Option | Sketch | Cost |
|---|---|---|
| A. Per-project main-tree mutex | An in-process `Mutex<HashSet<project_id>>`: a run entering main-tree mode acquires it; a second run either queues or fails fast with a clear error. | Small. Doesn't cover the human-WIP case. |
| B. Namespace `.kronn/` per run | `.kronn/runs/<run_id>/…` for machine files; triage reads its own run's dir. | Medium — every reader/writer of the fixed paths moves. Fixes contamination, not the reset. |
| C. Guard the reset | Before `reset_to_checkpoint`, refuse (→ COMMENT-style degrade) if the tree has changes NOT introduced by this run (diff vs checkpoint SHA on files the run touched). | Medium. Fixes the destructive half only. |
| D. Kill main-tree fallback | Make `require_isolation` the only mode; a failed worktree creation fails the run. | Smallest code, biggest behavior change (breaks setups where worktrees can't be created — exotic filesystems). |

**Recommendation:** A + C (cheap, orthogonal, cover both halves); B if/when
`.kronn/` machine files grow more consumers.

## Pointers

- `backend/src/workflows/runner.rs` — fallback (~:315), triage machine files
  (~:1206), checkpoint commit (~:875) and reset (~:1957).
- `backend/src/workflows/gate_checkpoint.rs` — `reset_to_checkpoint`,
  `StagedChangesPresent` (commit-time only).
- `backend/src/workflows/workspace.rs` — worktree naming (run-id suffixed —
  the isolated path is already collision-free).
