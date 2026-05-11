# TD-20260512 — Exec step runs on main tree by default (worktree discoverability)

## Context

`StepType::Exec` (shell allowlist-gated, shipped 0.7.0 Phase 5) is wired
to run in the workflow's computed `work_dir` (`runner.rs:152`). When the
workflow has `workspace_mode = Worktree`, that path points at a fresh
git worktree where the Agent step's edits land — `Exec` correctly runs
against the modified code. ✅

When `workspace_mode` is unset (default for workflows created via the
wizard's "Simple" mode), `work_dir = project.path` — the main checkout.
Agent edits land on whatever branch the user has checked out, then Exec
runs `make test` on that same tree. Often what the user wants for read-
only audits, BUT silently broken for the "edit + test in isolation"
pattern (the typical autoBot / PR-draft flow):

1. User opens a workflow: `ApiCall fetch ticket → Agent draft impl →
   Exec cargo test → Notify`.
2. Workflow runs. Agent edits files on `main`. Exec runs `cargo test` on
   `main` with the agent's uncommitted edits sitting in the working
   tree. Test result is for the agent's edits — fine.
3. BUT: if the user has any in-flight work on `main`, it gets mixed
   with the agent's edits. The "test passes" signal is contaminated.
4. Worse: if the user wants to inspect the diff before merging, they
   can't `git diff` cleanly because their own work is intermingled.

The fix: surface `workspace_mode = Worktree` as the obvious default
for any workflow that pairs an Agent step (writes files) with an Exec
step (reads files). User reported on 2026-05-12: "il faut bien tester
le code modifié, pas celui de main".

## Why we can't fix now (constraint)

Picked up at the end of the 0.8.1 cycle, after the release was already
"complete enough" per user assessment. Three viable approaches (auto-
enable / wizard banner / per-Exec opt-in) — each has trade-offs that
need a UX call. Better to land in 0.8.2 with the right approach than
in 0.8.1 with a rushed one.

## Impact

- **dev friction** : users testing autoBot-style workflows get
  contaminated test results when they have local in-flight work.
- **correctness** : passing tests on main+overlay aren't a strict
  signal that the agent's edits alone pass — false-positive risk
  on CI gates that consume the workflow's exit code.

## Where (pointers)

- `backend/src/workflows/runner.rs:114-152` — `workspace_mode` →
  `work_dir` computation. Already worktree-aware, no backend change
  needed if we pick option A or B.
- `frontend/src/components/workflows/WorkflowWizard.tsx` — "Simple"
  mode doesn't expose the worktree toggle. The "Advanced" mode does
  (via the `workspace_mode` selector on the workflow form).
- `backend/src/models/workflows.rs:296` (`batch_workspace_mode`) +
  `backend/src/workflows/workspace.rs:164` — the existing per-step
  override path for batch workflows; would be the pattern for
  per-Exec-step opt-in (option C).
- `backend/src/workflows/exec_step.rs:96-104` — already surfaces
  "no work_dir" diagnostic; could be enriched to suggest the worktree
  toggle.

## Suggested direction (non-binding)

Probably **option B** (info banner in the wizard when the workflow has
both an Agent step and an Exec step and `workspace_mode` is unset).
Non-intrusive, educational, doesn't force the worktree on workflows
that intentionally read main (audit-only, reporting workflows). User
clicks the banner → wizard switches to Advanced and pre-selects
`workspace_mode = Worktree`.

Option A (auto-enable) is tempting but breaks the "I want to audit
main as-is" use case. Option C (per-Exec opt-in) adds form surface
without saving the common case.

## Next step

Create ticket in 0.8.2 sprint. Schedule a quick UX call (2 mockups,
30 min) before implementing.

## Status

Draft — captured 2026-05-12 from user feedback at end of 0.8.1 cycle.

## Effort

M (1-4h) — wizard banner + workflow form check + i18n.

## Blast radius

local (1 file `WorkflowWizard.tsx` + i18n keys) — no backend change for
option B.
