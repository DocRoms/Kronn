# Joined CLI work and listening

## Evidence and boundary

The previous join contract put an unbounded `disc_wait_for_peer()` before its
instruction to take the next plan task during quiet periods. The bridge does
not return on quiet inner polls without an explicit total budget, so an agent
following that order could wait indefinitely before reaching the work clause.
It also requested a substantive first reply before reading the shared plan.
This is an instruction-order defect, not proof that a particular CLI host
caused a reported stop or imposed a specific timeout.
[src: file: backend/scripts/disc-introspection-mcp.py:6839-6848]
[src: file: backend/scripts/test_disc_introspection_mcp.py:10113-10132]

The join protocol now orders introduction, plan read, addressed initial turns,
scope announcement, then repeated bounded work. Between real steps it requests
a 20-second total wait budget, which can return to the plan or existing
execution monitoring even in a quiet room. Idle listening remains unbounded
when no actionable work or execution needs following. The bridge's wait,
routing, cancellation and durable-cursor functions are unchanged.
[src: file: backend/src/api/disc_invite.rs:487-519]
[src: file: backend/src/api/disc_invite.rs:557-597]
[src: file: backend/scripts/disc-introspection-mcp.py:6866-6937]

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
[src: file: backend/scripts/test_disc_introspection_mcp.py:10134-10141]
[src: file: backend/scripts/test_disc_introspection_mcp.py:10188-10201]

## Qualification scope

Three new join-contract tests failed before the instruction change; the six
join tests pass afterward. The 22 existing bridge wait tests pass unchanged,
including simulated long silence, bounded waits, interruption and routing
visibility. No user configuration, provider, credential or host process was
changed by these tests. Python instruction alignment is coordinated separately
with the KT-619 owner; these results do not qualify its authority changes.
[src: file: backend/src/api/disc_invite.rs:6584-6628]

The unfiltered offline backend suite completed successfully (run 35315,
exit 0). Its final captured output was truncated, so this checkpoint does not
assert an aggregate test count. Strict all-target Clippy passed in 15.08 seconds;
existing ts-rs attribute warnings remain. These are source-checkout results,
not proof that the running server has loaded the new instructions or that a
host will execute the requested loop. KT-629 remains open until the Python
instruction surfaces are aligned and independently qualified too.
