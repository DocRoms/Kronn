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
exactly `AwaitingWorkerAcceptance` — the ONE hold where the worker never
started, so nothing durable needs preserving beyond the row itself. Every
other `Blocked` reason (`WorkerSessionCommittedElsewhere`, or an
`Applying`-origin infra/dirty-main hold) still falls through to the refusal.
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
acceptance. A concurrent second reassignment call loses the CAS inside that
same `transition_execution` and is refused the ordinary way ("not a resumable
worker state") rather than silently double-applying.
[src: file: backend/src/db/orchestration.rs:4454-4474]

The caller decides what Provisioning becomes next, exactly like the initial
launch: a native worker dispatches straight to `Working` (added to the
existing `Interrupted | ChangesRequested | Escalated` resume match), while a
CLI target reuses the pre-existing generic "not-Interrupted → Interrupted,
then open a fresh `cli_reassignment` offer" path unchanged — no per-status
branch was needed there.
[src: file: backend/src/api/orchestration.rs:7517-7527]

Regression coverage: the success path (offer invalidation, race refusal),
the `WorkerSessionCommittedElsewhere` refusal, and the `Applying`-origin
refusal all live next to the other `reassign_execution_worker` tests.
[src: file: backend/src/db/orchestration_tests.rs:1044-1318]
