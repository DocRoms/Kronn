# Reassigning a `Blocked` execution awaiting CLI acceptance (KT-640)

`reassign_execution_worker` used to refuse every `Blocked` execution outright:
its `resumable_worker_state` gate only accepted `Working`, `ChangesRequested`,
`Escalated`, and an `Interrupted` row whose checkpoint pointed back at one of
those two. A CLI worker offer still `Blocked(awaiting_worker_acceptance)` — the
exact CLI never accepted the KT-328 control offer, or its re-offer after a
review round — had no path back to the principal: `task_exec_reassign` failed
with "Blocked, not a resumable worker state", stranding real work (checkout,
history, attempts, budget) behind a worker that never showed up.
[src: file: backend/src/db/orchestration.rs:4429-4447]

The gate now also accepts a `Blocked` row whose `blocked_reason_code` is
exactly `AwaitingWorkerAcceptance` — the ONE hold where the worker process
itself never started for this attempt, so its checkout, sub-discussion,
attempt number and budget carry over completely untouched; only the worker
identity changes. This hold is not limited to the initial KT-328 handshake —
the KT-319 rework re-offer parks the SAME Provisioning-origin `Blocked` after
a review round, so a redirect here can also be reclaiming an attempt with
real prior review history, not only a brand-new one. Every other `Blocked`
reason (`WorkerSessionCommittedElsewhere`, or an `Applying`-origin
infra/dirty-main hold) still falls through to the refusal.
[src: file: backend/src/db/orchestration.rs:4439-4447]

Before doing anything else, the function resumes the checkpoint the ADR §3
state machine actually allows: `Blocked` clears back ONLY to
`blocked_from_status`, which for this reason code is always `Provisioning`
(the only place that ever sets it — the initial KT-328 handshake and the
KT-319 rework re-offer). The internal `transition_execution(..., Provisioning,
...)` call also clears `blocked_reason`/`blocked_reason_code`/
`blocked_from_status`, and the existing unconditional
`cancel_live_offers_for_execution` cancels the stale offer row — so the
superseded CLI's late accept resolves to
`AcceptOutcome::NotAcceptable { status: Cancelled }`, never a usurped
acceptance. A second, sequential reassignment call against the same row loses
the CAS inside that same `transition_execution` and is refused the ordinary
way ("not a resumable worker state") rather than silently double-applying —
this is NOT a test of two simultaneous callers racing each other.
[src: file: backend/src/db/orchestration.rs:4454-4474]

The caller decides what Provisioning becomes next, exactly like the initial
launch: a native worker dispatches straight to `Working` (added to the
existing `Interrupted | ChangesRequested | Escalated` resume match), while a
CLI target reuses the pre-existing generic "not-Interrupted → Interrupted,
then open a fresh `cli_reassignment` offer" path unchanged — no per-status
branch was needed there.
[src: file: backend/src/api/orchestration.rs:7517-7527]

Regression coverage, and exactly what each layer proves:

- DB-layer `reassign_execution_worker` primitive: the success path (offer
  invalidation, a second sequential call on the SAME once-Blocked row losing
  its CAS), the `WorkerSessionCommittedElsewhere` refusal, the
  `Applying`-origin refusal, and a malformed row (the acceptance code paired
  with a non-`Provisioning` checkpoint) refused by the checkpoint guard, not
  by the code check. This proves the primitive's own state transition and
  CAS; it does NOT exercise either HTTP handler, so removing the API-layer
  branches below would not fail these tests.
  [src: file: backend/src/db/orchestration_tests.rs:1044-1318]
  [src: file: backend/src/db/orchestration_tests.rs:1320-1394]
- API-layer `reassign_native_execution`: drives the SAME fixture through the
  Provisioning→Working dispatch branch, asserting the execution reaches
  `Working` with its sub-discussion/workspace/attempt unchanged, exactly one
  replacement dispatch job, and the stale CLI offer refusing a late accept.
  A second test removes the CLI child binding. Its guard fails inside the DB
  savepoint after the CAS resumed to `Provisioning`, but before stale-offer
  cancellation. Every execution field must equal the pre-call snapshot and
  the old offer must remain `pending`. This proves checkpoint rollback at the
  CLI-origin restoration boundary, not a later API failure after cancellation.
- API-layer `task_exec_reassign` (CLI target): the same fixture redirected to
  a different joined CLI session opens a genuinely fresh offer (distinct id,
  correct target session) after the `Blocked`→`Provisioning`→`Interrupted`
  checkpoint sequence, the OLD offer's late accept is refused, and accepting
  the NEW offer reaches `Working`.
  [src: file: backend/src/api/orchestration.rs:20028-20311]

None of this is a proof of concurrent-call idempotence for the API handlers —
the "second call loses the CAS" coverage is a sequential DB-layer race check
on `transition_execution`'s `run_state::claim_status`, not a claim about two
simultaneous HTTP requests through `reassign_native_execution` or
`task_exec_reassign`.
