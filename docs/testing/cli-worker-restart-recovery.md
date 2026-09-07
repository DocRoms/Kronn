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
