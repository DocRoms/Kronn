# Agent identities & addressing matrix (KT-211, 0.9.3)

Reference matrix of every addressable identity in a Kronn discussion, the
exact alias that reaches it, and what actually happens at send, wake,
reply and reload time. Each row is verified against the code cited next
to it — nothing here is "usually" or "should": if the code diverges, the
code or this file is wrong and one of them must change.

Companion contract: `docs/architecture/discussion-agent-routing.md`
(wake ≠ see, scan/offer/ack awareness delivery).

## 1. The identities

| Identity | Alias (composer & MCP prose) | Typed target kind | Backed by |
|---|---|---|---|
| Discussion agent (the room's configured native brain) | `@<provider>` when that provider IS the room's agent (e.g. `@claude` in a ClaudeCode room) | `discussion_agent` | native runner, room-configured model |
| Punctual native agent (one-shot spawn) | `@<provider>` when that provider is NOT the room's agent | `agent` | fresh native runner, billed per spawn |
| Joined CLI session (durable) | `@<provider>-cli`, then `@<provider>-cli-2`, … in `joined_at` order | `cli` + exact `cli_session_id` | an external CLI (Claude Code, Codex, …) holding a durable session |
| Everyone | `@all` | `target_all` flag, no typed target | fan-out to designated responders |
| Human | `@user` (canonical trigger; pseudos like `@romu` are NOT mentions) | — | the user; `@user` addresses the human in prose and never wakes a runner |

The same word therefore names TWO different identities depending on the
room: `@claude` is the discussion agent in a ClaudeCode-configured room
and a punctual spawn everywhere else. This is intentional and the
autocomplete must always show which one is being picked (KT-211).
[src: file: frontend/src/lib/messageTargets.ts]
[src: file: backend/scripts/disc-introspection-mcp.py]

## 2. Alias resolution, exactly

- **Composer (human)**: `composerMentions` builds the option list —
  `@all`, one option per installed provider (discussion agent when
  principal, punctual otherwise), one option per joined CLI session with
  `-cli[-N]` ordinals. `targetsFromComposerText` matches triggers in
  prose only (fenced/inline code is documentation), keeps textual order,
  deduplicates identical targets.
  [src: file: frontend/src/lib/messageTargets.ts]
- **MCP append (agent)**: `_structured_message_targets` parses the same
  grammar from prose (code stripped first). `@x-cli-N` resolves against
  the live participants list and RAISES when the ordinal does not
  identify a joined CLI — an agent cannot invent a session. A short
  alias resolves to `discussion_agent` when principal, else `agent`.
  Multiple mentions fan out once per identity, in textual order.
  [src: file: backend/scripts/disc-introspection-mcp.py]
- **No presence-based substitution.** A short alias never silently
  reroutes to a joined CLI (and never refuses to spawn) just because a
  same-provider CLI is present: the punctual agent and the CLI are
  distinct identities that may both be needed (sealed 2026-08-02).
  The one exception is the reply-coherence guard in §5.

## 3. Wake vs awareness (per identity, post-KT-189)

| Turn | Discussion agent | Punctual agent | Joined CLI (exact target) | Other joined CLIs |
|---|---|---|---|---|
| Untargeted human turn, native room | wakes (owns the reply) | — | awareness at next wake | awareness |
| Untargeted human turn, joined-only room | — | — | wakes (designated responders) | wakes |
| `@x` short alias | wakes if principal | spawns+wakes if not principal | never (unless also listed) | awareness |
| `@x-cli[-N]` | never by coincidence | never | wakes, exactly that session | awareness |
| Untargeted agent turn | per dispatch rules | — | awareness | awareness |
| Revision events | — | — | wake only if addressed; awareness otherwise | awareness |

Awareness batches are bounded (20/wake), explicitly flagged
`awareness: true`, delivered with a durable scan/offer/ack contract and
never end a quiet wait. The bridge itself does not return merely because one
server poll was quiet.
[src: file: backend/src/api/disc_invite.rs]
[src: file: docs/architecture/discussion-agent-routing.md]

## 3 bis — Bridge guarantee vs host capability

That protocol property is distinct from a host capability. Claude Code 2.x
backgrounds an MCP tool call after 120 seconds and emits a model-visible task
notification. The original wait continues: the agent MUST NOT start another
`disc_wait_for_peer` when it is merely moved to the background, and must wait
for its terminal notification before deciding whether to re-arm. This removes
stacked waits, but the host notification still costs one wake; universal
zero-turn silence requires a push channel outside MCP tool calls.

## 4. Replies

A reply to a specific turn MUST carry `reply_to_message_id` (room rule,
2026-08-02). WITHOUT an explicit target, delivery then follows the
AUTHOR of the replied message: a CLI-authored message exposes
`reply_target` (exact provider + session) so the reply wakes that
precise CLI even among same-provider siblings; replying to a
native-authored message keeps untargeted routing. An explicit typed
target or fan-out always wins over the inferred author.
[src: file: backend/src/api/disc_source.rs]
[src: file: backend/src/api/disc_invite.rs]

## 5. Reply-coherence guard (KT-211)

Observed failure: CLI A replies to CLI B while B briefly reconnects, and
types `@b` (short alias) — the reply spawns/wakes a punctual agent while
B never wakes. Because the reply context makes the intent unambiguous,
this exact case is REFUSED at the bridge with the corrective suggestion
(`@b-cli[-N]` of the replied author), instead of being silently
rerouted: when `reply_to_message_id` points to a CLI-authored message of
provider X, the content mentions a native identity of X (punctual OR
discussion agent) and the replied author's exact CLI is ABSENT from the
targets, the append fails closed with the exact alias to use — or, when
the author or alias cannot be verified, with a refusal that never
fabricates an ordinal. A deliberate fan-out that lists the exact replied
CLI alongside its native identity passes untouched, and outside a reply
context the short alias stays free (both identities may be wanted).
[src: file: backend/scripts/disc-introspection-mcp.py]

## 6. Presence, fallback, reload

- **Presence/eligibility**: an open wait is `listening`; a delivered
  batch flips `reading`; a timed-out wait records `waiting` +
  `next_poll_at`. Eligibility = unexpired lease or next_poll grace. A
  session identified only by its session id (provider unknown) keeps
  full presence via the agent type stored on its row.
  [src: file: backend/src/api/disc_invite.rs]
- **Fallback**: when no joined CLI is an eligible responder, an
  untargeted human turn falls back to the native discussion agent so the
  room never goes silent; the turn reaches the CLIs as awareness.
  [src: file: backend/src/db/discussions.rs]
- **Reload**: a bridge reload re-attaches the durable session through
  the 0600 resume credential (`/peer-resume`, no token). A CLI whose
  native identity rotated (fresh session id) needs one `disc_join` with
  a token, which mints its credential — one-shot, by design. Unacked
  awareness survives reloads (server cursors, migrations 105/106).
  [src: file: backend/scripts/disc-introspection-mcp.py]
  [src: file: backend/src/db/sql/106_awareness_offered_cursor.sql]

## 7. Autocomplete presentation (KT-211 contract)

The composer menu must make the three identity kinds visually distinct
(discussion agent / punctual agent / joined CLI), show joined CLIs first
when present, and never present two identities under one undifferentiated
label. The menu is a disambiguation device, not a shortcut: picking an
entry always yields exactly the identity it names.
[src: file: frontend/src/components/ChatInput.tsx]
