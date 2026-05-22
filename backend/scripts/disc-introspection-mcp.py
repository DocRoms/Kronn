#!/usr/bin/env python3
"""MCP stdio bridge — exposes the 3 Kronn discussion-introspection
endpoints as standard MCP tools any compatible agent can call.

Auto-spawned by the agent runtime via the per-discussion `.mcp.json`
that Kronn writes for `summary_strategy != Off` discussions:

    {
      "mcpServers": {
        "kronn-internal": {
          "command": "python3",
          "args": ["/path/to/disc-introspection-mcp.py"],
          "env": {
            "KRONN_DISCUSSION_ID": "abc-123",
            "KRONN_BACKEND_URL":   "http://127.0.0.1:3140",
            "KRONN_AUTH_TOKEN":    "<bearer>"  # optional, only for non-localhost
          }
        }
      }
    }

The script speaks the standard MCP JSON-RPC over stdin/stdout: handles
`initialize`, `tools/list`, `tools/call`. Each tool call boils down to
one HTTP request to the matching backend route.

This is intentionally tiny (no MCP SDK dependency) so it can ship
inside the Kronn install without pulling in npm/uv packages — the
agent CLIs all run with system Python by virtue of vibe-runner.py
already requiring it.
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid


# ─── Tool catalogue ────────────────────────────────────────────────────────

TOOLS = [
    {
        "name": "disc_meta",
        "description": (
            "Return metadata about the current discussion (message_count, "
            "agent, tier, has_cached_summary, msgs_since_last_summary, "
            "summary_strategy, language, project_id). Call this FIRST "
            "when you need to decide whether to fetch context. Cheap "
            "(single DB read, no token cost)."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "disc_get_message",
        "description": (
            "Return one message by 0-indexed position. Negative idx "
            "counts from the end (-1 = last). Use this when you need "
            "the verbatim content of a specific past message you can't "
            "see in the current prompt window. Cheap."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "idx": {
                    "type": "integer",
                    "description": "0-based index, or negative for from-end (-1=last)."
                }
            },
            "required": ["idx"],
        },
    },
    {
        "name": "disc_summarize",
        "description": (
            "Generate (or return cached) summary of a message range. "
            "EXPENSIVE — runs an eco-tier agent call (~500-1500 tokens). "
            "Only call this when disc_meta indicates msgs_since_last_summary "
            "is high AND you actually need the older context. Returns "
            "{summary, from_idx, to_idx, generated, tokens_used}."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "from": {
                    "type": "integer",
                    "description": "Inclusive start index. Defaults to 0.",
                },
                "to": {
                    "type": "integer",
                    "description": "Exclusive end index. Defaults to the latest message.",
                },
                "force_refresh": {
                    "type": "boolean",
                    "description": "Skip cache and regenerate. Default false.",
                    "default": False,
                },
            },
            "required": [],
        },
    },
    # ─── 0.8.4 (#294) cross-agent memory tools ─────────────────────────
    # Each one is a 1:1 mirror of a backend route in
    # `backend/src/api/disc_source.rs`. They let an external CLI
    # session (Claude Code, Cursor, Codex, …) push its conversation
    # history into Kronn DB so a DIFFERENT agent can pick up the same
    # thread later.
    {
        "name": "disc_create",
        "description": (
            "Create a new discussion in Kronn, optionally bound to the "
            "current source session. When `source_agent` + "
            "`source_session_id` are provided and a disc already exists "
            "for that pair, returns the existing disc_id (idempotent — "
            "safe to call on every CLI bootstrap). Use this once at the "
            "start of a session to grab a stable Kronn disc_id you can "
            "later append to."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Discussion title."},
                "agent": {"type": "string", "description": "Agent type — e.g. ClaudeCode, Cursor, Codex."},
                "language": {"type": "string", "description": "Locale (fr/en/es). Default 'en'."},
                "project_id": {"type": "string", "description": "Bind to a Kronn project, optional."},
                "source_agent": {"type": "string", "description": "Source CLI label, e.g. 'ClaudeCode'."},
                "source_session_id": {"type": "string", "description": "Session id from the CLI runtime."},
            },
            "required": ["title", "agent"],
        },
    },
    {
        "name": "disc_append",
        "description": (
            "Post a message in the currently-bound Kronn discussion. "
            "⚠ THIS IS HOW YOU TALK TO OTHER AGENTS IN A MULTI-AGENT "
            "ROOM (after `disc_join`). Replying only in your own "
            "terminal is INVISIBLE to peers. \n\n"
            "TWO USAGE MODES :\n"
            "  • SIMPLE (recommended for live chat) — pass just "
            "`content` : `disc_append({content: \"Hi, I'm Codex. "
            "Ready to play.\"})`. The bridge auto-fills disc_id "
            "(from disc_join binding), generates a fresh message id, "
            "defaults role=Agent, and stamps your agent_type from "
            "the MCP clientInfo handshake.\n"
            "  • BULK (for cross-agent-memory transcript import, "
            "0.8.4) — pass `messages: [{source_msg_id, role, "
            "content, agent_type}, …]` to push a whole conversation "
            "history at once. Idempotent on (disc_id, source_msg_id) "
            "— re-pushing the same transcript does NOT duplicate.\n\n"
            "Returns `{appended, skipped_as_duplicates, diverged}`. "
            "`diverged=true` means the Kronn UI was edited after a "
            "previous import — warn the user before more updates."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "Simple mode : the text to post. Bridge wraps it into a messages[]."},
                "role": {"type": "string", "description": "Simple mode : role override (default Agent). Use User if you're echoing the user's words."},
                "agent_type": {"type": "string", "description": "Simple mode : explicit author override (default = auto from clientInfo)."},
                "disc_id": {"type": "string", "description": "Defaults to the runtime-bound disc from disc_join. Override only when you need to post to a DIFFERENT disc."},
                "messages": {
                    "type": "array",
                    "description": "Bulk mode : explicit array of messages (used for transcript import).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "source_msg_id": {"type": "string"},
                            "role": {"type": "string", "description": "User | Agent | System"},
                            "content": {"type": "string"},
                            "agent_type": {"type": "string"},
                        },
                        "required": ["source_msg_id", "role", "content"],
                    },
                },
            },
            # Either `content` (simple) OR `messages` (bulk) is required.
            # The bridge enforces the OR at runtime ; we leave `required`
            # empty here so MCP clients with strict schema validation
            # don't reject the simple-mode call shape.
            "required": [],
        },
    },
    {
        "name": "disc_link",
        "description": (
            "Bind an existing Kronn disc to a (source_agent, "
            "source_session_id) pair. Last-link-wins: any previous "
            "binding is closed automatically. Use this when transferring "
            "ownership of a thread from one agent CLI to another."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "disc_id": {"type": "string"},
                "source_agent": {"type": "string"},
                "source_session_id": {"type": "string"},
            },
            "required": ["disc_id", "source_agent", "source_session_id"],
        },
    },
    {
        "name": "disc_unlink",
        "description": (
            "Release the current source binding on a disc. The "
            "append-only history chain is preserved so the UI can still "
            "show 'was previously imported from X'."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"disc_id": {"type": "string"}},
            "required": ["disc_id"],
        },
    },
    {
        "name": "disc_find_by_session",
        "description": (
            "Look up the Kronn disc_id currently bound to a (source_agent, "
            "source_session_id) pair, or `null` if none. Call this FIRST "
            "to decide between `disc_create` (no prior thread) and "
            "`disc_append` (resume existing thread)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "source_agent": {"type": "string"},
                "source_session_id": {"type": "string"},
            },
            "required": ["source_agent", "source_session_id"],
        },
    },
    {
        "name": "disc_search",
        "description": (
            "LIKE-based full-text search across disc titles + message "
            "content. Returns up to `limit` (default 20) hits with "
            "snippet + source binding metadata. Use this to find a past "
            "thread by keyword when the user references it loosely."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "q": {"type": "string", "description": "Search string. Wildcards: any char (LIKE-escaped)."},
                "limit": {"type": "integer", "description": "Max hits (1-50, default 20)."},
            },
            "required": ["q"],
        },
    },
    # ─── 0.8.6 phase 2 — disc-first / cross-agent collab ──────────────
    # `disc_join` is the key that unlocks host-launched cross-agent
    # use cases : without it, only Kronn-launched agents (env-injected
    # `KRONN_DISCUSSION_ID`) could use the `disc_*` tools. With it, an
    # agent launched directly in a terminal can rebind itself to a
    # Kronn disc at runtime by consuming an invite token.
    {
        "name": "disc_join",
        "description": (
            "Join a Kronn shared discussion using an invite token "
            "(the `kr-join-…` string the disc owner gave you). On "
            "success, binds THIS bridge process to the resolved "
            "disc so every subsequent `disc_meta`, `disc_append`, "
            "`disc_load_other`, `disc_wait_for_peer`, etc. operates "
            "on it. ⚠ AFTER JOINING : you are now in a MULTI-AGENT "
            "ROOM. Other CLI agents (Claude, Codex, Gemini, Vibe, …) "
            "are listening. To talk to them you MUST call "
            "`disc_append({content: \"...\"})` — anything you reply "
            "only in your own terminal is INVISIBLE to peers. The "
            "response includes a `next_steps` field with the full "
            "protocol; READ AND FOLLOW IT before doing anything "
            "else. Tokens are single-use and expire after 10 min."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "token": {
                    "type": "string",
                    "description": "Invite token (kr-join-… form).",
                }
            },
            "required": ["token"],
        },
    },
    # ─── 0.8.6 (#56) Full-MCP cross-agent bootstrap ────────────────
    # Two convenience tools so an agent can spin up a multi-agent
    # room WITHOUT bouncing the user through the Kronn UI. Both reuse
    # the existing `POST /api/discussions/:id/invite-peer` route the
    # UI calls; just exposed at the MCP surface for full-MCP flows.
    {
        "name": "disc_invite_peer",
        "description": (
            "Mint an invite token for the discussion currently bound "
            "to this bridge (the one you joined via `disc_join` or "
            "created via `disc_create` upstream). Returns "
            "`{token, instruction_text, expires_at, ttl_seconds}`. "
            "`instruction_text` is a ready-to-share message the user "
            "can paste into another CLI to bring it into the room. "
            "Tokens are multi-use within their TTL (10 min) so the "
            "same invite can onboard multiple peers."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "disc_create_room",
        "description": (
            "One-shot bootstrap of a multi-agent room from the MCP "
            "surface : creates a fresh discussion AND mints an invite "
            "token in a single call. Returns `{disc_id, title, token, "
            "instruction_text, expires_at, next_step}`.\n\n"
            "⚠ IMPORTANT — this tool does NOT switch your current "
            "bridge binding. Your existing disc (the one you are "
            "currently talking in) stays the active one. The new room "
            "is created server-side and the token lets a peer join "
            "it, but YOU stay where you were. This is intentional : "
            "silent context-switch would risk losing the thread of "
            "the conversation that asked for the room.\n\n"
            "After this call, decide explicitly :\n"
            "  (a) Stay in the current disc → share `instruction_text` "
            "with the user (paste it in another CLI to bring it in).\n"
            "  (b) Switch your own bridge to the new room → call "
            "`disc_join({token})` with the returned token. Your "
            "previous disc binding is replaced ; calling `disc_leave` "
            "first is cleanest if you want to formally leave.\n\n"
            "The `next_step` field in the response is a plain-text "
            "hint about what makes sense given the current context — "
            "follow it OR explicitly diverge with a one-line rationale "
            "so the user knows what's happening."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Room title shown in the Kronn UI."},
                "language": {"type": "string", "description": "Locale (fr/en/es). Default 'en'."},
                "project_id": {"type": "string", "description": "Bind to a Kronn project, optional."},
            },
            "required": ["title"],
        },
    },
    {
        "name": "disc_leave",
        "description": (
            "Leave the current Kronn discussion : marks the calling "
            "session as `left` server-side and clears this bridge's "
            "disc binding. Idempotent — calling twice doesn't error. "
            "Use at the end of a multi-agent collab session, or when "
            "the user explicitly tells you to disconnect. Other "
            "participants will see you disappear from the header on "
            "next refresh."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "disc_wait_for_peer",
        "description": (
            "Long-poll the current Kronn discussion for new messages "
            "from OTHER agents. Blocks server-side (up to 90 s) until "
            "either a new message appears (newer than `since_sort_order`, "
            "from an agent type different from this CLI's) or the "
            "timeout fires. Cheap on tokens — replaces polling loops "
            "where the agent kept calling `disc_meta` every few "
            "seconds. Returns `{timed_out, messages, "
            "latest_sort_order}`. Pass back `latest_sort_order` as "
            "the next `since_sort_order` to chain calls without "
            "re-receiving the same messages."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "since_sort_order": {
                    "type": "integer",
                    "description": "Highest sort_order already seen (default -1 = from the start).",
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Max blocking seconds (default 60, clamped server-side to [1, 90]).",
                },
            },
            "required": [],
        },
    },
    {
        "name": "disc_load_other",
        "description": (
            "Load a slice of messages from a Kronn disc OTHER than the "
            "current one. Returns `{disc_id, title, total_messages, "
            "from_idx, to_idx, messages}`. Defaults: from=0, to=total. "
            "Useful when the user says 'go look at what we decided in "
            "the auth thread last week'."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "disc_id": {"type": "string"},
                "from": {"type": "integer", "description": "Inclusive start (0-based). Default 0."},
                "to": {"type": "integer", "description": "Exclusive end. Default total length."},
            },
            "required": ["disc_id"],
        },
    },
    # ─── 0.8.5 — read-only listings of existing artifacts ───────────────
    # Always call the relevant `*_list` tool BEFORE drafting a new
    # artifact: if a fitting one already exists, reference its id
    # (`quick_prompt_id`, `quick_api_id`, `api_config_id`) instead of
    # duplicating. Compact payload (no full bodies) to keep the agent
    # context tight; the `GET /api/<surface>/<id>` route returns the
    # full record when the agent really needs it.
    {
        "name": "workflow_list",
        "description": (
            "List every workflow in the user's Kronn instance — compact "
            "view (id, name, enabled, project_id, trigger_type, "
            "step_count, step_names, last_run_status, last_run_at). "
            "Use this to (a) avoid drafting a duplicate workflow, (b) "
            "surface the existing workflow id when the user asks "
            "'have I already built something like X?'."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "qp_list",
        "description": (
            "List every Quick Prompt in the user's library — compact "
            "view (id, name, agent, description, variable_names, "
            "skill_ids, project_id, tier). Use this to (a) reuse a "
            "matching QP via `quick_prompt_id` / "
            "`batch_quick_prompt_id` in a workflow step instead of "
            "drafting a duplicate, (b) answer 'do I already have a QP "
            "for X?'."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "qa_list",
        "description": (
            "List every Quick API in the user's library — compact view "
            "(id, name, api_plugin_slug, api_endpoint_path, api_method, "
            "description, project_id, variables[]). The `variables[]` "
            "entries are `{name, label, required, description}` — pass "
            "values matching those names to `qa_run`. Use this to (a) "
            "discover the right QA for an action via `qa_run`, (b) "
            "reuse a matching QA via `quick_api_id` in a workflow "
            "`ApiCall` / `BatchApiCall` step instead of re-specifying "
            "the endpoint inline."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "mcp_list",
        "description": (
            "List every MCP / API plugin wired in the user's Kronn "
            "instance. Returns `{configs, servers_with_api}` where "
            "`configs` lists the user's instances (id + server_id + "
            "project scoping) and `servers_with_api` lists every "
            "plugin that exposes a REST API spec, with: `description`, "
            "`docs_url`, `is_custom`, `config_keys[]` (env keys + auth-"
            "managed flag), `endpoints[]` (path/method/description/"
            "side_effect), and a `hint` field.\n\n"
            "**`config_keys[]`** — each entry is `{env_key, label, "
            "auth_managed}`. The `env_key` (UPPER_SNAKE) is the slug "
            "you can reference in `api_call` arguments via the "
            "`${ENV.<env_key>}` placeholder syntax (works in "
            "`endpoint_path`, `path_params`, `query`, `headers`, "
            "`body`). Kronn substitutes server-side from the encrypted "
            "config — you never see the actual value. When "
            "`auth_managed: true`, Kronn handles that key for you via "
            "the plugin's auth scheme (Bearer/OAuth/etc.) — DO NOT "
            "reference it via `${ENV.X}` (it would either be redundant "
            "or leak a secret to the prompt). Free-form identifiers "
            "(account_id, organization_id, workspace_slug) typically "
            "show `auth_managed: false` — that's your `${ENV.X}` "
            "playground.\n\n"
            "**Always read `hint` before acting** — it tells you "
            "whether the plugin is ready for ApiCall, or whether you "
            "need to fetch the docs first (when endpoints are empty "
            "but `docs_url` is set), or whether to ask the user (when "
            "neither is set). Use this to pick the right "
            "`api_plugin_slug` + `api_config_id` when drafting an "
            "`ApiCall` step — without it the agent would have to "
            "guess plugin slugs."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    # ─── 0.8.5 — autonomous draft creation tools ────────────────────────
    # Symmetric to the `KRONN:WORKFLOW_READY` / `KRONN:QP_IMPROVED`
    # signal+button path: these tools let the agent CREATE the artifact
    # directly when the conversation has converged on a clear design,
    # at the cost of the user's one-click review. Safety: both tools
    # force `enabled: false` on the workflow path (no auto-fire on
    # cron), and the artifact appears in the user's Workflows / QP
    # tab marked as a draft. The signal+button path stays the
    # recommended default; the draft tools are for the "agent has
    # nailed the design, let's accelerate adoption" scenario.
    {
        "name": "workflow_create_draft",
        "description": (
            "Create a Kronn workflow in DRAFT state (`enabled: false`). "
            "The workflow appears in the user's Workflows page and can "
            "be reviewed + enabled with one click — no cron fires until "
            "the user explicitly enables it. Use this when the design "
            "has converged in the conversation and the user signaled "
            "they want autonomous creation; otherwise emit a "
            "`KRONN:WORKFLOW_READY` block and let the user one-click "
            "deploy via the existing UI CTA.\n\n"
            "Payload mirrors `CreateWorkflowRequest`: name (required), "
            "trigger (required, e.g. `{ \"type\": \"Manual\" }`), steps "
            "(required, ≥ 1 ≤ 20 items). Optional: project_id, "
            "actions, safety, workspace_config, concurrency_limit, "
            "guards, artifacts, on_failure, exec_allowlist, variables.\n\n"
            "Returns the created workflow JSON (id, all fields) so the "
            "agent can echo the id back to the user (`Workflow drafted "
            "as <id> — review and enable in your Workflows page`)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Workflow name (1-200 chars)."},
                "trigger": {
                    "type": "object",
                    "description": "Workflow trigger spec (Manual / Cron / Tracker). E.g. `{ \"type\": \"Manual\" }` or `{ \"type\": \"Cron\", \"schedule\": \"0 9 * * 1-5\" }`.",
                },
                "steps": {
                    "type": "array",
                    "description": "Workflow steps (1-20 items). Each step matches the `WorkflowStep` shape — see the `workflow-architect` skill for the canonical schema.",
                },
                "project_id": {"type": "string", "description": "Optional Kronn project id to bind the workflow to."},
                "variables": {"type": "array", "description": "Optional manual-launch variables (each `{ name, label?, placeholder?, required?, description? }`)."},
                "guards": {"type": "object", "description": "Optional execution guards (timeout, max_llm_calls, loop_revisits)."},
                "on_failure": {"type": "array", "description": "Optional rollback step chain (Notify / Agent / ApiCall steps)."},
                "exec_allowlist": {"type": "array", "items": {"type": "string"}, "description": "Whitelisted binaries for any Exec steps."},
                "artifacts": {"type": "object", "description": "Optional artifact declarations (name → spec)."},
                "concurrency_limit": {"type": "integer", "description": "Optional max concurrent runs."},
                "safety": {"type": "object", "description": "Optional WorkflowSafety overrides."},
                "actions": {"type": "array", "description": "Optional actions (legacy slot)."},
                "workspace_config": {"type": "object", "description": "Optional workspace mode (Direct / Isolated)."},
            },
            "required": ["name", "trigger", "steps"],
        },
    },
    {
        "name": "qp_create_draft",
        "description": (
            "Create a Kronn Quick Prompt (QP) in the user's QP library. "
            "QPs are manual-launch templates; there is no enabled flag "
            "(every QP can be launched on demand) so this is roughly "
            "the symmetric tool to `workflow_create_draft` but without "
            "an auto-fire risk. Use when the conversation converged on "
            "a reusable prompt template the user will want to launch "
            "again later (e.g. recurring audit prompt, triage prompt). "
            "For one-off improvements to an existing QP, prefer the "
            "`KRONN:QP_IMPROVED` signal+button flow (`qp-improver` "
            "skill) which targets an existing QP by id.\n\n"
            "Returns the created QP JSON (id, all fields) so the "
            "agent can echo the id back to the user."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "QP name (1-200 chars, displayed on the QP card)."},
                "prompt_template": {"type": "string", "description": "The template body. Use `{{var}}` for required variables."},
                "agent": {"type": "string", "description": "Default agent: `ClaudeCode` / `Codex` / `Vibe` / `GeminiCli` / `Kiro` / `CopilotCli` / `Ollama` / `Custom`."},
                "variables": {"type": "array", "description": "Variable definitions (each `{ name, label?, placeholder?, required?, description? }`)."},
                "description": {"type": "string", "description": "Optional one-line description (~120 chars max) shown on the QP card."},
                "icon": {"type": "string", "description": "Optional single-emoji prefix shown on the QP card (e.g. `⚡` / `🔍` / `📝`)."},
                "tier": {"type": "string", "description": "Default model tier: `default` / `economy` / `reasoning`."},
                "project_id": {"type": "string", "description": "Optional Kronn project id to bind the QP to."},
                "skill_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional skill bindings."},
                "profile_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional profile bindings."},
                "directive_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional directive bindings."},
            },
            "required": ["name", "prompt_template", "agent"],
        },
    },
    {
        "name": "qa_create_draft",
        "description": (
            "Create a Kronn Quick API (QA) in the user's QA library. "
            "Closes the symmetry gap with `workflow_create_draft` + "
            "`qp_create_draft` — agents can now SAVE a reusable API "
            "request shape (endpoint + method + query + body + extract "
            "+ pagination) for later one-shot invocation via `qa_run`. "
            "Use this when the conversation has converged on a "
            "recurring API call the user will want to launch again "
            "later (e.g. \"fetch active sprint tickets\", \"check "
            "domain config\", \"trigger a webhook\"). Saves token cost "
            "on every future invocation : `qa_run(id, vars)` instead "
            "of rebuilding the `api_call` payload from scratch.\n\n"
            "**⚠ Recommended workflow — PROBE then PERSIST** :\n"
            "  1. **Probe** : do ONE `api_call` to the target endpoint "
            "WITHOUT `extract` (or with `extract: null`). Read the "
            "response shape — many vendors (JIRA, Confluence, AWS, "
            "GitHub) return verbose payloads with `changelog`, "
            "`renderedFields`, ADF nodes, ARN-heavy refs that bloat "
            "an agent's context (10-40k tokens for a single ticket).\n"
            "  2. **Decide** : pick the JSONPath that keeps only what "
            "downstream agents need (often `$.fields`, `$.data`, or "
            "`$.items[*].{id,title,status}`). When in doubt, ask the "
            "user what they care about.\n"
            "  3. **Persist** : call `qa_create_draft` with the "
            "optimised `api_extract` AND vendor-side filters in "
            "`api_query` (e.g. `fields=summary,status` for JIRA, "
            "`expand=` knobs for Confluence). Persist both for "
            "max token economy — server-side filtering AND client-side "
            "extraction stack.\n\n"
            "Persisting a QA without `api_extract` is fine for "
            "small-payload vendors (Resend, Mailjet, simple webhooks) "
            "but ALWAYS measure first — the next `qa_update` call to "
            "add `api_extract` post-hoc is just MCP friction the user "
            "can avoid by getting it right at create time.\n\n"
            "**Discovery first** : call `mcp_list` to find the right "
            "`api_plugin_slug` + `api_config_id`. The QA's endpoint "
            "must match one of the plugin's allow-listed endpoints "
            "(or the executor will refuse it at run time).\n\n"
            "**Templating** : `endpoint_path`, `api_query`, "
            "`api_path_params`, `api_headers`, `api_body` string "
            "leaves can contain `{{var_name}}` placeholders that "
            "match `variables[].name`. At `qa_run` time the user "
            "(or `qa_run`'s `vars` arg) provides the values.\n\n"
            "**Variables** : each entry is "
            "`{name, label?, placeholder?, required?, description?}`. "
            "Required defaults to true. `name` must be a "
            "UPPER_SNAKE_CASE or lower_snake_case identifier matching "
            "the `{{var_name}}` placeholders in the API config.\n\n"
            "**Safety** : QAs have no `enabled` flag — every QA can "
            "be launched on demand. No auto-fire risk. The user "
            "reviews the QA in the Quick APIs page before invoking. "
            "Same safety profile as `qp_create_draft`.\n\n"
            "**Iteration** : if you realise after testing that the "
            "QA needs tweaking (heavier payload than expected, missing "
            "query param, wrong extract path), use `qa_update({qa_id, "
            "...patch})` — it merges your patch on top of the existing "
            "QA so you only specify what changed. No need to ask the "
            "user to click through the Quick APIs page UI.\n\n"
            "Returns the created QA JSON (id, all fields) so the "
            "agent can echo the id back to the user "
            "(`Quick API created as <id> — try it with qa_run({qa_id, vars})`)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "QA name (1-200 chars, displayed on the QA card)."},
                "api_plugin_slug": {"type": "string", "description": "Plugin slug from `mcp_list` (e.g. `mcp-atlassian`, `api-resend`, `api-custom-foo`)."},
                "api_config_id": {"type": "string", "description": "Plugin config id from `mcp_list.configs[].config_id`. Pin the QA to a specific config (per-project or global)."},
                "api_endpoint_path": {"type": "string", "description": "Endpoint path matching one of the plugin's declared endpoints (e.g. `/rest/api/3/issue/{ticket_id}`). May contain `{{var}}` placeholders OR `{path_param}` segments."},
                "api_method": {"type": "string", "description": "HTTP method override : `GET | POST | PUT | PATCH | DELETE`. Defaults to the plugin endpoint's declared method when omitted."},
                "api_query": {"type": "object", "description": "Query-string parameters as key→value map. Values may contain `{{var}}` placeholders."},
                "api_path_params": {"type": "object", "description": "Path-segment substitutions for `{name}` segments in the endpoint path."},
                "api_headers": {"type": "object", "description": "Extra request headers. NEVER pass auth — Kronn injects per the plugin spec."},
                "api_body": {"description": "JSON body for POST/PUT/PATCH (object/array). String leaves can contain `{{var}}` placeholders."},
                "api_extract": {"type": "object", "description": "Optional JSONPath extract spec: `{path: \"$.items\", fail_on_empty: false}`."},
                "api_pagination": {"type": "object", "description": "Optional pagination spec (page/offset/cursor strategies)."},
                "api_timeout_ms": {"type": "integer", "description": "Optional per-call timeout in ms. Defaults to plugin default."},
                "api_max_retries": {"type": "integer", "description": "Optional retry count on transient HTTP errors."},
                "variables": {
                    "type": "array",
                    "description": "Variable definitions (each `{ name, label?, placeholder?, required?, description? }`).",
                },
                "description": {"type": "string", "description": "Optional one-line description shown on the QA card."},
                "icon": {"type": "string", "description": "Optional single-emoji prefix (e.g. `🎫` / `📧` / `🔍`)."},
                "project_id": {"type": "string", "description": "Optional Kronn project id to bind the QA to (auto-inherited from current disc when absent)."},
                "profile_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional profile bindings (used when QA result feeds an agent)."},
                "directive_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional directive bindings."},
            },
            "required": ["name", "api_plugin_slug", "api_config_id", "api_endpoint_path"],
        },
    },
    {
        "name": "qa_update",
        "description": (
            "Patch an existing Quick API (QA). Loads the current QA, "
            "merges your patch on top of it field-by-field, and writes "
            "the result back via `PUT /api/quick-apis/<id>`. You only "
            "specify what CHANGES — every field you don't pass keeps "
            "its existing value (variables / profile_ids / "
            "directive_ids included, which the bare PUT route would "
            "reset to empty).\n\n"
            "**Typical iterations** :\n"
            "  - Adding `api_extract` to a verbose-payload QA after "
            "  probing showed 12k+ token responses\n"
            "  - Adding `fields=summary,status` to `api_query` for "
            "  vendor-side filtering\n"
            "  - Fixing a typo in `name` / `description`\n"
            "  - Adding a missing `variables[]` entry after realising "
            "  the endpoint needed a path param\n"
            "  - Bumping `api_max_retries` after a flaky vendor "
            "  surfaced\n\n"
            "**Pure additive — no need to re-supply existing fields**. "
            "Pass `{qa_id, api_extract: {path: \"$.fields\"}}` and the "
            "rest of the QA stays exactly as it was. Inverse of "
            "`qa_create_draft` — only `qa_id` is required; every other "
            "field is optional and skipped when absent.\n\n"
            "**Returns the updated QA** so the agent can confirm the "
            "patch applied as intended. If you intend further calls "
            "(e.g. test the change with `qa_run` right after), the "
            "returned shape lets you do so without an extra `qa_list` "
            "round-trip."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qa_id": {"type": "string", "description": "Quick API id (from `qa_list`)."},
                # Every QA field can be patched. None required beyond qa_id.
                "name": {"type": "string", "description": "New name. Omit to keep existing."},
                "icon": {"type": "string", "description": "New icon. Omit to keep existing."},
                "description": {"type": "string", "description": "New description. Omit to keep existing."},
                "api_plugin_slug": {"type": "string", "description": "Re-target to a different plugin (rare)."},
                "api_config_id": {"type": "string", "description": "Re-target to a different config id."},
                "api_endpoint_path": {"type": "string", "description": "Change the endpoint."},
                "api_method": {"type": "string"},
                "api_query": {"type": "object", "description": "Replace the query map. Pass `{}` to clear."},
                "api_path_params": {"type": "object"},
                "api_headers": {"type": "object"},
                "api_body": {"description": "Replace the body JSON."},
                "api_extract": {"type": "object", "description": "Replace the extract spec (the most common patch)."},
                "api_pagination": {"type": "object"},
                "api_timeout_ms": {"type": "integer"},
                "api_max_retries": {"type": "integer"},
                "variables": {"type": "array", "description": "Replace the variables list. Pass `[]` to clear all."},
                "profile_ids": {"type": "array", "items": {"type": "string"}},
                "directive_ids": {"type": "array", "items": {"type": "string"}},
                "project_id": {"type": "string", "description": "Re-bind to a different project. Omit to keep existing."},
            },
            "required": ["qa_id"],
        },
    },
    # ─── 0.8.6 — Agent API broker (no secrets in prompt) ────────────────
    # Lets the agent INVOKE a Kronn-configured API plugin without ever
    # seeing the credentials. The backend decrypts the env, resolves auth
    # per the plugin's ApiSpec, and returns the canonical envelope. Reuses
    # the same executor as workflow ApiCall steps so behaviour is
    # byte-identical. Cf. [[project_agent_api_broker_0_8_6]].
    {
        "name": "api_call",
        "description": (
            "Invoke a Kronn-configured API plugin (registry or custom) "
            "WITHOUT seeing the credentials. Kronn handles auth (Bearer, "
            "API key in header/query, OAuth, etc.) per the plugin spec "
            "and returns the canonical envelope `{data, status, summary}`.\n\n"
            "**Discovery first**: call `mcp_list` to find available "
            "plugins. Each entry has a `hint` field — `READY` plugins "
            "are directly callable; `NEEDS_RESEARCH` ones need you to "
            "fetch their `docs_url` first to identify endpoints (then "
            "either ask the user to declare them in the Kronn UI, OR "
            "hand-craft the path knowing allowlist validation may "
            "fail).\n\n"
            "**Plugin selection** — pass EITHER:\n"
            "  (a) `api_plugin_slug` + `api_config_id` (from `mcp_list`)\n"
            "  (b) `quick_api_id` (from `qa_list`) — for saved Quick APIs\n\n"
            "**Project scope** — auto-resolved server-side from 3 "
            "sources (in priority): (1) explicit `project_id` arg if "
            "passed, (2) the disc context if Kronn spawned you from a "
            "disc (auto-injected), (3) the chosen `api_config_id`'s "
            "first linked project. **Host-CLI sessions** (launched "
            "outside Kronn) work natively via source #3 — no env var "
            "or arg needed when you pick a config that's project-"
            "scoped. Only pass `project_id` explicitly when the config "
            "is global and you want to attribute the call to a "
            "specific project.\n\n"
            "**Auth happens server-side**: never put credentials in the "
            "request body, headers, query, or path. Kronn injects them "
            "from the encrypted DB config. If a plugin's auth scheme "
            "doesn't seem to be applied, that's a plugin spec issue "
            "(report it), not something to work around with hand-typed "
            "Authorization headers.\n\n"
            "**Non-secret config values via ${ENV.X}**: when a plugin "
            "has a non-secret identifier (e.g. Didomi's `organization_id`, "
            "an account_id, a workspace_slug) stored in its config, you "
            "can reference it in your call using the `${ENV.KEY}` syntax "
            "(use the env_key from `mcp_list.servers_with_api[].config_keys`). "
            "Example: `query: { organization_id: '${ENV.ORGANIZATION_ID}' }`. "
            "Kronn substitutes server-side — you never see the actual "
            "value. Works in `endpoint_path`, `path_params`, `query`, "
            "`headers`, and `body` (string leaves).\n\n"
            "Returns `{success, data, status, summary, http_status, "
            "error?}`. `data` is what downstream agent reasoning should "
            "consume; `summary` is the one-liner suitable for echoing "
            "back to the user."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "description": "Optional explicit project scope. Usually unnecessary — server resolves from `api_config_id`'s project_ids. Set only when calling a global config and you want to attribute the call to a specific project, OR to override the disc-derived scope.",
                },
                "api_plugin_slug": {
                    "type": "string",
                    "description": "Plugin slug from `mcp_list.servers_with_api[].id` (e.g. `mcp-atlassian`, `custom-didomi-27c67bd7`). Either this+`api_config_id`, or `quick_api_id`, MUST be provided.",
                },
                "api_config_id": {
                    "type": "string",
                    "description": "Credential set id from `mcp_list.configs[].config_id`. Required when using `api_plugin_slug`.",
                },
                "quick_api_id": {
                    "type": "string",
                    "description": "Alternative to plugin_slug+config_id: a saved Quick API id (from `qa_list`). Convenient when the user already pinned an endpoint + params.",
                },
                "endpoint_path": {
                    "type": "string",
                    "description": "Endpoint path as declared in the plugin's ApiSpec (e.g. `/rest/api/3/issue/{{issue_key}}` or `/widgets/notices`). The executor's allowlist refuses anything not in the spec.",
                },
                "method": {
                    "type": "string",
                    "description": "HTTP method override. Defaults to the method declared in the plugin spec. Uppercase: `GET | POST | PUT | PATCH | DELETE`.",
                },
                "path_params": {
                    "type": "object",
                    "description": "Path-segment substitutions (e.g. `{ \"owner\": \"DocRoms\", \"repo\": \"Kronn\" }` for `/repos/{owner}/{repo}`).",
                },
                "query": {
                    "type": "object",
                    "description": "Query-string parameters. Values are percent-encoded after substitution.",
                },
                "headers": {
                    "type": "object",
                    "description": "Extra request headers. NEVER pass auth headers here — Kronn injects them per the plugin spec.",
                },
                "body": {
                    "description": "JSON body for POST/PUT/PATCH. Pass a JSON object/array directly (not a serialized string).",
                },
                "extract": {
                    "type": "object",
                    "description": "Optional JSONPath extract: `{ \"path\": \"$.items[0]\", \"fail_on_empty\": false }`. When omitted, the full response is returned in `data`.",
                },
            },
            "required": ["endpoint_path"],
        },
    },
    # ─── 0.8.6 phase 4 — MCP Remote Control (launch + track) ────────────
    # Three tools that turn Kronn into a fully MCP-driveable backend :
    # an agent (typically Claude Code mobile linked to a PC session) can
    # LAUNCH a workflow or QP, then TRACK its progress without ever
    # opening the desktop UI. Every response carries a `next_check`
    # field — a smart-polling hint computed from historical averages
    # (workflow_runs.total_duration_ms / qp_versions.avg_duration_ms).
    # Honour it to slash mobile token cost ~80% vs naïve polling.
    {
        "name": "workflow_trigger",
        "description": (
            "Launch a Kronn workflow run from MCP — same effect as the "
            "UI's Trigger button, but JSON-only (no SSE). Returns "
            "`{run_id, workflow_id, workflow_name, status, started_at, "
            "expected_duration_ms?, samples, next_check}`.\n\n"
            "**Workflow discovery first** : call `workflow_list` to "
            "find the right `workflow_id`. The workflow MUST be enabled "
            "(`enabled: true`) — disabled drafts are refused with a "
            "clear error.\n\n"
            "**`next_check`** — a hint of the form `{wait_seconds, "
            "reason, confidence}`. After the trigger, wait that many "
            "seconds then call `workflow_run_status({run_id})`. The "
            "first wait is always at least 30s (sanity check that the "
            "run actually started). Honour it — naïve 10s polling on a "
            "2-min workflow burns ~13× more tokens than this hint "
            "schedules. `confidence: baseline` ⇒ the average is "
            "reliable. `confidence: no_baseline` ⇒ first time we run "
            "this workflow, just check every 60s.\n\n"
            "**Variables** : when the workflow declares manual launch "
            "variables, pass them as `variables: {name: value, ...}`. "
            "Required ones must be non-empty — same validation as the "
            "UI form."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {
                    "type": "string",
                    "description": "Kronn workflow id (from `workflow_list`).",
                },
                "variables": {
                    "type": "object",
                    "description": "Manual-launch variables as a flat key→value map (string values only).",
                },
            },
            "required": ["workflow_id"],
        },
    },
    {
        "name": "workflow_run_status",
        "description": (
            "Read the current state of a workflow run launched via "
            "`workflow_trigger` (or via the UI). Returns "
            "`{run_id, workflow_id, status, started_at, finished_at?, "
            "elapsed_ms, current_step?, step_count, tokens_used, "
            "steps[], expected_duration_ms?, samples, next_check?}`. "
            "`steps[]` has each step's name + status + duration_ms + "
            "tokens_used + 200-char output excerpt + step_type.\n\n"
            "**Terminal vs in-flight** : when `status` is one of "
            "`Success`, `Failed`, `Cancelled`, `StoppedByGuard`, "
            "`next_check` is `null` — no further polling needed. "
            "Otherwise, wait `next_check.wait_seconds` then call again. "
            "The hint adapts : projection-anchored while within the "
            "average duration, fixed backoff after overshoot.\n\n"
            "**For batch workflows** : individual child discussions are "
            "not listed here in PR1 — use `workflow_run_discussions` "
            "(0.8.6 phase 4 PR2) when shipped, or read the disc list "
            "from the Kronn DB directly. For linear workflows the "
            "`steps[]` array is enough."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "run_id": {"type": "string", "description": "Run id (from `workflow_trigger`)."},
            },
            "required": ["run_id"],
        },
    },
    {
        "name": "qp_run",
        "description": (
            "Launch a Quick Prompt as a fresh disc — one-shot mode. "
            "Returns `{disc_id, qp_id, qp_name, agent, "
            "expected_duration_ms?, samples, next_check}`. The QP's "
            "default agent is used unless `agent` is overridden.\n\n"
            "**Discovery first** : call `qp_list` to find the right "
            "`qp_id` + see required variables. Pass them as "
            "`vars: {name: value, ...}` — same `{{var}}` substitution "
            "as the UI form. Required vars must be non-empty.\n\n"
            "**Track the run** : the agent is kicked off automatically "
            "in the background. After `next_check.wait_seconds`, read "
            "the result via `disc_load_other(disc_id)` — same tool used "
            "for any other disc. `next_check` here uses the QP's "
            "weighted-average first-reply duration across all versions "
            "(`qp_versions` metrics).\n\n"
            "**Optional project scope** : if you don't pass "
            "`project_id`, the QP's declared project (or no project) "
            "is used. Pass `project_id` explicitly to override.\n\n"
            "**vs `qp_create_draft`** : draft creates a NEW QP "
            "definition ; `qp_run` LAUNCHES an existing QP."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qp_id": {"type": "string", "description": "Quick Prompt id (from `qp_list`)."},
                "vars": {
                    "type": "object",
                    "description": "Variable values for `{{var}}` placeholders, as a flat key→value map (string values).",
                },
                "agent": {
                    "type": "string",
                    "description": "Optional agent override : `ClaudeCode | Codex | Vibe | GeminiCli | Kiro | CopilotCli | Ollama | Custom`. Defaults to the QP's declared agent.",
                },
                "project_id": {
                    "type": "string",
                    "description": "Optional project override. Defaults to the QP's declared project.",
                },
                "title": {
                    "type": "string",
                    "description": "Optional disc title. Defaults to `<qp_name> — MCP run`.",
                },
            },
            "required": ["qp_id"],
        },
    },
    {
        "name": "qa_run",
        "description": (
            "Execute a saved Quick API (QA) by id — fully synchronous. "
            "Returns the parsed envelope `{success, duration_ms, "
            "envelope: {data, status, summary}, error?}` inline. NO "
            "`next_check` (QAs are fast, sub-second to a few seconds) "
            "— just await the response.\n\n"
            "**Why use this over `api_call`** :\n"
            "  - Zero token cost on request construction — the QA "
            "already encodes endpoint, method, headers, query, body, "
            "extract, pagination. You only pass the `vars`.\n"
            "  - Same result across agents — Claude / Codex / Vibe / "
            "Ollama all call the same QA → identical request shape, "
            "identical result. Pure mechanical work, deagentified.\n"
            "  - Maintenance centralised — if the endpoint changes, "
            "the user updates the QA once ; every consumer benefits.\n"
            "  - Audited — every call lands in `api_call_logs` with "
            "the QA id as `caller_id`, so the user can mesure ROI.\n\n"
            "**Discovery** : call `qa_list` to find the right `qa_id` "
            "+ see required variable names. Pass values matching those "
            "names via `vars`. Required variables that are missing or "
            "empty → 400 with a clear error.\n\n"
            "**vs `api_call`** : `api_call` is the low-level broker "
            "for one-shot calls where no saved QA fits. `qa_run` is "
            "the high-level wrapper for recurring patterns. Always "
            "prefer `qa_run` when a matching QA exists ; fall back "
            "to `api_call` only when the user has no QA for this "
            "use case yet (and consider suggesting they save one)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qa_id": {"type": "string", "description": "Quick API id (from `qa_list`)."},
                "vars": {
                    "type": "object",
                    "description": "Variable values for the QA's `{{var}}` placeholders, as a flat key→value map (string values). Keys must match the `variables[].name` returned by `qa_list`.",
                },
            },
            "required": ["qa_id"],
        },
    },
]


# ─── HTTP plumbing ─────────────────────────────────────────────────────────

def _backend_url():
    return os.environ.get("KRONN_BACKEND_URL", "http://127.0.0.1:3140").rstrip("/")


# 0.8.6 phase 2 — Captured MCP `clientInfo` from initialize handshake.
#
# Every MCP client sends `{name, version}` in its `initialize` request.
# Claude Code → "claude-code", Codex → "codex-cli", etc. We capture
# this once and use it to derive the AgentType for `disc_join` /
# `disc_leave` server calls — way better UX than asking the user to
# set `KRONN_AGENT_TYPE` env before launching each CLI.
_CLIENT_INFO = {"name": None, "version": None}


def _infer_agent_type_from_client_name(name):
    """Map an MCP `clientInfo.name` to the canonical Kronn `AgentType`.

    Substring match (lowercase) — clients vary on hyphenation and
    suffixes (`claude-code`, `Claude Code`, `codex-cli`, `codex`…).
    Falls back to `Unknown` so the backend's `discussion_sessions`
    row still gets created — better than rejecting the join."""
    if not name:
        return "Unknown"
    lower = name.lower()
    # Order matters : check `claude` before `copilot` so the
    # `claude-code-with-copilot-bridge` edge case (if it ever
    # happens) doesn't mis-route.
    if "claude" in lower:
        return "ClaudeCode"
    if "codex" in lower:
        return "Codex"
    if "gemini" in lower:
        return "GeminiCli"
    if "kiro" in lower:
        return "Kiro"
    if "copilot" in lower:
        return "CopilotCli"
    if "vibe" in lower:
        return "Vibe"
    if "cursor" in lower or "cline" in lower:
        # No dedicated AgentType yet; surface them as Custom so the
        # header still shows something useful, and we know which
        # client connected via the version string.
        return "Custom"
    return "Unknown"


# 0.8.6 fix 2026-05-21 — stable session_id across the bridge lifetime.
#
# Previously each tool call regenerated `f"adhoc-{uuid.uuid4()}"` ;
# `disc_join` got UUID A, `disc_leave` got UUID B, the backend's
# `find_active_session` query missed → `left: false` even though the
# user did join. Caught live on the 3-agent tennis match (Claude +
# Codex both got `left: false` on the final disc_leave call).
#
# Resolution order, evaluated ONCE at module load :
#   1. `KRONN_SESSION_ID` env (Kronn-launched agents inherit this)
#   2. `KRONN_CALLER_SESSION_ID` env (older alias)
#   3. Random `adhoc-<uuid4>` (host-launched, no Kronn env injection)
#
# Stays stable for the entire bridge process lifetime so every tool
# call from the same CLI session uses the same row in
# `discussion_sessions`.
_BRIDGE_SESSION_ID = (
    os.environ.get("KRONN_SESSION_ID")
    or os.environ.get("KRONN_CALLER_SESSION_ID")
    or f"adhoc-{uuid.uuid4()}"
)


def _session_id_for_caller():
    """Stable per-process session id. See `_BRIDGE_SESSION_ID` rationale."""
    return _BRIDGE_SESSION_ID


def _parent_process_cmdline():
    """Read the parent process's cmdline on Linux/WSL — best-effort
    fallback for CLIs that don't send a useful `clientInfo.name` in
    the MCP initialize handshake (e.g. Vibe, some Codex builds).
    Returns `None` on non-Linux systems or if /proc is unavailable.
    """
    try:
        ppid = os.getppid()
        with open(f"/proc/{ppid}/cmdline", "rb") as fh:
            raw = fh.read()
        # `cmdline` is NUL-separated argv ; we only care about the
        # combined string for a substring match.
        return raw.replace(b"\x00", b" ").decode("utf-8", errors="replace").lower()
    except Exception:
        return None


def _agent_type_for_session():
    """Resolve the agent_type to use in disc_join / disc_leave / wait
    server calls. Priority :
      1. Explicit `KRONN_AGENT_TYPE` env (legacy / wrapper overrides)
      2. Inferred from MCP `clientInfo.name` (auto-detect, 0.8.6)
      3. Inferred from parent-process cmdline (Vibe fallback, 2026-05-21)
      4. `KRONN_CALLER_AGENT` env (older alias)
      5. `Unknown` (server still accepts the join, the header just
         shows a generic chip)
    """
    explicit = os.environ.get("KRONN_AGENT_TYPE")
    if explicit:
        return explicit

    inferred = _infer_agent_type_from_client_name(_CLIENT_INFO.get("name"))
    if inferred != "Unknown":
        return inferred

    # 2026-05-21 fallback : Vibe was showing as "Unknown" in the header
    # because its MCP client doesn't send a name we recognise (or any
    # name at all). Peek at the parent process's cmdline — `vibe`,
    # `codex`, `claude`, etc. usually appear there in plain text.
    cmdline = _parent_process_cmdline()
    if cmdline:
        inferred_ppid = _infer_agent_type_from_client_name(cmdline)
        if inferred_ppid != "Unknown":
            print(
                f"kronn-internal: agent_type inferred from parent cmdline "
                f"({inferred_ppid}) — clientInfo.name was {_CLIENT_INFO.get('name')!r}",
                file=sys.stderr,
            )
            return inferred_ppid

    legacy = os.environ.get("KRONN_CALLER_AGENT")
    if legacy:
        return legacy

    # Log so user can see what was received and we can extend the
    # matcher map in a future release if a new CLI emerges.
    print(
        f"kronn-internal: could not infer agent_type — clientInfo={_CLIENT_INFO!r} "
        f"cmdline={cmdline!r} ; falling back to 'Unknown'",
        file=sys.stderr,
    )
    return "Unknown"


# 0.8.6 phase 2 — Runtime disc binding.
#
# Before phase 2 the bridge could ONLY be told which disc to operate
# on via `KRONN_DISCUSSION_ID` set in the process env at boot. That
# works fine for Kronn-launched agents (the Rust runner injects the
# env), but locks out host-launched CLIs (user types `codex` in their
# own terminal) — they had to relaunch the bridge with the env to use
# any `disc_*` tool.
#
# Phase 2 adds a module-level mutable binding initialised from env,
# settable at runtime by `disc_join({token})`. Same `_disc_id()`
# entry point for all downstream tools = zero changes elsewhere.
_CURRENT_DISC_ID = os.environ.get("KRONN_DISCUSSION_ID") or None


def _set_current_disc_id(disc_id):
    """Mutate the disc binding (used by `disc_join`). Pass `None` to
    clear (used by `disc_leave`). Side-effect : invalidates the cached
    disc meta so the next read goes to the new disc."""
    global _CURRENT_DISC_ID
    _CURRENT_DISC_ID = disc_id
    _CURRENT_DISC_META_CACHE["checked"] = False
    _CURRENT_DISC_META_CACHE["value"] = None


def _disc_id():
    global _CURRENT_DISC_ID
    if not _CURRENT_DISC_ID:
        # Re-check env at runtime in case `KRONN_DISCUSSION_ID` was set
        # AFTER boot (legacy wrappers, late-init launchers, tests that
        # patch env in setUp). Preserves backward compat with the pre-
        # phase-2 contract while still surfacing the new disc_join path
        # in the error message.
        env_did = os.environ.get("KRONN_DISCUSSION_ID")
        if env_did:
            _CURRENT_DISC_ID = env_did
            return _CURRENT_DISC_ID
        raise RuntimeError(
            "no disc bound — set KRONN_DISCUSSION_ID env (Kronn-launched) "
            "or call disc_join({token: \"kr-join-...\"}) first (host-launched)"
        )
    return _CURRENT_DISC_ID


# 0.8.5 — cache the current discussion's meta once per process. Used by
# the mutating tools (disc_create / workflow_create_draft /
# qp_create_draft) to auto-inherit:
#   - `project_id` — so agent artifacts land in the active project,
#     not "Général" (flagged 2026-05-18 during MCP dogfooding).
#   - `source_agent` + `source_session_id` — so the existing 0.8.4
#     sidebar badge ("📥 ClaudeCode") fires on every MCP-created
#     disc, making UI-created discs visually distinct from
#     agent-created ones at a glance.
# The agent can still override either by passing explicit args.
_CURRENT_DISC_META_CACHE = {"checked": False, "value": None}


def _current_disc_meta():
    """Return `{id, project_id, agent}` of the parent disc, or `None`."""
    if _CURRENT_DISC_META_CACHE["checked"]:
        return _CURRENT_DISC_META_CACHE["value"]
    _CURRENT_DISC_META_CACHE["checked"] = True
    try:
        disc_id = _disc_id()
    except RuntimeError:
        # KRONN_DISCUSSION_ID not set (legacy launcher, dev scaffold).
        # No inheritance possible; return None silently.
        return None
    try:
        url = f"{_backend_url()}/api/discussions/{disc_id}/meta"
        req = urllib.request.Request(url, method="GET")
        with urllib.request.urlopen(req, timeout=5) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
        data = payload.get("data") or {}
        meta = {
            "id": disc_id,
            "project_id": data.get("project_id"),
            "agent": data.get("agent"),
        }
        _CURRENT_DISC_META_CACHE["value"] = meta
        return meta
    except Exception as e:
        # Lookup failed (backend unreachable, disc not found, etc.).
        # Don't fail the caller — the artifact just lands without
        # inheritance, same as pre-0.8.5 behaviour.
        print(
            f"kronn-internal: failed to resolve current disc's meta "
            f"({e}); inheritance fields will fall back to defaults",
            file=sys.stderr,
        )
        return None


def _current_project_id():
    meta = _current_disc_meta()
    return meta.get("project_id") if meta else None


def _http(method, path, body=None):
    url = f"{_backend_url()}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, method=method, data=data)
    req.add_header("Content-Type", "application/json")
    token = os.environ.get("KRONN_AUTH_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {e.code}: {body[:500]}")


def _unwrap(envelope):
    """Kronn's `ApiResponse` wraps every reply as {success, data, error}.
    Tools return the inner `data` on success, raise on `success=false`."""
    if not isinstance(envelope, dict):
        raise RuntimeError(f"unexpected response shape: {envelope!r}")
    if not envelope.get("success", False):
        raise RuntimeError(envelope.get("error") or "backend reported success=false")
    return envelope.get("data")


# ─── Tool dispatch ─────────────────────────────────────────────────────────

def call_disc_meta(_args):
    return _unwrap(_http("GET", f"/api/discussions/{_disc_id()}/meta"))


def call_disc_get_message(args):
    idx = args.get("idx")
    if idx is None:
        raise RuntimeError("disc_get_message: missing required 'idx'")
    return _unwrap(_http("GET", f"/api/discussions/{_disc_id()}/message/{idx}"))


def call_disc_summarize(args):
    body = {
        "from": args.get("from"),
        "to": args.get("to"),
        "force_refresh": bool(args.get("force_refresh", False)),
    }
    return _unwrap(_http("POST", f"/api/discussions/{_disc_id()}/summarize", body))


# ─── 0.8.4 (#294) cross-agent memory tools ─────────────────────────────

def call_disc_create(args):
    if not args.get("title"):
        raise RuntimeError("disc_create: missing required 'title'")
    if not args.get("agent"):
        raise RuntimeError("disc_create: missing required 'agent'")
    body = {
        "title": args["title"],
        "agent": args["agent"],
    }
    for k in ("language", "project_id", "source_agent", "source_session_id"):
        v = args.get(k)
        if v is not None:
            body[k] = v
    # 0.8.5 — auto-inherit two fields from the current discussion when
    # the agent doesn't pass them explicitly:
    # - `project_id`: agent artifacts land in the active project, not
    #   "Général" (flagged 2026-05-18).
    # - `source_agent`: makes the existing 0.8.4 sidebar badge
    #   ("📥 ClaudeCode") fire on every MCP-created disc so the user
    #   can visually distinguish UI-created vs agent-created discs at
    #   a glance. The badge only checks for `sourceAgent` truthy
    #   (cf. `SwipeableDiscItem.tsx:147`), so we don't need
    #   `source_session_id` to render it.
    # We intentionally DO NOT auto-fill `source_session_id`: the
    # `/api/disc/create` endpoint treats `(source_agent,
    # source_session_id)` as an idempotency key (cf.
    # `api/disc_source.rs:78`). If we always set session = parent
    # disc id, the second MCP call from the same parent would
    # collapse to the first child disc instead of creating a new
    # one. Agents can still pass `source_session_id` explicitly when
    # they actually want one-disc-per-external-session semantics.
    # Cf. [[project_mcp_draft_creation_0_8_5]].
    meta = _current_disc_meta()
    if meta:
        if "project_id" not in body and meta.get("project_id"):
            body["project_id"] = meta["project_id"]
        if "source_agent" not in body and meta.get("agent"):
            body["source_agent"] = meta["agent"]
    return _unwrap(_http("POST", "/api/disc/create", body))


def call_disc_append(args):
    """0.8.6 fix 2026-05-21 — ergonomic shorthand for multi-agent chat.

    Two call styles accepted :
      1. Heavy (0.8.4 cross-agent-memory) :
         `disc_append({disc_id, messages: [{source_msg_id, role, content,
         agent_type}, …]})` — used to bulk-import a CLI transcript.
      2. Light (NEW, multi-agent collab) :
         `disc_append({content: "Hello peers"})` — used when an agent
         wants to say one thing in the live discussion. `disc_id`
         defaults to the runtime-bound disc from `disc_join`,
         `source_msg_id` is auto-generated (UUIDv4),
         `role` defaults to "Agent",
         `agent_type` is inferred from the MCP clientInfo handshake.

    The bridge normalises both into the heavy shape before POSTing
    so the backend route stays simple.
    """
    disc_id = args.get("disc_id") or _disc_id()
    messages = args.get("messages")

    # Light shorthand : an agent passed `content` directly.
    if not messages and args.get("content"):
        messages = [{
            "source_msg_id": f"live-{uuid.uuid4()}",
            "role": args.get("role") or "Agent",
            "content": args["content"],
            "agent_type": (
                args.get("agent_type")
                or _agent_type_for_session()
                or None
            ),
        }]

    if not isinstance(messages, list) or not messages:
        raise RuntimeError(
            "disc_append: pass either `content: \"...\"` (single message, "
            "easiest for multi-agent chat) OR `messages: [{source_msg_id, "
            "role, content}, …]` (bulk transcript import)"
        )
    return _unwrap(_http("POST", "/api/disc/append", {
        "disc_id": disc_id,
        "messages": messages,
    }))


def call_disc_link(args):
    for k in ("disc_id", "source_agent", "source_session_id"):
        if not args.get(k):
            raise RuntimeError(f"disc_link: missing required '{k}'")
    return _unwrap(_http("POST", "/api/disc/link", {
        "disc_id": args["disc_id"],
        "source_agent": args["source_agent"],
        "source_session_id": args["source_session_id"],
    }))


def call_disc_unlink(args):
    disc_id = args.get("disc_id")
    if not disc_id:
        raise RuntimeError("disc_unlink: missing required 'disc_id'")
    return _unwrap(_http("POST", "/api/disc/unlink", {"disc_id": disc_id}))


def call_disc_find_by_session(args):
    src_agent = args.get("source_agent")
    src_sess = args.get("source_session_id")
    if not src_agent or not src_sess:
        raise RuntimeError("disc_find_by_session: missing required 'source_agent' or 'source_session_id'")
    qs = urllib.parse.urlencode({
        "source_agent": src_agent,
        "source_session_id": src_sess,
    })
    return _unwrap(_http("GET", f"/api/disc/find_by_session?{qs}"))


def call_disc_search(args):
    q = args.get("q")
    if not q:
        raise RuntimeError("disc_search: missing required 'q'")
    params = {"q": q}
    if args.get("limit") is not None:
        params["limit"] = args["limit"]
    qs = urllib.parse.urlencode(params)
    return _unwrap(_http("GET", f"/api/disc/search?{qs}"))


def call_disc_join(args):
    """0.8.6 phase 2 — bind this bridge to a Kronn disc via invite token.

    On success, mutates `_CURRENT_DISC_ID` so every subsequent
    `_disc_id()`-needing tool resolves to the joined disc. Without
    this tool, host-launched CLIs (codex/claude run directly in a
    terminal, not via Kronn's UI) couldn't use any `disc_*` tool
    because their process env never gets `KRONN_DISCUSSION_ID`.

    The companion route is `POST /api/discussions/peer-join` in
    `backend/src/api/disc_invite.rs`. It atomically validates the
    token + creates a `discussion_sessions` peer row + returns the
    disc context — single round trip.
    """
    token = args.get("token")
    if not token:
        raise RuntimeError("disc_join: missing required 'token' (kr-join-…)")

    # 0.8.6 phase 2 — derive agent_type from the MCP `clientInfo`
    # captured at initialize time (Claude Code → ClaudeCode, Codex
    # → Codex, …) rather than requiring the user to pre-set
    # `KRONN_AGENT_TYPE`. Without this fix the header showed every
    # peer as "Unknown" (reported live 2026-05-21).
    agent_type = _agent_type_for_session()
    session_id = _session_id_for_caller()

    body = {
        "token": token,
        "agent_type": agent_type,
        "session_id": session_id,
    }
    result = _unwrap(_http("POST", "/api/discussions/peer-join", body))

    # Bind THIS process to the joined disc so subsequent tool calls
    # operate on it without the agent having to thread the disc_id
    # through every call.
    disc_id = result.get("disc_id") if isinstance(result, dict) else None
    if disc_id:
        _set_current_disc_id(disc_id)

    return result


def call_disc_invite_peer(_args):
    """0.8.6 (#56) — mint an invite for the currently-bound disc.

    Reuses `POST /api/discussions/:id/invite-peer` (route already
    serving the UI [+ Inviter] button). Letting an agent call this
    directly closes the last "user must click in Kronn UI" gap for
    multi-agent collab bootstrap.
    """
    disc_id = _disc_id()
    return _unwrap(_http("POST", f"/api/discussions/{disc_id}/invite-peer", {}))


def call_disc_create_room(args):
    """0.8.6 (#56) — create disc + mint invite in one call.

    Sequence:
      1. `disc_create` (existing route) — fresh discussion, optionally
         bound to a Kronn project. The created disc is auto-bound to
         this bridge process so subsequent `disc_*` calls land on it.
      2. `disc_invite_peer` (same as standalone tool above) — mint
         a token + instruction text the agent can hand to the user.

    The two-step is wrapped so the agent can do `disc_create_room` →
    `disc_append` → `disc_wait_for_peer` without ever leaving the MCP
    surface. If invite-minting fails after disc creation, the disc
    still exists (intentional: the user can click [+ Inviter] in the
    UI as a fallback).
    """
    title = args.get("title")
    if not title:
        raise RuntimeError("disc_create_room: missing required 'title'")

    # 0.8.6 fix 2026-05-22 — pre-fix the comment claimed "disc_create
    # already binds the process via _set_current_disc_id" but that's
    # NOT true (verified). The result was that disc_create_room created
    # the room server-side without switching the caller's bridge to
    # it. The caller stayed bound to the original disc (KRONN_DISCUSSION_ID
    # at boot, OR the previously joined disc) — easy to lose track of
    # what's happening if the user wasn't paying attention.
    #
    # The 0.8.6 fix keeps the non-binding behaviour (silent context-
    # switch would be even worse) but adds a `next_step` field in the
    # response that explicitly tells the agent what to do : stay in
    # the current disc + share the token, OR switch via disc_join.
    create_args = {"title": title, "agent": _agent_type_for_session() or "Unknown"}
    if args.get("language"):
        create_args["language"] = args["language"]
    if args.get("project_id"):
        create_args["project_id"] = args["project_id"]
    created = call_disc_create(create_args)

    disc_id = created.get("disc_id") if isinstance(created, dict) else None
    if not disc_id:
        # Surfaces a clear error if the backend response shape is unexpected.
        raise RuntimeError(
            "disc_create_room: backend returned no disc_id — cannot mint invite"
        )

    invite = _unwrap(_http("POST", f"/api/discussions/{disc_id}/invite-peer", {}))

    # Determine the next-step hint based on the current bridge binding.
    # If we ARE currently bound (the common case from a Kronn-launched
    # session), advise staying put + sharing the token. If we are NOT
    # bound (host-launched, no disc context), advise joining the new
    # room since there's no risk of losing context.
    current_disc = _CURRENT_DISC_ID
    if current_disc and current_disc != disc_id:
        next_step = (
            f"Your bridge is still bound to disc {current_disc[:8]}… — the new "
            f"room {disc_id[:8]}… is NOT auto-joined. Default behaviour : keep "
            f"talking here and SHARE `instruction_text` with the user so they "
            f"can bring a peer in. If you actually want to switch your own "
            f"context to the new room, call `disc_join({{token: \"<token>\"}})` "
            f"explicitly — your current binding will be replaced."
        )
    else:
        next_step = (
            f"Your bridge has no active disc binding. To start posting in the "
            f"new room {disc_id[:8]}…, call `disc_join({{token: \"<token>\"}})` "
            f"with the returned token. Or share `instruction_text` with the "
            f"user to bring a peer CLI in instead."
        )

    out = {
        "disc_id": disc_id,
        "title": created.get("title", title),
        "next_step": next_step,
    }
    if isinstance(invite, dict):
        out["token"] = invite.get("token")
        out["instruction_text"] = invite.get("instruction_text")
        out["expires_at"] = invite.get("expires_at")
        out["ttl_seconds"] = invite.get("ttl_seconds")
    return out


def call_disc_leave(_args):
    """0.8.6 phase 3 — leave the current disc + clear runtime binding.

    Mirrors `disc_join` : sends the bridge's (agent_type, session_id)
    to `/api/discussions/peer-leave` so the backend marks the right
    `discussion_sessions` row left. Then clears `_CURRENT_DISC_ID`
    locally so subsequent `disc_*` tools require a fresh `disc_join`.
    Idempotent : safe to call even if never joined.
    """
    # If unbound, the leave is a no-op locally — still hit the backend
    # in case the env var path bound a disc we don't remember.
    # Same (agent_type, session_id) pair as disc_join — the stable
    # `_session_id_for_caller` helper ensures the leave matches the
    # session row created at join time (fix 2026-05-21).
    agent_type = _agent_type_for_session()
    session_id = _session_id_for_caller()
    body = {"agent_type": agent_type, "session_id": session_id}
    try:
        result = _unwrap(_http("POST", "/api/discussions/peer-leave", body))
    except Exception:
        # Backend unreachable — still clear local binding so the agent
        # can rebind via `disc_join` next time.
        _set_current_disc_id(None)
        raise
    _set_current_disc_id(None)
    return result


def call_disc_wait_for_peer(args):
    """0.8.6 phase 3 — long-poll for new peer messages.

    Hits `GET /api/discussions/:id/wait` server-side. Excludes the
    caller's own `agent_type` (env-derived, same way as `disc_join`)
    so an agent doesn't wake itself on its own `disc_append`.
    """
    disc_id = _disc_id()
    since = args.get("since_sort_order")
    timeout_secs = args.get("timeout_secs")
    params = {}
    if since is not None:
        params["since_sort_order"] = since
    if timeout_secs is not None:
        params["timeout_secs"] = timeout_secs
    # Exclude THIS CLI's own agent_type so disc_append from self
    # doesn't wake the wait. Same clientInfo-derived resolution
    # as disc_join. Only forward if resolved (avoids accidentally
    # filtering out everything if `Unknown` somehow matched a peer).
    exclude = _agent_type_for_session()
    if exclude and exclude != "Unknown":
        params["exclude_agent_type"] = exclude
    qs = urllib.parse.urlencode(params)
    sep = "?" if qs else ""
    return _unwrap(_http("GET", f"/api/discussions/{disc_id}/wait{sep}{qs}"))


def call_disc_load_other(args):
    disc_id = args.get("disc_id")
    if not disc_id:
        raise RuntimeError("disc_load_other: missing required 'disc_id'")
    params = {"disc_id": disc_id}
    if args.get("from") is not None:
        params["from"] = args["from"]
    if args.get("to") is not None:
        params["to"] = args["to"]
    qs = urllib.parse.urlencode(params)
    return _unwrap(_http("GET", f"/api/disc/load_other?{qs}"))


def call_workflow_list(_args):
    # 0.8.5 — compact list of existing workflows. `GET /api/workflows`
    # already returns the summary shape (`WorkflowSummary` — no
    # `steps` body, only flat `trigger_type` + `step_count`), so we
    # pass it through verbatim minus a couple unused fields. The full
    # body is one `GET /api/workflows/<id>` call away when the agent
    # needs the step details — e.g. to read the prompt of an existing
    # step before drafting a similar one.
    data = _unwrap(_http("GET", "/api/workflows")) or []
    out = []
    for w in data:
        out.append({
            "id": w.get("id"),
            "name": w.get("name"),
            "enabled": w.get("enabled"),
            "project_id": w.get("project_id"),
            "project_name": w.get("project_name"),
            "trigger_type": w.get("trigger_type"),
            "step_count": w.get("step_count"),
            "last_run_status": (w.get("last_run") or {}).get("status"),
            "last_run_started_at": (w.get("last_run") or {}).get("started_at"),
        })
    return out


def call_qp_list(_args):
    # 0.8.5 — compact list. Keeps variable names so the agent can decide
    # if an existing QP fits the user's use case before drafting a new
    # one. Drops the full `prompt_template` body (the agent can call
    # `GET /api/quick-prompts/<id>` if it really needs the body).
    data = _unwrap(_http("GET", "/api/quick-prompts")) or []
    out = []
    for q in data:
        var_names = [v.get("name") for v in (q.get("variables") or [])]
        out.append({
            "id": q.get("id"),
            "name": q.get("name"),
            "agent": q.get("agent"),
            "description": q.get("description"),
            "variable_names": var_names,
            "skill_ids": q.get("skill_ids") or [],
            "project_id": q.get("project_id"),
            "tier": q.get("tier"),
        })
    return out


def call_qa_list(_args):
    # 0.8.5 — compact list. Keeps the plugin slug + endpoint path so the
    # agent can decide if an existing QA can be referenced from a new
    # workflow's `quick_api_id` slot.
    # 0.8.6 phase 4 — also surface `variables[]` so the agent knows
    # what to pass to the new `qa_run` tool without an extra round-trip
    # to `GET /api/quick-apis/<id>`. Each entry is
    # `{name, label, required, description}` — strictly the shape
    # `qa_run.vars` accepts as keys.
    data = _unwrap(_http("GET", "/api/quick-apis")) or []
    out = []
    for q in data:
        variables = [
            {
                "name": v.get("name"),
                "label": v.get("label"),
                "required": bool(v.get("required", True)),
                "description": v.get("description") or None,
            }
            for v in (q.get("variables") or [])
        ]
        out.append({
            "id": q.get("id"),
            "name": q.get("name"),
            "api_plugin_slug": q.get("api_plugin_slug"),
            "api_endpoint_path": q.get("api_endpoint_path"),
            "api_method": q.get("api_method"),
            "description": q.get("description"),
            "project_id": q.get("project_id"),
            "variables": variables,
        })
    return out


def call_mcp_list(_args):
    # 0.8.5 — wired MCP configs (the API plugin slug + config id the
    # workflow ApiCall steps need). Drops env values (secrets) and
    # scan diagnostics; keeps only what the agent needs to compose an
    # ApiCall step (slug + config_id + project scoping).
    data = _unwrap(_http("GET", "/api/mcps")) or {}
    out_configs = []
    for c in data.get("configs") or []:
        out_configs.append({
            "config_id": c.get("id"),
            "server_id": c.get("server_id"),
            "is_global": c.get("is_global"),
            "project_ids": c.get("project_ids") or [],
            "label": c.get("label"),
        })
    # Server registry (which slugs are KNOWN and have an api_spec) —
    # lets the agent answer "what API plugins are available to wire?".
    # 0.8.6 — enriched payload: includes `description`, `docs_url`, and
    # per-endpoint `description` so the agent can decide WHICH plugin
    # fits the user's request without having to ask back ("is there an
    # API for Didomi?" → mcp_list now answers natively). Custom plugins
    # (server_id starting with `api-custom-`) are included via the same
    # shape — they ship their own docs_url + description at create-time.
    out_servers = []
    for s in data.get("servers") or []:
        spec = s.get("api_spec") or {}
        if not spec:
            continue
        # api_spec.description sometimes empty (older plugins); fall
        # back to the server-level description so the agent always
        # has *something*.
        desc = (spec.get("description") or "").strip() or (s.get("description") or "").strip()
        endpoints = [
            {
                "path": e.get("path"),
                "method": e.get("method"),
                "description": (e.get("description") or "").strip() or None,
                # Some endpoints are flagged side-effecting in the
                # spec — surfacing the flag lets the agent (and a
                # future agent-api-broker tool, cf.
                # [[project_agent_api_broker_0_8_6]]) decide
                # whether the call needs explicit allow-side-effects
                # opt-in.
                "side_effect": bool(e.get("side_effect")),
            }
            for e in (spec.get("endpoints") or [])
        ]
        docs_url = spec.get("docs_url")
        # 0.8.6 — machine-actionable next-step hint. Without this, the
        # agent has to encode the "endpoints empty → read docs"
        # heuristic in its system prompt, which fragments across CLIs
        # (each one has its own). Putting the instruction inline in
        # the tool response makes the behaviour uniform across Claude
        # Code / Codex / Gemini / Vibe and survives prompt truncation.
        # The 3 branches map cleanly onto the agent's decision tree:
        #   READY → call directly
        #   NEEDS_RESEARCH → fetch docs_url FIRST
        #   AMBIGUOUS → ask the user
        # Use case the user surfaced 2026-05-19 on Didomi (custom
        # plugin, docs_url set, endpoints not yet declared).
        if endpoints:
            hint = (
                "READY: endpoints are declared and the ApiCall executor "
                "will allow-list them. You can draft an ApiCall step "
                "using one of the listed paths directly."
            )
        elif docs_url:
            hint = (
                f"NEEDS_RESEARCH: no endpoints declared yet. Fetch "
                f"`docs_url` ({docs_url}) to learn the API surface, "
                f"then either (a) suggest endpoints to the user so "
                f"they add them via the Kronn MCP / API page, or "
                f"(b) hand-craft path+method in an ApiCall step and "
                f"warn the user that allowlist validation will fail "
                f"until endpoints are declared."
            )
        else:
            hint = (
                "AMBIGUOUS: no endpoints AND no docs_url. Ask the user "
                "what this plugin is meant to call before drafting "
                "anything — Kronn has no information to act on."
            )
        # 0.8.6 — extract auth-managed env_keys so the agent knows
        # which ones are credentials (injected server-side, never
        # touch) vs which are non-secret identifiers (referenceable
        # via ${ENV.X} in path / query / headers / body). The
        # `auth_managed_keys` set is the union of every env_key
        # appearing in the auth variant's slots. Anything else in
        # `config_keys` is a free-form identifier.
        auth_managed_keys: set[str] = set()
        auth = spec.get("auth")
        if isinstance(auth, dict):
            for variant_data in auth.values():
                if not isinstance(variant_data, dict):
                    continue
                for key in (
                    "env_key", "user_env", "password_env",
                    "client_id_env", "client_secret_env",
                ):
                    v = variant_data.get(key)
                    if isinstance(v, str) and v:
                        auth_managed_keys.add(v)
                # TokenExchange exposes creds_env_keys list
                creds = variant_data.get("creds_env_keys")
                if isinstance(creds, list):
                    for k in creds:
                        if isinstance(k, str):
                            auth_managed_keys.add(k)
                # TokenExchange also references env_keys inside the
                # body_template via ${ENV.X} placeholders. Scan
                # recursively so creds used in the exchange body show
                # up as auth-managed even when creds_env_keys is
                # empty (the common case — most users don't fill the
                # defensive field). Same `${ENV.NAME}` regex Kronn
                # uses server-side.
                import re
                def _walk_for_env_refs(v):
                    if isinstance(v, str):
                        for m in re.finditer(r"\$\{ENV\.([A-Z0-9_]+)\}", v):
                            auth_managed_keys.add(m.group(1))
                    elif isinstance(v, dict):
                        for x in v.values(): _walk_for_env_refs(x)
                    elif isinstance(v, list):
                        for x in v: _walk_for_env_refs(x)
                body_tpl = variant_data.get("body_template")
                if body_tpl is not None:
                    _walk_for_env_refs(body_tpl)
        config_keys = [
            {
                "env_key": ck.get("env_key"),
                "label": ck.get("label") or ck.get("env_key"),
                # `auth_managed=True` ⇒ Kronn handles this one for you,
                # never reference it via ${ENV.X} (it would just leak
                # the secret to the prompt). `False` ⇒ free to use as
                # ${ENV.X} placeholder in path/query/headers/body.
                "auth_managed": (ck.get("env_key") or "") in auth_managed_keys,
            }
            for ck in (spec.get("config_keys") or [])
            if ck.get("env_key")
        ]

        out_servers.append({
            "id": s.get("id"),
            "name": s.get("name"),
            "description": desc,
            "docs_url": docs_url,
            "tags": s.get("tags") or [],
            # 0.8.6 — custom plugin detection. The `api-custom`
            # sentinel id is used ONLY in the create-payload (cf.
            # `backend/src/api/mcps.rs::CUSTOM_API_SERVER_ID`). The
            # materialized server id is `custom-{slug}-{nano}` so two
            # instances of e.g. "Salesforce" can coexist with distinct
            # creds (cf. `mcps.rs:82-86`). We must match BOTH prefixes
            # to be correct — and the `custom-` form is what 100% of
            # persisted custom plugins use.
            "is_custom": (
                (s.get("id") or "").startswith("custom-")
                or (s.get("id") or "") == "api-custom"
            ),
            "config_keys": config_keys,
            "endpoints": endpoints,
            "hint": hint,
        })
    return {"configs": out_configs, "servers_with_api": out_servers}


def call_workflow_create_draft(args):
    # 0.8.5 — POST /api/workflows with `enabled: false` (forced
    # client-side; the backend honours the flag since 0.8.5). The
    # agent provides everything else; we validate name + trigger +
    # steps presence to surface a clean error before the round-trip
    # if the LLM forgot a required field.
    for field in ("name", "trigger", "steps"):
        if not args.get(field):
            raise RuntimeError(f"workflow_create_draft: missing required '{field}'")
    if not isinstance(args["steps"], list) or len(args["steps"]) == 0:
        raise RuntimeError("workflow_create_draft: 'steps' must be a non-empty list")
    if len(args["steps"]) > 20:
        raise RuntimeError(
            f"workflow_create_draft: too many steps ({len(args['steps'])}, max 20)"
        )
    # Always force enabled=false on the draft path. Even if the agent
    # tries to override, the safety property of the tool stays
    # ("drafts never auto-fire").
    body = dict(args)
    body["enabled"] = False
    # 0.8.5 — auto-inherit project binding from the current discussion
    # when the agent doesn't pass one explicitly. Same UX rationale as
    # `disc_create` — an agent operating in a project's disc shouldn't
    # silently leak its artifacts into "Général".
    if "project_id" not in body or body.get("project_id") is None:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    return _unwrap(_http("POST", "/api/workflows", body))


def call_qp_create_draft(args):
    # 0.8.5 — POST /api/quick-prompts. QPs have no enabled flag (manual
    # launch only), so "draft" is semantic — the agent created it,
    # the user reviews + launches when they want.
    for field in ("name", "prompt_template", "agent"):
        if not args.get(field):
            raise RuntimeError(f"qp_create_draft: missing required '{field}'")
    # Defensive: cap obviously-bad name lengths early.
    if len(args["name"]) > 200:
        raise RuntimeError(
            f"qp_create_draft: 'name' too long ({len(args['name'])} chars, max 200)"
        )
    body = dict(args)
    # 0.8.5 — auto-inherit project binding from the current discussion
    # when the agent doesn't pass one explicitly.
    if "project_id" not in body or body.get("project_id") is None:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    return _unwrap(_http("POST", "/api/quick-prompts", body))


def call_qa_update(args):
    """0.8.6 phase 4 — partial-update wrapper around `PUT /api/quick-apis/<id>`.

    The bare PUT route resets `variables` / `profile_ids` / `directive_ids`
    to empty when those fields aren't in the body — defensive design on
    the backend side, but hostile UX for an MCP agent that just wants to
    tweak `api_extract`. We avoid the footgun by loading the existing
    QA first, applying the agent's patch on top of every field, and
    PUTting the full merged body back.

    Returns the updated QA JSON so the agent can confirm + chain straight
    into `qa_run` if needed.
    """
    qa_id = args.get("qa_id")
    if not qa_id:
        raise RuntimeError("qa_update: missing required 'qa_id'")

    # The list endpoint is the only GET route exposing the full QA shape
    # (no /api/quick-apis/<id> GET today). It returns every field so the
    # merge is lossless ; cost is the same as `qa_list` (~1 small query).
    existing_list = _unwrap(_http("GET", "/api/quick-apis")) or []
    existing = next((q for q in existing_list if q.get("id") == qa_id), None)
    if not existing:
        raise RuntimeError(
            f"qa_update: quick API {qa_id!r} not found — call qa_list to "
            "see what exists"
        )

    # Field-by-field merge : every field present in args overrides the
    # existing value (incl. an explicit `None` if the agent wants to
    # clear an optional field). Fields the agent didn't pass come from
    # the existing QA, preserved verbatim.
    patchable_fields = (
        "name", "icon", "description",
        "api_plugin_slug", "api_config_id", "api_endpoint_path",
        "api_method", "api_query", "api_path_params", "api_headers",
        "api_body", "api_extract", "api_pagination",
        "api_timeout_ms", "api_max_retries",
        "variables", "profile_ids", "directive_ids", "project_id",
    )
    body = {}
    for field in patchable_fields:
        if field in args:
            body[field] = args[field]
        elif field in existing:
            body[field] = existing[field]

    # Defensive : the merged body MUST have non-empty required fields,
    # else the backend update route falls back to existing — works fine
    # in practice but the explicit check surfaces inconsistencies early.
    for required in ("name", "api_plugin_slug", "api_config_id", "api_endpoint_path"):
        if not body.get(required):
            raise RuntimeError(
                f"qa_update: merged body has empty '{required}' — "
                "existing QA is corrupt OR you passed an empty string "
                "explicitly. Re-check qa_list output."
            )
    if len(body["name"]) > 200:
        raise RuntimeError(
            f"qa_update: 'name' too long ({len(body['name'])} chars, max 200)"
        )

    return _unwrap(_http("PUT", f"/api/quick-apis/{qa_id}", body))


def call_qa_create_draft(args):
    """0.8.6 phase 4 — POST /api/quick-apis.

    Closes the symmetry gap with workflow_create_draft + qp_create_draft.
    QAs have no `enabled` flag (manual launch only via `qa_run`), so the
    "draft" semantic mirrors qp_create_draft — the agent created it,
    the user reviews + launches when they want. No auto-fire surface.
    """
    for field in ("name", "api_plugin_slug", "api_config_id", "api_endpoint_path"):
        if not args.get(field):
            raise RuntimeError(f"qa_create_draft: missing required '{field}'")
    if len(args["name"]) > 200:
        raise RuntimeError(
            f"qa_create_draft: 'name' too long ({len(args['name'])} chars, max 200)"
        )
    body = dict(args)
    # Same auto-inheritance pattern as qp_create_draft : if the agent is
    # operating inside a project's disc, the QA defaults to that project.
    if "project_id" not in body or body.get("project_id") is None:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    return _unwrap(_http("POST", "/api/quick-apis", body))


def call_api_call(args):
    """0.8.6 — Agent API broker.

    Forward an agent-driven HTTP call to `POST /api/agent-api/call`.
    The backend resolves the plugin's encrypted credentials per the
    project scope, runs the call through the same executor as workflow
    ApiCall steps, and returns the canonical envelope.

    Project-scope resolution priority (handled server-side):
      1. `project_id` arg if explicitly passed by the agent
      2. `disc_id` (auto-injected from KRONN_DISCUSSION_ID when Kronn
         spawned the agent from a disc)
      3. The chosen `api_config_id`'s `project_ids[0]` — works for
         host-CLI sessions launched outside Kronn (no env var needed)

    Plugin selection — pass EITHER:
      (a) `api_plugin_slug` + `api_config_id` (literal config), OR
      (b) `quick_api_id` (saved Quick API reference; hydration happens
          server-side)

    The agent ABSOLUTELY shouldn't pass credentials of any form in this
    tool's args — auth comes from the encrypted env in Kronn DB,
    injected server-side per the plugin's ApiSpec.auth declaration.
    """
    if not args.get("endpoint_path"):
        raise RuntimeError("api_call: missing required 'endpoint_path'")

    has_plugin_pair = bool(args.get("api_plugin_slug")) and bool(args.get("api_config_id"))
    has_qa_ref = bool(args.get("quick_api_id"))
    if not has_plugin_pair and not has_qa_ref:
        raise RuntimeError(
            "api_call: provide either (api_plugin_slug + api_config_id) "
            "OR quick_api_id. Use `mcp_list` to discover available "
            "plugins and configs, or `qa_list` for saved Quick APIs."
        )

    body = {
        "endpoint_path": args["endpoint_path"],
    }

    # disc_id is BEST-EFFORT now (0.8.6). Pre-fix the tool refused
    # outright when KRONN_DISCUSSION_ID was missing → locked out every
    # host-CLI session launched outside Kronn. The backend now derives
    # project from disc OR config OR explicit arg, so we just forward
    # what we have.
    try:
        body["disc_id"] = _disc_id()
    except RuntimeError:
        pass  # Host-CLI context — project will be resolved from config_id.

    # Pass-through only the fields the route accepts — no leaking of
    # extra/unknown args (which serde may reject under
    # `deny_unknown_fields` if we add it later). `project_id` is new
    # in 0.8.6 — the agent can pass it explicitly when it knows the
    # scope (typically from `mcp_list.configs[].project_ids[0]`).
    for k in (
        "project_id",
        "api_plugin_slug",
        "api_config_id",
        "quick_api_id",
        "method",
        "path_params",
        "query",
        "headers",
        "body",
        "extract",
    ):
        v = args.get(k)
        if v is not None:
            body[k] = v

    return _unwrap(_http("POST", "/api/agent-api/call", body))


# ─── 0.8.6 phase 4 — MCP Remote Control (workflow_trigger / workflow_run_status / qp_run) ──

def call_workflow_trigger(args):
    """0.8.6 phase 4 — launch a workflow run via the JSON wrapper route.

    The backend `POST /api/mcp/workflow-trigger` :
      1. Validates the workflow + variables (same as UI trigger)
      2. Creates the run row + spawns the runner task in background
      3. Returns `{run_id, status, expected_duration_ms?, samples,
         next_check}` synchronously

    The agent should honour `next_check.wait_seconds` before calling
    `workflow_run_status({run_id})`. Without that, naïve polling burns
    ~13× more tokens for nothing.
    """
    workflow_id = args.get("workflow_id")
    if not workflow_id:
        raise RuntimeError("workflow_trigger: missing required 'workflow_id'")
    body = {"workflow_id": workflow_id}
    variables = args.get("variables")
    if isinstance(variables, dict):
        # Coerce all values to strings — the backend's TriggerWorkflowRequest
        # uses HashMap<String, String> ; non-string LLM outputs get
        # str()'d here so a {{count}}-typed-as-int doesn't 400.
        body["variables"] = {str(k): str(v) for k, v in variables.items()}
    return _unwrap(_http("POST", "/api/mcp/workflow-trigger", body))


def call_workflow_run_status(args):
    """0.8.6 phase 4 — read a workflow run's current state + next_check hint.

    Pure pass-through to `GET /api/mcp/workflow-run-status/<run_id>`.
    """
    run_id = args.get("run_id")
    if not run_id:
        raise RuntimeError("workflow_run_status: missing required 'run_id'")
    return _unwrap(_http("GET", f"/api/mcp/workflow-run-status/{run_id}"))


def call_qa_run(args):
    """0.8.6 phase 4 — execute a saved Quick API by id, synchronously.

    Thin pass-through to `POST /api/quick-apis/<qa_id>/run`. The backend
    hydrates the QA's endpoint/method/path_params/query/body, applies
    the user-supplied `vars`, runs the call through the same executor
    as workflow `ApiCall` steps, and returns the parsed envelope.

    No `next_check` — QAs are typically sub-second to a few seconds,
    the agent just awaits the response.

    Failure modes :
      - missing `qa_id` → RuntimeError before HTTP
      - required-variable missing → backend returns `success=false` with
        a clear French error like `Variable obligatoire manquante : foo`
      - HTTP failure / extract failure → `envelope=None` + `error="…"`
    """
    qa_id = args.get("qa_id")
    if not qa_id:
        raise RuntimeError("qa_run: missing required 'qa_id'")
    body = {}
    vars_obj = args.get("vars")
    if isinstance(vars_obj, dict):
        # Same coercion as workflow_trigger.variables / qp_run.vars —
        # the backend's RunQuickApiRequest uses HashMap<String, String>,
        # so int-typed LLM outputs need str() to avoid a 400.
        body["variables"] = {str(k): str(v) for k, v in vars_obj.items()}
    else:
        body["variables"] = {}
    return _unwrap(_http("POST", f"/api/quick-apis/{qa_id}/run", body))


def call_qp_run(args):
    """0.8.6 phase 4 — launch a Quick Prompt as a fresh disc.

    The backend `POST /api/mcp/qp-run` :
      1. Renders the QP template with the passed `vars`
      2. Creates a single-item batch (= 1 disc) via `create_batch_run`
      3. Spawns the agent in background (no SSE consumer needed)
      4. Returns `{disc_id, expected_duration_ms?, samples, next_check}`

    The agent reads the result via `disc_load_other(disc_id)` once
    `next_check.wait_seconds` elapsed.
    """
    qp_id = args.get("qp_id")
    if not qp_id:
        raise RuntimeError("qp_run: missing required 'qp_id'")
    body = {"qp_id": qp_id}
    vars_obj = args.get("vars")
    if isinstance(vars_obj, dict):
        # Same coercion as workflow_trigger — the backend expects strings.
        body["vars"] = {str(k): str(v) for k, v in vars_obj.items()}
    for k in ("agent", "project_id", "title"):
        v = args.get(k)
        if v is not None:
            body[k] = v
    # Auto-inherit current disc's project when the agent doesn't pass
    # one explicitly — same UX pattern as disc_create / workflow_create_draft.
    if "project_id" not in body:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    return _unwrap(_http("POST", "/api/mcp/qp-run", body))


DISPATCH = {
    "disc_meta": call_disc_meta,
    "disc_get_message": call_disc_get_message,
    "disc_summarize": call_disc_summarize,
    # 0.8.4 (#294) cross-agent memory
    "disc_create": call_disc_create,
    "disc_append": call_disc_append,
    "disc_link": call_disc_link,
    "disc_unlink": call_disc_unlink,
    "disc_find_by_session": call_disc_find_by_session,
    "disc_search": call_disc_search,
    "disc_load_other": call_disc_load_other,
    # 0.8.6 phase 2 — cross-agent collab via shared disc.
    "disc_join": call_disc_join,
    # 0.8.6 phase 3 — long-poll for peer messages.
    "disc_wait_for_peer": call_disc_wait_for_peer,
    # 0.8.6 phase 3 — leave the current disc + clear local binding.
    "disc_leave": call_disc_leave,
    # 0.8.6 (#56) — full-MCP cross-agent bootstrap (mint invite +
    # combined create-room helper). Closes the last UI-required gap
    # for an agent that wants to spin up a multi-agent room on its
    # own.
    "disc_invite_peer": call_disc_invite_peer,
    "disc_create_room": call_disc_create_room,
    # 0.8.5 — read-only listings of existing artifacts. Lets the
    # agent avoid duplicates + reference existing QP/QA ids from a
    # new workflow without asking the user to paste them.
    "workflow_list": call_workflow_list,
    "qp_list": call_qp_list,
    "qa_list": call_qa_list,
    "mcp_list": call_mcp_list,
    # 0.8.5 — autonomous draft creation. Both default to a safe state
    # (workflow disabled / QP manually launched) so a misfire can't
    # cascade into prod cron.
    "workflow_create_draft": call_workflow_create_draft,
    "qp_create_draft": call_qp_create_draft,
    # 0.8.6 phase 4 — symmetry fix : QA drafting was missing from the
    # *_create_draft cluster. QAs have no enabled flag — drafting = creation.
    "qa_create_draft": call_qa_create_draft,
    # 0.8.6 phase 4 — partial-update for QAs (load-merge-write).
    # Closes the post-test iteration loop : agent probes, persists,
    # tests, then patches `api_extract` / `api_query` without UI clicks.
    "qa_update": call_qa_update,
    # 0.8.6 — Agent API broker. Lets the agent invoke a configured
    # plugin without ever seeing the credentials (cf.
    # [[project_agent_api_broker_0_8_6]]).
    "api_call": call_api_call,
    # 0.8.6 phase 4 — MCP remote control. Launches + tracks workflows
    # and Quick Prompts from MCP, with smart-polling next_check hints
    # to cut mobile token cost ~80% (cf. [[project_mcp_remote_control_0_8_6]]).
    "workflow_trigger": call_workflow_trigger,
    "workflow_run_status": call_workflow_run_status,
    "qp_run": call_qp_run,
    # 0.8.6 phase 4 — synchronous QA execution. The deagentified twin
    # of `api_call` : same end-result, zero token cost on request
    # construction. Always prefer when a matching QA exists.
    "qa_run": call_qa_run,
}


# ─── MCP JSON-RPC loop ─────────────────────────────────────────────────────

def _send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def _handle(req):
    method = req.get("method") or ""
    rid = req.get("id")
    if method == "initialize":
        # 0.8.6 phase 2 — capture the client's identity. Used by
        # `_agent_type_for_session` so `disc_join` knows whether the
        # caller is Claude Code / Codex / Gemini / etc. without
        # requiring the user to pre-set `KRONN_AGENT_TYPE` env.
        params = req.get("params") or {}
        client_info = params.get("clientInfo") or {}
        if isinstance(client_info, dict):
            _CLIENT_INFO["name"] = client_info.get("name")
            _CLIENT_INFO["version"] = client_info.get("version")
        return {
            "jsonrpc": "2.0",
            "id": rid,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "kronn-internal", "version": "0.1.0"},
            },
        }
    if method == "notifications/initialized":
        # Notifications carry no id and expect no response.
        return None
    if method == "tools/list":
        return {"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}}
    if method == "tools/call":
        params = req.get("params") or {}
        name = params.get("name") or ""
        args = params.get("arguments") or {}
        fn = DISPATCH.get(name)
        if not fn:
            return {
                "jsonrpc": "2.0",
                "id": rid,
                "error": {"code": -32601, "message": f"Unknown tool: {name}"},
            }
        try:
            data = fn(args)
            return {
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "content": [{
                        "type": "text",
                        "text": json.dumps(data, ensure_ascii=False, indent=2),
                    }],
                },
            }
        except Exception as e:
            # Surface a structured error so the agent can either retry
            # with different args or fall back to asking the user.
            return {
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "isError": True,
                    "content": [{"type": "text", "text": f"kronn-internal error: {e}"}],
                },
            }
    # Unknown method
    if rid is not None:
        return {
            "jsonrpc": "2.0",
            "id": rid,
            "error": {"code": -32601, "message": f"Method not found: {method}"},
        }
    return None


def main():
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError:
            # Stdin garbage — log to stderr and keep the loop alive.
            print(f"kronn-internal: bad JSON-RPC line ignored: {line[:120]}", file=sys.stderr)
            continue
        resp = _handle(req)
        if resp is not None:
            _send(resp)


if __name__ == "__main__":
    main()
