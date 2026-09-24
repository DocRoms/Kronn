# MCP context — kronn-internal

**Server:** `kronn-internal` (Python stdio bridge — `backend/scripts/disc-introspection-mcp.py`)
**Source:** This repo. Auto-injected by Kronn into every supported CLI's MCP config (`.mcp.json`, `~/.codex/config.toml`, `.gemini/settings.json`, `.kiro/settings/mcp.json`, `.vibe/config.toml`).
**Auth:** stdio itself is unauthenticated (local pipe), but the bridge authenticates to the Kronn backend over `KRONN_BACKEND_URL` (default `http://127.0.0.1:3140`): when the backend has a token configured, it exports `KRONN_AUTH_TOKEN` into the process env, the sidecar inherits it and sends `Authorization: Bearer <token>` on every call. On a loopback-only instance the backend's local-trust bypass makes the token optional; on a LAN-exposed instance (e.g. WSL backend / Mac frontend) it is required — otherwise the sidecar's own calls get a silent 401. `[src: file: backend/scripts/disc-introspection-mcp.py:1970-1994]` `[src: file: backend/src/main.rs:102-115]`

## Human arbitration cards

A blocking human decision must use a `kronn-question` fence in an Agent/main
message sent with `disc_append`, never prose alone. First call
`disc_question_list({})` to avoid asking for an existing decision twice. Read
`tool_manual({tool: "disc_question_list"})` for field limits. Example:

````markdown
```kronn-question
{"version":1,"key":"quota-policy","question":"How should a depleted provider be re-enabled?","context":"This decision blocks only KT-593.","options":[{"id":"manual","label":"Explicit re-enable"},{"id":"expiry","label":"Expiry policy"}],"recommended_option_ids":["manual"],"task_ref":"KT-593"}
```
````

`version` must be the JSON **number** `1`. The string `"1"` is invalid: do not
copy the version type from delivery/review manifests, which use a different
protocol. The example above is complete and valid, including the closed fence.

After `disc_append`, call `disc_question_list({key: "quota-policy"})` with the
exact published key and verify that a row exists. A successful message write
receipt does not prove that its question was accepted. If absent, check the
fence and JSON field types, then republish the corrected payload with the same
key. An absent row is neither a pending nor an answered decision, and never
permits execution of the affected lot.

The stable `key` is unique within the room. Repeating it preserves one immutable
card, not a second card or a silent change behind an already-open human form.
Use a new key for a genuinely different question. Valid closed fences are
ingested atomically with the source message; malformed, nested example, user
and note fences do not create a card. `version`, `key`, and `question` are
required; choices, context, recommendations, multiple selection and task
reference are optional. Free-text answers are always available. A recommendation
is not a preselected answer or consent.

Pause execution, delegation and completion of the affected lot until the human
answers. Continue unrelated tasks and keep listening with `disc_wait_for_peer`.
This is a scoped agent protocol, not a global scheduler stop: `task_ref` does not
rewrite Planning status or block unrelated executions. After handoff or restart,
`disc_question_list({key: "quota-policy"})` returns the durable question and
answer, independently of the original CLI session or its read cursor.

Only the human UI submits an answer; there is no MCP answer tool. The authenticated
HTTP endpoint atomically writes the answer, a User receipt replying to the source,
and its routing intent. Exact replay is idempotent; a conflicting second answer
returns 409. Offline CLI provenance stays exact without spawning a substitute
native agent. Native answers enqueue one durable dispatch. Every room reader can
still recover the decision if the requester is no longer present.

A human can also **decline** an arbitration instead of answering it (0.13.0). A
question is not always a question worth answering: the agent may have asked the
wrong thing, the lot may have been abandoned, or the decision may have been taken
elsewhere. Before this, the only ways out were to answer something untrue or to
leave the card pending forever — and a pending card blocks its lot.

A declined question is a third settled state, not a deletion and not an answer.
It resolves the card and unblocks the lot; the agent is told the decision was
declined rather than being handed a choice nobody made. History keeps it: the
question, who declined it and when all remain readable, because "this was refused"
is itself an answer to the next agent that considers asking again.
`declined` was added to the state CHECK constraint by rebuilding the table —
SQLite cannot extend one in place (migration 175 is the precedent).
`[src: file: backend/src/db/sql/177_discussion_question_declined.sql:1]`

API contract: `GET /api/discussions/{id}/questions` returns
`{questions, pending_count}` including answered history;
`POST /api/discussions/{id}/questions/{question_id}/answer` accepts
`{selected_option_ids, text?, idempotency_key}` and returns the updated question;
`POST /api/discussions/{id}/questions/{question_id}/decline` settles it as
declined. Answering and declining share one resolution path
(`publish_human_resolution`), so a declined card produces the same receipt,
routing and idempotency guarantees as an answered one — they cannot drift apart.
Discussion list items expose `pending_question_count`, including pagination.
`[src: file: backend/src/db/discussion_questions.rs:1]`
`[src: file: backend/src/api/discussion_questions.rs:1]`
`[src: file: backend/src/db/sql/168_discussion_questions.sql:1]`

## Steering cards (`kronn-important`)

A decision, a scope change, a DoD waiver, a blocking alert, an action needed
from a human or an accepted delivery is published as a `kronn-important` fence,
which mints a DURABLE card — not a bold line, a colour or an emoji. The
discussion can then filter, count and step through them.

It is **not** a delivery report. An accepted delivery stays a
`delivery_summary`; a fact inside one worth steering on becomes a SEPARATE card
that `references` the delivery. Nothing is ever reclassified.

`dedup_key` is the identity of the FACT, so a replay after a restart collapses
onto the row it already wrote instead of doubling it. Only the FIRST fence in a
message publishes, and the `note` channel publishes none.

**Who may publish**, and the distinction matters:

- publishing **in your own name** — an agent's fence, or a human's from the
  composer — needs a credential enrolled in Settings plus a single-use proof
  bound to that room and that exact body. An install with no credential
  publishes none of these: the message still posts, no card is recorded, and
  the response says which;
- **Kronn's own orchestration events** — a terminal transition, a review that
  requests changes, a campaign parked on a human — publish with no credential
  at all. There is no caller to authenticate: the backend produces them from a
  transition that already happened.

A **worker never publishes**, holding a valid credential or not. Report through
your delivery, and put a decision to the human with `kronn-question`.

Full contract: [`../../architecture/important-messages.md`](../../architecture/important-messages.md)
and [`../../architecture/important-message-publication-authority.md`](../../architecture/important-message-publication-authority.md).
`[src: file: backend/src/db/discussion_important.rs:1]`

## What it does

Bidirectional gateway between a CLI agent (Claude Code, Codex, Gemini, Kiro, Vibe in host-launched mode, …) and the Kronn backend. Three tool families :

1. **Discussion introspection** (0.8.3+) — `disc_meta`, `disc_get_message`, `disc_note_list`, `disc_summarize`. Cheap reads of the current Kronn discussion.
2. **Cross-agent memory** (0.8.4) — `disc_create`, `disc_append`, `disc_update`, `disc_link`, `disc_transfer_session`, `disc_unlink`, `disc_find_by_session`, `disc_search`, `disc_load_other`. Push transcripts in / out of Kronn so the same thread can be picked up by a different agent later.


`disc_update` renames a discussion, pins it or archives it — and nothing else
(KT-21). The route behind it also accepts the agent, the model tier, the
connection, the project and the skills/profiles/directives bound to the room;
none of those is reachable from the tool. They decide who answers, with which
credentials and under which instructions, so they stay a human decision in the
UI. A rename is visible and reversible, but it is how a human finds their
thread again: ask before renaming a room you did not open.
3. **Catalog + actions** (0.8.5–0.10.0) — `mcp_list`, `workflow_list`, `qp_list`, `qa_list`, `qe_list`, `workflow_create_draft`, `qp_create_draft`, `qa_create_draft`, `qe_create_draft`, `qe_update`, `qe_run`, `api_call` (broker that invokes Kronn-configured APIs without credentials in the prompt).
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
   An accepted delegated CLI worker also persists a narrow child-to-origin
   handoff marker. After the execution reaches a terminal state, the bridge may
   return to the origin without a new join token only through the dedicated
   authenticated recovery endpoint. The backend requires the exact session,
   credential, accepted execution, child/origin pair, durable source ownership,
   membership, and both orchestrator return traces. Ordinary resume remains
   strict about its expected room, and a runtime already bound to a third room
   is never moved by the handoff marker. Child messages already returned by a
   wait are delivered before a later wait follows the proven return; the
   origin's own acknowledged cursor is restored rather than replacing it with
   the child's cursor.
   `[src: file: backend/scripts/disc-introspection-mcp.py:3521-3580]`
   `[src: file: backend/scripts/disc-introspection-mcp.py:4008-4074]`
   `[src: file: backend/src/db/cli_worker_bindings.rs:190-277]`
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
   Sessions in the same discussion may intentionally declare the same physical
   checkout. Before `rebase`, squash, `reset` or force-push in such a checkout,
   create a backup ref at the current HEAD (for example
   `git update-ref refs/kronn-backup/<name> HEAD`) and call
   `disc_workspace_history_lease({action: "acquire", backup_ref: "refs/kronn-backup/<name>"})`.
   Kronn verifies that the ref resolves to the declared HEAD and refuses a
   second session while the 15-minute lease is active. Release it afterward
   with `{action: "release"}`. This is deliberately advisory: Kronn arbitrates
   cooperating agents but cannot intercept a CLI that runs Git without asking.
   `[src: file: backend/src/api/disc_workspace.rs]`
   `[src: file: backend/src/db/sql/101_discussion_workspaces.sql]`
   `[src: file: backend/scripts/disc-introspection-mcp.py]`
5. **Planning** (0.9.1–0.9.3) — `plan_get`, `task_list`, `task_get` and
   `task_changes` provide compact, on-demand task context. Narrow
   `task_create`, `task_update`, `task_update_dod`, `task_link_discussion` and
   `task_add_blocker` and `task_remove_blocker` attribute every dependency
   change to the calling agent. Removal is a narrow, idempotent write: it
   removes only the selected edge and leaves task status and free-form blocked
   reason untouched.
   `proposal_list` and `proposal_get` expose the human validation inbox as
   read-only context; no MCP tool can accept or reject a proposal.
   `[src: file: backend/scripts/disc-introspection-mcp.py:169-365]`
   `[src: file: backend/scripts/disc-introspection-mcp.py:3400-3548]`
6. **Opaque-ID resolution** (0.9.2) — `resolve_id` identifies a pasted object id
   with one resolver request instead of making the agent probe each
   object-specific tool.

## Opaque IDs

Call `resolve_id({id})` first when the user pastes an ID without naming its
object type. The supported boundary is every **public MCP-addressable object**:
an object that Kronn exposes through a reading or discovery tool. The resolver
returns only compact routing context:

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

### Public ID routing matrix

| Resolved kind | Storage / selector | Canonical MCP reading route |
|---|---|---|
| `message` | SQLite UUID | `disc_get_message` |
| `discussion` | SQLite UUID | `disc_load_other` |
| `project` | SQLite UUID | `task_list(project_id=…)` |
| `workflow` | SQLite UUID | `workflow_get` |
| `workflow_run` | SQLite UUID; parent is the Workflow | `workflow_run_get` |
| `task` | SQLite UUID or copyable `KT-###` reference | `task_get` |
| `planning_proposal` | deterministic proposal id | `proposal_get` |
| `task_execution` | SQLite UUID | `task_exec_status` |
| `quick_prompt` | SQLite UUID | `qp_get` |
| `quick_api` | SQLite UUID | `qa_list` |
| `quick_exec` | SQLite UUID | `qe_list` |
| `page` | SQLite UUID or slug | `page_get` |
| `mcp_server` | stable plugin id | `mcp_list` |
| `mcp_config` | SQLite UUID; parent is the plugin | `mcp_list` |
| `skill` | builtin/custom library id | `skill_get` |
| `profile` | builtin/custom library id | `profile_get` |
| `directive` | builtin/custom library id | `directive_get` |

The SQLite families are checked in one indexed query; the three agent-library
selectors are then checked in-process. A value found in more than one family is
returned as a typed `conflict`, never as an arbitrary match. Unknown values are
returned as `not_found`. Internal evidence-row ids (dispatches, revisions,
delivery/review receipts, validation rows, judge rows) are deliberately outside
this contract because no standalone MCP reading tool exposes those rows; their
owning public object is the addressable boundary.

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
- A Kronn-launched CLI receives its current discussion UUID in the first-turn
  Planning notice. Prefer that explicit UUID for `plan_get` and `task_create`:
  it remains valid when a stale bridge reports `no disc bound` or
  `rejoin_required`, so the agent must not ask for a join token merely to write
  the room that launched it.
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

Ollama and LiteLLM do not run this stdio bridge. Kronn exposes their compact
Planning catalogue natively and executes it through the same HTTP handlers,
with the discussion scope and actor provenance supplied server-side. Vibe's
programmatic runner deliberately disables MCP; its supported Planning write
path is the human-gated `kronn-plan-action` fence.
`[src: file: backend/src/api/agent_tools.rs]`
`[src: file: backend/scripts/vibe-runner.py]`

## Task execution lifecycle

Principal agents that can mutate Planning receive the typed execution lifecycle:
`agent_list`, `task_exec_prepare`, `task_exec_launch`, `task_exec_status`,
`task_exec_resume`, `task_exec_deliver`, `task_exec_review`, `task_exec_cancel` and
`task_exec_reassign`. A principal must preflight before launch and reuse one
idempotency key after an uncertain response. Native HTTP workers receive a
narrowed surface: no backlog mutation or execution-status lookup, and
`task_exec_deliver` accepts only the manifest. They never merge, approve or
close the task.
The principal is either a CLI session joined to the room or the room's own
native agent during its turn. For the latter Kronn injects
`KRONN_ROOM_AGENT_CONTEXT` (room, provider, running dispatch, trigger message)
into the bridge environment; the backend accepts it only while that dispatch
runs in the room, for the same provider, and never for a delegated worker's
dispatch.
The optional `validations` passed to `task_exec_launch` are principal-owned and
persisted on the implicit single-task run. They use the same `ValidationSpec`
contract as campaign runs and cannot be supplied or changed by the delivery
manifest.

`task_exec_status` returns `next_action.tool = task_exec_resume` only for a
publicly recoverable Applying-origin checkpoint. The principal may then call
`task_exec_resume`, which uses the backend's guarded resume path: it rechecks
the parent checkout and recorded SHAs, refuses dirty or unrelated states, and
returns the existing terminal result when the same successful resume is
retried. The tool cannot advance provisioning- or review-owned checkpoints.
[src: file: backend/src/api/orchestration.rs:6492-6499]
[src: file: backend/src/api/orchestration.rs:7955-7997]
[src: file: backend/scripts/disc-introspection-mcp.py:921-941]
[src: file: backend/scripts/disc-introspection-mcp.py:5437-5475]

When the worker identity is not already known, call `agent_list()` first and
copy one returned `worker` object unchanged into `task_exec_prepare`. Native
HTTP providers are `discussion_agent` targets, punctual host processes are
`agent` targets, and a joined CLI is an exact `cli` target carrying its durable
`cli_session_id`.

Since 0.13.0 the catalogue also lists **every configured external API
connection**, one entry per connection, with its tiers and a `media` array
naming the modalities it can generate. A connection has no local binary, so
agent detection produced nothing for it: an OpenRouter or LiteLLM row an
operator had configured and could mention in a room was simply absent from the
catalogue, and no agent could delegate to it or learn that image and video
generation existed at all. `media` lists one entry per configured model and
stays absent otherwise, so reading it is enough — an agent is never told about a
modality the generation request would then refuse.

`agent_list` answers in every discussion, a room or not (0.13.2). Reading the
catalogue is not a delegation: nothing in it is secret or scoped to one party.
It used to sit behind the room-membership guard of `task_exec_prepare` and
`task_exec_launch`, which an agent Kronn launches in a plain discussion can
never pass, so image and video generation was unreachable there without a
human pasting a connection id. The two delegation tools keep that guard.
`media_generate` takes the entry's `worker.connection_id`, or the connection's
alias or display name; a name two connections share is refused rather than
guessed, and every refusal lists the connections that can generate the
modality asked for, with their ids.
[src: file: backend/src/api/orchestration.rs] [src: file: backend/src/api/media.rs]
[src: file: backend/src/db/external_api_connections.rs]

The catalogue reports `configured` and `reachable`
independently; its only strict implication is
`available => configured && reachable`. Availability is deliberately only a
transport preflight, never a claim about task fit or model quality. Probe
failures are reduced to stable reason codes and fixed phrases: upstream errors,
keys, endpoints and hostnames are not returned. For Ollama, this discovery does
not assert that the exact resolved tag is already pulled; a missing tag remains
a separate, explicit `/api/chat` launch failure.

The stdio bridge fingerprints the script contents it loaded. Every orchestration
mutation, including principal review/cancel and worker commit/delivery, passes a
central freshness guard before any HTTP request; recovery status reads remain
available while the bridge is stale.
The stale response is typed as `bridge_stale`, confirms that no mutation was
applied and schedules one preflighted self-reexec over the inherited stdio
transport. The bridge advertises tool-list change support and emits
`notifications/tools/list_changed` from the newly loaded process, so the
notification is both a schema-refresh request and a readiness barrier: an eager
host cannot race its retry into the stale reader thread. A versioned handoff preserves
queued requests, relevant cancellation generations, partial JSON-RPC lines and the
original MCP `clientInfo`; unread bytes remain in the inherited pipe. The handoff is
written to a bounded private `0600` inode and authenticated with a random nonce carried
separately in the inherited environment. Both temporary descriptors are unlinked
immediately after creation, before their first write, and their zero link count is
verified fail-closed. The replacement therefore reads the exact inherited handoff
descriptor after validating its type, owner, mode, link count and strict payload schema;
it never reopens a handoff pathname. The replacement also executes the same unlinked
artifact descriptor that passed import preflight and a
final digest check, then closes that bootstrap descriptor once Python has loaded the
new process image. After reload, source-relative resources and `bridge_info.script_path`
continue to use the canonical bridge source path rather than the temporary artifact.
An active audit SSE stream defers the reload with a typed diagnostic
instead of being killed. Retry the refused mutation once with the same idempotency
key. If preflight, handoff or reexec fails, the bridge stops automatic attempts and
reports one precise manual reconnect/recovery action instead.
`[src: file: backend/scripts/disc-introspection-mcp.py:9522-9550]`
`[src: file: backend/scripts/disc-introspection-mcp.py:75-95]`
`[src: file: backend/scripts/disc-introspection-mcp.py:2738-2810]`
`[src: file: backend/scripts/disc-introspection-mcp.py:10054-10222]`
`[src: file: backend/scripts/disc-introspection-mcp.py:10225-10253]`
`[src: file: backend/scripts/disc-introspection-mcp.py:10441-10495]`
`[src: file: backend/scripts/disc-introspection-mcp.py:10656-10669]`

The replacement process restores the room and durable read cursor through the
existing binding files; no invitation token is needed. A changed mtime with
identical contents is not stale. `task_exec_status` remains available for
recovery; every orchestration mutation stays fail-closed until the fresh bridge
is ready.
`[src: file: backend/scripts/disc-introspection-mcp.py:5894-5951]`
`[src: file: backend/scripts/disc-introspection-mcp.py:6156-6181]`
`[src: file: backend/scripts/disc-introspection-mcp.py:6345-6413]`

The first transition to this fingerprinted bridge cannot be self-protected by a
process that loaded the preceding schema. After upgrading to 0.11.0, reconnect
each already-running `kronn-internal` MCP once and confirm that `bridge_info`
reports `stale: false` before launching or reassigning work.

For a verified tiny native-HTTP mutation, `task_exec_prepare` and
`task_exec_launch` accept either
`worker_scope: {mode: "prelocalized_edit", path, start_line, end_line}` or, for
a pure insertion, `{mode: "prelocalized_insert_after", path, anchor_line}`.
Pass the identical scope to both calls. Kronn validates it in the pinned
worktree, then mechanically limits the worker to one exact read followed by one
receipt-bound mutation. `prelocalized_insert_after` exposes only the new text
and preserves the anchor; `prelocalized_edit` replaces its inclusive range,
which is limited to 200 lines. Both are refused for CLI workers. See
`docs/operations/ollama-local-models.md` for the complete eligibility and
fallback contract.
After reconnect, restore the room, refresh the plan and read the existing
execution status instead of launching another attempt. The on-demand
`tool_manual({tool: "task_exec_prepare"})` contains complete native and exact-CLI
examples.

Caller identity is transport-specific but always server-derived. The stdio
bridge injects its durable `(source_agent, source_session_id)` and the backend
resolves the exact active CLI session. Native Ollama, LiteLLM and NVIDIA tools
omit every caller field: Kronn authorizes the trusted executor's current parent
room for principal actions, or its child room plus exact typed provider for
worker delivery. Unknown executions and foreign callers share the same opaque
refusal. Planning event rows separately persist `actor_session_id`, so two
concurrent sessions of the same provider remain distinguishable in the audit
trail.

For a native HTTP worker, the execution id is also server-derived. The executor
uses its trusted dispatch job to resolve the execution, then revalidates the
child room, provider and trigger message before parsing the manifest. The
worker schema contains no execution-id field; if a client nevertheless injects
one, it has no influence. The regression test uses two real concurrent
executions and forges B's valid id in worker A's call to prove that A remains
the only target.
`[src: file: backend/src/api/agent_tools.rs]`
`[src: file: backend/src/api_tests.rs]`

Accepting a CLI worker control offer (`task_exec_accept_worker_offer`) crosses
two deliberately distinct identity domains (KT-421). The live
`(source_agent, source_session_id)` pair — the same one every other lifecycle
tool sends — proves the caller is the exact `discussion_sessions` row the
offer targets; it rotates across an MCP reload (`adhoc-*`). A separate
`source_binding_session_id` names the reload-stable `disc_source_history`
binding that actually moves the session origin -> child; it stays `cli-*`
even after the live identity above has rotated. Collapsing the two, as an
earlier revision did, makes a resumed CLI's own offer permanently
unacceptable: its active identity no longer matches the durable one the
accept step used to reuse for both checks. Both values are derived by the
trusted bridge from its own state and are absent from the tool's input
schema — the model supplies only `offer_id`; there is no fallback to
agent-type/alias matching or any other permissive resolution if either is
missing.
An old bridge that has not reloaded this contract sends only the legacy pair
and fails the request explicitly (`source_agent, source_session_id, and
source_binding_session_id are required`), not silently with a stale or wrong
attach. If `task_exec_accept_worker_offer` refuses this way, reconnect the
`kronn-internal` MCP server for that session before retrying.
`[src: file: backend/src/api/orchestration.rs]`
`[src: file: backend/scripts/disc-introspection-mcp.py]`

`task_exec_reassign(reason)` persists the reason and includes it verbatim in
the replacement worker's handoff message. It is therefore a real recovery
instruction, not an audit-only label. The chosen provider, tier, model and
profile are synchronised to the durable child discussion in the same database
transaction: that discussion is what the runtime resolves when it starts the
replacement. If the room cannot be updated, the reassignment is refused rather
than displaying one model in the execution while silently running another.
The child room's initial task brief is pinned across prompt truncation. For a
native HTTP worker or a spawned host CLI worker,
`task_exec_deliver(manifest)` exposes only semantic fields:
tests, ordered `{met, evidence}` DoD assertions, docs, migrations, risks,
limitations and summary. After exact worker authorization, Kronn derives the
contract version, task reference, committed HEAD/file inventory and the
launch-time snapshotted DoD ids; a later DoD reorder/replacement is refused.
The persisted result is still a complete DeliveryManifest v1. A joined CLI
session and public/principal delivery retain the full manifest schema.
Principal approval is bound to that delivery's exact HEAD and must include one
non-empty evidence record for every current DoD id, all marked met. Worker
claims and the task's live checkbox state remain useful context but never
substitute for the principal's attempt-scoped review. Only after approval does
Kronn execute the persisted mechanical validations on the ephemeral merge
candidate; a red command returns the execution to the worker instead of
advancing the parent branch.
Workspace mutations remain guarded by a
whole-file SHA-256 receipt; native tools accept only a 32-to-64-character
leading hexadecimal prefix, compare it against the current full hash, and
refuse shorter or stale values without writing. This tolerates a local model
truncating the tail of a long receipt without weakening the guard below 128
bits.

A Claude/Codex-style host CLI launched as a `kind=agent` task worker is not a
joined CLI session and therefore uses a third, deliberately smaller surface.
The runner injects an opaque execution/child/provider/dispatch capability into
that one process. In this mode `initialize` and `tools/list` expose only
`task_exec_deliver(manifest)`; every other Kronn MCP call is refused even if the
client guesses its name. The bridge ignores any out-of-schema execution id and
forwards the runner capability to the existing native delivery path, which
revalidates the exact child room, provider and dispatch trigger against SQLite.
Two concurrent workers therefore cannot deliver each other's execution.
Claude Code does not propagate session-level variables to stdio MCP children,
so its strict per-run MCP config explicitly maps only the worker capability,
discussion id, backend URL and optional auth token. The inline JSON contains
environment placeholders rather than their values; the opaque capability does
not appear in the process arguments.
Codex has an additional transport boundary: it forwards only variables named in
the MCP entry's `env_vars`. Kronn writes a narrow `KRONN_*` allowlist into the
global Codex config and repeats it as a per-run `-c` override, so the dynamic
discussion/task capability reaches that run immediately without rewriting
shared config or leaking unrelated host variables.
`[src: file: backend/src/agents/runner.rs]`
`[src: file: backend/src/api/discussions/streaming.rs]`
`[src: file: backend/scripts/disc-introspection-mcp.py]`

Native HTTP workers can finish the Git side of that contract without receiving
a shell. Their catalogue includes `git_commit`, but execution is refused unless
SQLite proves that the caller is the exact current provider/dispatch in the
child discussion and that the canonical directory is its attached, managed,
`Working` task worktree. The tool validates every named relative path before it
stages the first one, commits only those paths, and exposes no amend, checkout or
push option. Project-scoped Workflow Agent steps do not receive it. This keeps a
local Ollama/LiteLLM/NVIDIA worker capable of producing the clean committed HEAD
required by `task_exec_deliver` without turning the HTTP tool catalogue into
arbitrary Git access.
`[src: file: backend/src/api/agent_workspace_tools.rs]`
`[src: file: backend/src/api/agent_tools.rs]`
`[src: file: backend/src/db/orchestration.rs]`

`[src: file: backend/scripts/disc-introspection-mcp.py]`
`[src: file: backend/src/api/orchestration.rs]`
`[src: file: backend/src/api/agent_tools.rs]`
`[src: file: backend/src/db/sql/136_planning_actor_session.sql]`

## Rich discussion output

Every Kronn discussion message supports Markdown. Agents launched by Kronn and
CLI agents connected through `kronn-internal` receive the same compact rendering
contract:

- A fenced `mermaid` block renders as a diagram. Supported roots are
  `flowchart`/`graph`, `sequenceDiagram`, `classDiagram`, `stateDiagram`,
  `erDiagram`, `journey`, `gantt`, `pie`, `gitGraph`, the C4 families,
  `requirementDiagram`, `mindmap`, `timeline`, `sankey-beta`, `xychart-beta`,
  `block-beta` and `packet-beta`.
- A fenced `kronn-doc-preview` block renders its HTML in a sandboxed iframe and
  exposes PDF/DOCX actions. A normal `html` fence remains source code.
- A fenced `kronn-doc-data` JSON payload exposes CSV, XLSX or PPTX export when
  its `format` and payload shape match the Kronn Docs skill.
- In simple mode, `disc_append` accepts up to eight local paths through its
  `attachments` field. The bridge reads and uploads each regular file (10 MB
  maximum), then pins only those upload ids to the durable message returned by
  the append receipt. The operation is compensating-atomic: if one upload or
  the exact-message link fails, every file uploaded by that call is removed so
  retrying the same `source_msg_id` starts from a clean attachment batch.
  Historical message attachments do not consume the separate 20-file pending
  composer limit. Images use the ordinary authenticated attachment renderer:
  they appear as thumbnails below the message and open in Kronn's full-size
  gallery, with previous/next navigation and a separate new-tab action.
  The Discussion header's Assets panel indexes the full room history with
  search, type/pending filters, direct download for disk-backed files and a link
  back to each file's source message. Its header action is contextual and stays
  hidden while the room has no assets.
  Bulk transcript imports deliberately remain text-only.

The MCP `initialize` instructions expose this contract before any room tool is
called, and `disc_join` repeats it in the room protocol. The full document and
diagram examples remain on demand in the Kronn Docs skill so the permanent MCP
catalogue does not pay for a long manual.
`[src: file: frontend/src/components/MessageBubble.tsx:1376-1434]`
`[src: file: frontend/src/components/MermaidDiagram.tsx:97-108]`
`[src: file: backend/src/api/disc_prompts.rs:390-398]`
`[src: file: backend/scripts/disc-introspection-mcp.py:8212-8222]`
`[src: file: backend/scripts/disc-introspection-mcp.py:634-643]`
`[src: file: backend/scripts/disc-introspection-mcp.py:4101-4174]`
`[src: file: backend/scripts/disc-introspection-mcp.py:4871-4893]`
`[src: file: frontend/src/components/MessageAttachments.tsx:80-287]`
`[src: file: frontend/src/components/DiscussionAssetsPanel.tsx:16-146]`

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

## Workflow run detail

`workflow_run_get` preserves the API's stored `agent_provenance` attempt history
alongside compact step metadata while still truncating long step outputs.
Missing/null provenance stays absent for historical runs; an explicit empty
attempt list is preserved. The bridge does not look up today's model settings
or infer attempts from output text.
[src: file: backend/scripts/disc-introspection-mcp.py:7863]

## Joined CLI worktrees

- Call `disc_workspace_get({})` before editing when several peers may be
  active. It derives this bridge's **active joined CLI identity**, not the
  durable binding key used to restore the room after a restart. The same
  identity rule applies to declaration and history leases. Omit the optional
  identity arguments: matching values remain compatible, but foreign or
  durable-key overrides are refused before HTTP. A missing joined session is
  an error, never a reason to select another participant or reassign a worktree.
  Restore this caller's binding before retrying; do not substitute a peer's id.
  `[src: file: backend/scripts/disc-introspection-mcp.py:5992-6025]`
  `[src: file: backend/src/api/disc_workspace.rs:235-259]`
- Call `disc_workspace_set({task_ref: "KT-140"})` from the worktree root (or
  pass an explicit `workspace_path`) whenever the CLI changes worktree or
  branch. Kronn refreshes branch and HEAD from Git on each declaration.
- Before every destructive history rewrite in a shared checkout, create the
  mandatory `refs/kronn-backup/...` ref, acquire
  `disc_workspace_history_lease`, and stop immediately when `acquired` is
  false. Release the lease after the operation. The lease is advisory and
  expires after 15 minutes; renew by acquiring again if the operation runs
  longer.
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
   Then read `plan_get` (and `task_list` for the wider backlog) before a
   substantive reply or task action; process the join's addressed initial turns.
3. For a clear, actionable user request that has no matching planned task,
   check `plan_get` and `task_list` for duplicates before creating exactly one
   task with `task_create`. If intent, ownership, or scope is ambiguous, submit
   a human-gated `kronn-plan-action` proposal; do not create or delegate it.
4. Before launching a clear, independent task, announce its delegation scope in
   the room. Select an available worker with `agent_list`, then run
   `task_exec_prepare` and `task_exec_launch` only after a launchable preflight.
   Do not invent child rooms or launch duplicate executions; observe existing
   work through `task_exec_status`.
5. Repeat bounded work → report → listen. Before each real step, announce the
   task, scope and next action with `disc_append`. After each result, call
   `disc_wait_for_peer({max_total_secs: 20})` before starting another step;
   process addressed messages, report evidence/limits and continue the plan.
   Keep the parent room informed of delegated milestones, not just the child.
   Follow existing executions with `task_exec_status`, never duplicate launches.
   Use the unbounded `disc_wait_for_peer()` only when no actionable work or
   execution needs following: its quiet inner polls do not return to the model.
   A backgrounded wait stays active until its terminal result or your next
   Kronn call, which ends it and says so in that call's result
   (`wait_preempted`); never start another wait or end on a progress summary
   while it runs, and re-arm after any other Kronn call. A completed
   quiet result or interruption is not departure. Follow each message's routing
   hint and reply only when your exact CLI is addressed (or an untargeted Agent
   turn asks the room); `awareness` is context, not a turn to answer.
6. Call `disc_leave()` when the task is done or the user says stop.

[src: file: backend/src/api/disc_invite.rs:467-570]

The Python catalogue distinguishes bounded work from idle listening, and one
shared orientation supplies the join manual, wait manual and initialize text.
This is a source instruction contract, not a guarantee that a running host has
loaded or follows it. See [the diagnosis and qualification](../../gotchas/joined-cli-work-loop.md).
[src: file: backend/scripts/disc-introspection-mcp.py:1250-1274]
[src: file: backend/scripts/disc-introspection-mcp.py:9351-9368]

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
- **Rolling API windows**: call `workflow_step_schema` for the canonical `time.now` grammar. Time expressions are vendor-neutral, anchored once per run/call and work in Quick APIs, `ApiCall`, `BatchApiCall` and `CollectApiData` source variables.
- **Mutating tools** (`disc_create`, `qp_create_draft`, `qa_create_draft`, `qe_create_draft`, `workflow_create_draft`) default to safe states (workflows created as `enabled: false`). Safe to call ; the user reviews before activation.

## Discoverable discussion action contracts

`tool_manual({tool: "signals"})` returns `catalogue.schema_version` and the
supported discussion action signals. Omitting `tool` lists `signals` alongside
the other manuals. Native tools read the same registry that the MCP bridge
fetches through `GET /api/signals/catalog`; the bridge fails explicitly if that
read fails, rather than substituting a stale local schema.
[src: file: backend/src/api/signal_catalog.rs:1]
[src: file: backend/scripts/disc-introspection-mcp.py:1]

Version 1 describes the `kronn-action` Markdown fence for existing Quick Prompts,
Quick APIs, Quick Execs and workflows. It includes target-discovery tools, a
payload authoring schema, editable-input provenance, project inheritance and
human approval semantics. The authoring schema is deliberately narrower than
legacy deserialization: agents should not send unknown fields or server-resolved
provenance. It does not introduce a new runtime validator. Reading the registry
creates no card; saving an agent message persists its proposal through the
existing transaction. Only a human launch claims and executes the target. The
same message and fence position are idempotent; a new message is a new proposal.
[src: file: backend/src/api/signal_catalog.rs:1]
[src: file: backend/src/db/discussion_actions.rs:430]

The existing inline authoring guidance remains in the discussion prompt and
MCP instructions. A small-model comparison rejected replacing it with a shorter
pointer; see [the retained observations](../../research/signal-catalogue-2026-09-22.md).

This initial registry covers Automation proposals. Historical Planning, audit
and Quick Prompt improvement signals retain their existing parsers and
contracts; they are not claimed as entries in this first version.
[src: file: backend/src/api/signal_catalog.rs:1]

## Common use cases in Kronn

- Pick up a discussion started by another CLI agent (cross-agent memory).
- Participate in a live multi-agent discussion (e.g. 2 agents debugging together, one agent acting as reviewer).
- Surface the user's Kronn-configured plugins (`mcp_list`) without re-asking for credentials.
- Draft a workflow / QP from the agent side, then let the user review + enable in the Kronn UI.
- Discover and author shared Live Pages with `page_list`, `page_get`,
  `page_create` and `page_update_html`. Before drafting a
  `PublishPageData` step, resolve a real Page id; for a multi-source report,
  resolve every Quick API through `qa_list` and every reusable CLI collector
  through `qe_list`. If no QE exists, create it with `qe_create_draft`, test it
  with `qe_run`, reference its `quick_exec_id`, and include its bare command in
  the workflow `exec_allowlist`. Follow the canonical
  `CollectApiData -> TransformData -> PublishPageData` contract returned by
  `workflow_step_schema`. `page_create` links the artifact to the current
  Discussion when one is bound, accepts an explicit optional `discussion_id`,
  and also works from an unbound host CLI for a standalone Page. Pass
  `datasets: []` for standalone HTML or seed `initial` values for a mock-backed
  Page. `page_get` returns both Workflow and Discussion links.
- Put buttons on a Page that launch a real QP, QA, QE or Workflow for each data
  row. `tool_manual({tool: "page_create"})` and
  `tool_manual({tool: "page_update_html"})` carry the contract: one inert
  `application/kronn-action` block per action, one `data-kronn-bindings`
  selector per row, `data-kronn-action-state` on each button for its row's live
  state, and a native card that opens on the row's latest run and what it
  produced. Keep a block's reference stable across revisions.

## Related

- `backend/src/api/disc_invite.rs` — invite token + peer-join + wait-for-peer endpoints.
- `backend/src/db/discussion_sessions.rs` — sessions table that powers the header participants list.
- `backend/src/api/disc_source.rs` — cross-agent memory endpoints (`disc_create`, `disc_append`, …).
- `backend/src/api/agent_api.rs` — the broker behind `api_call`.
