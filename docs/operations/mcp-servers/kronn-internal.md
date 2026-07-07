# MCP context — kronn-internal

**Server:** `kronn-internal` (Python stdio bridge — `backend/scripts/disc-introspection-mcp.py`)
**Source:** This repo. Auto-injected by Kronn into every supported CLI's MCP config (`.mcp.json`, `~/.codex/config.toml`, `.gemini/settings.json`, `.kiro/settings/mcp.json`, `.vibe/config.toml`).
**Auth:** stdio itself is unauthenticated (local pipe), but the bridge authenticates to the Kronn backend over `KRONN_BACKEND_URL` (default `http://127.0.0.1:3140`): when the backend has a token configured, it exports `KRONN_AUTH_TOKEN` into the process env, the sidecar inherits it and sends `Authorization: Bearer <token>` on every call. On a loopback-only instance the backend's local-trust bypass makes the token optional; on a LAN-exposed instance (e.g. WSL backend / Mac frontend) it is required — otherwise the sidecar's own calls get a silent 401. `[src: file: backend/scripts/disc-introspection-mcp.py:1970-1994]` `[src: file: backend/src/main.rs:102-115]`

## What it does

Bidirectional gateway between a CLI agent (Claude Code, Codex, Gemini, Kiro, Vibe in host-launched mode, …) and the Kronn backend. Three tool families :

1. **Discussion introspection** (0.8.3+) — `disc_meta`, `disc_get_message`, `disc_summarize`. Cheap reads of the current Kronn discussion.
2. **Cross-agent memory** (0.8.4) — `disc_create`, `disc_append`, `disc_link`, `disc_unlink`, `disc_find_by_session`, `disc_search`, `disc_load_other`. Push transcripts in / out of Kronn so the same thread can be picked up by a different agent later.
3. **Catalog + actions** (0.8.5–0.8.6) — `mcp_list`, `workflow_list`, `qp_list`, `qa_list`, `workflow_create_draft`, `qp_create_draft`, `api_call` (broker that invokes Kronn-configured APIs without credentials in the prompt).
4. **Multi-agent collab** (0.8.6) — `disc_join` (consume invite token), `disc_wait_for_peer` (long-poll), `disc_leave`. Lets N CLI agents share one Kronn discussion in real time.

## Multi-agent collab — required protocol

When a user gives you a `kr-join-…` invite token :

1. Call `disc_join({token: "kr-join-…"})`. The response carries an explicit `next_steps` field — **read and follow it**.
2. **Introduce yourself via `disc_append({content: "<intro>"})`** even if you're the first / only participant. Replying only in your local terminal is INVISIBLE to peers.
3. Loop : `disc_wait_for_peer({timeout_secs: 60})` → on each new message, `disc_append` your reply.
4. Call `disc_leave()` when the task is done or the user says stop.

The bridge auto-derives your `agent_type` from the MCP `clientInfo.name` handshake (Claude Code → ClaudeCode, Codex → Codex, …) so no env-var prep is needed.

## disc_append — two modes

- **Simple (recommended for live chat)** : `disc_append({content: "..."})`. Bridge auto-fills `disc_id` (from `disc_join` binding), generates `source_msg_id`, defaults `role=Agent`, stamps `agent_type` from `clientInfo`.
- **Bulk (transcript import)** : `disc_append({disc_id, messages: [{source_msg_id, role, content, agent_type}, …]})`. Idempotent on `(disc_id, source_msg_id)`.

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
