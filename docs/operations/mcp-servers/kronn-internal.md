# MCP context — kronn-internal

**Server:** `kronn-internal` (Python stdio bridge — `backend/scripts/disc-introspection-mcp.py`)
**Source:** This repo. Auto-injected by Kronn into every supported CLI's MCP config (`.mcp.json`, `~/.codex/config.toml`, `.gemini/settings.json`, `.kiro/settings/mcp.json`, `.vibe/config.toml`).
**Auth:** stdio itself is unauthenticated (local pipe), but the bridge authenticates to the Kronn backend over `KRONN_BACKEND_URL` (default `http://127.0.0.1:3140`): when the backend has a token configured, it exports `KRONN_AUTH_TOKEN` into the process env, the sidecar inherits it and sends `Authorization: Bearer <token>` on every call. On a loopback-only instance the backend's local-trust bypass makes the token optional; on a LAN-exposed instance (e.g. WSL backend / Mac frontend) it is required — otherwise the sidecar's own calls get a silent 401. `[src: file: backend/scripts/disc-introspection-mcp.py:1970-1994]` `[src: file: backend/src/main.rs:102-115]`

## What it does

Bidirectional gateway between a CLI agent (Claude Code, Codex, Gemini, Kiro, Vibe in host-launched mode, …) and the Kronn backend. Three tool families :

1. **Discussion introspection** (0.8.3+) — `disc_meta`, `disc_get_message`, `disc_note_list`, `disc_summarize`. Cheap reads of the current Kronn discussion.
2. **Cross-agent memory** (0.8.4) — `disc_create`, `disc_append`, `disc_link`, `disc_transfer_session`, `disc_unlink`, `disc_find_by_session`, `disc_search`, `disc_load_other`. Push transcripts in / out of Kronn so the same thread can be picked up by a different agent later.
3. **Catalog + actions** (0.8.5–0.8.6) — `mcp_list`, `workflow_list`, `qp_list`, `qa_list`, `workflow_create_draft`, `qp_create_draft`, `api_call` (broker that invokes Kronn-configured APIs without credentials in the prompt).
4. **Multi-agent collab** (0.8.6) — `disc_join` (consume invite token), `disc_wait_for_peer` (long-poll), `disc_leave`. Lets N CLI agents share one Kronn discussion in real time.
   **0.9.2 (KT-76) — surviving an MCP reload without a new token.** `disc_join`
   now links the room to the agent's **durable** CLI session and reports the
   outcome as `session_bound` (plus a reason when false). The link deliberately
   does not use the bridge process id, which rotates on reconnect; it reuses the
   identity behind the resume credential (terminal + project, else
   `CLAUDE_CODE_SESSION_ID` for Claude; a verified native conversation UUID
   for Codex; otherwise the outermost CLI). After a reload,
   `disc_find_by_session({})` — every argument optional — restores the runtime
   binding and durable read cursor before returning the room, so an agent
   normally must not ask the human for another `kr-join` token. A legacy bridge
   may have written the server-side link without ever creating the new local
   resume credential/cursor; this one-time upgrade case returns
   `runtime_bound: false` and `rejoin_required: true` and needs one fresh join
   instead of silently claiming that append is safe. `disc_link({})` binds the
   current session to the bound disc for a room reached another way.
   Both refuse to act rather than guess when no durable identity exists, and
   neither ever passes `force_reassign`: a session owned by another discussion
   is reported, never stolen.
   When the human explicitly asks to move that durable session after joining a
   different room, first read its current owner with
   `disc_find_by_session({})`, then call
   `disc_transfer_session({from_disc_id: "<exact-old-id>",
   confirm_transfer: true})`. The pinned source id makes ownership races fail
   closed; the target is restricted to the room currently joined by the bridge.
   The bridge also requires the target's local resume credential to be durable
   on disk before moving server-side ownership.
   A successful handoff closes the old append-only history row, opens the new
   one atomically and returns `session_bound: true`. Retrying the same completed
   transfer is idempotent.
   Codex bridges upgrading from the former PID-keyed scheme accept the old
   owner-only credential during the first MCP-only reload, then persist the
   rotated successor under the conversation-keyed path so a later full
   `codex resume` reboot remains linked.
   `[src: file: backend/scripts/disc-introspection-mcp.py:2603-2635]`
   `[src: file: backend/scripts/disc-introspection-mcp.py:2839-2857]`
   `[src: file: backend/scripts/disc-introspection-mcp.py:3033-3118]`
   The bridge separately reports a native conversation UUID when Claude Code
   exposes `CLAUDE_CODE_SESSION_ID` or Codex exposes `CODEX_THREAD_ID`. Kronn
   stores it on the participant row and the UI copies the CLI's exact resume
   command. It never substitutes the durable binding key or invents an id for
   another client. When a Codex MCP launcher omits the environment variable, a
   resumed session may recover the same UUID from an ancestor whose command is
   structurally `codex resume <uuid>`; the anchored parser refuses UUIDs merely
   mentioned inside prompts.
   `[src: file: backend/scripts/disc-introspection-mcp.py:2656-2727]`
   **0.9.3 (KT-140) — worktree declaration per joined session.**
   `disc_workspace_get({})` returns the current CLI workspace plus the compact
   list for the room. `disc_workspace_set({task_ref?})` defaults
   `workspace_path` to the bridge process's current directory, while the
   backend canonicalizes the path and reads the real branch and HEAD from Git.
   Callers never submit guessed branch metadata. The path must be a registered
   worktree of the discussion project's primary or linked repositories; a
   physical worktree cannot be owned by two discussions. The declaration
   response carries compact structured blockers (dirty tree, same-branch
   concurrency, missing path, repository scope or ambiguous ownership), so
   agents never need to parse prose errors. The optional task
   reference creates the bidirectional Planning link shown in both the
   discussion header and the task detail.
   `[src: file: backend/src/api/disc_workspace.rs]`
   `[src: file: backend/src/db/sql/101_discussion_workspaces.sql]`
   `[src: file: backend/scripts/disc-introspection-mcp.py]`
5. **Planning** (0.9.1–0.9.3) — `plan_get`, `task_list`, `task_get` and
   `task_changes` provide compact, on-demand task context. Narrow
   `task_create`, `task_update`, `task_update_dod`, `task_link_discussion` and
   `task_add_blocker` writes attribute every change to the calling agent.
   `proposal_list` and `proposal_get` expose the human validation inbox as
   read-only context; no MCP tool can accept or reject a proposal.
   `[src: file: backend/scripts/disc-introspection-mcp.py:169-365]`
   `[src: file: backend/scripts/disc-introspection-mcp.py:3400-3548]`
6. **Opaque-ID resolution** (0.9.2) — `resolve_id` identifies a pasted UUID
   with one indexed backend read instead of making the agent probe each
   object-specific tool.

## Opaque IDs

Call `resolve_id({id})` first when the user pastes an ID without naming its
object type. It supports messages, discussions, projects, workflows, Planning
tasks, Quick Prompts and Quick APIs, and returns only compact routing context:

```json
{
  "kind": "task",
  "id": "…",
  "reference": "KT-62",
  "title": "Résolveur universel",
  "summary": "in_progress · normal",
  "parent": null,
  "suggested_tool": "task_get"
}
```

The endpoint is behind the same backend authentication middleware as the
object APIs. The bridge never receives credentials beyond its existing bearer
token, and the resolver does not return full descriptions, prompt templates,
request bodies or secrets. Follow `suggested_tool` only when the full object is
actually needed.
[src: file: backend/src/db/id_resolver.rs]
[src: file: backend/src/api/id_resolver.rs]
[src: file: backend/scripts/disc-introspection-mcp.py]

## Planning context discipline

- Call `plan_get` to understand the current discussion objective and ordered
  active/later plan; do not reread the transcript for task state.
- Use `task_list` for compact discovery and filtering. It deliberately omits
  descriptions, DoD bodies, links and event logs.
- Call `task_get` only after selecting one task; it accepts both `KT-142`
  references and UUIDs.
- Use `task_changes(discussion_id, since)` for delta refreshes. Planning data
  is not injected into every prompt.
- Keep the plan honest when tracked work starts or materially changes (status,
  DoD, priority, blocker). Do not reload or rewrite an unchanged task merely
  to report progress.
- Immediately before a direct `task_create`, call `plan_get` again so another
  agent's recent write is visible. Direct create links the task atomically to
  the MCP runtime's current discussion by default. Pass an explicit
  `discussion_id` to target another existing discussion, including one just
  returned by `disc_create`. This explicit form does not require the runtime to
  be bound and remains available when `disc_find_by_session` reports
  `rejoin_required`; an unknown target fails without leaving an orphan task.
  Direct create accepts an `idempotency_key`; the bridge scopes explicit
  keys to the effective target discussion, or derives one from
  `source_message_id`. A retry with the same key and
  identical content returns the existing task without a second `created` event.
  Reusing the key for different content is a conflict. Titles are ordinary
  content, never identity: distinct keys may create tasks with the same title.
  Creating several tasks from one source message therefore requires one
  explicit key per logical task.
- MCP writes stamp the client-derived agent identity automatically; callers
  may add `source_message_id` for provenance.
- If the Planning tools named by the contract are absent from the client's
  actual MCP surface, use the bounded `plan_snapshot` returned by `disc_join`
  as read-only context, ask the user to reconnect the Kronn MCP, and never
  fabricate a successful task update.
- When intent is ambiguous, emit a fenced `kronn-plan-action` JSON proposal
  (`create`, `create_many`, `status`, `complete`, `unblock` or `open`) instead
  of writing. Kronn persists mutation proposals with the source Agent message
  and renders them as a human-gated inbox. The user accepts or rejects each
  item; no mutation occurs before acceptance. `open` is navigation-only and is
  never persisted as a proposal.
  `[src: file: backend/src/db/planning.rs:801-850]`
  `[src: file: backend/scripts/disc-introspection-mcp.py:3035-3165]`

Agent-library catalogs deliberately stay compact:
`skills_list` / `profiles_list` / `directives_list` omit their potentially long
instruction bodies. After selecting an id, use `skill_get`, `profile_get`, or
`directive_get` to retrieve the complete object before applying or editing it.

## External CLI session ownership

- A `(source_agent, source_session_id)` pair is a durable resume key, not proof
  that the CLI is currently online. The UI and `/api/disc/session-status`
  expose ownership and live presence separately.
- One concrete session owns at most one discussion. `disc_link` is safe by
  default and rejects a pair already linked elsewhere.
- The converse does NOT hold: a discussion carries **one open binding per joined
  session** (KT-85). A cross-agent room is the normal case, and binding a second
  CLI no longer closes the first — that implicit eviction left every peer but the
  last with an empty resume lookup. A handoff is now explicit: `disc_unlink`, or
  moving the session to another discussion. `GET /api/disc/sources` therefore
  returns several rows for the same `disc_id`; the sidebar renders one chip per
  distinct agent.
- For agents, prefer the dedicated `disc_transfer_session` contract over
  `disc_link(force_reassign=true)`: it requires the exact previous room plus
  `confirm_transfer=true`, restricts the target to the currently joined room,
  and returns a structured durable-binding receipt. The previous history row is
  closed and remains auditable.
- `disc_find_by_session` is the idempotent resume lookup for Claude, Codex and
  other CLI session formats. A bare self-lookup resumes even if the server link
  already exists; an explicit third-party lookup remains a pure read. IDs are
  opaque and must never be guessed.
These reads reuse the existing list endpoints and do not mutate the library.
[src: file: backend/scripts/disc-introspection-mcp.py:1044-1088]
[src: file: backend/scripts/disc-introspection-mcp.py:4223-4234]

## Joined CLI worktrees

- Call `disc_workspace_get({})` before editing when several peers may be
  active. It is a compact read and derives this bridge's durable identity.
- Call `disc_workspace_set({task_ref: "KT-140"})` from the worktree root (or
  pass an explicit `workspace_path`) whenever the CLI changes worktree or
  branch. Kronn refreshes branch and HEAD from Git on each declaration.
- External worktrees are adopted, not managed: the discussion can target them
  for status, diff, commit, push, PR creation and allowlisted terminal commands,
  but lock/unlock never removes an external path. Legacy Isolated worktrees are
  backfilled as `managed` and retain their existing lock/unlock flow.
- The Git panel always makes its target explicit when more than one worktree is
  available. A missing selected worktree is marked `missing` instead of falling
  back silently to the project checkout.
- `task_get` includes a compact `workspaces` list only when the task has linked
  worktrees, so agents do not pay context tokens for empty metadata.

## Message references and local context

Every discussion message exposes the same `#xxxxxxxx` header pill as discussions
and workflows; clicking it copies the full stable message UUID.
[src: file: frontend/src/components/MessageBubble.tsx:289-404]
`disc_get_message` accepts that UUID, the compact `MSG-xxxxxxxx` reference
returned by the API, or the existing positive/negative `idx`. Pass `before`
and/or `after` (0–10) to retrieve only the nearby messages needed for context:

```json
{"message_id":"MSG-12345678","before":2,"after":1}
```

The target message retains its attachment descriptors. Surrounding entries use
a lean shape and omit attachment lookups so a context window remains a single
cheap discussion read plus the target's attachment query. A message that answers
another also exposes its durable `reply_to_message_id`; Kronn adds the compact
reply reference and excerpt to agent history, while portable imports remap the
relation to the imported message ids. Shared-discussion federation carries the
same relation, and the UI preserves a pending reply target across a transient
pre-receipt failure. Calls that use only `idx` remain backward compatible.
[src: file: backend/src/api/disc_introspection.rs]
[src: file: backend/src/api/disc_prompts.rs]
[src: file: backend/src/api/disc_portability.rs]
[src: file: backend/src/api/federation.rs]
[src: file: frontend/src/lib/chat-reply-drafts.ts]
[src: file: backend/scripts/disc-introspection-mcp.py:95-135]

## Out-of-context discussion notes

Messages have two channels: `main` (the normal conversation) and `note`.
Use a note for human/agent observations that must remain visible in the
chronological timeline without waking an agent or silently consuming context:

```json
{"content":"Decision to revisit after the release","channel":"note"}
```

The default agent-facing surface is deliberately silent: prompts, numeric
message indices and neighbours, `disc_join`, `disc_wait_for_peer`,
`disc_search`, `disc_load_other`, and summaries exclude notes unless their
contract exposes and receives an explicit `include_notes=true`. Hidden notes
still advance the durable wait cursor, so reconnecting cannot replay an
already-observed note forever.

Use `disc_note_list({limit, cursor})` for a bounded, paginated note-only read.
An exact `disc_get_message({message_id: "<note UUID>"})` is also allowed, while
ordinary numeric message indices remain `main`-only. `disc_join` reports only a
`note_count`, never note bodies. Export/import and shared-discussion federation
preserve the channel. Notes are routing metadata, **not protected secrets**:
portable discussion exports include their content and attachments like any
other visible timeline message.

[src: file: backend/src/api/disc_introspection.rs:585-626]
[src: file: backend/src/api/disc_invite.rs:249-416]
[src: file: backend/src/api/disc_source.rs:1029-1054]
[src: file: backend/src/api/discussions/messaging.rs:243-319]
[src: file: backend/src/db/discussions.rs:855-899]

## Multi-agent collab — required protocol

When a user gives you a `kr-join-…` invite token :

1. Call `disc_join({token: "kr-join-…"})`. The response carries an explicit
   `next_steps` field plus a bounded `plan_snapshot` — **read and follow them**.
2. **Introduce yourself via `disc_append({content: "<intro>"})`** even if you're the first / only participant. Replying only in your local terminal is INVISIBLE to peers.
3. Loop : `disc_wait_for_peer({timeout_secs: 170})` → on each new message,
   follow its routing hint and `disc_append` your reply only when your exact
   CLI session is addressed (or when an untargeted Agent turn asks the room).
4. Call `disc_leave()` when the task is done or the user says stop.

The bridge auto-derives your `agent_type` from the MCP `clientInfo.name` handshake (Claude Code → ClaudeCode, Codex → Codex, …) so no env-var prep is needed.

The bridge owns a separate durable **read cursor** per joined room. A normal
caller omits `since_sort_order`; the bridge resumes from the last wait result
acknowledged by the CLI's next subsequent tool call. A delivered batch remains
pending until that acknowledgement; after a reconnect before acknowledgement,
it is replayed rather than skipped. The bridge identifies that next call with
its own monotonic sequence, not the client-supplied JSON-RPC id (which may be
reused or absent). This acknowledges transport consumption only: it does not
claim that the model has semantically processed every message. An
append's `last_sort_order` is only its write receipt and must never be reused as
a read cursor: a concurrent peer message can have landed immediately before the
write. A fresh join seeds the cursor from the bounded `recent_messages` already
returned to the agent; a legacy binding without a cursor replays from the start
rather than risking a silent skip.
`withheld_by_routing` separately reports how many newer peer turns were omitted
because they target another identity; their content stays private, but cursor
movement is no longer silent or indistinguishable from transport loss.
Every delivered item also carries a durable `message_id`. The bridge echoes the
batch as `delivered_message_ids` and tells the caller to acknowledge those exact
ids; a reply to one transcript message should pass that id as
`reply_to_message_id`. A locally-authored CLI message additionally carries
`reply_target`, the exact joined session that authored it. With no explicit
mention/target, `disc_append` routes the reply to that identity automatically;
an explicit target still takes precedence. Revision events use their own
durable event id and retain the revised transcript id separately in
`target_message_id`.
[src: file: backend/scripts/disc-introspection-mcp.py]
[src: file: backend/scripts/test_disc_introspection_mcp.py]
[src: file: backend/src/api/disc_invite.rs]

Human turns use typed identities. `discussion_agent` means the configured
native agent, `agent` means a punctual native invocation, and `cli` means one
exact joined session. Provider equality is not ownership: a joined Codex CLI
must not answer a turn addressed to punctual Codex, but it still observes an
untargeted Agent turn authored by native Codex because that is a different
identity. Current bridges send their durable session id, and the wait endpoint
omits unrelated User prompts entirely to avoid consuming context tokens; its
cursor still advances past those hidden turns.

## disc_append — two modes

- **Simple (recommended for live chat)** : `disc_append({content: "..."})`. Bridge auto-fills `disc_id` (from `disc_join` binding), generates `source_msg_id`, defaults `role=Agent`, stamps `agent_type` from `clientInfo`, and resolves ordered mentions to typed targets. Canonical `@codex` / `@claude` aliases mean native identities; `@codex-cli`, `@codex-cli-2`, … mean exact joined sessions and are never interchangeable with the native alias. Pass the optional opaque `reply_to_message_id` to create a durable reply to a message in the same discussion. Unless the append also carries an explicit target, Kronn sends that reply to the exact local CLI author recorded on the referenced message.
- Mentions inside inline/fenced code are documentation and never dispatch. Use
  code formatting whenever you need to discuss an alias without invoking it.
- **Bulk (transcript import)** : `disc_append({disc_id, messages: [{source_msg_id, role, content, agent_type}, …]})`. Idempotent on `(disc_id, source_msg_id)`.
- An untargeted simple live Agent append wakes the discussion's native principal
  so it can answer the joined peer. Explicit native targets create their own
  durable one-shot jobs; an exact CLI target is delivered only to that session.
  The bridge session id distinguishes live chat from imported transcripts;
  duplicates, bulk appends and no-agent discussions do not launch a native
  runner.

## Project rules

- **Default to the simple mode** for any conversational `disc_append`. The bulk mode is for cross-agent-memory transcript replay only.
- **Never block waiting for confirmation** to call a `disc_*` tool — the protocol is in-band (each tool's `description` field carries enough context). This file is supplementary.
- **`api_call`** : invoke a configured API plugin without ever needing the credentials. The `mcp_list` tool returns the available endpoints with `${ENV.X}` placeholder support — the broker substitutes server-side.
- **Mutating tools** (`disc_create`, `qp_create_draft`, `workflow_create_draft`) default to safe states (workflows created as `enabled: false`). Safe to call ; the user reviews before activation.

## Common use cases in Kronn

- Pick up a discussion started by another CLI agent (cross-agent memory).
- Participate in a live multi-agent discussion (e.g. 2 agents debugging together, one agent acting as reviewer).
- Surface the user's Kronn-configured plugins (`mcp_list`) without re-asking for credentials.
- Draft a workflow / QP from the agent side, then let the user review + enable in the Kronn UI.

## Related

- `backend/src/api/disc_invite.rs` — invite token + peer-join + wait-for-peer endpoints.
- `backend/src/db/discussion_sessions.rs` — sessions table that powers the header participants list.
- `backend/src/api/disc_source.rs` — cross-agent memory endpoints (`disc_create`, `disc_append`, …).
- `backend/src/api/agent_api.rs` — the broker behind `api_call`.
