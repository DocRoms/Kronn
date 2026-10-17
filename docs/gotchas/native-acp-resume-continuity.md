# Native ACP resume continuity

OpenCode's native runtime and the Claude/Codex ACP adapters use a durable
conversation identity scoped to discussion, agent, runtime and resolved work
directory. The discussion layer selects the resume identity and delta together;
`start_agent_with_config` never independently reloads a rejected identity.
A first turn, incomplete checkpoint, missing marker or delta without a size
saving uses the ordinary bounded full prompt and no resume identity.
[src: file: backend/src/api/discussions/streaming.rs:2593-2725#resume_with_delta_if_possible]
[src: file: backend/src/agents/runner.rs:4314-4318#acp_resume_id]

## Two boundaries, one completed turn

The checkpoint records both the last message included in the input snapshot
and the exact response produced by that turn. These are different facts:

- Advancing the input frontier to the response would lose a peer message that
  arrived while the agent was working.
- Keeping only the input frontier would send the agent its own response again.
- Excluding all messages from the same agent family would also lose legitimate
  messages from another CLI using that provider.

The next delta therefore contains every message after the input frontier except
that exact, already-emitted response. Both markers must still be present.
Delta rendering bypasses the single-user-message shortcut and stale full-history
summary/pinning indices. It never silently truncates unseen messages: if the
complete delta is not smaller than the bounded full prompt, resume is declined.
[src: file: backend/src/api/disc_prompts.rs:400-420#build_agent_delta_prompt]
[src: file: backend/src/api/disc_prompts.rs:643-648#is_delta]
[src: file: backend/src/api/discussions/streaming.rs:2521-2543#messages_not_yet_seen]
[src: file: backend/src/api/discussions/streaming.rs:2670-2713#unseen]

Migration 173 adds the output marker and an active turn identity, without
inferring historical checkpoints. Native ACP invalidates old progress before
prompt dispatch. Adapter session-id updates and completion are guarded by that
turn identity: a delayed old turn cannot certify a replacement session. The
response and completed checkpoint are written in the same database transaction;
an interrupted/failed turn or rolled-back write leaves no completed proof.
[src: file: backend/src/db/sql/173_acp_completed_turn_checkpoint.sql:1-9]
[src: file: backend/src/db/acp_runtime_sessions.rs:110-195#begin_turn]
[src: file: backend/src/db/discussions.rs:1847-1871#insert_native_agent_message_with_checkpoint]
[src: file: backend/src/db/discussions.rs:1903-1911#checkpoint]
[src: file: backend/src/db/discussions.rs:2149-2152#complete_turn]

Claude CLI-print retains its separate `claude_cli_print_v1` identity. Its init
event and final response share the same per-turn store; ACP adapter identities
are never tested against a CLI-print conversation file. The direct CLI retires
old progress before spawning, even if the next init event never arrives; a
database failure there prevents the spawn rather than leaving a stale frontier.
[src: file: backend/src/agents/runner.rs:2870-2875#CLI_PRINT_RUNTIME]
[src: file: backend/src/agents/runner.rs:2884-2939#prepare_cli_print_turn]
[src: file: backend/src/agents/runner.rs:4565-4574#prepare_cli_print_turn]
[src: file: backend/src/api/discussions/streaming.rs:2657-2669#acp_delta]

## Negotiated resumption

This implementation calls `session/resume` only when
`agentCapabilities.sessionCapabilities.resume` is an object. An omitted or
`null` value does not advertise support; other non-object values are rejected
as capability evidence too. ACP distinguishes
this from `session/load`: loading replays the conversation **to the client**,
whereas resumption restores it without that client-side replay. A `loadSession`
capability alone does not authorize `session/resume`. Client-side history replay
is not, by itself, proof that the model receives a duplicated prompt.
[ACP session setup](https://agentclientprotocol.com/protocol/v1/session-setup)
[ACP capability schema](https://agentclientprotocol.com/protocol/v1/schema#sessioncapabilities)

The safe fresh-session fallback is restricted to absent resume capability or a
positively identified missing session before a prompt is sent. Authentication,
timeouts and ambiguous transport errors fail visibly. Prompt, cancellation and
persistence failures are not followed by an automatic fresh-session replay.
[src: file: backend/src/agents/runner.rs:5358-5414#resume_refused]
[src: file: backend/src/agents/runner.rs:5729-5794#acp_failure_diagnostic]

OpenCode v1.18.27 defines a structured missing-session error using code -32602,
the exact message `session not found: <id>` and `data.sessionId`. The transport
must correlate all three with the requested OpenCode resume identity; an error
prefix, another session's id or another lifecycle method cannot authorize this
fallback. Unstructured upstream failures remain ambiguous rather than being
guessed from a 404 or free text.
[Pinned OpenCode error contract](https://github.com/anomalyco/opencode/blob/v1.18.27/packages/opencode/src/acp/error.ts)
[src: file: backend/src/acp.rs:156-161#PendingRequest]
[src: file: backend/src/acp.rs:1131-1139#missing_session_id]
[src: file: backend/src/acp.rs:1188-1192#JSON_RPC_INVALID_PARAMS]
[src: file: backend/src/acp.rs:1194-1230#classify_response_error]

Each agent reply records which path its turn took in
`messages.session_continuity`: `resumed`, `new` or `fallback_full_history`
(resume refused by the runtime), with a reason when it did not resume. The
discussion layer decides `new` and its reason; the runner reports a refused
resume once the runtime answered it. Runtimes that never resume record nothing.
[src: file: backend/src/api/discussions/streaming.rs:2593-2725#resume_with_delta_if_possible]
[src: file: backend/src/api/discussions/streaming.rs:2727-2737#settle_turn_continuity]
[src: file: backend/src/agents/runner.rs:5809-5824#resume_refused]

## Deterministic qualification boundaries

The principal strengthened the recovered worker regression before correction:
it now persists and reloads real discussion messages, includes a peer message
between the input snapshot and native response, and closes every database
reference before reopening. It first failed on peer loss, then on own-response
duplication after the delta renderer was corrected.

The two-turn test connects `resume_with_delta_if_possible`, the actual
`start_agent_with_config` NativeAcp branch and the same transactional
response/checkpoint writer used by streaming. Only the ACP transport is a
fixture; this is not an end-to-end invocation of every HTTP/SSE preflight.
Additional tests cover scope isolation, stale turn/session events, incomplete
legacy progress and atomic response/checkpoint rollback. Existing ACP fixtures
cover capability absence, missing sessions, ambiguous errors, cancellation and
redacted prompt/persistence failures.
[src: file: backend/src/api/discussions/streaming.rs:8674-8993#two_turns_through_the_real_decision_and_spawn_chain_survive_a_db_reopen]
[src: file: backend/src/db/acp_runtime_sessions.rs:402-531#completed_checkpoint_is_scoped_and_a_new_turn_invalidates_it_before_prompt]
[src: file: backend/src/agents/runner.rs:14767-14868#cli_print_checkpoint_requires_the_same_turn_store_and_is_retired_before_init]
[src: file: backend/src/agents/runner.rs:14925-15129#run_acp_session_uses_resume_and_keeps_the_existing_fallback]

These tests do not launch a live provider or qualify real model behavior.
Kronn normalizes ACP token counts but does not yet normalize optional cumulative
session cost and currency into per-turn accounting. Unknown cost is not zero.
The release checklist records the actual commands, results and qualified tree;
earlier worker claims are not substituted for principal validation.
