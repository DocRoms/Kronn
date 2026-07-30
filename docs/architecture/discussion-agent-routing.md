# Discussion agent routing

Kronn routes human turns to durable **identities**, not provider names. Three
responders may all be Codex without being interchangeable:

- `discussion_agent`: the agent configured for the discussion;
- `agent`: a native punctual agent invoked for this turn only;
- `cli`: one exact joined CLI session, identified by its session row.

`message_targets` preserves those identities in textual order.
`messages.target_agent` and `target_agents` remain compatibility projections
for older clients and peers.
[src: file: backend/src/models/discussions.rs:340-430]
[src: file: backend/src/db/sql/099_typed_message_targets.sql:1-38]

The composer parses prose once and submits the typed target list. The backend
validates that every CLI session belongs to the discussion before accepting
the message; it never reparses Markdown to guess an owner.
[src: file: frontend/src/lib/messageTargets.ts:1-120]
[src: file: backend/src/api/discussions/messaging.rs:47-125]

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
[src: file: frontend/src/lib/messageTargets.ts:25-120]

Mentions inside inline code or fenced code are examples, not dispatch requests.
This is the supported escape hatch when a human or agent needs to discuss the
syntax itself (write `` `@codex` `` rather than a live mention). The UI composer
and MCP bridge apply the same exemption before resolving targets.
[src: file: frontend/src/lib/messageTargets.ts]
[src: file: backend/scripts/disc-introspection-mcp.py]

| Room mode | Target state | Result |
|---|---|---|
| Native enabled | No target or `discussion_agent` | One native principal job. |
| Native enabled | `agent` | One native job with that punctual override. Runtime unavailability keeps the durable obligation retryable; it is never reassigned by provider-name coincidence. |
| Any mode | `cli` | No local process; the exact joined session receives the User turn through `disc_wait_for_peer`. |
| Native enabled | Mixed identities | Native identities become durable jobs; CLI identities receive their own copy. |
| Native disabled (`no_agent`) | Untargeted | No native process; joined CLIs retain the autonomous-room broadcast behaviour. |
| Native disabled (`no_agent`) | Native identity | No local process starts. |
[src: file: backend/src/api/discussions/messaging.rs:120-305]
[src: file: backend/src/api/discussions/messaging.rs:430-485]

The room mode is an explicit persisted operator choice. Disabling the native
discussion agent sets `discussions.no_agent` without pausing, removing, or
hiding joined peers. The mode switch retires pending native dispatches in the
same transaction, and the dispatcher's final claim also checks that native
mode is still enabled.
[src: file: backend/src/db/agent_dispatch.rs:279-343]

Current bridges send their durable session id on every wait. For those callers,
an explicitly targeted User **or Agent** turn is returned only when an exact
`cli` target matches that session; native `agent` targets never wake a
same-provider CLI by coincidence. Hidden turns still advance
`latest_sort_order`, so an unrelated CLI neither wakes nor loops on the same
invisible message. Untargeted Agent turns remain room-visible for collaboration.
Legacy bridges without a session id retain the old awareness projection during
rolling upgrades.
[src: file: backend/src/api/disc_invite.rs:713-790]
[src: file: backend/src/api/disc_invite.rs:920-1065]
[src: file: backend/scripts/disc-introspection-mcp.py:4086-4160]

## Queue, restart, and edit/resend

| Situation | Contract |
|---|---|
| User sends while a response is streaming | The UI queue merges pending text into one later human turn and unions explicit targets in queue order, deduplicated. |
| Several local targets | Every job commits with the human message. The first is claimed for the active SSE stream; later jobs remain Pending and drain sequentially. |
| Backend restart after acceptance | Running jobs return to the durable queue and Pending jobs remain present; the per-message/per-target dedupe key prevents duplicate obligations. |
| HTTP retry with the same `client_message_id` | The existing human message is returned as a duplicate and no additional target job is created. |
| Edit/resend while a response is active | Rejected with `DispatchInProgress`; the current obligation is not mutated underneath a runner. |
| Successful edit/resend | Replaces the message's target list atomically with the content revision and creates at most one new durable job per absent listed target. |
| Federation/catch-up | The ordered target list travels with live messages and revision events; older peers degrade to the first `target_agent`. |
[src: file: frontend/src/hooks/useMessageQueue.ts:67-104]
[src: file: backend/src/db/discussions.rs:843-1028]
[src: file: backend/src/db/discussions.rs:1700-1920]
[src: file: backend/src/api/federation.rs:20-110]

## Joined agent turn (MCP)

A live MCP `disc_append` carries the same ordered `targets` model as the human
composer. The bridge resolves canonical native mentions (`@codex`) separately
from exact joined aliases (`@codex-cli`, `@codex-cli-2`, …) using the
discussion's participant order. Several mentions fan out once per durable
identity. The legacy single `target_agent` field remains accepted during
rolling upgrades, with its historical provider-level semantics.
[src: file: backend/scripts/disc-introspection-mcp.py]
[src: file: backend/src/api/disc_source.rs]

For each native identity, the message and its dispatch obligation commit in one
transaction. Exact CLI identities create no local runner: the addressed session
receives the Agent turn through its next poll, while every unrelated CLI skips
it and advances its cursor. Bulk imports, historical appends, duplicates and
appends without a live JOIN session never start a runner.
[src: file: backend/src/db/discussions.rs]
[src: file: backend/src/api/disc_invite.rs]

Every verified live MCP append also records its exact author session in
`message_cli_authors`. This is local routing provenance, not portable transcript
data: imports and federated messages never acquire a local CLI identity. When a
later live peer uses `reply_to_message_id` without an explicit target, Kronn
routes the reply to that exact author session. Explicit typed or legacy targets
still win. This lets two Codex CLIs answer one another without waking the native
Codex agent or filtering the sibling as “self”; only the author session excludes
its own append. An untargeted message from the native Codex agent remains a real
peer turn for a Codex CLI — provider equality alone is never treated as
self-authorship. Messages written before this provenance existed may be replayed
once when an old cursor is restored because Kronn cannot safely guess which
historical CLI wrote them. `disc_wait_for_peer` and `disc_get_message` expose
the identity as `reply_target` so agents can inspect the decision without
loading session tables.
[src: file: backend/src/db/sql/100_message_cli_authors.sql]
[src: file: backend/src/api/disc_source.rs]
[src: file: backend/src/api/disc_invite.rs]
[src: file: backend/src/api/disc_introspection.rs]

An untargeted live peer append in a native discussion deliberately addresses
the native principal: this is how an invited CLI hands work back to the
discussion's normal agent. Peer-to-peer exchanges that must exclude that
principal use a structured target; autonomous rooms use no-agent mode.
[src: file: backend/src/api/disc_source.rs:345-383]

## Persistence and interruption

- `message_targets` records every intended addressee kind, agent type and
  optional exact CLI session in order.
- `message_cli_authors` records the exact local CLI author for verified live MCP
  appends, enabling deterministic `reply_to` routing across same-provider peers.
- `messages.target_agent` remains the first-target compatibility projection.
- `agent_dispatch_jobs.agent_override_json` records the native one-shot for
  one target. Dedupe keys include the message (or revision) and target agent.
- A backend restart resets a claimed durable job to pending for the boot drain.
- A watchdog timeout is a completed failed attempt, not a restart. Its message
  names the elapsed deadline and offers an explicit retry.
[src: file: backend/src/db/discussions.rs:1480-1520]
[src: file: backend/src/db/discussions.rs:1852-1905]

The pure zero/single-target decision table remains in
`backend/src/api/discussions/routing.rs`. The HTTP adapter applies it once per
explicit target, and the MCP adapter now applies the same policy once per typed
target.
