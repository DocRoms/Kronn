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

- The lease is the single gate of every launch. [src: file: backend/src/api/audit/full.rs:358]
- The row is inserted `Running`. [src: file: backend/src/db/audit_runs.rs:18]
- `Completed` or `Interrupted`, with the validation discussion, is one transaction. [src: file: backend/src/api/audit/full.rs:3239]
- `Interrupted` only moves a `Running` row. [src: file: backend/src/db/audit_runs.rs:500]
- The worker marks a cancel before it acknowledges it. [src: file: backend/src/api/audit/full.rs:1123] [src: file: backend/src/db/audit_runs.rs:160]
- At boot, every `Running` row is a zombie and becomes `Interrupted`. [src: file: backend/src/db/mod.rs:281] [src: file: backend/src/db/audit_runs.rs:534]
- A run is resumable when it is the project's latest, `Interrupted`, with at least one done step. [src: file: backend/src/db/audit_runs.rs:613]

## What counts as a done step

A step is done when it succeeded, ended `unchanged` (KT-1130), or was accepted
by a human (KT-1131). An accepted step keeps its failure on record:
`cli_success` stays false and `step_warning` keeps the reason. It gains
`accepted_at`, `accepted_by` and `accepted_reason`.

- Done steps, as a resume reads them. [src: file: backend/src/api/audit/full.rs:3313]
- Per-run outcomes keep `succeeded`, `unchanged`, `accepted` and `failed` apart. [src: file: backend/src/api/audit/full.rs:3108]

## Recovering from failed steps

- **Resume** replays every failed step of the latest interrupted run in a new
  run. That run carries the done steps.
- **Retry one step** (`retry_steps` with `resume_run_id`) replays only the
  named steps. The other failed steps are copied as they ended and are not
  run (`step_skipped`, reason `not_retried`).
  [src: file: backend/src/api/audit/full.rs:3333] [src: file: backend/src/api/audit/full.rs:1178] [src: file: backend/src/db/audit_runs.rs:710]
- **Accept as it stands** is
  `POST /api/projects/{id}/audit-runs/{run_id}/steps/{step}/accept`. It is a
  human action only: the route is not on the bridge list, and the handler
  refuses any bridge caller. It is refused when the target is missing, empty
  or still holds template placeholders. It is also refused when the run is not
  the latest `Interrupted` run, or while the audit lease is held.
  [src: file: backend/src/lib.rs:1906] [src: file: backend/src/api/audit/accept.rs:93] [src: file: backend/src/api/audit/accept.rs:48] [src: file: backend/src/db/audit_runs.rs:742]
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
  [src: file: backend/src/api/audit/full.rs:646] [src: file: backend/src/api/audit/full.rs:226] [src: file: backend/src/api/discussions/runtime.rs:493] [src: file: backend/src/api/discussions/runtime.rs:1166] [src: file: backend/src/api/audit/full.rs:2897]
- **Cancel.** A cancel waits for the worker to acknowledge it. Without that
  acknowledgement it answers with an error, and the UI keeps the audit shown,
  marked "cancel pending". It deletes only the validation discussion linked to
  the run it cancelled, never one an earlier run left behind.
  [src: file: backend/src/api/audit/full.rs:3755] [src: file: backend/src/api/audit/full.rs:3687]

## Launching from the existing state (KT-1169)

- **A launch never ignores an audit it could resume.** A Full or specialized
  launch without `resume_run_id` is refused while the project's latest run is
  resumable, unless the request carries `fresh: true`. That flag is the
  confirmed "run the whole audit again" choice. A stale-sections refresh is
  refused in the same case: it would supersede the run. The refusal names the
  run, so a bridge agent knows what to resume. A partial run, which the Full
  launch cannot resume, never refuses a launch.
  [src: file: backend/src/api/audit/full.rs:450] [src: file: backend/src/api/audit/full.rs:3349] [src: file: backend/src/api/audit/drift.rs:374]
- **The existing docs are never injected again.** The template is installed
  only on a `NoTemplate` project. On a documented repository, every launch,
  fresh included, starts from the docs as they are: a filled document gets the
  re-verification prompt (KT-1132). A documented repository that never had the
  template v2 merge is offered that merge before its first launch from the
  card (KT-1181), and never before a resume.
  [src: file: backend/src/api/audit/full.rs:711]
- **Unknown is not "nothing to resume".** When the card cannot read whether a
  run is resumable, it says so and offers "Retry". Every launch button stays
  disabled until it knows. A step retried alone never falls back to a whole
  audit when its run is no longer resumable.
  [src: file: frontend/src/components/ProjectCard.tsx:503] [src: file: frontend/src/components/ProjectCard.tsx:771] [src: file: frontend/src/components/AuditTimeline.tsx:394]

## Every error carries its action (KT-1169)

| Surface | Error | Action next to it |
|---|---|---|
| Timeline, audit phase | A launch or a resume stopped on an error: refused by the server, the stream closed, the network failed, or a step retried alone lost its run. | "Resume" replays that same launch, with the state of the moment it is clicked. A lost run is read again instead. |
| Timeline, audit phase | Whether a run can be resumed could not be read. | "Retry" reads it again. Launching waits. |
| Timeline, audit phase | The recorded runs could not be read. | "Retry" reads them again. The audit is never shown as not started. |
| Timeline, failed step | Acceptance refused, for example because the target still holds placeholders. | The form stays open with its reason. "Accept" tries again. |
| Timeline, lease | Orphaned lease. | "Unlock". |
| Timeline, live audit | Cancel not acknowledged. | "Retry the cancel". |
| Timeline, validation | Validation refused. | "Validate" stays, next to the discussion. |
| Recap panel | History or steps could not be read. | "Retry". |
| Template merge panel | Plan, start, accept or discard failed. | "Resume" replays that operation (KT-1181). |
| Card header | A stale-sections refresh failed or changed nothing. | The refresh button stays while the sections are stale. It is hidden while a run waits to be resumed, because the server refuses it then. |
| Toasts during a run | A step failed or warned. | Informational: the run ends with that step's "Retry this step" and "Accept as it stands". |

- Launch error banner. [src: file: frontend/src/components/AuditTimeline.tsx:561] [src: file: frontend/src/components/ProjectCard.tsx:948] [src: file: frontend/src/components/ProjectCard.tsx:448]
- Unknown resumable state, unreadable timeline. [src: file: frontend/src/components/AuditTimeline.tsx:580] [src: file: frontend/src/components/AuditTimeline.tsx:593]
- Refused acceptance. [src: file: frontend/src/components/AuditTimeline.tsx:408] [src: file: frontend/src/components/AuditTimeline.tsx:689]
- Recap read error. [src: file: frontend/src/components/AuditRecapPanel.tsx:324]

## The action offered in each state

| State (as the timeline sees it) | Primary action | Other actions |
|---|---|---|
| Never audited | Launch the audit | — |
| Running | Cancel | — |
| Cancel not acknowledged | Retry the cancel | The status poll clears it when the run stops. |
| `Interrupted`, steps to redo (first audit or re-audit) | Resume the failed steps | Per failed step: retry it alone, or accept it as it stands. Run the full audit again, behind a confirmation that offers the first two. The validation phase points to the same resume. |
| `Interrupted`, every failed step accepted | Finalize the audit | — |
| `Interrupted` with nothing done (restart during step 1) | Launch the audit | — |
| `Cancelled` | Run the audit again (re-verify on a documented repository) | Validate, when the project was already audited. |
| `Completed`, not validated | Validate | Run the audit again. |
| Validated | Run the audit again | Review the tech debt. |
| Orphaned lease | Unlock (cleanup) | Launch stays disabled until then. |
| Resumable state unknown | Retry the read | Launch stays disabled until then. |
| Any state, after a launch error | Resume that launch | Dismiss. |

The validation phase never reads "running" while an audit runs.

- Step status, accepted included. [src: file: frontend/src/components/AuditTimeline.tsx:151]
- Launch label: resume, finalize, run again. [src: file: frontend/src/components/AuditTimeline.tsx:384]
- Per-step actions only on the resumable run. [src: file: frontend/src/components/AuditTimeline.tsx:392]
- Validation phase is not running during an audit. [src: file: frontend/src/components/AuditTimeline.tsx:397] [src: file: frontend/src/components/ProjectCard.tsx:563]
- Validation phase hands over to the run. [src: file: frontend/src/components/AuditTimeline.tsx:816]
- Confirmation before a fresh full audit. [src: file: frontend/src/components/AuditTimeline.tsx:978]
- Orphaned lease banner. [src: file: frontend/src/components/AuditTimeline.tsx:548]
- Cancel pending. [src: file: frontend/src/components/AuditTimeline.tsx:545] [src: file: frontend/src/components/ProjectCard.tsx:723]
