# Audit run states and the action each one offers

This page lists the states an audit run (`audit_runs.status`) can be left in,
how each one is reached, and the action the audit timeline offers in it. Rule
(KT-1134, KT-1169): no state leaves the user without an action, and nothing
reads "running" once the run has stopped.

## States of a run

| Status | Reached by | Left by |
|---|---|---|
| `Running` | A launch takes the project's audit lease, then inserts the row. | The terminal write, a cancel, the drop guard, the boot reconcile, the cleanup action. |
| `Completed` | Every step succeeded, was `unchanged`, or was accepted, and the run wrote its drift baseline and validation discussion in the same transaction. | Terminal. A new run supersedes it. |
| `Interrupted` | At least one step is left to redo, or the run stopped early (drop guard, restart, cleanup). | Resume (every failed step), single-step retry, or acceptance then finalization. Each one is a new run that carries the done steps. |
| `Cancelled` | The worker acknowledged a cancel. | Terminal. A new run supersedes it. |
| `Failed` | Catastrophic failure of every step. | Terminal. A new run supersedes it. |

Sources:

- The lease is the single gate of every launch. [src: file: backend/src/api/audit/full.rs:357]
- The row is inserted `Running`. [src: file: backend/src/db/audit_runs.rs:18]
- `Completed` or `Interrupted`, with the validation discussion, is one transaction. [src: file: backend/src/api/audit/full.rs:3202]
- `Interrupted` only moves a `Running` row. [src: file: backend/src/db/audit_runs.rs:500]
- The worker marks a cancel before it acknowledges it. [src: file: backend/src/api/audit/full.rs:1107] [src: file: backend/src/db/audit_runs.rs:160]
- At boot, every `Running` row is a zombie and becomes `Interrupted`. [src: file: backend/src/db/mod.rs:281] [src: file: backend/src/db/audit_runs.rs:534]
- A run is resumable when it is the project's latest, `Interrupted`, with at least one done step. [src: file: backend/src/db/audit_runs.rs:613]

## What counts as a done step

A step is done when it succeeded, ended `unchanged` (KT-1130), or was accepted
by a human (KT-1131). An accepted step keeps its failure on record:
`cli_success` stays false and `step_warning` keeps the reason. It gains
`accepted_at`, `accepted_by` and `accepted_reason`.

- Done steps, as a resume reads them. [src: file: backend/src/api/audit/full.rs:3280]
- Per-run outcomes keep `succeeded`, `unchanged`, `accepted` and `failed` apart. [src: file: backend/src/api/audit/full.rs:3092]

## Recovering from failed steps

- **Resume** replays every failed step of the latest interrupted run in a new
  run. That run carries the done steps.
- **Retry one step** (`retry_steps` with `resume_run_id`) replays only the
  named steps. The other failed steps are copied as they ended and are not
  run (`step_skipped`, reason `not_retried`).
  [src: file: backend/src/api/audit/full.rs:3316] [src: file: backend/src/api/audit/full.rs:1162] [src: file: backend/src/db/audit_runs.rs:710]
- **Accept as it stands** is
  `POST /api/projects/{id}/audit-runs/{run_id}/steps/{step}/accept`. It is a
  human action only: the route is not on the bridge list, and the handler
  refuses any bridge caller. It is refused when the target is missing, empty
  or still holds template placeholders. It is also refused when the run is not
  the latest `Interrupted` run, or while the audit lease is held.
  [src: file: backend/src/lib.rs:1898] [src: file: backend/src/api/audit/accept.rs:93] [src: file: backend/src/api/audit/accept.rs:48] [src: file: backend/src/db/audit_runs.rs:742]
- Once no failed step is left, the resume runs no agent step and only
  finalizes the run. It goes to `Completed`, writes its validation discussion,
  which lists the accepted steps apart, and makes the project validatable.
  The validate gate still requires the latest run to be `Completed`.
  [src: file: backend/src/api/audit/validate.rs:143]

## Leases and agents that could block a project

- **Orphaned lease.** The drop guard settles a dropped run as `Interrupted`
  and then releases the lease. If the database write still fails after its
  retries, the lease stays held (fail-closed). It is also named in
  `AuditTracker::orphaned_leases`. The timeline reports it as
  `lease_orphaned`. The cleanup action (`POST /api/audit-runs/cleanup`) never
  touches a live audit. It settles the stale `Running` rows and releases the
  orphaned leases.
  [src: file: backend/src/api/audit/full.rs:162] [src: file: backend/src/lib.rs:265] [src: file: backend/src/api/audit/run.rs:210] [src: file: backend/src/api/audit/run.rs:106] [src: file: backend/src/db/audit_runs.rs:572]
- **Validation agents.** Before an audit touches `docs/`, it stops any agent
  still answering in a validation discussion linked to one of the project's
  runs. While an audit of the project shows progress, such an agent is not
  started: its dispatch goes back to the queue with its attempts intact. A
  run clears its progress before it spawns its own validation agent.
  [src: file: backend/src/api/audit/full.rs:630] [src: file: backend/src/api/audit/full.rs:226] [src: file: backend/src/api/discussions/runtime.rs:493] [src: file: backend/src/api/discussions/runtime.rs:1166] [src: file: backend/src/api/audit/full.rs:2881]
- **Cancel.** A cancel waits for the worker to acknowledge it. Without that
  acknowledgement it answers with an error, and the UI keeps the audit shown,
  marked "cancel pending". It deletes only the validation discussion linked to
  the run it cancelled, never one an earlier run left behind.
  [src: file: backend/src/api/audit/full.rs:3705] [src: file: backend/src/api/audit/full.rs:3592]

## The action offered in each state

| State (as the timeline sees it) | Primary action | Other actions |
|---|---|---|
| Never audited | Launch the audit | — |
| Running | Cancel | — |
| Cancel not acknowledged | Retry the cancel | The status poll clears it when the run stops. |
| `Interrupted`, steps to redo (first audit or re-audit) | Resume the failed steps | Per failed step: retry it alone, or accept it as it stands. Run the full audit again, behind a confirmation that offers the first two. The validation phase points to the same resume. |
| `Interrupted`, every failed step accepted | Finalize the audit | — |
| `Interrupted` with nothing done (restart during step 1) | Launch the audit | — |
| `Cancelled` | Run the audit again | Validate, when the project was already audited. |
| `Completed`, not validated | Validate | Run the audit again. |
| Validated | Run the audit again | Review the tech debt. |
| Orphaned lease | Unlock (cleanup) | Launch stays disabled until then. |

The validation phase never reads "running" while an audit runs.

- Step status, accepted included. [src: file: frontend/src/components/AuditTimeline.tsx:145]
- Launch label: resume, finalize, run again. [src: file: frontend/src/components/AuditTimeline.tsx:374]
- Per-step actions only on the resumable run. [src: file: frontend/src/components/AuditTimeline.tsx:382]
- Validation phase is not running during an audit. [src: file: frontend/src/components/AuditTimeline.tsx:385] [src: file: frontend/src/components/ProjectCard.tsx:544]
- Validation phase hands over to the run. [src: file: frontend/src/components/AuditTimeline.tsx:753]
- Confirmation before a fresh full audit. [src: file: frontend/src/components/AuditTimeline.tsx:912]
- Orphaned lease banner. [src: file: frontend/src/components/AuditTimeline.tsx:533]
- Cancel pending. [src: file: frontend/src/components/AuditTimeline.tsx:530] [src: file: frontend/src/components/ProjectCard.tsx:713]
