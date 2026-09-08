# Joined CLI recovery after an accepted handoff

An accepted worker offer moves the exact joined session from the principal
discussion into the execution's pinned child discussion. Boot recovery must
recognize that session in either permitted room. Checking only the principal
room falsely classified the accepted KT-613 worker as unavailable.

The lookup still requires the recorded session primary key, provider and an
active membership. A same-provider substitute, a third room, a departed worker
or a missing/archived child does not qualify. A persisted quota arbitration
remains an explicit human gate; this change introduces no grace period or
automatic quota rearm.

Explicitly reassigning the same worker can keep its existing child membership.
It creates an offer in that room and still requires the exact worker to accept.
Another CLI in the child is not an eligible replacement: ordinary replacement
selection remains scoped to the principal room. Duplicate acceptance preserves
one handoff and the existing workspace, branch, child and execution.

## Regression evidence

The four `cli_restart_*` tests in `backend/src/api/orchestration.rs` exercise:

- Real offer acceptance, membership transfer, live-key rotation and two backend
  restart transitions, with one durable resume message and no native dispatch.
- Explicit same-worker reassignment from an escalated state, a rejected foreign
  acceptor and repeated acceptance without duplicate handoff messages.
- Departed, third-room, wrong-provider and missing-child refusals, plus retained
  quota arbitration even when another same-provider CLI is present.
- Preservation of an unaccepted offer in the principal room.

Run with `cargo test --manifest-path backend/Cargo.toml --lib cli_restart`.
These are isolated SQLite/Git protocol tests, not a live provider or host-reboot
test. They do not authorize resuming an unrelated worker or editing user data.

On 2026-09-07 the principal observed all four regressions passing (4.81 s),
then all 336 tests selected by `--lib orchestration` passing (47.45 s), with
no failures or ignored tests. `cargo fmt --all -- --check`, `git diff --check`
and `cargo clippy --all-targets -- -D warnings` also passed. This scoped result
does not replace the final full-release test run.

## Observed KT-613 recovery

The original commit `e42a231c1a6a45eaae6f7668aeb8c61b880a2103` was preserved.
On 2026-09-07 the principal explicitly reassigned the existing execution to a
native Codex worker for two review corrections. The child and managed workspace
were reused; this is evidence of the existing cross-transport recovery path,
not proof that the new same-CLI recovery has run against the live instance.

## Durable return identity (KT-620)

The active row's `session_id` (for example `adhoc-*`) is not the bridge's durable
source-binding key (`cli-*`). Acceptance already carried both inputs separately,
but the terminal and reassignment return paths previously reread the active key.
Two regression tests reproduced the resulting stale child binding on cancellation
and CLI-to-native reassignment; both passed after the identity fix. This is a
separate defect from KT-615's accepted-child eligibility check and from the stale
session catalogue observed after a reload.
[src: file: backend/src/api/orchestration.rs:14885]
[src: file: backend/src/api/orchestration.rs:14905]

Migration 171 adds `task_execution_cli_bindings`, keyed by execution and exact CLI
session primary key. Offer acceptance records the server-derived source identity
in its staging transaction, before the child transfer. Identical acceptance is
idempotent; a divergent identity for the same assignment is refused. Old
assignments remain recorded when a replacement is explicitly selected. The two
return paths use this retained key in their existing terminal/reassignment
transaction, without selecting an owner by agent or room. A different replacement
CLI now returns its predecessor too; recovering the same exact worker still keeps
that worker in the child.
[src: file: backend/src/db/sql/171_cli_worker_bindings.sql:1]
[src: file: backend/src/db/worker_offers.rs:526]
[src: file: backend/src/db/cli_worker_bindings.rs:35]
[src: file: backend/src/db/orchestration.rs:4478]

The return refuses a source binding or active membership moved to a third room,
and rolls back the associated state transition. An explicitly unlinked known
binding stays unlinked: it is not recreated. Missing identity/binding evidence is
recorded in execution audit and a warning rather than silently assumed expired.
The additive migration does **not** backfill or repair user bindings from room
contents. A legacy execution whose active key exactly resolves remains supported;
if that key is unresolved while same-agent child bindings remain, the return
fails closed and requests explicit recovery instead of choosing one of them.
An existing nonterminal worker can explicitly replay its own accepted
`task_exec_accept_worker_offer` to record the missing identity after both live
session and durable-binding checks. It must still be the execution's exact
assigned worker; a different PK or a third-room binding is not repaired through
this path. The migration itself never infers this information. Historical offers
cannot recapture workers after completion or replacement. Already terminal
deliveries and their evidence are never rewritten.
[src: file: backend/src/db/cli_worker_bindings.rs:77]
[src: file: backend/src/api/orchestration.rs:15045]
[src: file: backend/src/api/orchestration.rs:15131]

The original KT-618 report remains a historical independent review: it did not
execute these later regressions and did not establish the subsequently reproduced
identity mismatch. Its accepted delivery is not retroactively changed.

Regression commands:

```sh
cargo test --manifest-path backend/Cargo.toml --lib cli_durable_return
cargo test --manifest-path backend/Cargo.toml --lib cli_worker_bindings
cargo test --manifest-path backend/Cargo.toml --lib orchestration
```

These exercise cancellation, Done, failure after staged acceptance, replacement
by native and joined CLI workers, reopen/live-key rotation, transaction rollback,
legacy identity refusal/explicit reconfirmation, stale-offer refusal, retained
closed bindings, other peers and Unicode keys.
They use isolated test databases; no live provider or host reboot is implied.

The principal's final frozen-source replay on 2026-09-07 passed **6,815 backend
tests, zero failed and six existing ignored tests**, including 6,033 library
tests (372.93 s) and 595 API integration tests (153.61 s). The eight focused
`cli_durable_return` regressions also passed together (2.18 s). Strict
all-target Clippy passed on 2026-09-08 (37.29 s); format, diff hygiene and the
`with_conn` lint plus its seven tests passed. Regenerating 664 TypeScript exports
produced no tracked drift. These results qualify KT-620's local correction,
not subsequent KT-619 changes or a live return of a pre-migration worker.
