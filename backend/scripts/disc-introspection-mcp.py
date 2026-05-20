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
            "description, project_id). Use this to reuse a matching QA "
            "via `quick_api_id` in a workflow `ApiCall` / `BatchApiCall` "
            "step instead of re-specifying the endpoint inline."
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
    data = _unwrap(_http("GET", "/api/quick-apis")) or []
    out = []
    for q in data:
        out.append({
            "id": q.get("id"),
            "name": q.get("name"),
            "api_plugin_slug": q.get("api_plugin_slug"),
            "api_endpoint_path": q.get("api_endpoint_path"),
            "api_method": q.get("api_method"),
            "description": q.get("description"),
            "project_id": q.get("project_id"),
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
    # 0.8.6 — Agent API broker. Lets the agent invoke a configured
    # plugin without ever seeing the credentials (cf.
    # [[project_agent_api_broker_0_8_6]]).
    "api_call": call_api_call,
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
