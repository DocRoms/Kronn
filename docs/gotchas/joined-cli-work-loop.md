# Joined CLI work and listening

## Evidence and boundary

The previous join contract put an unbounded `disc_wait_for_peer()` before its
instruction to take the next plan task during quiet periods. The bridge does
not return on quiet inner polls without an explicit total budget, so an agent
following that order could wait indefinitely before reaching the work clause.
It also requested a substantive first reply before reading the shared plan.
This is an instruction-order defect, not proof that a particular CLI host
caused a reported stop or imposed a specific timeout.
[src: file: backend/scripts/disc-introspection-mcp.py:6838-6847]
[src: file: backend/scripts/test_disc_introspection_mcp.py:10166-10185]

The join protocol now orders introduction, plan read, addressed initial turns,
scope announcement, then repeated bounded work. Between real steps it requests
a 20-second total wait budget, which can return to the plan or existing
execution monitoring even in a quiet room. Idle listening remains unbounded
when no actionable work or execution needs following. The bridge's wait,
routing, cancellation and durable-cursor functions are unchanged.
[src: file: backend/src/api/disc_invite.rs:487-519]
[src: file: backend/src/api/disc_invite.rs:557-597]
[src: file: backend/scripts/disc-introspection-mcp.py:6865-6936]

Each bounded step has a room announcement and an evidence/limits update.
Delegated milestones belong in the parent room too. An exact CLI target in
another room must not silently become a native-provider dispatch. Addressed
turns require reading; `awareness` is background context. An append receipt
never substitutes for the durable read cursor.
[src: file: backend/src/api/disc_invite.rs:564-594]

A host may move a tool call to the background. That call remains owned and
must reach a terminal result before another wait starts. A queued MCP request
can interrupt the wait normally; handle the request and resume the work loop.
Neither condition is a command to leave. Kronn does not guarantee that every
host will follow instructions or hide all background notifications.
[src: file: backend/scripts/test_disc_introspection_mcp.py:10187-10194]
[src: file: backend/scripts/test_disc_introspection_mcp.py:10241-10254]

The Python catalogue distinguishes active work from idle listening too. One
shared `ROOM_WORK_PROTOCOL` supplies both join/wait manuals and the initialize
orientation, including plan-first ordering, parent-room milestones and exact
CLI routing. It no longer promises that cursor reads can never be replayed or
that quiet returns cost nothing. The bridge's runtime functions did not change.
[src: file: backend/scripts/disc-introspection-mcp.py:1250-1274]
[src: file: backend/scripts/disc-introspection-mcp.py:9354-9371]
[src: file: backend/scripts/disc-introspection-mcp.py:9502]
[src: file: backend/scripts/disc-introspection-mcp.py:9743-9747]
[src: file: backend/scripts/disc-introspection-mcp.py:10815]

## Qualification scope

Three new join-contract tests failed before the instruction change; the six
join tests pass afterward. The 22 existing bridge wait tests pass unchanged,
including simulated long silence, bounded waits, interruption and routing
visibility. No user configuration, provider, credential or host process was
changed by these tests. The KT-619 owner confirmed the boundary on September 10
at 05:53:21 UTC: KT-629 owns instruction text and its contract regressions;
KT-619 owns credential/grant publication and preserves the isolated test-loader
harness. These results do not qualify KT-619's authority changes.
[src: file: backend/src/api/disc_invite.rs:6584-6628]

The unfiltered offline backend suite completed successfully (run 35315,
exit 0). Its final captured output was truncated, so this checkpoint does not
assert an aggregate test count. Strict all-target Clippy passed in 15.08 seconds;
existing ts-rs attribute warnings remain. These are source-checkout results,
not proof that the running server has loaded the new instructions or that a
host will execute the requested loop.

The final Python alignment first failed three tests (four assertion failures
across catalogue, both manuals and initialize). After the correction, the
focused protocol/wait/manual replay passed 42 tests and the full isolated bridge
suite passed **795 tests** in 9.412 seconds. The unfiltered offline backend
replay 84186 passed **6,860 tests, 0 failed, 6 existing ignored**, across 21 suite
results; library time was 386.62 seconds and cold API time 153.79 seconds.
Strict all-target Clippy passed in 1.69 seconds. Backend tree:
`6040c89897a4913b9fb91a6a90bc62cba0df706f`; frontend tree remains
`f8a2df947aa4ba9d8899caf8e121c1ee49e106bc`, with no new frontend qualification.
[src: file: backend/scripts/test_disc_introspection_mcp.py:10021-10073]

The MCP catalogue remains at 110 tools and shrinks to 86,824 bytes; its ceiling
is ratcheted down from 86,900, without a waiver. Repository context remains
18,842 / 18,968 bytes. Both budget regression suites pass 11 tests each; the
task execution matrix passes 10. These are deterministic source-contract
checks, not a live-host obedience test, deployment, provider run or new coverage
measurement. No host configuration, restart, account or authority was changed.
