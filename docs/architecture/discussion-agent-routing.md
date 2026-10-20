# Discussion agent routing

> Identity & addressing reference: `docs/architecture/agent-identities.md`
> (every addressable identity, its exact alias, wake/awareness/reply/reload
> behaviour — KT-211).

Kronn routes human turns to durable **identities**, not provider names. Three
responders may all be Codex without being interchangeable:

- `discussion_agent`: the agent configured for the discussion;
- `agent`: a native punctual agent invoked for this turn only;
- `cli`: one exact joined CLI session, identified by its session row.

`message_targets` preserves those identities in textual order.
`messages.target_agent` and `target_agents` remain compatibility projections
for older clients and peers.
[src: file: backend/src/models/discussions.rs:742-768#MessageTargetKind]
[src: file: backend/src/db/sql/099_typed_message_targets.sql:1-37#message_targets]

The composer parses prose once and submits the typed target list. The backend
validates that every CLI session belongs to the discussion before accepting
the message; it never reparses Markdown to guess an owner.
[src: file: frontend/src/lib/messageTargets.ts:176-208#positionedTargetsFromComposerText]
[src: file: backend/src/api/discussions/messaging.rs:191-316#canonical_targets]

## Human turn (UI): complete target matrix

| Explicit mentions | Composer/API result | Reply owners |
|---|---|---|
| None | `targets=[]` | Only the configured discussion agent answers. Joined CLIs do not suppress or duplicate it. |
| Configured agent mention | One `discussion_agent` target | The configured discussion agent answers. |
| Other installed agent mention | One `agent` target | One durable native one-shot is created for that agent. |
| Joined CLI mention | One `cli` target with `cli_session_id` | That exact CLI session owns the reply, even when another CLI or native agent has the same provider type. |
| Several usable mentions | Ordered, deduplicated typed list | Each identity independently owns one reply; no unlisted third answer and no automatic synthesis. |
| `@all` | `target_all=true` | Expands to the active configured agent, previously addressed punctual agents, and every non-left joined CLI already visible in the discussion. A configured agent disabled by `no_agent` is excluded. It never means every installed agent. |
| Same mention repeated | One target at its first text position | One reply obligation, never one per textual occurrence. |
| Disabled/unavailable agent mentioned in the UI | Not offered by autocomplete | It remains prose and follows the zero-target route. A direct API client may still submit an explicit typed target. |
[src: file: frontend/src/lib/messageTargets.ts:26-125#composerMentions]
[src: file: backend/src/api/discussions/messaging.rs:191-316#canonical_targets]

Mentions inside inline code or fenced code are examples, not dispatch requests.
This is the supported escape hatch when a human or agent needs to discuss the
syntax itself (write `` `@codex` `` rather than a live mention). The UI composer
and MCP bridge apply the same exemption before resolving targets.
[src: file: frontend/src/lib/messageTargets.ts:131-147#proseOnly]
[src: file: backend/scripts/disc-introspection-mcp.py:5124-5135#_structured_message_targets]

| Room mode | Target state | Result |
|---|---|---|
| Native enabled | No target or `discussion_agent` | One native principal job. |
| Native enabled | `agent` | One native job with that punctual override. Runtime unavailability keeps the durable obligation retryable; it is never reassigned by provider-name coincidence. |
| Any mode | `cli` | No local process; the exact joined session receives the User turn through `disc_wait_for_peer`. |
| Native enabled | Mixed identities | Native identities become durable jobs; CLI identities receive their own copy. |
| Native disabled (`no_agent`) | Untargeted | No native process; joined CLIs retain the autonomous-room broadcast behaviour. |
| Native disabled (`no_agent`) | Native identity | No local process starts. |
[src: file: backend/src/api/discussions/routing.rs:20-40#route_human_turn]
[src: file: backend/src/api/discussions/messaging.rs:65-113#NativeDispatchTarget]

The room mode is an explicit persisted operator choice. Disabling the native
discussion agent sets `discussions.no_agent` without pausing, removing, or
hiding joined peers. The mode switch retires pending native dispatches in the
same transaction, and the dispatcher's final claim also checks that native
mode is still enabled.
[src: file: backend/src/db/discussions.rs:951-970#set_disc_no_agent_within_tx]
[src: file: backend/src/db/agent_dispatch.rs:556-664#claim_with_limits]
[src: file: backend/src/db/agent_dispatch.rs:838-856#mark_agent_started]

Current bridges send their durable session id on every wait (resolved by
session id alone when the provider is unknown). Waking and seeing are separate
concerns for those callers. A session is WOKEN only by a turn whose exact
`cli` target matches it, or by an untargeted human turn in a room whose joined
CLIs are the designated responders (no native agent). Everything else —
untargeted User and Agent traffic, turns addressed to another responder —
never wakes it: it accumulates and attaches to its next legitimate wake as
bounded `awareness: true` context (at most 20 per wake, overflow counted in
`awareness_omitted` and re-attached later). `@` therefore picks who answers;
it never makes a turn invisible to the other participants, and a quiet or
awareness-only window does not make the bridge return on its own. Legacy
bridges without a session id retain the old room-visible projection during
rolling upgrades. Host runtimes can still impose their own wake semantics:
Claude Code 2.x backgrounds a tool call at 120 seconds and exposes a
model-visible task notification. That notification is not permission to stack
a second wait while the first is active. The bridge serves one request at a
time, so any other Kronn call ends that wait; the call's result carries
`wait_preempted` and the agent must re-arm. True zero-turn silence on such a
host needs an out-of-band push capability.
[src: file: backend/src/api/disc_invite.rs:1691-1760#addressed_to_caller]
[src: file: backend/src/api/disc_invite.rs:1071-1075#AWARENESS_MAX_MESSAGES]
[src: file: backend/scripts/disc-introspection-mcp.py:7326-7357#_WAIT_PREEMPTED_NOTICE]
[src: file: backend/scripts/disc-introspection-mcp.py:12120-12127#_with_wait_preempted]

Awareness delivery follows a scan / offer / ack contract with two durable
per-session cursors. When a wake response attaches a batch, the server
persists the OFFER high-water mark (`awareness_offered_upto`, migration 106).
The bridge stages that value at emission, commits it on the model's next tool
call, and echoes it back as `ack_awareness_upto`; only then does the acked
cursor (`user_catchup_cursor`) advance, clamped to the offer — so a cancelled
call, a bridge crash, an oversized or racing ack can only cause a bounded
replay, never a skipped turn. Message revision events ride the same backlog.
New and migrated sessions start both cursors at the room's current tip.
[src: file: backend/src/db/sql/105_user_turn_catchup.sql:12-23#user_catchup_cursor]
[src: file: backend/src/db/sql/106_awareness_offered_cursor.sql:1-11#awareness_offered_upto]
[src: file: backend/src/api/disc_invite.rs:1362-1390#ack_awareness_upto]
[src: file: backend/scripts/disc-introspection-mcp.py:3500-3509#_PENDING_AWARENESS_ACK_BY_DISC]
[src: file: backend/scripts/disc-introspection-mcp.py:3969-3986#_ack_pending_read_cursors]
[src: file: backend/scripts/disc-introspection-mcp.py:4011-4023#_stage_awareness_ack]
[src: file: backend/src/db/discussion_sessions.rs:450-484#create_session]

## Queue, restart, and edit/resend

| Situation | Contract |
|---|---|
| User sends while a response is streaming | The UI queue keeps each follow-up as its own durable entry with its own targets, stored before it is accepted, and sends the entries one at a time; a failed send retries the same id. |
| Several local targets | Every job commits with the human message. The first is claimed for the active SSE stream; later jobs remain Pending and drain sequentially. |
| Backend restart after acceptance | Running jobs return to the durable queue and Pending jobs remain present; the per-message/per-target dedupe key prevents duplicate obligations. |
| A sibling answers first, then a restart | The recovered job is kept. Recovery supersedes a job whose turn the room has moved past, and a sibling's reply to the **same** trigger is not the room moving on — it is the turn being served. Comparing against the trigger message, rather than against any newer message, is what tells those apart. |
| HTTP retry with the same `client_message_id` | The existing human message is returned as a duplicate and no additional target job is created. |
| Edit/resend while a response is active | Rejected with `DispatchInProgress`; the current obligation is not mutated underneath a runner. |
| Successful edit/resend | Replaces the message's target list atomically with the content revision and creates at most one new durable job per absent listed target. |
| Federation/catch-up | The ordered target list travels with live messages and revision events. A frame from an older peer that carries only `target_agents` is read as plain `agent` targets, and the stored `target_agent` is the first target. |
[src: file: frontend/src/hooks/useMessageQueue.ts:13-26#QueuedMessage]
[src: file: frontend/src/hooks/useMessageQueue.ts:162-258#processingDiscsRef]
[src: file: frontend/src/hooks/useMessageQueue.ts:260-280#enqueue]
[src: file: backend/src/db/discussions.rs:1305-1325#insert_user_message_with_dispatches]
[src: file: backend/src/db/agent_dispatch.rs:677-826#recover_after_restart]
[src: file: backend/src/db/discussions.rs:3584-3586#DispatchInProgress]
[src: file: backend/src/api/discussions/messaging.rs:1013-1021#DispatchInProgress]
[src: file: backend/src/db/discussions.rs:3535-3795#revise_message_with_dispatch]
[src: file: backend/src/api/federation.rs:16-73#federate_message]
[src: file: backend/src/api/federation.rs:102-147#federate_message_revision]
[src: file: backend/src/api/ws.rs:1164-1172#resolved_targets]
[src: file: backend/src/api/ws.rs:920#target_agent]

New-discussion aliases follow the same independent fan-out contract. Creation
persists the initial typed targets and keeps every native target visible as a
room participant; the subsequent `/run` call materializes one durable dispatch
per native identity. It does **not** call the orchestration endpoint, add debate
framing, or append a synthesis. Because those jobs drain sequentially inside
one discussion, each later runner receives a bounded prompt containing the
shared trigger plus any direct sibling replies already completed for that same
turn. This lets it complement or answer a concrete peer request without an
extra dispatch. Later human turns and unrelated handoff branches remain hidden.
Agent-to-agent handoffs remain governed separately by the opt-in collaboration
setting below.
Within the same root user turn, an agent already named in the user's target list
or admitted by an earlier handoff is considered scheduled: sibling replies are
told not to wait for or relaunch it, and the persistence boundary rejects any
redundant handoff. Natural `@alias` references in generated prose are inert. A
new native dispatch requires the explicit hidden
`<!-- kronn:handoff @alias -->` marker described in the collaboration prompt;
Kronn removes that marker before persistence. When a joined CLI of that
provider wrote the answered turn, or is the only one joined and the provider is
not the room's agent, the alias (marker or prose) wakes that CLI instead and
spawns nothing (KT-1201, `agent-identities.md` §2). An agent not scheduled yet can
still be delegated a concrete follow-up under the normal collaboration limits.

The UI makes this contract visible (KT-1109). A turn naming several agents is
labelled as a parallel launch in the composer and on the sent message. When
the first named agent is told to delegate to the others, the composer asks
before sending: "orchestrate" sends `attach_agents` (participants only, no
dispatch) and then addresses the first agent alone, so its handoff markers can
reach the others; "parallel" keeps the fan-out above. A native reply's prose
mention that launched nobody is annotated with its reason (already scheduled
on the root turn, or no marker), computed from `message_targets`.
[src: file: frontend/src/lib/agentDelegation.ts:62-149#detectDelegation]
[src: file: backend/src/db/discussions.rs:1760-1804#native_agents_scheduled_for_root_turn]
[src: file: backend/src/api/discussions/crud.rs:545-553#initial_participants]
[src: file: backend/src/api/discussions/messaging.rs:115-158#enqueue_dispatches_for_trigger]
[src: file: backend/src/api/discussions/crud.rs:896-900#attach_discussion_participants]
[src: file: backend/src/api/discussions/streaming.rs:2236-2348#independent_sibling_notice]
[src: file: backend/src/api/discussions/streaming.rs:2129-2184#discussion_at_dispatch_trigger]
[src: file: backend/src/api/discussions/streaming.rs:2191-2226#thread_under_trigger]
[src: file: backend/src/db/discussions.rs:2019-2043#already_scheduled]

## Joined agent turn (MCP)

A live MCP `disc_append` carries the same ordered `targets` model as the human
composer. The bridge resolves canonical native mentions (`@codex`) separately
from exact joined aliases (`@codex-cli`, `@codex-cli-2`, …) using the
discussion's participant order. Several mentions fan out once per durable
identity. The legacy single `target_agent` field remains accepted during
rolling upgrades, with its historical provider-level semantics.
[src: file: backend/scripts/disc-introspection-mcp.py:5124-5210#_structured_message_targets]
[src: file: backend/src/api/disc_source.rs:434-454#legacy_requested_target]

For each native identity, the message and its dispatch obligation commit in one
transaction. Exact CLI identities create no local runner: the addressed session
receives the Agent turn through its next poll, while every unrelated CLI skips
it and advances its cursor. Bulk imports, historical appends, duplicates and
appends without a live JOIN session never start a runner.
[src: file: backend/src/db/discussions.rs:1543-1562#insert_message_with_targets_and_dispatches]
[src: file: backend/src/api/disc_invite.rs:1698-1717#addressed_to_caller]

Every verified live MCP append also records its exact author session in
`message_cli_authors`. This is local routing provenance, not portable transcript
data: imports and federated messages never acquire a local CLI identity. When a
later live peer uses `reply_to_message_id` without an explicit target, Kronn
routes the reply to that exact author session. Explicit typed or legacy targets
still win. A native agent's reply to such a message defaults to the same exact
session unless it names someone else, and delivery treats an untargeted agent
reply to a session's own message as addressed to it (KT-1201). This lets two Codex CLIs answer one another without waking the native
Codex agent or filtering the sibling as “self”; only the author session excludes
its own append. An untargeted message from the native Codex agent remains a real
peer turn for a Codex CLI — provider equality alone is never treated as
self-authorship. Messages written before this provenance existed may be replayed
once when an old cursor is restored because Kronn cannot safely guess which
historical CLI wrote them. `disc_wait_for_peer` and `disc_get_message` expose
the identity as `reply_target` so agents can inspect the decision without
loading session tables.
[src: file: backend/src/db/sql/100_message_cli_authors.sql:1-14#message_cli_authors]
[src: file: backend/src/api/disc_source.rs:455-488#author_cli_session_id] [src: file: backend/src/api/disc_source.rs:542-577#message_cli_author_target]
[src: file: backend/src/api/disc_invite.rs:1615-1673#reply_target]
[src: file: backend/src/api/disc_invite.rs:1979-1999#addressed_to_session]
[src: file: backend/src/api/disc_introspection.rs:83-86#reply_target] [src: file: backend/src/api/disc_introspection.rs:515-528#reply_target]

An untargeted live peer append in a native discussion deliberately addresses
the native principal: this is how an invited CLI hands work back to the
discussion's normal agent. Peer-to-peer exchanges that must exclude that
principal use a structured target; autonomous rooms use no-agent mode.
[src: file: backend/src/api/discussions/routing.rs:42-69#route_joined_peer_turn]

A workflow Agent step joined by `room_id` is the exception (KT-1253): it
reports to the room rather than handing it the turn, so its untargeted append
reaches the joined peers and never wakes the native principal. Without this,
every progress report of an orchestrator woke the room's agent for a reply
nobody asked for. An explicit target (typed, legacy `target_agent`, connection
mention, or the CLI author of a `reply_to`) keeps its route, and a failed
lookup fails closed like the rest of the presence snapshot.
[src: file: backend/src/api/discussions/routing.rs:42-69#route_joined_peer_turn]
[src: file: backend/src/api/disc_source.rs:737-781#session_identity]

## When a start is refused

A dispatch that never reaches the agent is settled as a refused preflight, and the
reason it was refused is persisted on the job (`agent_dispatch_jobs.last_error`) and
shown in the room. That is recent: until 0.13.0 both code paths that settle a refusal
reported the same nine words, "agent execution preflight failed", for every condition
but two — a missing project path, an endpoint that no longer resolves, a connection
that no longer matches the target, a model the catalogue refuses. Kronn knew which one
and replaced it with a sentence nobody can act on. Mentioning three agents and watching
two disappear twenty seconds later left no trace anywhere; the cause could only be read
out of the database.

Two distinctions the reason must not blur, and does not:

- **Refused is not unavailable.** A settled refusal and a retryable outage
  (`RuntimeUnavailable`) remain different outcomes with different handling. Carrying the
  diagnosis never depended on merging them.
- **The tracked path is the one a room goes through.** It settles a durable dispatch
  job, so its text is what lands in `last_error` and in the transcript. A fix applied to
  the untracked path alone changes nothing a room user sees — which is exactly how the
  second producer of the generic string survived the first fix. `strings` on the built
  binary, not a source grep, is what proved a second one existed.
[src: file: backend/src/api/discussions/streaming.rs:1270-1315#AgentExecutionOutcome] [src: file: backend/src/api/discussions/streaming.rs:1481-1496#finish_tracked_preflight]

## Persistence and interruption

- `message_targets` records every intended addressee kind, agent type,
  optional exact CLI session and optional per-turn reasoning tier in order.
  New-discussion `@alias` routing is persisted on the initial user message
  before its independent dispatches start, so every responder resolves its own tier;
  targets without an override inherit the discussion tier.
  [src: file: backend/src/api/discussions/crud.rs:611-652#replace_message_targets]
  [src: file: backend/src/api/discussions/runtime.rs:636-655#dispatch_target_tier]
- `message_cli_authors` records the exact local CLI author for verified live MCP
  appends, enabling deterministic `reply_to` routing across same-provider peers.
- `messages.target_agent` remains the first-target compatibility projection.
- `agent_dispatch_jobs.agent_override_json` records the native one-shot for
  one target. Dedupe keys include the message (or revision) and target agent.
- A backend restart resets a claimed durable job to pending for the boot drain.
- A watchdog timeout is a completed failed attempt, not a restart. Its message
  names the elapsed deadline and offers an explicit retry.
[src: file: backend/src/db/discussions.rs:1498-1541#insert_message_with_targeted_dispatch]
  [src: file: backend/src/api/discussions/messaging.rs:115-158#enqueue_dispatches_for_trigger]
[src: file: backend/src/db/agent_dispatch.rs:677-826#recover_after_restart]
[src: file: backend/src/api/discussions/streaming.rs:658-674#timeout_notice]

The pure zero/single-target decision table remains in
`backend/src/api/discussions/routing.rs`. The HTTP adapter applies it once per
explicit target, and the MCP adapter now applies the same policy once per typed
target.
