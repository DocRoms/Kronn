# Joined CLI work and listening

## Evidence and boundary

The previous join contract put an unbounded `disc_wait_for_peer()` before its
instruction to take the next plan task during quiet periods. The bridge does
not return on quiet inner polls without an explicit total budget, so an agent
following that order could wait indefinitely before reaching the work clause.
It also requested a substantive first reply before reading the shared plan.
This is an instruction-order defect, not proof that a particular CLI host
caused a reported stop or imposed a specific timeout.
[src: file: backend/scripts/disc-introspection-mcp.py:7502-7510#call_disc_wait_for_peer]
[src: file: backend/scripts/test_disc_introspection_mcp.py:12382-12401#test_default_wait_is_unbounded_and_does_not_return_on_inner_quiet_polls]
[src: file: backend/src/api/disc_invite.rs:7496-7505#join_protocol_reads_plan_before_substantive_first_reply]

The join protocol now orders introduction, plan read, addressed initial turns,
scope announcement, then repeated bounded work. Between real steps it requests
a 20-second total wait budget, which can return to the plan or existing
execution monitoring even in a quiet room. Idle listening remains unbounded
when no actionable work or execution needs following. The bridge's wait,
routing, cancellation and durable-cursor functions are unchanged.
[src: file: backend/src/api/disc_invite.rs:604-667#plan_get]
[src: file: backend/src/api/disc_invite.rs:683-689#max_total_secs]
[src: file: backend/src/api/disc_invite.rs:698-703#disc_wait_for_peer]
[src: file: backend/src/api/disc_invite.rs:7507-7516#join_protocol_does_not_put_unbounded_wait_before_ready_work]
[src: file: backend/scripts/disc-introspection-mcp.py:10003-10039#ROOM_WORK_PROTOCOL]

Each bounded step has a room announcement and an evidence/limits update.
Delegated milestones belong in the parent room too. An exact CLI target in
another room must not silently become a native-provider dispatch. Addressed
turns require reading; `awareness` is background context. An append receipt
never substitutes for the durable read cursor.
[src: file: backend/src/api/disc_invite.rs:676-707#attention_required]
[src: file: backend/src/api/disc_invite.rs:7518-7539#join_protocol_reports_each_step_and_preserves_wait_ownership]

A host may move a tool call to the background. That call remains owned and
must reach a terminal result before another wait starts. A queued MCP request
can interrupt the wait normally; handle the request and resume the work loop.
Neither condition is a command to leave. Kronn does not guarantee that every
host will follow instructions or hide all background notifications.
[src: file: backend/src/api/disc_invite.rs:704-709#CAVEAT]
[src: file: backend/scripts/disc-introspection-mcp.py:7502-7510#call_disc_wait_for_peer]
[src: file: backend/scripts/test_disc_introspection_mcp.py:12403-12410#test_wait_description_prevents_stacked_host_background_tasks]

The Python catalogue distinguishes active work from idle listening too. One
shared `ROOM_WORK_PROTOCOL` supplies both join/wait manuals and the initialize
orientation, including plan-first ordering, parent-room milestones and exact
CLI routing. It no longer promises that cursor reads can never be replayed or
that quiet returns cost nothing. The bridge's runtime functions did not change.
[src: file: backend/scripts/disc-introspection-mcp.py:10003-10039#ROOM_WORK_PROTOCOL]
[src: file: backend/scripts/disc-introspection-mcp.py:10295-10296#ROOM_WORK_PROTOCOL]
[src: file: backend/scripts/disc-introspection-mcp.py:10680-10684#ROOM_WORK_PROTOCOL]
[src: file: backend/scripts/disc-introspection-mcp.py:12091#ROOM_WORK_PROTOCOL]
[src: file: backend/scripts/test_disc_introspection_mcp.py:12157-12207#JoinedRoomWorkProtocolTests]

## Deaf windows in the bridge wait (KT-703)

The bridge loop applied the server's `pacing.next_delay_seconds` as a sleep
after every quiet 15-second poll. That pacing was designed for agents that
re-arm the wait themselves; once the bridge chained polls itself, the sleep
only removed listening time. It lasted 40 s while a human was active and up to
480 s after about 23 quiet minutes. A turn posted during the sleep was seen at
the next poll, so a reply could take more than 8 minutes while presence showed
the session as dormant but present. The bridge now chains polls without a pause,
with a 1-second floor if the server answers early. The server's pacing values
and the `next_poll_at` presence projection are unchanged.
[src: file: backend/scripts/disc-introspection-mcp.py:7598-7619#_WAIT_MIN_QUIET_POLL_SECS]
[src: file: backend/scripts/test_disc_introspection_mcp.py:12528-12542#test_quiet_polls_are_chained_without_a_listening_gap]
[src: file: backend/src/api/disc_introspection.rs:175-185]

The `session_budget` block on a wake said "rotate this session" to any long
session, including the principal of a shared room. A session that read it
stopped listening there. The verdict is now attached only in a delegated
task's execution room.
[src: file: backend/src/api/disc_invite.rs:1844-1876#session_budget]

These fixes do not cover host behaviour. A host can still move a long wait to
the background. Any other Kronn call ends the wait (`wait_preempted`), and an
interactive turn that ends while the wait runs in the background is outside
Kronn's control. A real Claude Code run is still needed to check that case.
[src: file: backend/scripts/disc-introspection-mcp.py:12211-12219#_with_wait_preempted]

## Qualification scope

Three new join-contract tests failed before the instruction change; the six
join tests pass afterward. The 22 existing bridge wait tests pass unchanged,
including simulated long silence, bounded waits, interruption and routing
visibility. No user configuration, provider, credential or host process was
changed by these tests. The KT-619 owner confirmed the boundary on September 10
at 05:53:21 UTC: KT-629 owns instruction text and its contract regressions;
KT-619 owns credential/grant publication and preserves the isolated test-loader
harness. These results do not qualify KT-619's authority changes.
[src: file: backend/src/api/disc_invite.rs:7496-7539#join_protocol_reads_plan_before_substantive_first_reply]
[src: file: backend/scripts/test_disc_introspection_mcp.py:12290-12418#WaitOutsideLlmLoopTests]

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
[src: file: backend/scripts/test_disc_introspection_mcp.py:12157-12207#JoinedRoomWorkProtocolTests]

The MCP catalogue remains at 110 tools and shrinks to 86,824 bytes; its ceiling
is ratcheted down from 86,900, without a waiver. Repository context remains
18,842 / 18,968 bytes. Both budget regression suites pass 11 tests each; the
task execution matrix passes 10. These are deterministic source-contract
checks, not a live-host obedience test, deployment, provider run or new coverage
measurement. No host configuration, restart, account or authority was changed.
