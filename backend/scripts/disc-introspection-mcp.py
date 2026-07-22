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

import contextlib
import hashlib
import json
import os
import secrets
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


# ─── Tool catalogue ────────────────────────────────────────────────────────

# Loaded-vs-on-disk staleness capture: the MCP client spawns this script at
# session start and never reloads it — a release can leave every live session
# running an outdated bridge with no visible signal (tools missing, stale
# descriptions). bridge_info compares these against the file's current mtime.
_BRIDGE_LOADED_AT = time.time()
try:
    _BRIDGE_SCRIPT_MTIME_AT_LOAD = os.path.getmtime(__file__)
except OSError:
    _BRIDGE_SCRIPT_MTIME_AT_LOAD = 0.0

TOOLS = [
    {
        "name": "kronn_intro",
        "description": (
            "0.8.13 — A 60-second guided tour of what the user can do with "
            "Kronn from this CLI (discussions, workflows, quick prompts, "
            "audits, API broker) with 3 starter examples. Call it when the "
            "session instructions flag a FIRST CONTACT (then present the "
            "guide conversationally, in the user's language), or anytime "
            "the user asks what Kronn can do. Calling it marks onboarding "
            "done for this client."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "bridge_info",
        "description": (
            "0.8.13 — Health/staleness check of THIS bridge process: script "
            "path, when it was loaded, and whether the on-disk script is "
            "NEWER than the loaded copy (`stale: true`). Call it when a tool "
            "you expect is missing or behaves oddly after a Kronn release. "
            "If stale, ask the user to reconnect the MCP — BEFORE launching "
            "anything session-bound (an audit dies with this session)."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
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
            "Return one message by either 0-indexed position (`idx`) or its "
            "copyable `MSG-xxxxxxxx` / full UUID (`message_id`). Negative idx "
            "counts from the end (-1 = last). Optional `before` / `after` "
            "return a bounded surrounding window (maximum 10 each). Use this "
            "when you need verbatim local context without loading or "
            "summarising the whole discussion. Cheap."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "idx": {
                    "type": "integer",
                    "description": "0-based index, or negative for from-end (-1=last)."
                },
                "message_id": {
                    "type": "string",
                    "description": "Copyable MSG-xxxxxxxx reference, raw ID prefix, or full message UUID."
                },
                "before": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 10,
                    "default": 0,
                    "description": "Number of preceding messages to return."
                },
                "after": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 10,
                    "default": 0,
                    "description": "Number of following messages to return."
                }
            },
            "oneOf": [
                {"required": ["idx"], "not": {"required": ["message_id"]}},
                {"required": ["message_id"], "not": {"required": ["idx"]}}
            ],
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
            "⚠ `content` is a MESSAGE for your peers — NEVER pass a tool "
            "name as content (e.g. posting the literal text "
            "'disc_wait_for_peer' instead of CALLING that tool: to wait, "
            "invoke disc_wait_for_peer; to speak, append prose).\n\n"
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
            "Returns `{appended, skipped_as_duplicates, diverged, "
            "last_sort_order}`. ALWAYS use `last_sort_order` as the "
            "`since_sort_order` of your next `disc_wait_for_peer` — "
            "NEVER estimate your position (+1 per post drifts under "
            "concurrent posters and silently skips messages). "
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
    {
        "name": "disc_list",
        "description": (
            "List available discussions (compact: disc_id, title, shared_id, "
            "message_count, updated_at), newest first. By default only SHARED "
            "(cross-instance / P2P) discussions — pass shared_only=false for all. "
            "Use this to BROWSE rooms (e.g. to pick one to disc_load_other or "
            "disc_join) without needing a search keyword. To read a listed disc's "
            "messages call disc_load_other({disc_id}); to reply once bound, "
            "disc_append."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "shared_only": {"type": "boolean", "description": "Only shared/P2P discs (default true)."},
                "limit": {"type": "integer", "description": "Max discs (1-100, default 30)."},
            },
            "required": [],
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
            "re-receiving the same messages. IMPORTANT: `timed_out=true` "
            "(no peer activity in the window) is NORMAL in an active "
            "collaboration — call this tool AGAIN to keep waiting. A timeout "
            "is NOT end-of-conversation; only stop/`disc_leave()` when the "
            "task is done or the user says stop."
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
                "disc_id": {"type": "string", "description": (
                    "Discussion id. The user can copy it from the UI: the "
                    "#-prefixed pill in the chat header (click = full UUID)."
                )},
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
        "name": "workflow_active_runs",
        "description": (
            "In-flight board: list every workflow run that is NOT finished "
            "right now (status Running / WaitingApproval / Pending), across "
            "ALL workflows — so you can see what else is happening before "
            "you act (avoid stepping on a run another agent started, or "
            "wait on a gate). Returns [{workflow_id, workflow_name, "
            "project_id, run_id, status, started_at}]. For the live step of "
            "a given run, drill down with `workflow_run_status(run_id)`. "
            "(Shows the latest run per workflow.)"
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "workflow_runs",
        "description": (
            "RUN HISTORY of one workflow (most recent first) — the past runs, "
            "not just active (`workflow_active_runs`) or the latest. Lean per-run "
            "summary: status · run_type · started/finished · tokens · batch "
            "counts · parent_run_id. Use it to debrief a cron/scheduled workflow "
            "(how many runs, which failed). To enumerate the foreach/batch "
            "CHILDREN of a parent run, call this on the CHILD workflow's id and "
            "filter by `parent_run_id == <parent run id>`. Drill into one run "
            "with `workflow_run_get`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string"},
                "limit": {"type": "integer", "description": "Optional: keep only the N most recent."},
            },
            "required": ["workflow_id"],
        },
    },
    {
        "name": "workflow_run_get",
        "description": (
            "Full detail of ONE run, incl per-step results (step_name · status · "
            "duration_ms · tokens · kind · agent · truncated output) — for "
            "debriefing a failed/finished run: which step failed and why. For an "
            "agent step's full produced content, read the run's discussions via "
            "`workflow_run_discussions`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string"},
                "run_id": {"type": "string"},
            },
            "required": ["workflow_id", "run_id"],
        },
    },
    {
        "name": "workflow_cancel_run",
        "description": (
            "Cancel a RUNNING run (MCP equivalent of the UI 'Arrêter'). "
            "DESTRUCTIVE — stops the run + its in-flight agents; completed "
            "steps/commits are kept. Use to stop a stuck or duplicate run (e.g. "
            "an overlapping cron tick). Confirm with the user before cancelling a "
            "run you didn't start."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string"},
                "run_id": {"type": "string"},
            },
            "required": ["workflow_id", "run_id"],
        },
    },
    {
        "name": "workflow_resume_run",
        "description": (
            "Resume an INTERRUPTED run (backend restart/crash killed it "
            "mid-flight). Continues from the step after the last completed "
            "one, re-attached to the preserved worktree; a foreach step "
            "re-runs only the items not yet done. Refused when the run is "
            "not Interrupted, is a sub-workflow child (resume the parent) "
            "or a batch, or when its worktree is gone."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "run_id": {"type": "string"},
            },
            "required": ["run_id"],
        },
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
            "**⚠ CALL THIS BEFORE every `workflow_create_draft`, "
            "`qp_create_draft`, `qa_create_draft`, `qa_update`, and "
            "`api_call` whose plugin/config you have NOT just listed "
            "this session.** Plugin slugs (`api_plugin_slug`), config "
            "ids (`api_config_id`), endpoint paths, and env keys are "
            "NOT memorizable across sessions — Kronn's allowlist "
            "refuses any value not declared, and a fabricated slug "
            "surfaces only at execution time (then fails opaquely). "
            "This tool is the only source of truth for those ids; "
            "guessing from a prior session's memory is the #1 failure "
            "mode for downstream MCP calls.\n\n"
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
    {
        "name": "convention_get",
        "description": (
            "Fetch the canonical Kronn documentation convention. The "
            "convention defines how to author `docs/AGENTS.md` (and other "
            "agent-context files) — the `<!-- kronn:section name/curated/"
            "audit -->` markers, the 9-type `[src: …]` provenance grammar "
            "(file / url / user / commit / api / code-comment / inferred / "
            "hypothesis / training-data), and the `curated=\"ai\"` vs "
            "`curated=\"human\"` ownership rules.\n\n"
            "**Call this BEFORE writing to a `curated=\"ai\"` section of "
            "any `docs/AGENTS.md`** — the embedded spec is the source of "
            "truth (the GitHub `main` copy may have moved on; this tool "
            "returns the convention THIS Kronn installation actually "
            "implements + lints against).\n\n"
            "Returns the markdown spec verbatim. `name` defaults to "
            "`agents-md-format`, `version` to `v1` (only shipped today). "
            "Future conventions will use the same tool with different "
            "names."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Convention name (default: 'agents-md-format').",
                },
                "version": {
                    "type": "string",
                    "description": "Convention version (default: 'v1').",
                },
            },
        },
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
            "**⚠ Discovery first — DO NOT INVENT.** A fabricated step "
            "fails at run time, not at draft time. Verify every reference "
            "AT CALL TIME: each `step_type` is in the closed 9-set below; "
            "each ApiCall's `api_plugin_slug`+`api_config_id` exists in "
            "`mcp_list`; each Agent's `skill_ids`/`profile_ids`/"
            "`directive_ids` exists (enumerate them with `skills_list` / "
            "`profiles_list` / `directives_list`). If you can't enumerate a "
            "binding, ASK — never guess (hallucinated ids pass schema "
            "validation and fail opaquely at the first run).\n\n"
            "Payload mirrors `CreateWorkflowRequest`: name (required), "
            "trigger (required, e.g. `{ \"type\": \"Manual\" }`), steps "
            "(required, ≥ 1 ≤ 20 items). Optional: project_id, "
            "actions, safety, workspace_config, concurrency_limit, "
            "guards, artifacts, on_failure, exec_allowlist, variables.\n\n"
            "**WorkflowStep shape:** the type-specific fields sit at the TOP "
            "LEVEL (never nested under a sub-object), BUT `step_type` itself is a "
            "TAGGED OBJECT `{\"type\": \"Agent\"}` — NOT a bare string (serde "
            "`#[serde(tag=\"type\")]`). Same for `output_format` "
            "(`{\"type\":\"Structured\"}`) and the workflow `trigger` "
            "(`{\"type\":\"Manual\"}`). (This tool is forgiving — it also accepts a "
            "bare-string `step_type` and wraps it — but the canonical form, the "
            "one `workflow_get` returns, is the tagged object.) `step_type.type` is "
            "a CLOSED set — one of: **Agent · ApiCall · BatchApiCall · "
            "BatchQuickPrompt · Exec · Gate · Notify · JsonData · SubWorkflow**. "
            "(Don't infer the available types from one workflow you happened to "
            "open — it may use only Agent steps. This list is the whole taxonomy.)\n"
            "Common shapes (copy then adapt):\n"
            "  • Agent: `{\"name\":\"Triage\",\"step_type\":{\"type\":\"Agent\"},\"agent\":\"ClaudeCode\",\"prompt_template\":\"Analyse {{previous_step.output}}\",\"output_format\":{\"type\":\"Structured\"}}`\n"
            "  • ApiCall: `{\"name\":\"Fetch\",\"step_type\":{\"type\":\"ApiCall\"},\"api_plugin_slug\":\"mcp-atlassian\",\"api_config_id\":\"<id from mcp_list>\",\"api_endpoint_path\":\"/rest/api/2/search\",\"api_method\":\"GET\",\"api_query\":{\"jql\":\"...\"}}`\n"
            "  • Exec: `{\"name\":\"Tests\",\"step_type\":{\"type\":\"Exec\"},\"exec_command\":\"make\",\"exec_args\":[\"test\"],\"exec_timeout_secs\":600}` (binary must be in the workflow `exec_allowlist`; for a LARGE input use `\"exec_stdin\":\"{{steps.fetch.data_json}}\"` — piped to stdin, no argv size limit — instead of a huge arg)\n"
            "  • Gate: `{\"name\":\"Validate\",\"step_type\":{\"type\":\"Gate\"},\"gate_message\":\"Approve?\",\"gate_request_changes_target\":\"<step name to loop back to>\"}`\n"
            "  • Notify: `{\"name\":\"Done\",\"step_type\":{\"type\":\"Notify\"},\"notify_config\":{...}}`\n"
            "  • BatchQuickPrompt: `{\"name\":\"Fan out\",\"step_type\":{\"type\":\"BatchQuickPrompt\"},\"batch_quick_prompt_id\":\"<qp id>\",\"batch_items_from\":\"{{previous_step.data}}\",\"batch_wait_for_completion\":true}`\n"
            "  • BatchApiCall: `{\"name\":\"Bulk\",\"step_type\":{\"type\":\"BatchApiCall\"},\"batch_items_from\":\"{{previous_step.data}}\",\"api_plugin_slug\":\"…\",\"api_config_id\":\"…\",\"api_endpoint_path\":\"…\",\"api_method\":\"POST\"}` (fan one ApiCall over a list, zero tokens)\n"
            "  • JsonData: `{\"name\":\"Seed\",\"step_type\":{\"type\":\"JsonData\"},\"json_data_payload\":\"[{...}]\"}` (deterministic data source, zero tokens — feeds `{{steps.Seed.data}}`)\n"
            "  • SubWorkflow: `{\"name\":\"Implement\",\"step_type\":{\"type\":\"SubWorkflow\"},\"sub_workflow_id\":\"<child id>\"}` — runs ANOTHER workflow as a step. Add `\"sub_workflow_foreach_file\":\".kronn/tasks.json\"` to run the child ONCE PER ITEM. **FOREACH RUNTIME CONTRACT (run-breaking): the engine exposes each item to the child as template vars `{{current_task.<field>}}` (e.g. path `/pulls/{{current_task.number}}/reviews`) AND as the file `.kronn/current_task.json`. Accessor is FIXED `current_task.*` — NOT `{{item.*}}`, NOT derived from the foreach file name.** Child must exist first. Call `workflow_step_schema` for the full contract.\n"
            "Advanced (Agent step): `output_format` = `{\"type\":\"TypedSchema\",\"schema\":{…}}` (validates+repairs the agent's JSON) or `{\"type\":\"Structured\"}`; `multi_agent_review:true` = second agent debates the output.\n"
            "Better than hand-authoring: `workflow_get`/`workflow_clone` a RICH workflow (e.g. AutoPilot) and adapt. Returns the created workflow JSON (id + all fields)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Workflow name (1-200 chars)."},
                "trigger": {
                    "type": "object",
                    "description": "Workflow trigger spec (Manual / Cron / Tracker). E.g. `{ \"type\": \"Manual\" }` or `{ \"type\": \"Cron\", \"schedule\": \"0 9 * * 1-5\" }`. For Cron/Tracker, `concurrency_limit` auto-defaults to 1 (no self-overlap) unless you set it — see that field.",
                },
                "steps": {
                    "type": "array",
                    "description": (
                        "Workflow steps (1-20). Type-specific fields at top level, but "
                        "`step_type` is a TAGGED OBJECT `{\"type\":\"Agent\"}` (not a bare "
                        "string; bare string is tolerated + wrapped). `step_type.type` ∈ "
                        "closed 9-set: Agent · ApiCall · BatchApiCall · BatchQuickPrompt · "
                        "Exec · Gate · Notify · JsonData · SubWorkflow. SubWorkflow foreach runtime contract: "
                        "`sub_workflow_foreach_file` is your SOURCE list; the engine exposes "
                        "each item to the child as `{{current_task.<field>}}` template vars "
                        "(e.g. `{{current_task.number}}`) AND as the FIXED file "
                        "`.kronn/current_task.json`. Accessor is `current_task.*` (fixed — not "
                        "`{{item.*}}`, not the source-file name). Call `workflow_step_schema` "
                        "for the full per-type schema + examples."
                    ),
                },
                "project_id": {"type": "string", "description": "Optional Kronn project id to bind the workflow to."},
                "variables": {"type": "array", "description": "Optional manual-launch variables (each `{ name, label?, placeholder?, required?, description? }`)."},
                "guards": {"type": "object", "description": "Optional execution guards (timeout, max_llm_calls, loop_revisits)."},
                "on_failure": {"type": "array", "description": "Optional rollback step chain (Notify / Agent / ApiCall steps)."},
                "exec_allowlist": {"type": "array", "items": {"type": "string"}, "description": "Whitelisted binaries for any Exec steps."},
                "artifacts": {"type": "object", "description": "Optional artifact declarations (name → spec)."},
                "concurrency_limit": {"type": "integer", "description": "Max concurrent runs of THIS workflow. When active runs ≥ limit, the scheduler SKIPS the tick (not queued). For a Cron/Tracker trigger, leave unset → it defaults to 1 (no self-overlap); set explicitly higher only if you truly want overlapping runs. Overlap on a state-mutating cron = double work + duplicate side-effects."},
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
            "**⚠ Discovery first — DO NOT INVENT bindings.** "
            "`skill_ids`, `profile_ids`, `directive_ids` and `agent` "
            "must reference REAL Kronn ids. If you can't enumerate "
            "them via `qp_list` (which echoes the user's existing "
            "bindings catalog) or via the dedicated skills/profiles/"
            "directives list endpoints, ASK the user — never guess a "
            "UUID. A QP drafted with a fabricated `skill_id` silently "
            "strips that binding at run time: the QP runs without "
            "the skill, the user only notices via missing behaviour, "
            "and the debug session blames the wrong layer.\n\n"
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
        "name": "workflow_get",
        "description": (
            "Fetch a workflow's FULL definition (every step + all fields) "
            "by id. Unlike `workflow_list` (compact summary, no steps), "
            "this returns the exact shape `workflow_create_draft` / "
            "`workflow_update` accept — so READ a real workflow here "
            "before cloning or patching, instead of guessing the step "
            "schema and discovering required fields one 422 at a time."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string", "description": "Workflow id (from `workflow_list`, or the user can copy it from the #-prefixed pill in the workflow detail header)."},
            },
            "required": ["workflow_id"],
        },
    },
    {
        "name": "workflow_clone",
        "description": (
            "Duplicate an existing workflow. Mints fresh ids, re-bundles "
            "and rewrites referenced Quick Prompt ids, strips per-user "
            "notify URLs. The clone lands DISABLED with a distinct name "
            "(default `<name> (copie)`) so it never auto-fires and you "
            "never get two identically-named workflows. Typical loop: "
            "`workflow_clone` → `workflow_update` (patch a few fields) → "
            "`workflow_set_enabled` (test). Cheaper + safer than "
            "re-authoring from scratch."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string", "description": "Source workflow id to clone."},
                "new_name": {"type": "string", "description": "Optional name for the clone (default `<source name> (copie)`)."},
                "project_id": {"type": "string", "description": "Optional project to bind the clone to (default: current discussion's project)."},
            },
            "required": ["workflow_id"],
        },
    },
    {
        "name": "workflow_update",
        "description": (
            "Patch an existing workflow IN PLACE. TRUE patch semantics: "
            "any field you omit keeps its current value; send a field to "
            "replace it. Same field shapes as `workflow_create_draft` "
            "(name, trigger, steps, variables, guards, on_failure, "
            "exec_allowlist, artifacts, …) plus `enabled`. NOTE: `steps` "
            "is replaced WHOLESALE, not merged — to edit one step, fetch "
            "the full `steps` via `workflow_get`, change what you need, "
            "and send the whole array back."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string", "description": "Workflow id to patch."},
                "name": {"type": "string"},
                "trigger": {"type": "object", "description": "e.g. `{ \"type\": \"Manual\" }`."},
                "steps": {"type": "array", "description": "Full replacement steps array (1-20). Fetch + edit via `workflow_get` first."},
                "variables": {"type": "array", "description": "Launch-time variables. `label`/`placeholder` auto-default to the name/empty if omitted."},
                "guards": {"type": "object"},
                "on_failure": {"type": "array"},
                "exec_allowlist": {"type": "array", "items": {"type": "string"}},
                "artifacts": {"type": "object"},
                "enabled": {"type": "boolean", "description": "Toggle enabled. For Cron/Tracker triggers prefer `workflow_set_enabled` (it gates auto-firing)."},
                "project_id": {"type": "string"},
                "concurrency_limit": {"type": "integer"},
                "safety": {"type": "object"},
                "workspace_config": {"type": "object"},
                "actions": {"type": "array"},
            },
            "required": ["workflow_id"],
        },
    },
    {
        "name": "workflow_set_enabled",
        "description": (
            "Enable or disable a workflow. Disabling is always allowed. "
            "Enabling a MANUAL workflow is free (it only runs when "
            "explicitly triggered). Enabling a CRON/TRACKER workflow is "
            "REFUSED unless you pass `force: true` — that would schedule "
            "autonomous runs with no human in the loop; prefer letting the "
            "user enable scheduled workflows from the Kronn UI."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workflow_id": {"type": "string", "description": "Workflow id."},
                "enabled": {"type": "boolean", "description": "true to enable, false to disable."},
                "force": {"type": "boolean", "description": "Pass true to enable a Cron/Tracker-triggered workflow (otherwise refused)."},
            },
            "required": ["workflow_id", "enabled"],
        },
    },
    {
        "name": "qp_update",
        "description": (
            "Patch an existing Quick Prompt IN PLACE (by id). Loads the "
            "current QP, applies your patch field-by-field, saves the "
            "merged result — tweak just `prompt_template` or `agent` "
            "without resetting the rest. Use this to iterate a QP "
            "(e.g. v2 → v2.1) instead of creating an orphan copy. "
            "Same field shapes as `qp_create_draft`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qp_id": {"type": "string", "description": "Quick Prompt id (from `qp_list`)."},
                "name": {"type": "string"},
                "prompt_template": {"type": "string"},
                "agent": {"type": "string"},
                "variables": {"type": "array", "description": "`label`/`placeholder` auto-default to the name/empty if omitted."},
                "description": {"type": "string"},
                "icon": {"type": "string"},
                "tier": {"type": "string"},
                "project_id": {"type": "string"},
                "skill_ids": {"type": "array", "items": {"type": "string"}},
                "profile_ids": {"type": "array", "items": {"type": "string"}},
                "directive_ids": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["qp_id"],
        },
    },
    {
        "name": "qp_get",
        "description": (
            "Fetch a Quick Prompt's FULL definition by id — including the "
            "`prompt_template` BODY that `qp_list` omits, plus all bindings "
            "(variables, skill/profile/directive ids, agent, tier). Use it to "
            "understand what a QP actually does so you can RUN it yourself "
            "(render the template with the variables, then act), or to read a "
            "QP before editing it with `qp_update`. `qp_list` only tells you a "
            "QP exists; `qp_get` tells you what it does."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qp_id": {"type": "string", "description": "Quick Prompt id (from `qp_list`)."},
            },
            "required": ["qp_id"],
        },
    },
    {
        "name": "qp_delete",
        "description": (
            "Delete a Quick Prompt by id. Use to clean up an orphan draft "
            "(e.g. after replacing a QP rather than patching it via "
            "`qp_update`)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qp_id": {"type": "string", "description": "Quick Prompt id to delete."},
            },
            "required": ["qp_id"],
        },
    },
    {
        "name": "skills_list",
        "description": (
            "List Kronn SKILLS (builtin + custom) — id · name · description · "
            "category. These are the valid values for an Agent step's "
            "`skill_ids` (and a QP's). Drops the markdown body for brevity. Call "
            "this to PICK skill ids when authoring/editing a workflow or QP "
            "instead of guessing slugs or asking the user to paste them."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "profiles_list",
        "description": (
            "List Kronn PROFILES / personas (builtin + custom) — id · name · "
            "role · persona_name · default_engine. Valid values for an Agent "
            "step's `profile_ids` (and a QP's). Drops the persona prompt body; "
            "list to PICK ids."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "directives_list",
        "description": (
            "List Kronn DIRECTIVES (builtin + custom) — id · name · description "
            "· conflicts. Valid values for an Agent step's `directive_ids` (and "
            "a QP's). Keeps `conflicts` so you don't pick mutually-exclusive "
            "directives; list to PICK ids."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "skill_get",
        "description": (
            "Fetch one skill's FULL definition by id, including its markdown "
            "`content`, license and allowed-tools fields omitted by "
            "`skills_list`. Read before editing or applying a skill."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "skill_id": {"type": "string", "description": "Skill id from `skills_list`."},
            },
            "required": ["skill_id"],
        },
    },
    {
        "name": "profile_get",
        "description": (
            "Fetch one profile's FULL definition by id, including the "
            "`persona_prompt` omitted by `profiles_list`. Read before editing "
            "or using a persona."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "profile_id": {"type": "string", "description": "Profile id from `profiles_list`."},
            },
            "required": ["profile_id"],
        },
    },
    {
        "name": "directive_get",
        "description": (
            "Fetch one directive's FULL definition by id, including its "
            "`content` omitted by `directives_list`. Read before editing or "
            "applying a directive."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "directive_id": {"type": "string", "description": "Directive id from `directives_list`."},
            },
            "required": ["directive_id"],
        },
    },
    {
        "name": "skill_create",
        "description": (
            "Create a CUSTOM skill in the user's library. Required: `name`, "
            "`description`, `icon`, `category` (one of Language/Domain/Business), "
            "`content` (the markdown skill body). Optional: `license`, "
            "`allowed_tools`. The new skill is immediately bindable via an Agent "
            "step's / QP's `skill_ids`. Returns the created skill incl its id."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "icon": {"type": "string", "description": "Emoji or short icon token."},
                "category": {"type": "string", "enum": ["Language", "Domain", "Business"]},
                "content": {"type": "string", "description": "Markdown body of the skill."},
                "license": {"type": "string"},
                "allowed_tools": {"type": "string"},
            },
            "required": ["name", "description", "icon", "category", "content"],
        },
    },
    {
        "name": "skill_update",
        "description": (
            "Patch a CUSTOM skill (load-merge-write; only fields you pass change). "
            "Builtin skills are rejected. ⚠ The backend recreates the skill so its "
            "id CHANGES — use the id in the returned object afterwards."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "skill_id": {"type": "string", "description": "Id of the custom skill to patch."},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "icon": {"type": "string"},
                "category": {"type": "string", "enum": ["Language", "Domain", "Business"]},
                "content": {"type": "string"},
                "license": {"type": "string"},
                "allowed_tools": {"type": "string"},
            },
            "required": ["skill_id"],
        },
    },
    {
        "name": "skill_delete",
        "description": "Delete a custom skill by id (builtins are protected).",
        "inputSchema": {
            "type": "object",
            "properties": {"skill_id": {"type": "string"}},
            "required": ["skill_id"],
        },
    },
    {
        "name": "profile_create",
        "description": (
            "Create a CUSTOM profile/persona. Required: `name`, `role`, `avatar`, "
            "`color`, `category` (Technical/Business/Meta), `persona_prompt`. "
            "Optional: `persona_name`, `default_engine`. Bindable via an Agent "
            "step's `profile_ids`. Returns the created profile incl id."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "role": {"type": "string"},
                "avatar": {"type": "string", "description": "Emoji/avatar token."},
                "color": {"type": "string", "description": "Hex or token, e.g. #6C5CE7."},
                "category": {"type": "string", "enum": ["Technical", "Business", "Meta"]},
                "persona_prompt": {"type": "string"},
                "persona_name": {"type": "string"},
                "default_engine": {"type": "string"},
            },
            "required": ["name", "role", "avatar", "color", "category", "persona_prompt"],
        },
    },
    {
        "name": "profile_update",
        "description": "Patch a custom profile (load-merge-write; builtins rejected).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "profile_id": {"type": "string"},
                "name": {"type": "string"},
                "role": {"type": "string"},
                "avatar": {"type": "string"},
                "color": {"type": "string"},
                "category": {"type": "string", "enum": ["Technical", "Business", "Meta"]},
                "persona_prompt": {"type": "string"},
                "persona_name": {"type": "string"},
                "default_engine": {"type": "string"},
            },
            "required": ["profile_id"],
        },
    },
    {
        "name": "profile_delete",
        "description": "Delete a custom profile by id (builtins are protected).",
        "inputSchema": {
            "type": "object",
            "properties": {"profile_id": {"type": "string"}},
            "required": ["profile_id"],
        },
    },
    {
        "name": "directive_create",
        "description": (
            "Create a CUSTOM directive. Required: `name`, `description`, `icon`, "
            "`category` (Output/Language), `content`. Optional: `conflicts` (list "
            "of directive ids it's mutually exclusive with). Bindable via an Agent "
            "step's `directive_ids`. Returns the created directive incl id."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "icon": {"type": "string"},
                "category": {"type": "string", "enum": ["Output", "Language"]},
                "content": {"type": "string"},
                "conflicts": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["name", "description", "icon", "category", "content"],
        },
    },
    {
        "name": "directive_update",
        "description": "Patch a custom directive (load-merge-write; builtins rejected).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "directive_id": {"type": "string"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "icon": {"type": "string"},
                "category": {"type": "string", "enum": ["Output", "Language"]},
                "content": {"type": "string"},
                "conflicts": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["directive_id"],
        },
    },
    {
        "name": "directive_delete",
        "description": "Delete a custom directive by id (builtins are protected).",
        "inputSchema": {
            "type": "object",
            "properties": {"directive_id": {"type": "string"}},
            "required": ["directive_id"],
        },
    },
    {
        "name": "workflow_step_schema",
        "description": (
            "Return the CANONICAL WorkflowStep schema as a tool RESULT (never "
            "truncated, unlike a tool description): the closed 9-set of "
            "`step_type`s, the flat shape, the required + optional fields PER "
            "type, and the RUNTIME CONTRACTS that break a workflow at run time "
            "if missed (e.g. SubWorkflow foreach → the engine writes each item "
            "to the fixed path `.kronn/current_task.json`). Zero args. Call this "
            "BEFORE authoring or editing a workflow instead of inferring the "
            "schema from one `workflow_get` sample or from the (possibly "
            "client-truncated) `workflow_create_draft` description."
        ),
        "inputSchema": {"type": "object", "properties": {}},
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
                "api_pagination": {"type": "object", "description": "Optional pagination spec — internally tagged `{\"type\": ...}`: Auto | Offset | Cursor | Page | LinkHeader (GitHub-style bare array + `Link: rel=next` header; fields page_size_param/page_size/max_pages)."},
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
            "**Reuse first (cheapest)**: before hand-building a call, run "
            "`qa_list` — if a saved Quick API matches the action, run it via "
            "`qa_run` (zero token cost on request construction; same shape "
            "across agents). Only fall through to a fresh `api_call` when no "
            "QA fits.\n\n"
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
            "back to the user.\n\n"
            "**Persist after (close the loop)**: if you just hand-built a "
            "call the user will likely run again, PROPOSE saving it as a "
            "Quick API via `qa_create_draft` (PROBE-then-PERSIST) — next "
            "time it's a `qa_run` at zero construction cost, deagentified, "
            "and identical across agents. Don't silently rebuild the same "
            "payload every session."
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
            "not listed here — call `workflow_run_discussions({run_id})` "
            "to get the child `disc_id`s, then `disc_load_other` each. "
            "For linear workflows the `steps[]` array is enough.\n\n"
            "**Prefer `workflow_wait_for_completion`** for short runs when "
            "you just want the final verdict in a single call."
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
        "name": "qp_batch_run",
        "description": (
            "Fan a Quick Prompt out to N discussions in ONE call — the "
            "batch twin of `qp_run`. Returns `{run_id, qp_id, qp_name, "
            "disc_ids[], batch_total, expected_duration_ms?, samples, "
            "next_check}`.\n\n"
            "**Items** : pass `items: [{title?, vars?}, ...]` — one entry "
            "per child disc. Each item's `vars` render the QP `{{var}}` "
            "placeholders independently, so you can run the same prompt "
            "over a list (10 tickets, 5 hosts, 3 regions…). `title` is "
            "optional (defaults to `<qp_name> #<n>`). Required QP vars must "
            "be non-empty on EVERY item. Max 50 items.\n\n"
            "**Discovery first** : `qp_list` for the `qp_id` + its required "
            "vars.\n\n"
            "**Track progress** : all children link under one batch "
            "`run_id`. Poll it with `workflow_run_status({run_id})` "
            "(batch_completed / batch_total) or list the children with "
            "`workflow_run_discussions({run_id})`, then `disc_load_other` "
            "the ones you care about. `next_check` is a per-item baseline "
            "(single-launch avg) — the batch finishes when all items do, "
            "so treat it as a floor.\n\n"
            "**vs `qp_run`** : `qp_run` = 1 disc ; `qp_batch_run` = N discs "
            "under one trackable batch."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qp_id": {"type": "string", "description": "Quick Prompt id (from `qp_list`)."},
                "items": {
                    "type": "array",
                    "description": "One entry per child disc: `{title?: string, vars?: {name: value}}`. Max 50.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string", "description": "Optional disc title."},
                            "vars": {"type": "object", "description": "Per-item `{{var}}` values (string map)."},
                        },
                    },
                },
                "project_id": {
                    "type": "string",
                    "description": "Optional project override. Defaults to the QP's project, else the current disc's project.",
                },
                "batch_name": {
                    "type": "string",
                    "description": "Optional sidebar group name. Defaults to `MCP batch · <qp_name> · <time>`.",
                },
            },
            "required": ["qp_id", "items"],
        },
    },
    {
        "name": "workflow_run_discussions",
        "description": (
            "List the discussions a run spawned (batch children, or a "
            "workflow's `BatchQuickPrompt` fan-out). Returns `{run_id, "
            "disc_count, discussions: [{disc_id, title, agent, "
            "message_count, archived, created_at}]}`.\n\n"
            "Empty list for a pure linear workflow (those have no child "
            "discs — read `workflow_run_status({run_id}).steps[]` "
            "instead). After getting the list, `disc_load_other(disc_id)` "
            "to read any child's full conversation.\n\n"
            "Pairs with `qp_batch_run` / `workflow_trigger` : trigger → "
            "wait/poll → `workflow_run_discussions` → read children."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "run_id": {"type": "string", "description": "Batch/workflow run id."},
            },
            "required": ["run_id"],
        },
    },
    {
        "name": "workflow_wait_for_completion",
        "description": (
            "Block (long-poll) until a run reaches a terminal status or "
            "`timeout_s` elapses — saves the back-and-forth of repeated "
            "`workflow_run_status` calls on short runs. Returns `{run_id, "
            "workflow_id, status, finished_at?, elapsed_ms, tokens_used, "
            "timed_out, next_check?}`.\n\n"
            "**timeout_s** : how long to hold the connection (default 60, "
            "clamped to [1, 60]). If the run finishes first you get the "
            "terminal status immediately with `timed_out: false` and "
            "`next_check: null`. If the timeout wins, `timed_out: true` + a "
            "`next_check` hint tells you when to call again.\n\n"
            "**When to use** : short/medium runs where you want the verdict "
            "in one call. For long runs (multi-minute), prefer "
            "`workflow_run_status` + honour `next_check` so you don't hold "
            "a connection open. Terminal statuses : `Success | Failed | "
            "Cancelled | StoppedByGuard` (and the run pauses on "
            "`WaitingApproval` — that's NOT terminal, so a Gate'd workflow "
            "will time out here, by design)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "run_id": {"type": "string", "description": "Run id to wait on."},
                "timeout_s": {
                    "type": "integer",
                    "description": "Max seconds to wait (default 60, clamped [1, 60]).",
                },
            },
            "required": ["run_id"],
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
    {
        "name": "learning_propose",
        "description": (
            "0.9.0 — Propose a DURABLE learning Kronn should remember across "
            "discussions (a project convention, a user preference, a verified "
            "fact, a pitfall). Use when something emerges that future sessions "
            "would otherwise re-learn.\n\n"
            "Typed + evidence-mandatory by design: every learning MUST cite at "
            "least one `evidence` source — there is no free-form path. The "
            "server verifies the evidence resolves (Gate-1) and — when a "
            "faithfulness backend is enabled (off by default) — scores whether "
            "the claim follows from it (Gate-2), then a HUMAN validates before "
            "anything is written to a truth file. So propose freely: a weak or "
            "wrong candidate is caught downstream, never silently persisted.\n\n"
            "`kind`: `fact` (mechanically verifiable — cite file:line or url), "
            "`preference` (the user stated it — cite a user confirmation), "
            "`inference` (you derived it — needs stronger validation). "
            "Avoid absolutes (always/never) without a scope. "
            "disc/project/agent are auto-inherited from the current discussion."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "claim": {
                    "type": "string",
                    "description": "The learning, one clear sentence. Scope it (e.g. 'In this repo, ...').",
                },
                "kind": {
                    "type": "string",
                    "enum": ["fact", "preference", "inference"],
                    "description": "fact | preference | inference.",
                },
                "evidence": {
                    "type": "array",
                    "minItems": 1,
                    "description": "≥1 source backing the claim. MANDATORY — no evidence = refused.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": ["file", "url", "disc", "cmd", "user"],
                                "description": "Source type.",
                            },
                            "ref": {
                                "type": "string",
                                "description": "Resolvable ref: 'path/file.ext:line', a URL, a disc id, a command, or 'user:YYYY-MM-DD'.",
                            },
                            "quote": {
                                "type": "string",
                                "description": "Short supporting excerpt (the premise the faithfulness check reads). Recommended.",
                            },
                        },
                        "required": ["kind", "ref"],
                    },
                },
                "confidence": {
                    "type": "number",
                    "description": "Optional self-confidence 0.0–1.0 (a haircut is applied server-side).",
                },
                "project_id": {"type": "string", "description": "Optional — auto-inherited from the current disc."},
                "discussion_id": {"type": "string", "description": "Optional — auto-inherited."},
                "source_agent": {"type": "string", "description": "Optional — auto-inherited (e.g. 'ClaudeCode')."},
            },
            "required": ["claim", "kind", "evidence"],
        },
    },
    {
        "name": "audit_prepare",
        "description": (
            "0.8.12 — Read a project's audit surface BEFORE launching: the "
            "docs/ files with their filled/unfilled status, the open TODOs "
            "and the tech-debt items. Returns the backend's AuditInfo "
            "verbatim (`files`, `todos`, `tech_debt_items`) plus the "
            "project's `audit_status`. Empty arrays do NOT mean 'clean': "
            "when `audit_status` is `NoTemplate` there is simply nothing "
            "to audit yet — call `audit_install_template` first. Use it to "
            "brief yourself, pick between full/partial, and know what to "
            "validate once the audit completes."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {"type": "string", "description": "Kronn project id."},
            },
            "required": ["project_id"],
        },
    },
    {
        "name": "audit_install_template",
        "description": (
            "0.8.13 — Step 0 of the audit pipeline (template → audit → "
            "validation): install the docs/ template into a `NoTemplate` "
            "project so `audit_launch` has a surface to fill. Idempotent "
            "and non-destructive (never overwrites existing docs). Returns "
            "the project's new `audit_status`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {"type": "string", "description": "Kronn project id."},
            },
            "required": ["project_id"],
        },
    },
    {
        "name": "audit_launch",
        "description": (
            "0.8.12 — Launch a project audit (mode `full` or `partial`) and "
            "return IMMEDIATELY with a correlation (project_id, mode, "
            "started_at). The audit is driven by an SSE stream this bridge "
            "keeps reading in a background thread.\n\n"
            "⚠️ LIFECYCLE — NOT a detached execution: the audit lives only "
            "as long as THIS MCP session lives. Reloading the MCP or "
            "closing the CLI interrupts the audit mid-flight. Never assume "
            "it survived a reload — call `audit_status` to observe the "
            "truth, and relaunch consciously (an Interrupted FULL/specialized "
            "run is resumable via resume_run_id; an interrupted PARTIAL is "
            "not — relaunch it on its still-stale scope). One audit per "
            "project at a time: launching while "
            "one runs is an ERROR, not a silent no-op.\n\n"
            "`full` creates a validation discussion at the end, and so does a "
            "FULLY-successful `partial` (scoped to the refreshed sections) "
            "— discussion_id lands in audit_status once done, null when the "
            "run was interrupted. `partial` requires `steps` (1-based "
            "indices of the analysis steps to re-run).\n\n"
            "BRIEFING: the audit quality depends on user context (goals, "
            "known pain points). `audit_prepare` reports whether a briefing "
            "exists (`briefing.present`); when it doesn't, consider running "
            "the project briefing in the UI first — the launch response "
            "carries the same warning."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {"type": "string", "description": "Kronn project id."},
                "mode": {
                    "type": "string",
                    "enum": ["full", "partial"],
                    "description": "full = whole pipeline + validation discussion; partial = selected steps only (a fully-successful partial ALSO creates a validation discussion scoped to the refreshed sections and gets its own audit_runs row).",
                },
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "items": {"type": "integer", "minimum": 1},
                    "description": "REQUIRED for partial: 1-based step indices to re-run. Ignored for full.",
                },
                "resume_run_id": {
                    "type": "string",
                    "description": (
                        "full mode only: resume an Interrupted run by its id "
                        "(see audit_status.resumable.id). The backend derives "
                        "the kind AND the checkpoint from that run — you cannot "
                        "oversize a step count or resume the wrong pipeline. "
                        "Omit to start fresh."
                    ),
                },
                "agent": {
                    "type": "string",
                    "description": "Agent that runs the audit steps (default: this bridge's agent type).",
                },
            },
            "required": ["project_id", "mode"],
            # partial ⇒ steps required — the contract states what the
            # implementation enforces (schema-aware MCP clients validate
            # client-side instead of discovering it via a RuntimeError).
            "allOf": [
                {
                    "if": {"properties": {"mode": {"const": "partial"}}},
                    "then": {"required": ["steps"]},
                }
            ],
        },
    },
    {
        "name": "audit_status",
        "description": (
            "0.8.12 — Consolidated audit state for a project, three layers "
            "kept SEPARATE (never merged):\n"
            "· `bridge_stream` — what THIS bridge's reader thread saw "
            "(running / done / error / launch_timeout / bridge_timeout / "
            "stream_closed / protocol_error, plus discussion_id + "
            "audit_run_id once done);\n"
            "· `live` — the backend's in-memory progress tracker. "
            "⚠️ `live: null` means 'no LIVE state known' — NOT 'finished': "
            "a backend restart wipes the tracker while an agent may still "
            "be working;\n"
            "· `latest` / `resumable` — DB history, fetched when `live` is "
            "null: the last completed run (with run_id) and the last "
            "Interrupted-but-resumable run. Statuses are exposed verbatim "
            "(Running/Completed/Interrupted/Failed)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {"type": "string", "description": "Kronn project id."},
            },
            "required": ["project_id"],
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
#   3. `adhoc-<ppid>-<parent start token>` — derived from the DIRECT
#      parent, stable for THIS bridge process's lifetime
#   4. Random `adhoc-<uuid4>` when the parent identity is unreadable
#
# Stays stable for the entire bridge process LIFETIME so every tool call
# from the same running bridge uses the same `discussion_sessions` row.
#
# NB (0.8.13): this id is NOT relied on to survive an MCP reload anymore.
# A reconnect spawns a new bridge under a (possibly) new ppid, so the
# adhoc id legitimately changes — the PR 118 assumption that the direct
# parent's identity survives reloads was fragile (unreadable start-token ⇒
# uuid fallback) and is now obsolete. Reload continuity is handled by the
# resume credential (`_attempt_resume`): the bridge re-attaches to its
# existing room and the backend rebinds the row to the NEW session_id.


def _start_token_of(pid):
    """Opaque token identifying a process INSTANCE — pid reuse alone would
    alias two different processes, the start time disambiguates. Linux/WSL
    reads /proc, macOS falls back to `ps lstart`. `None` when neither source
    is available (callers then take their own fallback path).
    """
    try:
        with open(f"/proc/{pid}/stat", "rb") as fh:
            raw = fh.read().decode("ascii", errors="replace")
        # comm (field 2) may contain spaces/parens — everything after
        # the LAST ')' is positional. starttime is field 22 overall,
        # i.e. index 19 once fields 1-2 are stripped.
        return raw.rsplit(")", 1)[1].split()[19]
    except Exception:
        pass
    try:
        out = subprocess.check_output(
            ["ps", "-p", str(pid), "-o", "lstart="],
            stderr=subprocess.DEVNULL, timeout=2,
        ).strip()
        if out:
            return hashlib.sha256(out).hexdigest()[:12]
    except Exception:
        pass
    return None


def _parent_start_token():
    """Start token of the DIRECT parent — the pre-077 session-id ingredient."""
    return _start_token_of(os.getppid())


def _ppid_of(pid):
    """Parent pid of `pid`. Linux/WSL via /proc/<pid>/stat field 4, macOS via
    `ps -o ppid=`. `None` when unreadable."""
    try:
        with open(f"/proc/{pid}/stat", "rb") as fh:
            raw = fh.read().decode("ascii", errors="replace")
        # After the last ')' the fields are positional: state(3), ppid(4)…,
        # so ppid is index 1 once comm is stripped.
        return int(raw.rsplit(")", 1)[1].split()[1])
    except Exception:
        pass
    try:
        out = subprocess.check_output(
            ["ps", "-p", str(pid), "-o", "ppid="],
            stderr=subprocess.DEVNULL, timeout=2,
        ).strip()
        return int(out) if out else None
    except Exception:
        return None


def _cmdline_of(pid):
    """Lowercased command line of `pid`, for CLI-ancestor detection. Linux/WSL
    via /proc/<pid>/cmdline, macOS via `ps -o command=`. `None` if unreadable."""
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            raw = fh.read()
        return raw.replace(b"\x00", b" ").decode("utf-8", errors="replace").lower()
    except Exception:
        pass
    try:
        out = subprocess.check_output(
            ["ps", "-p", str(pid), "-o", "command="],
            stderr=subprocess.DEVNULL, timeout=2,
        )
        return out.decode("utf-8", errors="replace").lower()
    except Exception:
        return None


# Substrings that mark an ancestor as the launching CLI. Same family as
# `_infer_agent_type_from_client_name`; kept lax on purpose (a node-wrapped
# `claude` or `codex` still matches on the combined cmdline).
_CLI_CMDLINE_HINTS = ("claude", "codex", "gemini", "kiro", "copilot", "vibe", "cursor", "cline")


def _cli_ancestor_identity():
    """`(pid, start_token)` of the OUTERMOST ancestor that looks like the CLI
    the user launched (claude/codex/…). Walk the whole chain up to init and
    keep the LAST (topmost) CLI-looking match — NOT the first. An MCP
    reconnect may re-spawn an intermediate runner whose cmdline also carries
    the CLI name, so the NEAREST match can still rotate on reload; only the
    outermost CLI process (the one the user actually launched) is durable
    across reconnects. That identity is also unique per session (distinct
    terminal tabs are distinct CLI processes), which is what lets a reloaded
    bridge find its own binding file again.

    `None` when no ancestor matches. The caller then DISABLES persisted resume
    (fail-closed) rather than key on anything unstable: we deliberately do NOT
    fall back to the direct parent (its identity is precisely what rotates on
    reload) nor to the cwd (two CLI tabs in the same repo would then share one
    resume credential). Fail-closed just means that rare session re-joins with
    a fresh token — never that it can hijack another session's row.
    """
    cur = os.getppid()
    seen = set()
    outermost = None
    for _ in range(24):  # real trees are <10 deep; bound guards a cycle
        if cur is None or cur <= 1 or cur in seen:
            break
        seen.add(cur)
        cmd = _cmdline_of(cur)
        if cmd and any(h in cmd for h in _CLI_CMDLINE_HINTS):
            tok = _start_token_of(cur)
            if tok is not None:
                outermost = (cur, tok)  # keep climbing; the last match wins
        cur = _ppid_of(cur)
    return outermost


def _platform_session_identity():
    """Return the strongest Claude-provided session scope available.

    On desktop terminals Claude's foreground process and daemon ``bg-spare``
    inherit the same terminal id and project directory even though they have
    different process trees *and* conversation UUIDs.  That pair is the right
    continuity boundary: it survives ``/clear`` and daemon dispatch, while a
    second terminal/project remains isolated.  A tmux pane further scopes a
    terminal id shared by sibling panes.

    When no complete terminal scope is available, Claude Code's logical
    ``CLAUDE_CODE_SESSION_ID`` is still stronger than a daemon PID.  Treat all
    values as best-effort platform hints and fall back to the process tree if
    Claude removes or changes them.
    """
    # Environment variables are inherited.  A different CLI launched from a
    # Claude shell must not accidentally reuse Claude's binding credential.
    client_agent = _infer_agent_type_from_client_name(_CLIENT_INFO.get("name"))
    if client_agent == "Unknown":
        client_agent = _infer_agent_type_from_client_name(_parent_process_cmdline())
    if client_agent != "ClaudeCode":
        return None

    def safe_part(name, max_length):
        value = os.environ.get(name)
        if not value or len(value) > max_length or value.strip() != value:
            return None
        if not value.isprintable():
            return None
        return value

    project = safe_part("CLAUDE_PROJECT_DIR", 4096)
    terminal_kind = None
    terminal_id = None
    for candidate in ("TERM_SESSION_ID", "WT_SESSION"):
        value = safe_part(candidate, 512)
        if value:
            terminal_kind, terminal_id = candidate, value
            break
    if terminal_id and project:
        pane = safe_part("TMUX_PANE", 128) or ""
        return ("claude-terminal", terminal_kind, terminal_id, pane, project)

    raw = os.environ.get("CLAUDE_CODE_SESSION_ID")
    if not raw:
        return None
    try:
        canonical = str(uuid.UUID(raw))
    except (AttributeError, ValueError):
        return None
    if raw.lower() != canonical:
        return None
    return ("claude-session", canonical)


def _binding_identity():
    """Best available identity for this logical CLI session.

    Prefer a platform session id because daemon/spare process topology is not
    a logical-session boundary.  Other clients retain the proven outermost-CLI
    fallback, so existing Codex and generic CLI binding keys do not rotate.
    """
    return _platform_session_identity() or _cli_ancestor_identity()


def _identity_key_from(identity):
    """Hash a stable CLI identity into a short filesystem-safe binding key.
    `None` in → `None` out (fail-closed: no durable identity ⇒ no persisted
    resume). Pure (no I/O) so the stable/distinct invariants are unit-testable:
    a given identity always maps to the same key, distinct identities to
    distinct keys.  Preserve the historical process-key encoding so clients
    without a platform session id keep finding their existing binding file."""
    if identity is None:
        return None
    if identity[0] in ("claude-terminal", "claude-session"):
        raw = json.dumps(identity, separators=(",", ":"), ensure_ascii=False)
    else:
        raw = f"pid:{identity[0]}:{identity[1]}"
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()[:16]


def _resolve_bridge_session_id():
    env_sid = os.environ.get("KRONN_SESSION_ID") or os.environ.get("KRONN_CALLER_SESSION_ID")
    if env_sid:
        return env_sid
    start_token = _parent_start_token()
    if start_token is not None:
        return f"adhoc-{os.getppid()}-{start_token}"
    return f"adhoc-{uuid.uuid4()}"


_BRIDGE_SESSION_ID = _resolve_bridge_session_id()


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


# 0.8.13 — presence root-fix: reload recovery via a persisted resume
# credential. An MCP reconnect re-spawns this sidecar (new PPID), wiping the
# in-memory `_CURRENT_DISC_ID` AND rotating the fallback session_id — the
# human then had to paste a fresh kr-join token every time. Instead, at join
# we stash `{disc_id, resume_token}` in a 0600 file keyed by the STABLE CLI
# identity (`_binding_identity`); on the next tool call after a reload,
# `_disc_id()` re-attaches to the same server row via `/peer-resume` — no
# token, and the backend rebinds the row in place (no ghost participant).
# The resume_token is a CREDENTIAL: 0600, never logged, never shown to the model.
_BINDING_DIR = os.path.expanduser("~/.config/kronn")
_BINDING_PATH_CACHE = {"computed": False, "path": None}
_BINDING_THREAD_LOCK = threading.Lock()


def _binding_path():
    """Absolute path of this session's binding file, or `None` when no durable
    CLI identity resolved (fail-closed — persisted resume is then disabled)."""
    if not _BINDING_PATH_CACHE["computed"]:
        key = _identity_key_from(_binding_identity())
        _BINDING_PATH_CACHE["path"] = (
            os.path.join(_BINDING_DIR, f"disc-binding-{key}.json") if key else None
        )
        _BINDING_PATH_CACHE["computed"] = True
    return _BINDING_PATH_CACHE["path"]


def _write_binding(
    disc_id,
    resume_token,
    agent_type=None,
    pending_resume_token=None,
):
    """Persist the reload credential atomically, mode 0600. No-op when there is
    no durable identity (fail-closed). `pending_resume_token` is written before
    the server mutates its hash, so a lost response or failed promotion can
    replay the exact same rotation. Returns whether the state reached disk."""
    if not disc_id or not resume_token:
        return False
    path = _binding_path()
    if not path:
        return False
    import tempfile
    try:
        os.makedirs(_BINDING_DIR, exist_ok=True)
        # Random exclusive temp in the SAME dir (mkstemp → O_CREAT|O_EXCL,
        # 0600, no symlink follow), then atomic rename over `path`. A symlink
        # pre-placed at a PREDICTABLE temp name can no longer redirect the
        # truncating write to an arbitrary file.
        fd, tmp = tempfile.mkstemp(prefix=".disc-binding-", suffix=".tmp", dir=_BINDING_DIR)
        try:
            with os.fdopen(fd, "w") as f:
                state = {"disc_id": disc_id, "resume_token": resume_token}
                if agent_type:
                    state["agent_type"] = agent_type
                if pending_resume_token:
                    state["pending_resume_token"] = pending_resume_token
                json.dump(state, f)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, path)  # atomic; renames over any symlink at `path`
            # File fsync persists bytes; directory fsync persists the rename.
            # Windows does not support opening directories this way, while its
            # replace durability is handled by the platform API.
            if os.name != "nt":
                dir_fd = os.open(_BINDING_DIR, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
        except Exception:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
        return True
    except Exception as e:
        # Never surface the token in the error; just note we couldn't persist.
        print(f"kronn-internal: could not persist resume binding ({e})", file=sys.stderr)
        return False


def _open_binding_lock():
    """Open and exclusively lock a per-binding sidecar lock file.

    Returns `(fd, platform)` or `None`. Thread + OS locks cover concurrent tool
    calls in this process and overlapping sidecars after a reload. The lock is
    held across prepare → HTTP CAS → promotion, preventing two divergent
    pending successors from overwriting one another.
    """
    import stat as _stat
    path = _binding_path()
    if not path:
        return None
    lock_path = f"{path}.lock"
    try:
        os.makedirs(_BINDING_DIR, exist_ok=True)
        fd = os.open(
            lock_path,
            os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0),
            0o600,
        )
        lst = os.lstat(lock_path)
        st = os.fstat(fd)
        if not _stat.S_ISREG(lst.st_mode) or not _stat.S_ISREG(st.st_mode):
            raise RuntimeError("binding lock is not a regular file")
        if (lst.st_dev, lst.st_ino) != (st.st_dev, st.st_ino):
            raise RuntimeError("binding lock path changed while opening")
        if hasattr(os, "getuid") and st.st_uid != os.getuid():
            raise RuntimeError("binding lock is not owned by this user")
        if st.st_mode & 0o077:
            raise RuntimeError("binding lock is group/world accessible")

        if os.name == "nt":
            import msvcrt
            if st.st_size == 0:
                os.write(fd, b"\0")
                os.fsync(fd)
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_LOCK, 1)
            return fd, "windows"

        import fcntl
        fcntl.flock(fd, fcntl.LOCK_EX)
        return fd, "unix"
    except Exception as e:
        try:
            os.close(fd)
        except (OSError, UnboundLocalError):
            pass
        print(f"kronn-internal: could not lock resume binding ({e})", file=sys.stderr)
        return None


def _close_binding_lock(lock):
    if not lock:
        return
    fd, platform = lock
    try:
        if platform == "windows":
            import msvcrt
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


@contextlib.contextmanager
def _binding_transaction_lock():
    with _BINDING_THREAD_LOCK:
        lock = _open_binding_lock()
        try:
            yield lock is not None
        finally:
            _close_binding_lock(lock)


def _read_binding():
    """Read the credential, refusing anything an attacker could have swapped
    in: a symlink (O_NOFOLLOW), a non-regular file, one not owned by us, or one
    readable by group/world. Returns the dict or `None`."""
    import stat as _stat
    path = _binding_path()
    if not path:
        return None
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    except OSError:
        return None  # missing, or a symlink (O_NOFOLLOW ⇒ ELOOP)
    try:
        # Symlink defence that does NOT depend on O_NOFOLLOW (which is 0 on
        # platforms lacking it, e.g. Windows, so os.open would follow the
        # link): require the path — statted WITHOUT following its final
        # component — to be a regular file that is the SAME inode as the fd we
        # hold. A followed symlink makes lstat(path) a link (or points the fd
        # at a different (dev, ino)), and we refuse.
        lst = os.lstat(path)
        st = os.fstat(fd)
        if not _stat.S_ISREG(lst.st_mode):
            return None  # path itself is a symlink / non-regular
        if (lst.st_dev, lst.st_ino) != (st.st_dev, st.st_ino):
            return None  # fd is not the inode the path names — refuse
        if not _stat.S_ISREG(st.st_mode):
            return None
        if hasattr(os, "getuid") and st.st_uid != os.getuid():
            return None  # not our file — refuse a planted credential
        if st.st_mode & 0o077:
            return None  # group/world bits set — treat as tampered
        with os.fdopen(fd, "r") as f:
            fd = None  # fdopen took ownership; the with-block closes it
            data = json.load(f)
    except Exception:
        return None
    finally:
        if fd is not None:
            try:
                os.close(fd)
            except OSError:
                pass
    if isinstance(data, dict) and data.get("disc_id") and data.get("resume_token"):
        return data
    return None


def _clear_binding():
    path = _binding_path()
    if not path:
        return
    try:
        os.remove(path)
    except Exception:
        pass


def _attempt_resume():
    """Reload recovery. Re-attach to the disc bound before an MCP reload using
    the persisted resume credential — no fresh kr-join token. The backend
    rotates the credential (returns a new one) and rebinds the server row in
    place; we update the binding file to the rotated value. Returns the
    disc_id on success, `None` otherwise (missing binding, rotated/invalid
    credential, or backend unreachable — in which case the agent falls back
    to a manual disc_join). The binding is kept on failure: a transient
    backend outage (e.g. a rebuild) must not cost the reload capability."""
    with _binding_transaction_lock() as locked:
        if not locked:
            return None
        b = _read_binding()
        if not b:
            return None
        disc_id_before = b["disc_id"]
        old_token = b["resume_token"]
        stored_agent_type = b.get("agent_type")
        agent_type = stored_agent_type or _agent_type_for_session()
        next_token = b.get("pending_resume_token")
        if not next_token:
            next_token = f"kr-resume-{secrets.token_hex(16)}"
            if not _write_binding(
                disc_id_before,
                old_token,
                # Old binding files have no agent_type. Do not fossilize an
                # unverified reload-time inference: persist it only after the
                # backend accepted it, so a transient `Unknown` can self-heal.
                agent_type=stored_agent_type,
                pending_resume_token=next_token,
            ):
                return None  # never mutate the server before pending is durable

        body = {
            "agent_type": agent_type,
            "session_id": _session_id_for_caller(),
            "resume_token": old_token,
            "next_resume_token": next_token,
        }
        try:
            result = _unwrap(_http("POST", "/api/discussions/peer-resume", body))
        except Exception:
            return None  # pending stays durable for the exact retry
        if not isinstance(result, dict):
            return None
        disc_id = result.get("disc_id")
        acknowledged_token = result.get("resume_token")
        if disc_id != disc_id_before or acknowledged_token != next_token:
            return None  # fail closed on a mismatched ack; keep pending
        _set_current_disc_id(disc_id)
        # Promotion failure is recoverable: the pending file still contains
        # `(old,next)` and the backend accepts that exact replay.
        _write_binding(disc_id, next_token, agent_type=agent_type)
        return disc_id


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
        # Reload recovery (0.8.13): the in-memory binding is lost on every
        # MCP reconnect. Before failing, try to re-attach to the disc we were
        # bound to via the persisted resume credential — the whole point is
        # the human no longer re-pastes a kr-join token after each reload.
        resumed = _attempt_resume()
        if resumed:
            return resumed
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
        # Same bearer as _http(): without it, an auth-enforced instance 401s
        # this read and the silent fallback drops project/agent inheritance.
        token = os.environ.get("KRONN_AUTH_TOKEN")
        if token:
            req.add_header("Authorization", f"Bearer {token}")
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


def _http_transport_retry(method, path, attempts=6, delays=(2, 4, 8, 12, 16)):
    """`_http` with a BOUNDED retry on TRANSPORT failures only (connection
    refused/reset, remote disconnect, socket timeout) — the signature of a
    backend restart, e.g. `cargo watch` rebuilding for 30-60s. HTTP errors
    (4xx/5xx) are application-level and never retried. Safe only for
    idempotent calls: the caller re-sends the same request verbatim.
    Total worst-case wait ≈ sum(delays) ≈ 42s + in-flight time."""
    last_err = None
    for i in range(attempts):
        try:
            return _http(method, path)
        except RuntimeError:
            raise  # HTTPError path from _http — application error, no retry
        except (urllib.error.URLError, ConnectionError, TimeoutError, OSError) as e:
            last_err = e
            if i + 1 < attempts:
                time.sleep(delays[min(i, len(delays) - 1)])
    raise RuntimeError(
        f"backend unreachable after {attempts} attempts (~{sum(delays)}s — rebuild in "
        f"progress?): {last_err}. Nothing is lost: messages persist in the DB — call "
        "disc_wait_for_peer again with the SAME since_sort_order."
    )


def _http_text(method, path):
    """Variant of `_http` for endpoints that ship raw text (not JSON / not the
    `ApiResponse` envelope) — e.g. `/api/conventions/agents-md-format-v1`
    which returns the embedded `text/markdown` spec verbatim."""
    url = f"{_backend_url()}{path}"
    req = urllib.request.Request(url, method=method)
    token = os.environ.get("KRONN_AUTH_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            return resp.read().decode("utf-8")
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
    message_id = args.get("message_id")
    if (idx is None) == (message_id is None):
        raise RuntimeError("disc_get_message: provide exactly one of 'idx' or 'message_id'")
    before = args.get("before", 0)
    after = args.get("after", 0)
    for name, value in (("before", before), ("after", after)):
        if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= 10:
            raise RuntimeError(f"disc_get_message: '{name}' must be an integer from 0 to 10")
    selector = message_id if message_id is not None else idx
    encoded_selector = urllib.parse.quote(str(selector), safe="")
    query = urllib.parse.urlencode({"before": before, "after": after})
    return _unwrap(_http(
        "GET",
        f"/api/discussions/{_disc_id()}/message/{encoded_selector}?{query}",
    ))


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
    # 0.8.13 — carry the caller's session id so the backend scopes the
    # append heartbeat + activity-clear to THIS (possibly resumed) row only,
    # never to every session of the same agent_type (multi-machine / sibling
    # peer safety). A legacy bridge that omits it gets no presence refresh on
    # append — deliberately conservative, matching disc_wait_for_peer.
    return _unwrap(_http("POST", "/api/disc/append", {
        "disc_id": disc_id,
        "messages": messages,
        "session_id": _session_id_for_caller(),
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


def call_disc_list(args):
    """Browse available discussions, compact + newest-first. Shared/P2P only by
    default (the rooms worth joining cross-instance); shared_only=false for all.
    No search keyword needed — complements disc_search (keyword) and
    disc_load_other (read one by id)."""
    shared_only = args.get("shared_only", True)
    try:
        limit = int(args.get("limit") or 30)
    except (TypeError, ValueError):
        limit = 30
    limit = max(1, min(limit, 100))

    data = _unwrap(_http("GET", "/api/discussions"))
    discs = data if isinstance(data, list) else (data.get("discussions") or [])
    out = []
    for d in discs:
        if shared_only and not d.get("shared_id"):
            continue
        out.append({
            "disc_id": d.get("id"),
            "title": d.get("title"),
            "shared_id": d.get("shared_id"),
            "message_count": d.get("message_count"),
            "updated_at": d.get("updated_at"),
        })
    out.sort(key=lambda x: x.get("updated_at") or "", reverse=True)
    out = out[:limit]
    return {"disc_count": len(out), "shared_only": shared_only, "discussions": out}


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
        # 0.8.13 — stash the resume credential so a later MCP reload can
        # re-attach to THIS disc via `/peer-resume` without a fresh token.
        resume_token = result.get("resume_token") if isinstance(result, dict) else None
        if resume_token:
            _write_binding(disc_id, resume_token, agent_type=agent_type)

    # The resume credential is a secret persisted 0600 — it must NEVER reach
    # the model's context. Strip it from the value handed back to the agent.
    if isinstance(result, dict):
        result.pop("resume_token", None)
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
        _clear_binding()
        raise
    _set_current_disc_id(None)
    # 0.8.13 — a deliberate leave drops the resume capability: the next
    # session must join explicitly, not silently reclaim this row.
    _clear_binding()
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
    # Presence phase 1 — identify THIS session so the activity placeholder
    # (listening/reading) lands on OUR row only, never on a concurrent
    # session of the same agent type (multi-machine setups).
    params["session_id"] = _session_id_for_caller()
    qs = urllib.parse.urlencode(params)
    sep = "?" if qs else ""
    # Transport-level retry (bounded): a backend restart mid-poll must not
    # surface as a tool error — the wait is idempotent on since_sort_order.
    result = _unwrap(_http_transport_retry("GET", f"/api/discussions/{disc_id}/wait{sep}{qs}"))
    # A timed-out wait (no peer activity in the window) is NORMAL in an ongoing
    # collaboration — but literal agents (notably Codex) otherwise read the empty
    # result as "conversation over" and STOP after ~60s. Surface an explicit
    # next-action hint so the agent keeps waiting instead of leaving.
    if isinstance(result, dict) and result.get("timed_out"):
        pacing = result.get("pacing") or {}
        delay = pacing.get("next_delay_seconds")
        regime = pacing.get("regime", "cold")
        pace_line = (
            f"PACING (server-computed, regime={regime}): wait ~{delay}s before "
            "your next disc_wait_for_peer."
            if delay is not None else
            "PACING: apply the room's poll_policy (disc_join/disc_meta) — back "
            "off 30s,30s,1m,1m,2m,2m,4m,4m cap 8m, reset on peer message."
        )
        result["hint"] = (
            "No peer posted during this window. This is NORMAL — the other "
            "agent may still be thinking. Call disc_wait_for_peer AGAIN to keep "
            "waiting (pass latest_sort_order as since_sort_order). Do NOT stop "
            "or disc_leave() just because the wait timed out — only leave when "
            "the task is done or the user explicitly says stop. " + pace_line
        )
    return result


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


def call_workflow_active_runs(_args):
    # In-flight board (2026-06-11). Reuses `GET /api/workflows` (each summary
    # carries its latest run) and keeps only the ones whose last run is still
    # in flight — zero extra endpoint. The agent gets "what is running /
    # awaiting approval right now" in one call; for the live step of a run it
    # drills down via `workflow_run_status(run_id)`.
    active = {"Running", "WaitingApproval", "Pending"}
    data = _unwrap(_http("GET", "/api/workflows")) or []
    out = []
    for w in data:
        lr = w.get("last_run") or {}
        if lr.get("status") in active:
            out.append({
                "workflow_id": w.get("id"),
                "workflow_name": w.get("name"),
                "project_id": w.get("project_id"),
                "run_id": lr.get("id"),
                "status": lr.get("status"),
                "started_at": lr.get("started_at"),
            })
    return out


def call_qp_list(_args):
    # 0.8.5 — compact list. Keeps variable names so the agent can decide
    # if an existing QP fits the user's use case before drafting a new
    # one. Drops the full `prompt_template` body — call `qp_get(qp_id)` to
    # read it (understand what the QP does / run it yourself / pre-edit).
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


# Allowlist of (name, version) → backend path. Keeps the surface tight
# (an agent can't bait this tool into fetching arbitrary URLs) and gives a
# clean error when a misspelled name is requested.
_CONVENTION_PATHS = {
    ("agents-md-format", "v1"): "/api/conventions/agents-md-format-v1",
}


def call_convention_get(args):
    """Fetch a Kronn documentation convention spec verbatim.

    Defaults to the only convention shipped in 0.8.7 (`agents-md-format` v1).
    Returns `{name, version, content_markdown}` so the agent gets the spec
    body inline (no follow-up call needed). The list is allowlisted — bogus
    names raise instead of issuing the GET.
    """
    name = (args.get("name") or "agents-md-format").strip()
    version = (args.get("version") or "v1").strip()
    key = (name, version)
    path = _CONVENTION_PATHS.get(key)
    if path is None:
        known = ", ".join(f"{n}@{v}" for (n, v) in _CONVENTION_PATHS)
        raise RuntimeError(
            f"convention_get: unknown convention {name}@{version}. "
            f"Known: {known}"
        )
    content = _http_text("GET", path)
    return {
        "name": name,
        "version": version,
        "content_markdown": content,
    }


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
    # 0.8.8 — wrap bare-string step_type/output_format/mode into the tagged
    # `{"type": ...}` form serde requires (see _normalize_steps).
    body["steps"] = _normalize_steps(body["steps"])
    # 0.8.8 — SAFETY DEFAULT: a Cron/Tracker workflow with no concurrency_limit
    # lets a new tick start while the previous run is STILL going → overlapping
    # runs = double work + duplicate side-effects (real incident: a 2.5h PR-review
    # cron fired its 10h tick on top of itself). Default to 1 (scheduler skips a
    # tick while a run is active) unless the agent set it explicitly. To allow
    # overlap, pass a higher concurrency_limit on purpose.
    trig_type = (args.get("trigger") or {}).get("type")
    if trig_type in ("Cron", "Tracker") and args.get("concurrency_limit") is None:
        body["concurrency_limit"] = 1
    # 0.8.8 — fill PromptVariable's required label/placeholder (see
    # _normalize_variables) so launch-time vars don't 422 on `{name}` alone.
    if "variables" in body:
        body["variables"] = _normalize_variables(body["variables"])
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
    # 0.8.8 — fill PromptVariable's required label/placeholder so the agent
    # can pass `{name}` alone instead of 422-ing (cf.
    # [[project_mcp_workflow_crud_gap]]).
    if "variables" in body:
        body["variables"] = _normalize_variables(body["variables"])
    # 0.8.5 — auto-inherit project binding from the current discussion
    # when the agent doesn't pass one explicitly.
    if "project_id" not in body or body.get("project_id") is None:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    return _unwrap(_http("POST", "/api/quick-prompts", body))


# ─── 0.8.8 (2026-06-23) WF/QP read · clone · update tools ────────────────
# Thin wrappers over REST routes the UI already uses. Closes the gap an
# agent hit when it had to draft a workflow BLIND — `workflow_list` is
# compact (no steps) and there was no get/clone/update, so the agent
# reverse-engineered the WorkflowStep schema from a chain of 422s and
# left an orphan QP on every edit. Cf. [[project_mcp_workflow_crud_gap]].

def _normalize_variables(vars_list):
    """`PromptVariable` (Rust model) requires `name` + `label` +
    `placeholder` — all non-Option. Agents routinely pass only `name`,
    which 422s. Fill the two cosmetic fields so the agent can omit them:
    `label` → the name, `placeholder` → "". Idempotent; leaves anything
    already-present untouched."""
    if not isinstance(vars_list, list):
        return vars_list
    out = []
    for v in vars_list:
        if isinstance(v, dict) and v.get("name"):
            v = dict(v)
            v.setdefault("label", v["name"])
            v.setdefault("placeholder", "")
        out.append(v)
    return out


# `WorkflowStep` has serde `#[serde(tag = "type")]` enum fields — on the wire
# they are TAGGED OBJECTS `{"type": "Agent"}`, NOT bare strings. An LLM very
# often writes `"step_type": {"type": "Agent"}`, which fails deserialization with an
# opaque 422 ("invalid type: string ... expected internally tagged enum"). We
# wrap any bare-string value of these fields into `{"type": <value>}` so BOTH
# forms work — killing a whole class of 422 ping-pong. Idempotent: an already-
# tagged object (e.g. from `workflow_get` round-trip) is left untouched.
_TAGGED_STEP_FIELDS = ("step_type", "output_format", "mode")


def _normalize_steps(steps):
    if not isinstance(steps, list):
        return steps
    out = []
    for s in steps:
        if isinstance(s, dict):
            s = dict(s)
            for f in _TAGGED_STEP_FIELDS:
                if isinstance(s.get(f), str):
                    s[f] = {"type": s[f]}
        out.append(s)
    return out


def call_workflow_get(args):
    """Full workflow definition (steps + every field) — NOT the compact
    `workflow_list` shape. This is what an agent reads before cloning or
    patching an existing workflow."""
    wid = args.get("workflow_id") or args.get("id")
    if not wid:
        raise RuntimeError("workflow_get: missing required 'workflow_id'")
    return _unwrap(_http("GET", f"/api/workflows/{wid}"))


def _run_summary(r):
    """Lean projection of a WorkflowRun for the history list — drops the heavy
    `step_results` + `state`. `parent_run_id` is KEPT so foreach/batch children
    are identifiable (a child run carries the parent RUN's id here)."""
    return {
        "id": r.get("id"),
        "status": r.get("status"),
        "run_type": r.get("run_type"),
        "started_at": r.get("started_at"),
        "finished_at": r.get("finished_at"),
        "tokens_used": r.get("tokens_used"),
        "batch_total": r.get("batch_total"),
        "batch_completed": r.get("batch_completed"),
        "batch_failed": r.get("batch_failed"),
        "parent_run_id": r.get("parent_run_id"),
        "produced_branches": r.get("produced_branches"),
    }


def call_workflow_runs(args):
    """List the RUN HISTORY of a workflow (most recent first) — not just the
    active ones (`workflow_active_runs`) or the last one. Lean per-run summary
    (status · run_type · started/finished · tokens · batch counts ·
    parent_run_id); call `workflow_run_get` for a run's per-step detail.
    Enumerate foreach/batch CHILDREN of a parent run by calling this on the
    CHILD workflow's id and filtering by `parent_run_id == <parent run id>`
    (children belong to the child workflow, each tagged with the parent RUN)."""
    wid = args.get("workflow_id") or args.get("id")
    if not wid:
        raise RuntimeError("workflow_runs: missing required 'workflow_id'")
    runs = _unwrap(_http("GET", f"/api/workflows/{wid}/runs")) or []
    out = [_run_summary(r) for r in runs]
    limit = args.get("limit")
    if isinstance(limit, int) and limit > 0:
        out = out[:limit]
    return out


def call_workflow_run_get(args):
    """Full detail of ONE workflow run, incl per-step results — for debriefing
    a finished/failed run (which step failed, durations, tokens). Step outputs
    are truncated to keep the payload manageable; for an agent's produced
    content read the run's discussions via `workflow_run_discussions`."""
    wid = args.get("workflow_id") or args.get("id")
    rid = args.get("run_id")
    if not wid or not rid:
        raise RuntimeError("workflow_run_get: requires 'workflow_id' and 'run_id'")
    run = _unwrap(_http("GET", f"/api/workflows/{wid}/runs/{rid}"))
    if isinstance(run, dict) and isinstance(run.get("step_results"), list):
        steps = []
        for s in run["step_results"]:
            out = s.get("output")
            if isinstance(out, str) and len(out) > 1500:
                out = out[:1500] + f"… [truncated, {len(s['output'])} chars total]"
            steps.append({
                "step_name": s.get("step_name"),
                "status": s.get("status"),
                "duration_ms": s.get("duration_ms"),
                "tokens_used": s.get("tokens_used"),
                "step_kind": s.get("step_kind"),
                "step_agent": s.get("step_agent"),
                "output": out,
            })
        run = dict(run)
        run["step_results"] = steps
    return run


def call_workflow_cancel_run(args):
    """Cancel a RUNNING workflow run (the MCP equivalent of the UI's "Arrêter").
    DESTRUCTIVE — stops the run + its in-flight agents; already-completed steps
    /commits are kept. Use to stop a stuck or duplicate run (e.g. an overlapping
    cron tick). Confirm with the user before cancelling a run you didn't start."""
    wid = args.get("workflow_id") or args.get("id")
    rid = args.get("run_id")
    if not wid or not rid:
        raise RuntimeError("workflow_cancel_run: requires 'workflow_id' and 'run_id'")
    return _unwrap(_http("POST", f"/api/workflows/{wid}/runs/{rid}/cancel"))


def call_workflow_resume_run(args):
    """Resume an Interrupted run — atomic claim backend-side, so a double
    call gets one resume + one clear error."""
    rid = args.get("run_id")
    if not rid:
        raise RuntimeError("workflow_resume_run: missing required 'run_id'")
    return _unwrap(_http("POST", f"/api/workflow-runs/{rid}/resume"))


def call_workflow_update(args):
    """Patch an existing workflow. `UpdateWorkflowRequest` is already a
    TRUE patch backend-side (any omitted field preserves its current
    value), so we forward exactly the patchable keys the agent supplied
    — no GET-merge needed."""
    wid = args.get("workflow_id") or args.get("id")
    if not wid:
        raise RuntimeError("workflow_update: missing required 'workflow_id'")
    patchable = (
        "name", "project_id", "trigger", "steps", "actions", "safety",
        "workspace_config", "concurrency_limit", "guards", "artifacts",
        "on_failure", "exec_allowlist", "variables", "enabled",
    )
    body = {k: args[k] for k in patchable if k in args}
    if not body:
        raise RuntimeError(
            "workflow_update: no patchable field provided "
            f"(allowed: {', '.join(patchable)})"
        )
    if "steps" in body:
        body["steps"] = _normalize_steps(body["steps"])
    if "variables" in body:
        body["variables"] = _normalize_variables(body["variables"])
    return _unwrap(_http("PUT", f"/api/workflows/{wid}", body))


def call_workflow_clone(args):
    """Duplicate a workflow via export→import: mints fresh ids, re-bundles
    + rewrites referenced QP ids, strips per-user notify URLs. Safer than
    GET→POST (which would share QP ids and reuse the source name verbatim).
    The clone always lands DISABLED (draft discipline — clones never
    auto-fire) with a distinct name, so the user never stares at two
    identically-named workflows. The agent enables it via
    `workflow_set_enabled` when ready to test."""
    wid = args.get("workflow_id") or args.get("id")
    if not wid:
        raise RuntimeError("workflow_clone: missing required 'workflow_id'")
    envelope = _http_text("GET", f"/api/workflows/{wid}/export")
    import_body = {"content": envelope}
    pid = args.get("project_id")
    if pid is None:
        pid = _current_project_id()
    if pid is not None:
        import_body["project_id"] = pid
    cloned = _unwrap(_http("POST", "/api/workflows/import", import_body))
    new_id = cloned.get("id")
    new_name = args.get("new_name") or f"{cloned.get('name', 'Workflow')} (copie)"
    return _unwrap(_http("PUT", f"/api/workflows/{new_id}",
                         {"enabled": False, "name": new_name}))


def call_workflow_set_enabled(args):
    """Enable/disable a workflow. Disabling is always allowed. ENABLING a
    Cron/Tracker workflow is refused unless `force=true` — that would
    schedule autonomous runs without a human in the loop. Manual
    workflows (only run when explicitly triggered) enable freely."""
    wid = args.get("workflow_id") or args.get("id")
    if not wid:
        raise RuntimeError("workflow_set_enabled: missing required 'workflow_id'")
    if "enabled" not in args:
        raise RuntimeError("workflow_set_enabled: missing required 'enabled' (bool)")
    enabled = bool(args["enabled"])
    if enabled and not bool(args.get("force")):
        wf = _unwrap(_http("GET", f"/api/workflows/{wid}"))
        ttype = (wf.get("trigger") or {}).get("type")
        if ttype in ("Cron", "Tracker"):
            raise RuntimeError(
                f"workflow_set_enabled: refusing to enable a {ttype}-triggered "
                "workflow — that would schedule autonomous runs with no human in "
                "the loop. Enable it from the Kronn UI, or pass force=true if you "
                "are certain. (Manual workflows enable freely.)"
            )
    return _unwrap(_http("PUT", f"/api/workflows/{wid}", {"enabled": enabled}))


def call_qp_update(args):
    """Patch an existing Quick Prompt. `PUT /api/quick-prompts/<id>` takes
    the FULL request and REPLACES (omitted fields reset — same footgun as
    `qa_update`), and there is no single-QP GET route, so we load the QP
    from `qp_list`, apply the patch field-by-field, and PUT the merged
    body. Lets the qp-improver / QP-iteration loop patch a QP in place
    instead of creating an orphan vN.1."""
    qid = args.get("qp_id") or args.get("id")
    if not qid:
        raise RuntimeError("qp_update: missing required 'qp_id'")
    existing_list = _unwrap(_http("GET", "/api/quick-prompts")) or []
    existing = next((q for q in existing_list if q.get("id") == qid), None)
    if not existing:
        raise RuntimeError(
            f"qp_update: quick prompt {qid!r} not found — call qp_list to see "
            "what exists"
        )
    patchable = (
        "name", "icon", "prompt_template", "variables", "agent",
        "project_id", "skill_ids", "profile_ids", "directive_ids",
        "tier", "description",
    )
    body = {}
    for field in patchable:
        if field in args:
            body[field] = args[field]
        elif field in existing:
            body[field] = existing[field]
    if "variables" in body:
        body["variables"] = _normalize_variables(body["variables"])
    if not body.get("name"):
        raise RuntimeError("qp_update: merged body has empty 'name' — re-check qp_list output")
    if len(body["name"]) > 200:
        raise RuntimeError(f"qp_update: 'name' too long ({len(body['name'])} chars, max 200)")
    return _unwrap(_http("PUT", f"/api/quick-prompts/{qid}", body))


def call_qp_get(args):
    """Full Quick Prompt definition — including the `prompt_template` BODY that
    `qp_list` drops for brevity (and all bindings: variables, skill/profile/
    directive ids, agent, tier). This is what you need to (a) understand what a
    QP actually does so you can RUN it yourself, or (b) read it before a
    `qp_update` surgery. There is no single-QP GET route, so we fetch the list
    and filter by id — same lossless source as `qp_update`."""
    qid = args.get("qp_id") or args.get("id")
    if not qid:
        raise RuntimeError("qp_get: missing required 'qp_id'")
    qps = _unwrap(_http("GET", "/api/quick-prompts")) or []
    qp = next((q for q in qps if q.get("id") == qid), None)
    if not qp:
        raise RuntimeError(
            f"qp_get: quick prompt {qid!r} not found — call qp_list to see what exists"
        )
    return qp


def call_qp_delete(args):
    """Delete a Quick Prompt by id. Use to clean up an orphan draft (e.g.
    after replacing a QP rather than patching it via `qp_update`)."""
    qid = args.get("qp_id") or args.get("id")
    if not qid:
        raise RuntimeError("qp_delete: missing required 'qp_id'")
    return _unwrap(_http("DELETE", f"/api/quick-prompts/{qid}"))


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

    # 2026-06-10 — normalize a stringified JSON `body`. Some MCP client
    # stacks serialize the object tool-arg as a JSON STRING; forwarded
    # as-is, the upstream request goes out double-encoded and the target
    # API silently no-ops (caught on Slides.com via an httpbin echo).
    # The backend broker normalizes too — this is defense-in-depth and
    # takes effect without a backend rebuild (the script is bind-mounted).
    if isinstance(body.get("body"), str):
        raw = body["body"]
        try:
            parsed = json.loads(raw)
            if isinstance(parsed, (dict, list)):
                body["body"] = parsed
        except (ValueError, TypeError) as e:
            # A plain-string body is legit for some APIs — keep it. BUT a body
            # that clearly MEANT to be JSON (starts with { or [) yet fails to
            # parse is an LLM brace/quote/escape slip. Forwarding it as a raw
            # string makes the target API reject it with an opaque 400
            # ("Invalid request payload") that looks like truncation. Fail LOUD
            # and actionable so the agent fixes the JSON instead of guessing.
            if raw.lstrip()[:1] in ("{", "["):
                raise RuntimeError(
                    f"api_call: the `body` looks like JSON but is not valid JSON "
                    f"({e}). The full body was received ({len(raw)} chars — NOT "
                    f"truncated); the error is in the JSON itself (check braces, "
                    f"quotes, escaping near the reported column). Fix it and retry."
                )
            # else: genuine non-JSON string body — forward as-is.

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


def call_qp_batch_run(args):
    """0.8.7 phase 4 PR2 — fan a Quick Prompt out to N discussions.

    The backend `POST /api/mcp/qp-batch-run` renders the QP template per
    item, creates ONE batch run linking all child discs, and kicks off
    every agent in the background (semaphore-throttled). Returns
    `{run_id, disc_ids[], batch_total, ..., next_check}`. Track via
    `workflow_run_status` / `workflow_run_discussions`.
    """
    qp_id = args.get("qp_id")
    if not qp_id:
        raise RuntimeError("qp_batch_run: missing required 'qp_id'")
    items = args.get("items")
    if not isinstance(items, list) or not items:
        raise RuntimeError("qp_batch_run: 'items' must be a non-empty array")
    norm_items = []
    for it in items:
        if not isinstance(it, dict):
            raise RuntimeError("qp_batch_run: each item must be an object {title?, vars?}")
        norm = {}
        title = it.get("title")
        if title is not None:
            norm["title"] = str(title)
        vars_obj = it.get("vars")
        if isinstance(vars_obj, dict):
            # Same str-coercion as qp_run.vars — backend expects HashMap<String, String>.
            norm["vars"] = {str(k): str(v) for k, v in vars_obj.items()}
        norm_items.append(norm)
    body = {"qp_id": qp_id, "items": norm_items}
    for k in ("project_id", "batch_name"):
        v = args.get(k)
        if v is not None:
            body[k] = v
    # Auto-inherit current disc's project (same UX as qp_run / disc_create).
    if "project_id" not in body:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    return _unwrap(_http("POST", "/api/mcp/qp-batch-run", body))


def call_workflow_run_discussions(args):
    """0.8.7 phase 4 PR2 — list the discussions a run spawned.

    Pure pass-through to `GET /api/mcp/workflow-run-discussions/<run_id>`.
    Empty for linear workflows (use `workflow_run_status.steps[]` there).
    """
    run_id = args.get("run_id")
    if not run_id:
        raise RuntimeError("workflow_run_discussions: missing required 'run_id'")
    return _unwrap(_http("GET", f"/api/mcp/workflow-run-discussions/{run_id}"))


def call_workflow_wait_for_completion(args):
    """0.8.7 phase 4 PR3 — long-poll a run until terminal or timeout.

    The backend `POST /api/mcp/workflow-wait-for-completion` holds the
    connection up to `timeout_s` (clamped [1, 60]) and returns the
    terminal status as soon as the run finishes, else `timed_out=true`
    plus a `next_check` hint for the next call.
    """
    run_id = args.get("run_id")
    if not run_id:
        raise RuntimeError("workflow_wait_for_completion: missing required 'run_id'")
    body = {"run_id": run_id}
    timeout_s = args.get("timeout_s")
    if timeout_s is not None:
        try:
            body["timeout_s"] = int(timeout_s)
        except (TypeError, ValueError):
            raise RuntimeError(
                "workflow_wait_for_completion: 'timeout_s' must be an integer"
            )
    return _unwrap(_http("POST", "/api/mcp/workflow-wait-for-completion", body))


def call_learning_propose(args):
    # 0.9.0 — Continual Learning. Propose a durable fact/preference/inference,
    # gated server-side (evidence existence + faithfulness + human validation).
    # Client-side guards mirror the server's hard rejects for a fast, clear error.
    claim = (args.get("claim") or "").strip()
    if not claim:
        raise RuntimeError("learning_propose: 'claim' is required and non-empty")
    kind = args.get("kind")
    if kind not in ("fact", "preference", "inference"):
        raise RuntimeError("learning_propose: 'kind' must be fact | preference | inference")
    evidence = args.get("evidence")
    if not isinstance(evidence, list) or not evidence:
        raise RuntimeError(
            "learning_propose: 'evidence' must be a non-empty array of "
            "{kind, ref[, quote]} — a learning with no source is refused"
        )
    for i, e in enumerate(evidence):
        if not isinstance(e, dict) or not (e.get("ref") or "").strip():
            raise RuntimeError(f"learning_propose: evidence[{i}] needs a non-empty 'ref'")
    body = {"claim": claim, "kind": kind, "evidence": evidence}
    if args.get("confidence") is not None:
        body["confidence"] = args["confidence"]
    # Auto-inherit disc + project + agent from the current discussion.
    meta = _current_disc_meta()
    if meta:
        body.setdefault("discussion_id", meta.get("id"))
        if meta.get("project_id"):
            body.setdefault("project_id", meta["project_id"])
        if meta.get("agent"):
            body.setdefault("source_agent", meta["agent"])
    # Explicit args win over inheritance.
    for k in ("discussion_id", "project_id", "source_agent"):
        if args.get(k):
            body[k] = args[k]
    return _unwrap(_http("POST", "/api/learnings/propose", body))


def call_skills_list(_args):
    """Lean catalog of Kronn SKILLS (builtin + custom). These are the valid
    values for an Agent step's `skill_ids` (and a QP's `skill_ids`). Drops the
    full markdown `content` for brevity — list to PICK ids, then the step
    injects the skill at run time. Call this instead of guessing skill ids or
    asking the user to paste them."""
    data = _unwrap(_http("GET", "/api/skills")) or []
    return [
        {
            "id": s.get("id"),
            "name": s.get("name"),
            "description": s.get("description"),
            "category": s.get("category"),
            "is_builtin": s.get("is_builtin"),
            "token_estimate": s.get("token_estimate"),
        }
        for s in data
    ]


def call_profiles_list(_args):
    """Lean catalog of Kronn PROFILES (personas — builtin + custom). Valid
    values for an Agent step's `profile_ids` (and a QP's `profile_ids`). Drops
    the full `persona_prompt` body; list to PICK ids."""
    data = _unwrap(_http("GET", "/api/profiles")) or []
    return [
        {
            "id": p.get("id"),
            "name": p.get("name"),
            "role": p.get("role"),
            "persona_name": p.get("persona_name"),
            "category": p.get("category"),
            "default_engine": p.get("default_engine"),
            "is_builtin": p.get("is_builtin"),
            "token_estimate": p.get("token_estimate"),
        }
        for p in data
    ]


def call_directives_list(_args):
    """Lean catalog of Kronn DIRECTIVES (builtin + custom). Valid values for an
    Agent step's `directive_ids` (and a QP's `directive_ids`). Drops the full
    `content` body; keeps `conflicts` so you don't pick mutually-exclusive
    directives. List to PICK ids."""
    data = _unwrap(_http("GET", "/api/directives")) or []
    return [
        {
            "id": d.get("id"),
            "name": d.get("name"),
            "description": d.get("description"),
            "category": d.get("category"),
            "conflicts": d.get("conflicts") or [],
            "is_builtin": d.get("is_builtin"),
            "token_estimate": d.get("token_estimate"),
        }
        for d in data
    ]


# ── Agent-library CRUD (skills / profiles / directives) ─────────────────────
# 0.8.8 (2026-06-24) — symmetry with the QP/QA/workflow CRUD cluster. The
# `*s_list` tools only READ; these author + edit + delete the bindings an
# Agent step references. Thin wrappers over the existing REST routes
# (POST/PUT/DELETE /api/{skills,profiles,directives}); update is load-merge-
# write (the bare PUT replaces the full body) and is CUSTOM-only (the backend
# rejects edits to builtin entries). Cf. [[project_mcp_workflow_crud_gap]].
_AGENT_LIB = {
    "skill": {
        "path": "/api/skills",
        "required": ("name", "description", "icon", "category", "content"),
        "optional": ("license", "allowed_tools"),
        "categories": ("Language", "Domain", "Business"),
        # skill update = delete+recreate server-side → the id CHANGES.
        "update_remints_id": True,
    },
    "profile": {
        "path": "/api/profiles",
        "required": ("name", "role", "avatar", "color", "category", "persona_prompt"),
        "optional": ("persona_name", "default_engine"),
        "categories": ("Technical", "Business", "Meta"),
        "update_remints_id": False,
    },
    "directive": {
        "path": "/api/directives",
        "required": ("name", "description", "icon", "category", "content"),
        "optional": ("conflicts",),
        "categories": ("Output", "Language"),
        "update_remints_id": False,
    },
}


def _lib_get(kind, args):
    """Return one full Agent-library item while keeping list tools lean."""
    iid = args.get(f"{kind}_id") or args.get("id")
    if not iid:
        raise RuntimeError(f"{kind}_get: missing required '{kind}_id'")
    items = _unwrap(_http("GET", _AGENT_LIB[kind]["path"])) or []
    existing = next((item for item in items if item.get("id") == iid), None)
    if not existing:
        raise RuntimeError(
            f"{kind}_get: {iid!r} not found — call {kind}s_list to see what exists"
        )
    return existing


def _lib_create(kind, args):
    spec = _AGENT_LIB[kind]
    for f in spec["required"]:
        if args.get(f) in (None, "", []):
            raise RuntimeError(f"{kind}_create: missing required '{f}'")
    cat = args.get("category")
    if cat not in spec["categories"]:
        raise RuntimeError(
            f"{kind}_create: category {cat!r} invalid — one of {list(spec['categories'])}"
        )
    body = {f: args[f] for f in spec["required"]}
    for f in spec["optional"]:
        if f in args:
            body[f] = args[f]
    return _unwrap(_http("POST", spec["path"], body))


def _lib_update(kind, args):
    spec = _AGENT_LIB[kind]
    iid = args.get(f"{kind}_id") or args.get("id")
    if not iid:
        raise RuntimeError(f"{kind}_update: missing required '{kind}_id'")
    items = _unwrap(_http("GET", spec["path"])) or []
    existing = next((x for x in items if x.get("id") == iid), None)
    if not existing:
        raise RuntimeError(
            f"{kind}_update: {iid!r} not found — call {kind}s_list to see what exists"
        )
    body = {}
    for f in spec["required"] + spec["optional"]:
        if f in args:
            body[f] = args[f]
        elif f in existing:
            body[f] = existing[f]
    if body.get("category") not in spec["categories"]:
        raise RuntimeError(
            f"{kind}_update: category {body.get('category')!r} invalid — one of {list(spec['categories'])}"
        )
    return _unwrap(_http("PUT", f"{spec['path']}/{iid}", body))


def _lib_delete(kind, args):
    iid = args.get(f"{kind}_id") or args.get("id")
    if not iid:
        raise RuntimeError(f"{kind}_delete: missing required '{kind}_id'")
    return _unwrap(_http("DELETE", f"{_AGENT_LIB[kind]['path']}/{iid}"))


def call_skill_create(args):
    """Create a custom SKILL (POST /api/skills). Required: name, description,
    icon, category (Language|Domain|Business), content (the markdown body).
    Optional: license, allowed_tools. Use to author a reusable skill an Agent
    step / QP can then bind via `skill_ids`. Returns the created skill (incl id)."""
    return _lib_create("skill", args)


def call_skill_get(args):
    """Return one FULL skill, including its markdown content body."""
    return _lib_get("skill", args)


def call_skill_update(args):
    """Patch a CUSTOM skill (load-merge-write over PUT /api/skills/<id>). Builtin
    skills are rejected by the backend. ⚠ The backend recreates the skill, so
    the id CHANGES — use the `id` in the returned object afterwards."""
    return _lib_update("skill", args)


def call_skill_delete(args):
    """Delete a custom skill by id (builtins are protected)."""
    return _lib_delete("skill", args)


def call_profile_create(args):
    """Create a custom PROFILE / persona (POST /api/profiles). Required: name,
    role, avatar, color, category (Technical|Business|Meta), persona_prompt.
    Optional: persona_name, default_engine. Bind via an Agent step's
    `profile_ids`. Returns the created profile (incl id)."""
    return _lib_create("profile", args)


def call_profile_get(args):
    """Return one FULL profile, including its persona_prompt body."""
    return _lib_get("profile", args)


def call_profile_update(args):
    """Patch a custom profile (load-merge-write over PUT /api/profiles/<id>)."""
    return _lib_update("profile", args)


def call_profile_delete(args):
    """Delete a custom profile by id (builtins are protected)."""
    return _lib_delete("profile", args)


def call_directive_create(args):
    """Create a custom DIRECTIVE (POST /api/directives). Required: name,
    description, icon, category (Output|Language), content. Optional: conflicts
    (list of directive ids it's mutually exclusive with). Bind via an Agent
    step's `directive_ids`. Returns the created directive (incl id)."""
    return _lib_create("directive", args)


def call_directive_get(args):
    """Return one FULL directive, including its content body."""
    return _lib_get("directive", args)


def call_directive_update(args):
    """Patch a custom directive (load-merge-write over PUT /api/directives/<id>)."""
    return _lib_update("directive", args)


def call_directive_delete(args):
    """Delete a custom directive by id (builtins are protected)."""
    return _lib_delete("directive", args)


def call_workflow_step_schema(_args):
    """Canonical WorkflowStep schema, returned as a tool RESULT (untruncatable).

    The `workflow_create_draft` description carries the same info, but some MCP
    clients truncate long tool descriptions mid-text — so the run-breaking bits
    (the SubWorkflow foreach contract in particular) can get cut before the
    agent ever sees them. A tool result is never truncated, so this is the
    authoritative, on-demand source for the step schema."""
    return {
        "shape": (
            "Each step's type-specific fields sit at the TOP LEVEL (never under a "
            "sub-object); `name` is required on every step. BUT `step_type` is a "
            "TAGGED OBJECT `{\"type\":\"Agent\"}`, NOT a bare string (serde "
            "internally-tagged); same for `output_format` (`{\"type\":\"Structured\"}`) "
            "and the workflow `trigger` (`{\"type\":\"Manual\"}`). "
            "`workflow_create_draft`/`workflow_update` also accept a bare-string "
            "`step_type` and wrap it, but the canonical form `workflow_get` returns "
            "is the tagged object."
        ),
        "step_types_closed_set": [
            "Agent",
            "ApiCall",
            "BatchApiCall",
            "BatchQuickPrompt",
            "Exec",
            "Gate",
            "Notify",
            "JsonData",
            "SubWorkflow",
        ],
        "fields_by_type": {
            "Agent": {
                "required": ["agent", "prompt_template"],
                "optional": [
                    "output_format (FreeText | {type:Structured} | {type:TypedSchema, schema:{...}})",
                    "skill_ids",
                    "profile_ids",
                    "directive_ids",
                    "multi_agent_review (bool — second agent debates the output)",
                ],
                "OUTPUT_PIPING": (
                    "With output_format {type:TypedSchema,schema:{...}} (or Structured), the "
                    "agent's emitted JSON is captured as `{{steps.<name>.data}}`, with nested "
                    "access `{{steps.<name>.data.<field>}}` / `{{steps.<name>.data.arr.0.k}}` "
                    "(`data_json.<field>` works identically). THIS is how you feed a "
                    "deterministic ApiCall/Exec step from an LLM step. TYPED INJECTION in an "
                    "api_body: a field whose value is EXACTLY one placeholder is replaced by the "
                    "REAL typed JSON (arrays/objects preserved, not stringified) — write it "
                    "QUOTED as a normal string leaf. E.g. a review step emits "
                    "{verdict, generalComment, inlineComments[]} and the next ApiCall posts "
                    "api_body = {\"event\":\"{{steps.review.data.verdict}}\", "
                    "\"body\":\"{{steps.review.data.generalComment}}\", "
                    "\"comments\":\"{{steps.review.data.inlineComments}}\"} — `comments` arrives "
                    "as a real array. (A placeholder embedded in surrounding text, e.g. "
                    "\"PR #{{n}}\", stays a string.) To run a Quick Prompt's logic in a PIPEABLE "
                    "way, put its `quick_prompt_id` on an Agent step with TypedSchema — NOT "
                    "BatchQuickPrompt (see its note)."
                ),
                "example": {
                    "name": "Triage",
                    "step_type": {"type": "Agent"},
                    "agent": "ClaudeCode",
                    "prompt_template": "Analyse {{previous_step.output}}",
                    "output_format": {"type": "Structured"},
                },
            },
            "ApiCall": {
                "required": ["api_plugin_slug", "api_config_id", "api_endpoint_path"],
                "optional": ["api_method (default GET)", "api_query", "api_body", "api_extract"],
                "note": "plugin_slug + config_id MUST exist in mcp_list. endpoint_path is INDICATIVE — any valid path on the API works; set api_method explicitly for a non-GET on an unlisted path.",
                "example": {
                    "name": "Fetch",
                    "step_type": {"type": "ApiCall"},
                    "api_plugin_slug": "mcp-atlassian",
                    "api_config_id": "<id from mcp_list>",
                    "api_endpoint_path": "/rest/api/2/search",
                    "api_method": "GET",
                    "api_query": {"jql": "..."},
                },
            },
            "Exec": {
                "required": ["exec_command"],
                "optional": [
                    "exec_args",
                    "exec_timeout_secs",
                    "exec_stdin (piped to stdin — use for LARGE input instead of a huge arg, no argv size limit)",
                ],
                "note": "exec_command binary MUST be in the workflow `exec_allowlist`.",
                "example": {
                    "name": "Tests",
                    "step_type": {"type": "Exec"},
                    "exec_command": "make",
                    "exec_args": ["test"],
                    "exec_timeout_secs": 600,
                    "exec_stdin": "{{steps.fetch.data_json}}",
                },
            },
            "Gate": {
                "required": ["gate_message"],
                "optional": [
                    "gate_request_changes_target (step name to loop back to on 'request changes')",
                    "gate_checkpoint_before (auto-commit before the gate)",
                    "gate_auto_approve_secs",
                ],
                "example": {
                    "name": "Validate",
                    "step_type": {"type": "Gate"},
                    "gate_message": "Approve?",
                    "gate_request_changes_target": "Implement",
                },
            },
            "Notify": {
                "required": ["notify_config"],
                "example": {"name": "Done", "step_type": {"type": "Notify"}, "notify_config": {}},
            },
            "BatchQuickPrompt": {
                "required": ["batch_quick_prompt_id", "batch_items_from"],
                "optional": ["batch_wait_for_completion"],
                "OUTPUT_IS_METADATA_ONLY": (
                    "RUN-BREAKING gotcha: `{{steps.<name>.data}}` is batch METADATA only — "
                    "`{batch_run_id, discussion_ids, ok, total}`. The QP's PRODUCED content "
                    "(the table/JSON each run emits) lives in the child DISCUSSIONS, NOT in the "
                    "step data, and there is NO `{{steps.<name>.results[]}}` / `.outputs` "
                    "accessor. So you CANNOT pipe a BatchQuickPrompt's output into a downstream "
                    "ApiCall/Exec. BatchQuickPrompt is FAN-OUT-to-discussions (kick off N runs, "
                    "humans/agents read the threads). If you need the produced result piped "
                    "onward deterministically, use a single Agent step with TypedSchema instead "
                    "(see Agent.OUTPUT_PIPING). An agent CAN still read a child disc via the "
                    "`disc_get_message`/`disc_summarize` MCP tools, but a deterministic step "
                    "cannot."
                ),
                "example": {
                    "name": "Fan out",
                    "step_type": {"type": "BatchQuickPrompt"},
                    "batch_quick_prompt_id": "<qp id>",
                    "batch_items_from": "{{previous_step.data}}",
                    "batch_wait_for_completion": True,
                },
            },
            "BatchApiCall": {
                "required": ["batch_items_from", "api_plugin_slug", "api_config_id", "api_endpoint_path"],
                "optional": ["api_method"],
                "note": "fan one ApiCall over a list, zero tokens.",
                "PER_ITEM_VARS": (
                    "Each item's fields are templatable in api_endpoint_path AND api_body/"
                    "api_query as `{{batch.item.<field>}}` (canonical), `{{item.<field>}}` "
                    "(alias), and bare `{{<field>}}`. So a per-item path works: "
                    "`/comments/{{batch.item.commentId}}/reactions`. Also `{{batch.index}}` "
                    "(0-based) and `{{batch.item}}` (whole item as JSON). NOTE: this is a "
                    "DIFFERENT name from the SubWorkflow-foreach item (`current_task.*`) — "
                    "batch fan-out uses `batch.item.*`/`item.*`."
                ),
                "example": {
                    "name": "Bulk",
                    "step_type": {"type": "BatchApiCall"},
                    "batch_items_from": "{{previous_step.data}}",
                    "api_plugin_slug": "…",
                    "api_config_id": "…",
                    "api_endpoint_path": "/repos/o/r/pulls/comments/{{batch.item.commentId}}/reactions",
                    "api_method": "POST",
                    "api_body": {"content": "{{batch.item.reaction}}"},
                },
            },
            "JsonData": {
                "required": ["json_data_payload"],
                "note": "deterministic data source, zero tokens — feeds {{steps.<name>.data}}.",
                "example": {"name": "Seed", "step_type": {"type": "JsonData"}, "json_data_payload": "[{...}]"},
            },
            "SubWorkflow": {
                "required": ["sub_workflow_id"],
                "optional": ["sub_workflow_foreach_file (workspace-relative JSON array → child runs once per item)"],
                "FOREACH_RUNTIME_CONTRACT": (
                    "RUN-BREAKING. `sub_workflow_foreach_file` is YOUR source list "
                    "(any name, e.g. .kronn/prs.json). Before each child run the "
                    "engine exposes the CURRENT item to the child TWO ways: "
                    "(1) TEMPLATE VARS — each top-level field as `{{current_task.<field>}}` "
                    "(e.g. an ApiCall path `/repos/o/r/pulls/{{current_task.number}}/reviews`, "
                    "a worktree `.kronn/pr-{{current_task.number}}`); scalars stringify, "
                    "null→\"\", nested arrays/objects render as compact JSON, and the whole "
                    "item is `{{current_task}}`. The accessor name is FIXED `current_task.*` "
                    "(it mirrors the file, NOT the source-file name; it is NOT `{{item.*}}` "
                    "or `{{foreach.*}}`). (2) FILE — the same item is written to the FIXED "
                    "path `.kronn/current_task.json` in the shared worktree, for an Agent/Exec "
                    "step that needs the full object. Bookkeeping vars `{{__subwf_item_id__}}` "
                    "(=item `id`) and `{{__subwf_item__}}` (index) are also set."
                ),
                "example": {
                    "name": "Implement",
                    "step_type": {"type": "SubWorkflow"},
                    "sub_workflow_id": "<child workflow id>",
                    "sub_workflow_foreach_file": ".kronn/tasks.json",
                },
            },
        },
        "discovery_rule": (
            "Do NOT infer the available step types from one workflow you opened — "
            "it may use only Agent steps. This 9-set IS the whole taxonomy. For a "
            "rich real example to adapt, workflow_get/workflow_clone the AutoPilot "
            "workflow (multi-step), not a single-Agent one."
        ),
        "template_vars": {
            "syntax": (
                "`{{namespace.path}}` in any string field (prompt_template, "
                "api_endpoint_path, api_query/api_body values, exec_args/exec_stdin, "
                "gate_message, notify_config, …). Dotted nested access works incl. "
                "array index: `{{steps.plan.data.subtasks.0.title}}`. An UNKNOWN ref "
                "is left VERBATIM (not blanked) so a typo is visible at run time "
                "rather than silently empty."
            ),
            "namespaces": {
                "steps.<name>.output": "raw text the step produced (FreeText).",
                "steps.<name>.data": "structured payload (Structured/TypedSchema agent, ApiCall/Exec/JsonData envelope). Nested fields: `steps.<name>.data.<field>` (incl. array index). Strings unwrapped for clean interpolation. In an api_body, a field whose value is EXACTLY one such placeholder is injected as the REAL typed JSON (array/object preserved) — write it quoted, e.g. `\"comments\": \"{{steps.review.data.inlineComments}}\"`.",
                "steps.<name>.data_json": "same payload; `data_json.<field>` resolves identically to `data.<field>` (alias). In a prompt/string it renders verbatim JSON; in an api_body whole-placeholder field it injects typed JSON just like `data`.",
                "steps.<name>.summary / .status": "the envelope summary line / OK|… status.",
                "previous_step.{output,data,data_json,summary,status}": "shorthand for the immediately preceding step.",
                "batch.item.<field> / item.<field> / <field>": "the current item inside a BatchApiCall / BatchQuickPrompt fan-out (templatable in api_endpoint_path + body/query). `{{batch.index}}` = 0-based index, `{{batch.item}}` = whole item JSON.",
                "current_task / current_task.<field>": "the current item inside a SubWorkflow foreach child (DIFFERENT name from batch fan-out's `batch.item.*`; see SubWorkflow.FOREACH_RUNTIME_CONTRACT).",
                "state.<key>": "run state written by a step via a `---STATE:<k>=<v>---` line; persists across Gate pauses + Goto loops.",
                "artifacts.<name>": "content a step emitted via a `---ARTIFACT:<name>---` block.",
                "issue.{title,body,number,url,labels}": "tracker-trigger fields (Cron/Tracker workflows).",
                "<launch_var>": "any manual-launch `variables[].name` is referenced bare as `{{name}}`.",
            },
            "gotcha": (
                "A BatchQuickPrompt's `steps.<name>.data` is batch METADATA only — its "
                "produced content is NOT here (see BatchQuickPrompt.OUTPUT_IS_METADATA_ONLY)."
            ),
        },
    }


# ─── Audit tools (0.8.12 PR A) ─────────────────────────────────────────────
#
# The backend audit endpoints are SSE-DRIVEN: the audit only advances while
# a client reads the stream (there is no detached server-side spawn). The
# bridge therefore consumes the stream in a daemon thread and the launch
# tool returns immediately with a correlation — the documented trade-off is
# that the audit dies with this bridge process (MCP reload = interruption;
# the run is then observable via audit_status and resumable).

_AUDIT_LOCK = threading.Lock()
# project_id -> mutable entry shared between the launcher and its reader
# thread. Public keys are returned by audit_status; keys prefixed `_` are
# internal (response object, start event). All state transitions happen
# under _AUDIT_LOCK.
_AUDIT_STREAMS = {}
_AUDIT_STREAM_MAX_SECONDS = 2 * 60 * 60  # hard bound on one stream read
_AUDIT_START_WAIT_SECONDS = 5
# Terminal entries older than this are purged (PR C — a long-lived bridge
# session auditing many projects must not accumulate dead entries).
_AUDIT_TERMINAL_TTL_SECONDS = 24 * 60 * 60
_AUDIT_TERMINAL_STATES = frozenset({
    "done", "error", "cancelled", "launch_timeout",
    "bridge_timeout", "stream_error", "stream_closed", "protocol_error",
})


def _audit_purge_terminal_entries():
    """Drop terminal entries past their TTL. Called under no lock by the
    tools' entry points — takes _AUDIT_LOCK itself. The freshest terminal
    entry per project survives until the TTL so audit_status keeps its
    bridge-side memory of the last outcome."""
    now = time.monotonic()  # clock-jump-safe — this is a TTL, not a date
    with _AUDIT_LOCK:
        stale = []
        for project_id, e in _AUDIT_STREAMS.items():
            if e.get("state") not in _AUDIT_TERMINAL_STATES:
                continue
            # Terminal entries created OUTSIDE the reader thread (e.g. an
            # open failure before any thread starts) never got the stamp —
            # a `now` default would make their age 0 forever and they'd
            # never purge. Self-heal: stamp at first observation, so the
            # TTL counts from here.
            if "_ended_monotonic" not in e:
                e["_ended_monotonic"] = now
                continue
            if now - e["_ended_monotonic"] > _AUDIT_TERMINAL_TTL_SECONDS:
                stale.append(project_id)
        for project_id in stale:
            del _AUDIT_STREAMS[project_id]


def _audit_entry_public(entry):
    return {k: v for k, v in entry.items() if not k.startswith("_")}


def _audit_handle_event(entry, event_name, payload_raw):
    """Update the shared entry from one SSE event. Payloads are parsed
    leniently — event shapes vary between modes (e.g. the legacy start
    event has no started_at) and must never kill the reader."""
    try:
        payload = json.loads(payload_raw) if payload_raw else {}
    except ValueError:
        payload = {"raw": payload_raw[:200]}
    with _AUDIT_LOCK:
        entry["events_seen"] = entry.get("events_seen", 0) + 1
        if event_name == "accepted":
            # Launch confirmation emitted BEFORE Phase 1 (template install /
            # migration), which can outlast the start-wait on a fresh project.
            # Confirming here means a slow install no longer trips the launch
            # timeout and interrupts a healthy audit (Codex #7). `start` still
            # follows with the step count.
            entry["state"] = "running"
            entry["_saw_accepted"] = True
            if payload.get("audit_run_id"):
                entry["audit_run_id"] = payload["audit_run_id"]
            entry["_start_evt"].set()
        elif event_name == "start":
            entry["state"] = "running"
            entry["_saw_start"] = True
            entry["total_steps"] = payload.get("total_steps")
            # Partial: canonical (resolved) steps — the done partition is
            # defined over this list, not over the raw request.
            if payload.get("requested_steps") is not None:
                entry["requested_steps"] = payload["requested_steps"]
            # started_at may be absent on some modes — keep the local one.
            if payload.get("started_at"):
                entry["started_at"] = payload["started_at"]
            entry["_start_evt"].set()
        elif event_name == "error":
            entry["state"] = "error"
            entry["error"] = (payload.get("error") or payload_raw)[:500]
            entry["_start_evt"].set()
        elif event_name in ("step_done", "step_error", "step_start", "step_unchanged"):
            entry["last_step_event"] = {"event": event_name, **{
                k: payload.get(k)
                for k in ("step", "label", "file", "outcome", "error")
                if k in payload
            }}
            if event_name == "step_error":
                entry["last_error"] = str(payload.get("error"))[:300]
        elif event_name == "warning":
            # Non-terminal (e.g. post-commit baseline write failure) — the
            # stream still ends with a coherent done.
            entry["last_warning"] = str(payload.get("message"))[:300]
        elif event_name == "cancelled":
            entry["state"] = "cancelled"
        elif event_name == "done":
            # Partial: same minimal contract as the UI validator (matrix
            # v2) — an MCP client must never see a terminal `done` the UI
            # would refuse as malformed.
            if entry.get("mode") == "partial":
                reason = _partial_done_violation(entry, payload)
                if reason is not None:
                    entry["state"] = "protocol_error"
                    entry["error"] = f"malformed done event: {reason}"
                    return
            entry["state"] = "done"
            # Matrix v2 partition — exposed so audit_status can explain a
            # `no_change`/`interrupted` refresh without re-reading the DB.
            for k in ("succeeded_steps", "unchanged_steps", "failed_steps"):
                if k in payload:
                    entry[k] = payload[k]
            # `full` AND a fully-successful `partial` yield a validation
            # discussion (partial: scoped to the refreshed sections, since
            # the A5 hardening); an interrupted run does not — expose an
            # explicit null either way (never absent).
            entry["discussion_id"] = payload.get("discussion_id")
            entry["audit_run_id"] = payload.get("audit_run_id")
            entry["done_status"] = payload.get("status")


def _partial_done_violation(entry, payload):
    """Mirror of the frontend's `parsePartialDone` (api.streaming.test.ts
    fixtures are the shared matrix): returns a reason string when the
    terminal payload violates the matrix-v2 contract, else None."""
    status = payload.get("status")
    if status not in ("complete", "interrupted", "no_change"):
        return f"unknown status {status!r}"
    run_id = payload.get("audit_run_id")
    if not isinstance(run_id, str) or not run_id:
        return "missing audit_run_id"
    # `type(x) is int` and not isinstance: Python bools ARE ints
    # (True == 1) and would forge a valid-looking partition the frontend
    # refuses.
    def _is_step_list(v):
        return isinstance(v, list) and all(type(x) is int and x > 0 for x in v)
    lists = {}
    for k in ("succeeded_steps", "unchanged_steps", "failed_steps"):
        v = payload.get(k)
        if not _is_step_list(v):
            return f"{k} is not a step list"
        lists[k] = v
    flat = lists["succeeded_steps"] + lists["unchanged_steps"] + lists["failed_steps"]
    if len(set(flat)) != len(flat):
        return "step lists overlap"
    requested = entry.get("requested_steps")
    if not _is_step_list(requested):
        return "no canonical requested_steps (done before start?)"
    if set(flat) != set(requested) or len(flat) != len(requested):
        return "step lists do not partition the requested steps"
    disc = payload.get("discussion_id")
    if status == "complete":
        if not lists["succeeded_steps"] or lists["failed_steps"]:
            return "complete requires succeeded steps and no failures"
        if not isinstance(disc, str) or not disc:
            return "complete requires a validation discussion"
    elif disc:
        return f"{status} cannot carry a discussion"
    if status == "interrupted" and not lists["failed_steps"]:
        return "interrupted requires failed steps"
    if status == "no_change" and (lists["succeeded_steps"] or lists["failed_steps"]
                                  or not lists["unchanged_steps"]):
        return "no_change requires an all-unchanged partition"
    return None


def _audit_stream_reader(entry):
    """Daemon thread: consume the SSE stream until done/error/EOF, the 2h
    hard bound, or an explicit close from the launcher. Every exit path
    leaves a terminal state and closes the response — no silent death."""
    resp = entry["_resp"]
    event_name = None

    # The 2h bound must hold even on a stream that goes IDLE — `for raw in
    # resp` blocks between bytes, so an in-loop clock check alone would
    # never fire (Copilot review). The watchdog force-closes the response,
    # which unblocks the read; the state is sealed BEFORE the close so the
    # finally below can't misread it as a server-side stream_closed.
    def _watchdog_close():
        with _AUDIT_LOCK:
            if entry["state"] in ("launching", "running"):
                entry["state"] = "bridge_timeout"
                entry["error"] = f"stream exceeded the {_AUDIT_STREAM_MAX_SECONDS}s bridge bound"
        try:
            resp.close()
        except Exception:
            pass

    watchdog = threading.Timer(_AUDIT_STREAM_MAX_SECONDS, _watchdog_close)
    watchdog.daemon = True
    watchdog.start()
    try:
        for raw in resp:
            line = raw.decode("utf-8", errors="replace").rstrip("\r\n")
            if line.startswith("event:"):
                event_name = line[len("event:"):].strip()
            elif line.startswith("data:"):
                _audit_handle_event(entry, event_name, line[len("data:"):].strip())
                with _AUDIT_LOCK:
                    if entry["state"] in ("done", "error", "cancelled", "protocol_error"):
                        break
    except Exception as e:  # noqa: BLE001 — reader must never die silently
        with _AUDIT_LOCK:
            if entry["state"] in ("launching", "running"):
                entry["state"] = "stream_error"
                entry["error"] = str(e)[:300]
        sys.stderr.write(f"[kronn-internal] audit stream reader error: {e}\n")
    finally:
        watchdog.cancel()
        try:
            resp.close()
        except Exception:
            pass
        with _AUDIT_LOCK:
            entry["ended_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            entry["_ended_monotonic"] = time.monotonic()  # purge TTL anchor (PR C)
            if entry["state"] in ("launching", "running"):
                # Server closed the stream without a terminal event (e.g.
                # backend restart) — distinct from done AND from error.
                entry["state"] = "stream_closed"
            entry["_start_evt"].set()  # never leave the launcher hanging


def _audit_open_sse(path, body):
    """Open the SSE POST. No read timeout: audits legitimately stream for
    20-40 min — the 2h bound and the launcher's close() do the policing."""
    url = f"{_backend_url()}{path}"
    req = urllib.request.Request(url, method="POST", data=json.dumps(body).encode())
    req.add_header("Content-Type", "application/json")
    req.add_header("Accept", "text/event-stream")
    token = os.environ.get("KRONN_AUTH_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    return urllib.request.urlopen(req, timeout=None)  # noqa: S310


def _briefing_state(project: dict) -> dict:
    """Filesystem check (same host as the backend): does the project carry a
    pre-audit briefing? Its absence measurably degrades the audit (user-known
    pain points never reach the steps — observed live on docroms-web)."""
    path = (project or {}).get("path") or ""
    for candidate in ("docs/briefing.md", "ai/briefing.md"):
        full = os.path.join(path, candidate)
        if path and os.path.isfile(full):
            return {"present": True, "path": candidate}
    return {
        "present": False,
        "hint": (
            "No pre-audit briefing found — the audit will run without user "
            "context (goals, known pain points). Consider running the "
            "project briefing in the Kronn UI first."
        ),
    }


def call_audit_prepare(args):
    project_id = (args.get("project_id") or "").strip()
    if not project_id:
        raise RuntimeError("audit_prepare: project_id is required")
    # AuditInfo verbatim — files/todos/tech_debt_items, no reshaping.
    info = _unwrap(_http("GET", f"/api/projects/{project_id}/audit-info"))
    # An empty surface is ambiguous: pristine project OR template never
    # installed. Surface the project's audit_status so the agent can tell,
    # and say explicitly what to do when the answer is "no template".
    try:
        project = _unwrap(_http("GET", f"/api/projects/{project_id}"))
        status = project.get("audit_status") if isinstance(project, dict) else None
        if status is not None:
            info["audit_status"] = status
            info["briefing"] = _briefing_state(project)
            if status == "NoTemplate":
                info["hint"] = (
                    "The docs template is NOT installed — files/todos are empty "
                    "because there is nothing to audit yet, not because the "
                    "project is clean. Call `audit_install_template` first."
                )
    except Exception:
        pass  # best-effort enrichment; the verbatim AuditInfo still stands
    return info


_ONBOARD_MARKER = os.path.expanduser("~/.config/kronn/mcp-onboarded.json")


def _onboarding_done_for(client: str) -> bool:
    try:
        with open(_ONBOARD_MARKER) as f:
            return client in json.load(f)
    except Exception:
        return False


def _mark_onboarded(client: str) -> None:
    data = {}
    try:
        with open(_ONBOARD_MARKER) as f:
            data = json.load(f)
    except Exception:
        pass
    data[client] = time.strftime("%Y-%m-%d")
    os.makedirs(os.path.dirname(_ONBOARD_MARKER), exist_ok=True)
    with open(_ONBOARD_MARKER, "w") as f:
        json.dump(data, f)


def call_kronn_intro(_args):
    client = (_CLIENT_INFO.get("name") or "unknown").strip() or "unknown"
    _mark_onboarded(client)
    return {
        "guide": (
            "# Kronn en 2 minutes — ce que tu peux faire d'ici, sans quitter ton terminal\n\n"
            "Kronn est ton orchestrateur d'agents AI self-hosted : il garde la mémoire, "
            "les pipelines et les credentials — et moi (ton CLI) je peux tout piloter.\n\n"
            "## 💬 Discussions sauvegardées — ta mémoire partagée\n"
            "Chaque conversation vit dans Kronn, cherchable et rechargeable par N'IMPORTE quel agent.\n"
            "→ 'Retrouve ce qu'on a décidé sur l'auth le mois dernier' (disc_search + disc_load_other)\n"
            "→ 'Crée une disc pour ce sujet et notes-y nos conclusions' (disc_create + disc_append)\n\n"
            "## 🤝 Mode join — plusieurs CLI agents dans la MÊME conversation\n"
            "Ton Claude Code, un Codex, un Gemini : tous peuvent rejoindre la même room et se répondre "
            "(même depuis deux machines différentes).\n"
            "→ 'Rejoins la disc X et attends les messages' (disc_join + disc_wait_for_peer)\n"
            "→ 'Invite Codex sur cette discussion pour un second avis' (disc_invite_peer)\n\n"
            "## ⚡ Quick Prompts — tes prompts transformés en produits réutilisables\n"
            "Un QP = un template avec variables, versionné, lançable à l'unité, en batch sur N tickets, "
            "ou sur PLUSIEURS AGENTS EN PARALLÈLE (mode compare : le même prompt sur Claude + GPT + "
            "Gemini, une discussion par agent, tu compares).\n"
            "→ 'Lance le QP triage sur les tickets EW-1 à EW-20' (qp_batch_run)\n"
            "→ Et ils S'AMÉLIORENT : quand une session aboutit à un meilleur prompt, je peux proposer "
            "la mise à jour du QP — toi tu valides.\n\n"
            "## 🔀 Workflows — des pipelines multi-étapes que tu crées en discutant\n"
            "Agents, appels API, conditions, boucles, gates d'approbation humaine, batchs — jusqu'à 20 steps.\n"
            "→ 'Crée un workflow : récupère les PRs ouvertes, review chacune, poste un résumé' "
            "(workflow_create_draft — je connais le schéma canonique des 9 types de steps)\n"
            "→ 'Lance le PR-review sur la 123' (workflow_trigger) · 'Qu'est-ce qui tourne ?' (workflow_active_runs)\n\n"
            "## 🌐 N'importe quelle API configurée — SANS toucher un secret\n"
            "Jira, Chartbeat, Cloudflare, GitHub… Kronn détient les credentials côté serveur et signe "
            "les appels pour moi.\n"
            "→ 'Combien de tickets ouverts sur le projet EW ?' (mcp_list → api_call, auth injectée)\n"
            "→ Un appel que tu referas ? Je le sauvegarde en Quick API rejouable (qa_create_draft).\n\n"
            "## 🧠 La désagentification (LE concept clé pour bien commencer)\n"
            "Un agent LLM qui fait un appel HTTP brûle des tokens pour RIEN : la requête est "
            "déterministe. Kronn exécute donc les steps mécaniques (API, extraction JSON, notifications) "
            "en Rust pur — ZÉRO token — et réserve les agents aux steps qui demandent du raisonnement. "
            "Même pipeline, ~5x moins cher, débogable step par step. Le réflexe à prendre : "
            "'ce step a-t-il besoin de réfléchir ?' Sinon → ApiCall/Exec/JSON, pas un agent.\n\n"
            "## 🔍 Audits — rends n'importe quel repo AI-ready\n"
            "16 étapes chaînées : docs complètes (architecture, conventions, glossaire…) puis sécurité, "
            "docker, perf, a11y, database, API, qualité de code — chaque dimension passe ou dit "
            "'non applicable'. À la fin : une discussion de validation où TU confirmes la dette "
            "trouvée. Ensuite, n'importe quel agent (même sans Kronn) comprend le projet en lisant docs/.\n"
            "→ 'Prépare l'audit de <projet>' (audit_prepare) puis 'lance-le' (audit_launch)\n\n"
            "## 🚀 Cinq trucs à essayer maintenant\n"
            "1. 'Qu'est-ce qui tourne en ce moment ?'\n"
            "2. 'Liste mes Quick Prompts et explique-moi le plus utilisé'\n"
            "3. 'Résume la dernière discussion sur <projet>'\n"
            "4. 'Crée un petit workflow qui checke <API> chaque matin et me notifie'\n"
            "5. 'Prépare l'audit de <projet> et dis-moi ce qui manque'\n\n"
            "**Envie de creuser un domaine ?** Demande — je détaille avec des exemples réels de TON instance.\n\n"
            "⚠️ **Secrets & credentials** : configuration UNIQUEMENT dans l'UI (Config → Tokens / "
            "Plugins) — jamais dans ce chat, jamais en clair. L'UI sert aussi pour le visuel (rooms, "
            "batchs, validation d'audit) : ouvre l'app Kronn (ou le serveur de dev http://localhost:5173 "
            "si tu lances Kronn depuis les sources)."
        ),
        "onboarding_marked_done_for": client,
    }


def call_bridge_info(_args):
    try:
        mtime_now = os.path.getmtime(__file__)
    except OSError:
        mtime_now = 0.0
    return {
        "script_path": os.path.abspath(__file__),
        "loaded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(_BRIDGE_LOADED_AT)),
        "script_mtime_at_load": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(_BRIDGE_SCRIPT_MTIME_AT_LOAD)),
        "script_mtime_now": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(mtime_now)),
        "stale": mtime_now > _BRIDGE_SCRIPT_MTIME_AT_LOAD + 1.0,
        "hint": (
            "stale=true: the on-disk bridge is newer than this process — ask "
            "the user to reconnect the MCP before launching session-bound work."
        ),
    }


def call_audit_install_template(args):
    project_id = (args.get("project_id") or "").strip()
    if not project_id:
        raise RuntimeError("audit_install_template: project_id is required")
    status = _unwrap(_http("POST", f"/api/projects/{project_id}/install-template"))
    return {"project_id": project_id, "audit_status": status}


def call_audit_launch(args):
    _audit_purge_terminal_entries()
    project_id = (args.get("project_id") or "").strip()
    mode = (args.get("mode") or "").strip()
    if not project_id:
        raise RuntimeError("audit_launch: project_id is required")
    if mode not in ("full", "partial"):
        raise RuntimeError("audit_launch: mode must be 'full' or 'partial'")
    steps = args.get("steps")
    if mode == "partial":
        if not isinstance(steps, list) or not steps or not all(
            isinstance(s, int) and s >= 1 for s in steps
        ):
            raise RuntimeError(
                "audit_launch: partial mode requires a non-empty `steps` list "
                "of 1-based integers — refused before any backend call"
            )
    resume_run_id = args.get("resume_run_id")
    if resume_run_id is not None and (not isinstance(resume_run_id, str) or not resume_run_id.strip()):
        # Validated BEFORE the stream entry exists — a raise past that point
        # would leave a phantom "launching" entry blocking future launches.
        # The backend derives kind + checkpoint from the run id, so all the
        # bridge must guarantee is a non-empty string.
        raise RuntimeError("audit_launch: resume_run_id must be a non-empty string")
    # Blank/whitespace agent falls back like an absent one — never forward
    # an empty attribution to the backend.
    agent = (args.get("agent") or "").strip() or _agent_type_for_session()

    started_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    # check-then-launch is atomic under the lock: two local calls can't
    # both open a stream and race the backend's own concurrency refusal.
    with _AUDIT_LOCK:
        existing = _AUDIT_STREAMS.get(project_id)
        if existing and existing["state"] in ("launching", "running"):
            raise RuntimeError(
                f"audit_launch: an audit for {project_id} is already being "
                "driven by THIS bridge — one at a time. audit_status to watch it."
            )
        entry = {
            "project_id": project_id,
            "mode": mode,
            "state": "launching",
            "started_at": started_at,
            "events_seen": 0,
            "_start_evt": threading.Event(),
        }
        _AUDIT_STREAMS[project_id] = entry

    if mode == "full":
        path = f"/api/projects/{project_id}/full-audit"
        body = {"agent": agent}
        if resume_run_id:
            body["resume_run_id"] = resume_run_id.strip()
    else:
        path = f"/api/projects/{project_id}/partial-audit"
        body = {"agent": agent, "steps": steps}

    try:
        resp = _audit_open_sse(path, body)
    except Exception as e:
        with _AUDIT_LOCK:
            entry["state"] = "error"
            entry["error"] = str(e)[:300]
        raise RuntimeError(f"audit_launch: could not open the audit stream: {e}")

    # Under the lock like every other entry mutation — audit_status
    # iterates this dict under _AUDIT_LOCK and a bare assignment here
    # could race it (dict resize during iteration).
    with _AUDIT_LOCK:
        entry["_resp"] = resp
    threading.Thread(
        target=_audit_stream_reader, args=(entry,),
        name=f"audit-sse-{project_id[:8]}", daemon=True,
    ).start()

    # Wait ONLY for the launch verdict: `start` or an early error event.
    if not entry["_start_evt"].wait(_AUDIT_START_WAIT_SECONDS):
        with _AUDIT_LOCK:
            entry["state"] = "launch_timeout"
            entry["error"] = f"no start/error event within {_AUDIT_START_WAIT_SECONDS}s"
        try:
            resp.close()  # unblocks the reader; its finally seals the entry
        except Exception:
            pass
        raise RuntimeError(
            "audit_launch: the backend sent no start/error event within "
            f"{_AUDIT_START_WAIT_SECONDS}s — launch NOT confirmed, stream closed. "
            "Check audit_status / backend logs before retrying."
        )
    with _AUDIT_LOCK:
        if entry["state"] == "error":
            raise RuntimeError(f"audit_launch refused: {entry.get('error')}")
        if not (entry.get("_saw_accepted") or entry.get("_saw_start")):
            # The event fired without an accepted/start (stream ended / early
            # close): launch NOT confirmed — never a hollow `launched`.
            raise RuntimeError(
                "audit_launch: the stream closed before any accepted/start "
                f"event — launch NOT confirmed (state: {entry['state']}). Check "
                "audit_status / backend logs before retrying."
            )
        # Briefing presence — best-effort: a warning, never a blocker.
    briefing = None
    try:
        project = _unwrap(_http("GET", f"/api/projects/{project_id}"))
        briefing = _briefing_state(project if isinstance(project, dict) else {})
    except Exception:
        pass
    out = {
            "launched": True,
            "project_id": project_id,
            "mode": mode,
            "started_at": entry.get("started_at", started_at),
            "total_steps": entry.get("total_steps"),
            "lifecycle_warning": (
                "This audit lives only as long as THIS MCP session: a reload "
                "or CLI exit interrupts it mid-flight. The run_id and the "
                "validation discussion_id (full, and fully-successful "
                "partial — scoped to the refreshed sections) become "
                "available via audit_status once done. An interrupted full/"
                "specialized run shows under audit_status.resumable; an "
                "interrupted PARTIAL does not — relaunch it on its "
                "still-stale scope."
            ),
        }
    if briefing and not briefing.get("present"):
        out["briefing_warning"] = briefing["hint"]
    return out


def call_audit_status(args):
    _audit_purge_terminal_entries()
    project_id = (args.get("project_id") or "").strip()
    if not project_id:
        raise RuntimeError("audit_status: project_id is required")
    with _AUDIT_LOCK:
        entry = _AUDIT_STREAMS.get(project_id)
        bridge_stream = _audit_entry_public(entry) if entry else None

    live = _unwrap(_http("GET", f"/api/projects/{project_id}/audit-status"))
    out = {
        "bridge_stream": bridge_stream,
        "live": live,
        "latest": None,
        "resumable": None,
        "note": None,
    }
    if live is None:
        # `live: null` = no LIVE state known — NOT "finished". Fall back to
        # DB history so the caller can tell done/interrupted/never-ran apart.
        out["latest"] = _unwrap(_http("GET", f"/api/projects/{project_id}/audit-latest"))
        out["resumable"] = _unwrap(_http("GET", f"/api/projects/{project_id}/audit-resumable"))
        out["note"] = (
            "live=null means the backend tracker has no LIVE entry (idle OR "
            "tracker wiped by a backend restart) — it never means 'completed'. "
            "`latest` is the last terminal run from the DB, `resumable` the "
            "last Interrupted-but-resumable one."
        )
    return out


DISPATCH = {
    # 0.8.12 PR A — audit surface
    "audit_prepare": call_audit_prepare,
    "audit_install_template": call_audit_install_template,
    "bridge_info": call_bridge_info,
    "kronn_intro": call_kronn_intro,
    "audit_launch": call_audit_launch,
    "audit_status": call_audit_status,
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
    "disc_list": call_disc_list,
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
    "workflow_active_runs": call_workflow_active_runs,
    # 0.8.8 (2026-06-25) — run HISTORY + per-run detail + cancel. The MCP only
    # exposed active runs / latest run; an agent debriefing a cron couldn't
    # enumerate past runs or their foreach children, and couldn't stop a
    # duplicate/overlapping run. Thin wrappers over existing REST routes.
    "workflow_runs": call_workflow_runs,
    "workflow_run_get": call_workflow_run_get,
    "workflow_cancel_run": call_workflow_cancel_run,
    "workflow_resume_run": call_workflow_resume_run,
    "qp_list": call_qp_list,
    "qa_list": call_qa_list,
    "mcp_list": call_mcp_list,
    # 0.8.7 — fetch a Kronn doc convention spec on demand (cheap if not
    # called; lets agents about to author AGENTS.md sections pull the
    # canonical [src:] grammar instead of guessing from training-data).
    "convention_get": call_convention_get,
    # 0.8.5 — autonomous draft creation. Both default to a safe state
    # (workflow disabled / QP manually launched) so a misfire can't
    # cascade into prod cron.
    "workflow_create_draft": call_workflow_create_draft,
    "qp_create_draft": call_qp_create_draft,
    # 0.8.8 (2026-06-23) — read · clone · update · enable for WF + QP.
    # Closes the gap where agents could only CREATE drafts, forcing them
    # to reverse-engineer the step schema from 422s and orphan QPs on
    # every edit. Thin wrappers over existing REST routes. Cf.
    # [[project_mcp_workflow_crud_gap]].
    "workflow_get": call_workflow_get,
    "workflow_clone": call_workflow_clone,
    "workflow_update": call_workflow_update,
    "workflow_set_enabled": call_workflow_set_enabled,
    "qp_update": call_qp_update,
    "qp_get": call_qp_get,
    "qp_delete": call_qp_delete,
    # 0.8.8 (2026-06-24) — canonical step schema as an untruncatable tool
    # RESULT. Closes the gap where the create_draft description (the only
    # schema doc) gets client-truncated mid-text, hiding the SubWorkflow
    # foreach runtime contract. Cf. [[project_mcp_workflow_crud_gap]].
    "workflow_step_schema": call_workflow_step_schema,
    # 0.8.8 (2026-06-24) — enumerate the Agent-step bindings (skill_ids /
    # profile_ids / directive_ids). Before this the create_draft desc said
    # "see the workflow-architect skill for the canonical lists" but the
    # agent had no way to READ them → guessed ids or asked the user.
    "skills_list": call_skills_list,
    "profiles_list": call_profiles_list,
    "directives_list": call_directives_list,
    "skill_get": call_skill_get,
    "profile_get": call_profile_get,
    "directive_get": call_directive_get,
    # 0.8.8 (2026-06-24) — author/edit/delete the Agent-step bindings, closing
    # the loop so an agent can retain · retrieve · evaluate · modify skills (+
    # profiles/directives), not just read them. Custom-only edits.
    "skill_create": call_skill_create,
    "skill_update": call_skill_update,
    "skill_delete": call_skill_delete,
    "profile_create": call_profile_create,
    "profile_update": call_profile_update,
    "profile_delete": call_profile_delete,
    "directive_create": call_directive_create,
    "directive_update": call_directive_update,
    "directive_delete": call_directive_delete,
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
    # 0.8.7 phase 4 PR2/PR3 — batch fan-out, child-disc listing, long-poll
    # wait. Completes the mobile remote-control surface.
    "qp_batch_run": call_qp_batch_run,
    "workflow_run_discussions": call_workflow_run_discussions,
    "workflow_wait_for_completion": call_workflow_wait_for_completion,
    # 0.8.6 phase 4 — synchronous QA execution. The deagentified twin
    # of `api_call` : same end-result, zero token cost on request
    # construction. Always prefer when a matching QA exists.
    "qa_run": call_qa_run,
    # 0.9.0 — Continual Learning. Propose a durable learning (typed, evidence
    # mandatory). Server gates it (existence + faithfulness) + a human validates
    # before it's ever written to a truth file. Free-form fences are NOT used.
    "learning_propose": call_learning_propose,
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
        client_name = (_CLIENT_INFO.get("name") or "unknown").strip() or "unknown"
        first_contact = "" if _onboarding_done_for(client_name) else (
            "🎉 **FIRST CONTACT** — this is the first Kronn session for this "
            "CLI on this machine. Once the user's immediate request is "
            "handled, offer ONCE, in the user's language: \"Je vois que "
            "Kronn vient d'être connecté — veux-tu un tour rapide de ce que "
            "je peux faire avec ?\" If they accept, call `kronn_intro` and "
            "present its guide conversationally (do not paste it raw). "
            "Accepted or declined, call `kronn_intro` afterwards anyway so "
            "the offer is never repeated.\n\n"
        )
        return {
            "jsonrpc": "2.0",
            "id": rid,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "kronn-internal", "version": "0.1.0"},
                # Top-level orientation the client surfaces to the model: WHAT
                # Kronn is + a MAP of the tool surface by area + how to navigate
                # it, so an agent doesn't have to reverse-engineer capabilities
                # from 40+ tool descriptions (and doesn't generalise the
                # system's abilities from one sample it happened to open — the
                # `workflow_get`-only-saw-Agent-steps trap). Kept concise: a
                # CLOSED map + pointers, not a manual (open catalogues stay
                # behind on-demand tools like `mcp_list`).
                "instructions": first_contact + (
                    "You're connected to **Kronn** — it orchestrates agents, "
                    "discussions, multi-step workflows and external APIs, and "
                    "owns ALL credentials server-side (never paste secrets). "
                    "Your tools, by area:\n"
                    "• Discussions (multi-agent threads): `disc_meta`/`disc_get_message`/`disc_search`/`disc_load_other`/`disc_create`/`disc_append`/`disc_join`/`disc_invite_peer`…\n"
                    "• Workflows (multi-step pipelines): `workflow_list` (compact) · `workflow_get` (FULL, every step) · `workflow_step_schema` (CANONICAL step schema as an untruncatable result — the closed 9 `step_type`s, per-type fields, runtime contracts; call before authoring) · `workflow_create_draft` · `workflow_clone`/`workflow_update`/`workflow_set_enabled` · `workflow_trigger`/`workflow_run_status` · run history `workflow_runs`/`workflow_run_get` · `workflow_active_runs`/`workflow_cancel_run`. Agent-step bindings (full CRUD): `skills_list`/`profiles_list`/`directives_list` enumerate valid ids; `skill_get`/`profile_get`/`directive_get` read FULL bodies; `skill_create`/`skill_update`/`skill_delete` (+ `profile_*`/`directive_*`) author & edit custom ones.\n"
                    "• Quick Prompts (reusable prompt templates): `qp_list` (no body) · `qp_get` (FULL incl `prompt_template` — read this to know what a QP does, or to run it yourself) · `qp_create_draft`/`qp_update`/`qp_delete` · `qp_run`/`qp_batch_run`.\n"
                    "• Quick APIs + API broker: `qa_list`/`qa_run`/`qa_create_draft`/`qa_update` · `mcp_list` → `api_call` (configured plugins, auth injected).\n"
                    "• Docs/conventions: `convention_get`. Continual learning: `learning_propose`.\n"
                    "**Navigation rule:** to understand a CAPABILITY, read the relevant tool's description AND `*_get` a REAL, rich example — never infer what the system can do from a single workflow/QP you happened to open.\n\n"
                    "**API actions — order to avoid burning tokens:** "
                    "1) REUSE: `qa_list` → matching saved Quick API? run it via `qa_run`. "
                    "2) CONSTRUCT: else `mcp_list` → `api_call` (never re-specify endpoints from memory; never paste creds). "
                    "3) PERSIST: after a hand-built call the user will rerun, propose `qa_create_draft`. "
                    "Same for prompts/workflows: prefer existing `qp_list`/`workflow_list` entries over rebuilding."
                ),
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
