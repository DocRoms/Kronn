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
import hmac
import importlib.util
import json
import os
import re
import secrets
import subprocess
import sys
import queue
import select
import stat
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


MAX_DISC_APPEND_ATTACHMENTS = 8
MAX_DISC_APPEND_ATTACHMENT_BYTES = 10 * 1024 * 1024
BRIDGE_TOOL_SURFACE_VERSION = "0.3.9"


class BridgeStaleError(RuntimeError):
    """Typed fail-closed signal used to schedule one transparent self-reload."""


_BRIDGE_RELOAD_STATE = {"status": "idle", "error": None}
_BRIDGE_RELOAD_READY_ENV = "_KRONN_MCP_RELOADED"
_BRIDGE_RELOAD_HANDOFF_ENV = "_KRONN_MCP_RELOAD_HANDOFF"
_BRIDGE_RELOAD_HANDOFF_FD_ENV = "_KRONN_MCP_RELOAD_HANDOFF_FD"
_BRIDGE_RELOAD_HANDOFF_NONCE_ENV = "_KRONN_MCP_RELOAD_HANDOFF_NONCE"
_BRIDGE_PREFLIGHT_ENV = "_KRONN_MCP_PREFLIGHT"
_BRIDGE_SOURCE_ENV = "_KRONN_MCP_SOURCE"
_BRIDGE_ARTIFACT_FD_ENV = "_KRONN_MCP_ARTIFACT_FD"
_BRIDGE_ARTIFACT_SHA_ENV = "_KRONN_MCP_ARTIFACT_SHA256"
_BRIDGE_HANDOFF_VERSION = 3
_BRIDGE_HANDOFF_MAX_BYTES = 1024 * 1024
_BRIDGE_PENDING_MAX_BYTES = 256 * 1024
_BRIDGE_SOURCE_PATH = os.environ.get(_BRIDGE_SOURCE_ENV) or os.path.abspath(__file__)
_BRIDGE_ARTIFACT_FD = None


# ─── Tool catalogue ────────────────────────────────────────────────────────

# Loaded-vs-on-disk staleness capture: the MCP client spawns this script at
# session start and never reloads it — a release can leave every live session
# running an outdated bridge with no visible signal (tools missing, stale
# descriptions). bridge_info compares these against the file's current mtime.
def _bridge_script_snapshot():
    """Return the on-disk (mtime, sha256), or an unverifiable sentinel.

    mtime keeps bridge_info human-readable and preserves its historical signal;
    the digest is authoritative so a rollback or a timestamp-preserving rewrite
    cannot make a different tool contract look fresh.
    """
    try:
        mtime = os.path.getmtime(_BRIDGE_SOURCE_PATH)
        with open(_BRIDGE_SOURCE_PATH, "rb") as script:
            digest = hashlib.sha256(script.read()).hexdigest()
        return mtime, digest
    except OSError:
        return 0.0, None


_BRIDGE_LOADED_AT = time.time()
_BRIDGE_SCRIPT_MTIME_AT_LOAD, _source_sha256_at_load = _bridge_script_snapshot()
_BRIDGE_SCRIPT_SHA256_AT_LOAD = (
    os.environ.get(_BRIDGE_ARTIFACT_SHA_ENV) or _source_sha256_at_load
)

TOOLS = [
    {
        "name": "kronn_intro",
        "description": (
            "0.9.0 — A 60-second guided tour of what the user can do with "
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
        "name": "tool_manual",
        "description": (
            "On-demand authoring guide for one Kronn tool. Call it when that "
            "tool's description points here; omit `tool` to list manuals."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "tool": {
                    "type": "string",
                    "description": "Tool name.",
                }
            },
        },
    },
    {
        "name": "bridge_info",
        "description": (
            "Report this bridge's loaded/on-disk identity and reload state. "
            "`stale: true` makes guarded mutations fail closed and schedule one "
            "transparent reload; retry once with the same idempotency key, or "
            "reconnect when reload failed."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "resolve_id",
        "description": (
            "Resolve any public MCP-addressable Kronn object id in one request. "
            "Returns compact type, reference/title/summary, parent context and "
            "the canonical reading tool; unknown or colliding ids fail explicitly. "
            "Use FIRST when the user pastes an id without naming its type."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Full opaque Kronn object id.",
                },
            },
            "required": ["id"],
        },
    },
    {
        "name": "disc_meta",
        "description": (
            "Return metadata about the current discussion (message_count, "
            "agent, tier, has_cached_summary, msgs_since_last_summary, "
            "summary_strategy, language, project_id), plus `addressable`: the "
            "exact @mention for every identity reachable in this room, each with "
            "its kind (`discussion_agent` = the room's own agent, `cli` = one "
            "joined session). `ambiguous_aliases` names the providers present as "
            "BOTH — for those, a bare @alias is refused, so read this BEFORE "
            "writing a mention rather than discovering it in a refusal. Call this "
            "FIRST when you need to decide whether to fetch context. Cheap."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "disc_get_message",
        "description": (
            "Return one message by either 0-indexed position (`idx`) or its "
            "copyable `MSG-xxxxxxxx` / full UUID (`message_id`). Negative idx "
            "counts from the end (-1 = last). Optional `before` / `after` "
            "return a bounded surrounding window (maximum 10 each). Replies "
            "expose their durable `reply_to_message_id`; locally-authored CLI "
            "messages also expose `reply_target`, the exact joined session to "
            "answer without guessing from the provider name. Use this "
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
        "name": "disc_note_list",
        "description": (
            "List out-of-context notes for the current discussion explicitly. "
            "Notes are visible in the human timeline but excluded from ordinary "
            "agent history, search, summaries and wait delivery. Bounded and "
            "cursor-paginated; returns message-equivalent metadata."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "cursor": {
                    "type": "integer",
                    "description": "Last note sort_order already read. Omit for the first page.",
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 50,
                },
            },
            "required": [],
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
                "include_notes": {
                    "type": "boolean",
                    "description": "Explicit audit opt-in: include out-of-context notes. Default false.",
                    "default": False,
                },
            },
            "required": [],
        },
    },
    # ─── 0.9.1 planning / discussion plans ─────────────────────────────
    {
        "name": "plan_get",
        "description": (
            "Return the compact structured Discussion plan (French UI: "
            "'Plan de discussion') for a discussion: primary objective, "
            "active tasks, later tasks and progress. Defaults to the current "
            "discussion. Call this FIRST when the user asks to read or update "
            "the discussion plan; that phrase means Kronn's shared task plan, "
            "not a prose/Markdown summary."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "discussion_id": {
                    "type": "string",
                    "description": "Discussion UUID. Omit for the current discussion.",
                },
            },
        },
    },
    {
        "name": "task_list",
        "description": (
            "List compact planning-task summaries with bounded pagination. "
            "Returns no Markdown description, DoD body, links or event log; "
            "call task_get only for the task you need. Filters: search, "
            "status, priority, project_id, discussion_id, tag, with_discussion."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "search": {"type": "string"},
                "status": {
                    "type": "string",
                    "enum": ["idea", "todo", "in_progress", "blocked", "done", "archived"],
                },
                "priority": {
                    "type": "string",
                    "enum": ["critical", "high", "normal", "low"],
                },
                "project_id": {"type": "string"},
                "discussion_id": {"type": "string"},
                "tag": {"type": "string"},
                "with_discussion": {"type": "boolean"},
                "cursor": {"type": "integer", "minimum": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 50},
            },
        },
    },
    {
        "name": "task_get",
        "description": (
            "Return one FULL planning task including description, Definition "
            "of Done, links, blockers/backlinks and attributed event history. "
            "Accepts the copyable KT-142 reference or the UUID."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"task_id": {"type": "string"}},
            "required": ["task_id"],
        },
    },
    {
        "name": "proposal_list",
        "description": (
            "List durable Planning PROPOSALS (kronn-plan-action fences) awaiting "
            "human validation in a discussion, with per-item states and pending "
            "counters. READ-ONLY: agents propose, only a human accepts/rejects — "
            "no acceptance tool is exposed to agents. Defaults to the current "
            "discussion; pending_only defaults to true."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "discussion_id": {"type": "string"},
                "pending_only": {"type": "boolean", "default": True},
            },
        },
    },
    {
        "name": "proposal_get",
        "description": (
            "Return one FULL Planning proposal with every item: action, state "
            "(pending/accepted/rejected), rejection reason and the task created "
            "or updated on acceptance. Read-only."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"proposal_id": {"type": "string"}},
            "required": ["proposal_id"],
        },
    },
    {
        "name": "task_changes",
        "description": (
            "Return at most 200 attributed task events for tasks linked to a "
            "discussion after an RFC3339 timestamp. Defaults to the current "
            "discussion. Use for delta refreshes; do not reload every task."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "discussion_id": {"type": "string"},
                "since": {
                    "type": "string",
                    "description": "Exclusive RFC3339 timestamp; omitted means all retained events.",
                },
            },
        },
    },
    {
        "name": "task_create",
        "description": (
            "Create one planning task. Keep quick creation compact: title is "
            "required; status defaults to idea and priority to normal. The "
            "new task is linked to the current discussion by default; pass "
            "`discussion_id` to target another existing discussion. An "
            "explicit target works even when this runtime has no bound "
            "discussion or reports `rejoin_required`. The "
            "bridge records this MCP client's agent identity in the event log. "
            "Immediately before a direct create, call `plan_get` again so a "
            "peer's recent write is visible. Use direct writes only when the "
            "user's intent is unambiguous. Pass `idempotency_key` for a stable "
            "retry identity; when it is omitted, `source_message_id` derives "
            "one. Multiple tasks from one message need distinct explicit keys. "
            "Titles are never identities."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "discussion_id": {
                    "type": "string",
                    "description": "Existing discussion to receive the task. Defaults to the discussion bound to this MCP runtime.",
                },
                "idempotency_key": {
                    "type": "string",
                    "description": "Stable caller key for this one logical create; scoped to the effective target discussion by the bridge.",
                },
                "description": {"type": "string"},
                "status": {
                    "type": "string",
                    "enum": ["idea", "todo", "in_progress", "blocked", "done", "archived"],
                },
                "priority": {
                    "type": "string",
                    "enum": ["critical", "high", "normal", "low"],
                },
                "parent_id": {"type": "string"},
                "project_ids": {"type": "array", "items": {"type": "string"}},
                "tags": {"type": "array", "items": {"type": "string"}},
                "definition_of_done": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "sentence": {"type": "string"},
                            "completed": {"type": "boolean"},
                        },
                        "required": ["sentence"],
                    },
                },
                "links": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": {"type": "string"},
                            "url": {"type": "string"},
                        },
                        "required": ["label", "url"],
                    },
                },
                "source_message_id": {"type": "string"},
            },
            "required": ["title"],
        },
    },
    {
        "name": "task_update",
        "description": (
            "Patch one planning task by KT reference or UUID. Only supplied "
            "fields change. Set parent_id or blocked_reason to null to clear "
            "it. Full-array fields (projects, tags, DoD, links) replace their "
            "current collection."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "title": {"type": "string"},
                "description": {"type": "string"},
                "status": {
                    "type": "string",
                    "enum": ["idea", "todo", "in_progress", "blocked", "done", "archived"],
                },
                "priority": {
                    "type": "string",
                    "enum": ["critical", "high", "normal", "low"],
                },
                "parent_id": {"type": ["string", "null"]},
                "blocked_reason": {"type": ["string", "null"]},
                "rank": {"type": "integer"},
                "project_ids": {"type": "array", "items": {"type": "string"}},
                "tags": {"type": "array", "items": {"type": "string"}},
                "definition_of_done": {"type": "array", "items": {"type": "object"}},
                "links": {"type": "array", "items": {"type": "object"}},
                "source_message_id": {"type": "string"},
            },
            "required": ["task_id"],
        },
    },
    {
        "name": "task_link_discussion",
        "description": (
            "Link a task to one discussion as active or later, optionally as "
            "its single primary objective. Defaults to the current discussion. "
            "Use after task_create when the user asks to add work to the "
            "Discussion plan."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "discussion_id": {"type": "string"},
                "placement": {"type": "string", "enum": ["active", "later"], "default": "active"},
                "is_primary": {"type": "boolean", "default": False},
                "position": {"type": "integer"},
                "source_message_id": {"type": "string"},
            },
            "required": ["task_id"],
        },
    },
    {
        "name": "task_update_dod",
        "description": (
            "Check or uncheck one Definition of Done item atomically. Use the "
            "DoD item id returned by task_get. Prefer this over replacing the "
            "whole definition_of_done array when only completion changed: two "
            "agents can update different items without overwriting each other."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "dod_id": {"type": "string"},
                "completed": {"type": "boolean"},
                "source_message_id": {"type": "string"},
            },
            "required": ["task_id", "dod_id", "completed"],
        },
    },
    {
        "name": "task_add_blocker",
        "description": (
            "Declare that task_id is blocked by blocker_task_id. Both accept "
            "KT references or UUIDs. Cross-project dependencies are allowed; "
            "cycles are rejected."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "blocker_task_id": {"type": "string"},
                "source_message_id": {"type": "string"},
            },
            "required": ["task_id", "blocker_task_id"],
        },
    },
    {
        "name": "task_remove_blocker",
        "description": (
            "Remove one dependency edge. KT references or UUIDs accepted. "
            "Statuses and blocked_reason stay unchanged; safe to retry."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_id": {"type": "string"},
                "blocker_task_id": {"type": "string"},
                "source_message_id": {"type": "string"},
            },
            "required": ["task_id", "blocker_task_id"],
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
                "no_agent": {"type": "boolean", "description": "Disable the native principal so only explicitly joined peers answer. Defaults to false for this low-level import tool; disc_create_room sets it to true."},
            },
            "required": ["title", "agent"],
        },
    },
    {
        "name": "disc_append",
        "description": (
            "Post prose to the bound room. NEVER invent a pseudo: the human is `@user`; "
            "native aliases are `@claude`, `@codex`, `@vibe`, `@gemini`, `@kiro`, "
            "`@copilot`, `@ollama`; a joined CLI requires its exact `@…-cli[-N]` alias "
            "from `disc_meta`. Ambiguous bare aliases are refused. Use "
            "`reply_to_message_id` for a durable reply. POSTING ALSO LISTENS unless "
            "`wait_for_reply:false`; `last_sort_order` is only a write receipt, never a "
            "read cursor. Bulk import and dedup: "
            "`tool_manual({tool: \"disc_append\"})`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "content": {"type": "string", "description": "Live message text."},
                "role": {"type": "string", "description": "Default Agent; User is for imports."},
                "channel": {"type": "string", "enum": ["main", "note"], "description": "Default main; notes do not wake."},
                "agent_type": {"type": "string", "description": "Normally inferred."},
                "target_agent": {"type": "string", "description": "Legacy responder override."},
                "targets": {
                    "type": "array",
                    "description": "Exact responders; prefer mentions.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "kind": {"type": "string", "enum": ["discussion_agent", "agent", "cli"]},
                            "agent_type": {"type": "string"},
                            "cli_session_id": {"type": "integer"},
                        },
                        "required": ["kind", "agent_type"],
                    },
                },
                "reply_to_message_id": {"type": "string", "description": "Message id from disc_get_message."},
                "attachments": {
                    "type": "array",
                    "maxItems": MAX_DISC_APPEND_ATTACHMENTS,
                    "items": {"type": "string"},
                    "description": "Unique local files; atomic upload, max 8 × 10 MB.",
                },
                "wait_for_reply": {"type": "boolean", "description": "Default true."},
                "source_msg_id": {"type": "string", "description": "Explicit dedup key."},
                "wait_timeout_secs": {"type": "integer", "description": "Wait seconds; default 60."},
                "disc_id": {"type": "string", "description": "Defaults to the bound discussion."},
                "messages": {
                    "type": "array",
                    "description": "Bulk transcript messages.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "source_msg_id": {"type": "string"},
                            "role": {"type": "string", "description": "User | Agent | System."},
                            "channel": {"type": "string", "enum": ["main", "note"]},
                            "content": {"type": "string"},
                            "agent_type": {"type": "string"},
                            "target_agent": {"type": "string", "description": "Legacy live responder."},
                            "targets": {
                                "type": "array",
                                "description": "Live typed responders.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "kind": {"type": "string", "enum": ["discussion_agent", "agent", "cli"]},
                                        "agent_type": {"type": "string"},
                                        "cli_session_id": {"type": "integer"},
                                    },
                                    "required": ["kind", "agent_type"],
                                },
                            },
                            "reply_to_message_id": {"type": "string", "description": "Existing replied-message id."},
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
            "Bind a Kronn disc to a durable CLI session, so a later MCP reload "
            "finds the room again via `disc_find_by_session` without a fresh "
            "invite token. ⚠ EVERY ARGUMENT IS OPTIONAL: called bare, "
            "`disc_link({})` binds THIS CLI session to the currently-bound "
            "disc — you cannot know your own durable session id, the bridge "
            "derives it. `disc_join` already does this for you; call this only "
            "to bind a disc you reached another way. Safe by default: if that "
            "session already owns another discussion the call fails instead of "
            "silently stealing it. Set force_reassign=true only when the user "
            "explicitly asks to transfer ownership."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "disc_id": {"type": "string", "description": "Defaults to the bound disc."},
                "source_agent": {"type": "string", "description": "Defaults to this client's agent type."},
                "source_session_id": {
                    "type": "string",
                    "description": "Defaults to this bridge's durable CLI session id.",
                },
                "force_reassign": {
                    "type": "boolean",
                    "default": False,
                    "description": "Transfer an already-bound session. Explicit user intent required.",
                },
            },
            "required": [],
        },
    },
    {
        "name": "disc_transfer_session",
        "description": (
            "Explicitly hand THIS durable CLI session from its previous room "
            "to the room currently joined by this bridge. Use only after the "
            "human clearly asks to change rooms. `from_disc_id` pins the room "
            "the caller expects to release and `confirm_transfer=true` is "
            "mandatory; ownership changes or missing confirmation fail closed. "
            "The backend atomically closes the previous append-only binding "
            "history row, opens the new one and returns `session_bound: true`. "
            "Afterward, a bare `disc_find_by_session({})` resumes this new room "
            "after an MCP reload. This transfers durable resume ownership only; "
            "it does not impersonate `disc_leave` or alter unrelated peers."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "from_disc_id": {
                    "type": "string",
                    "description": "The exact room currently returned by disc_find_by_session({}).",
                },
                "to_disc_id": {
                    "type": "string",
                    "description": "Defaults to the room currently joined by this bridge.",
                },
                "confirm_transfer": {
                    "type": "boolean",
                    "description": "Must be true after an explicit human request to change rooms.",
                },
            },
            "required": ["from_disc_id", "confirm_transfer"],
        },
    },
    {
        "name": "disc_unlink",
        "description": (
            "Release YOUR OWN session binding on a disc. Every argument is "
            "optional: bare, `disc_unlink({})` releases this CLI session's link "
            "on the bound disc. ⚠ It does NOT detach the other agents — a shared "
            "room holds one binding per joined session, and releasing them all is "
            "a human action from the Kronn UI, not an agent one. The append-only "
            "history chain is preserved either way."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "disc_id": {"type": "string", "description": "Defaults to the bound disc."},
                "source_agent": {"type": "string", "description": "Defaults to this client's agent type."},
                "source_session_id": {
                    "type": "string",
                    "description": "Defaults to this bridge's durable CLI session id.",
                },
            },
            "required": [],
        },
    },
    {
        "name": "disc_workspace_get",
        "description": (
            "Return the compact worktree state for THIS joined CLI session and "
            "the other worktrees declared in the current discussion. Called "
            "bare, the bridge derives its durable agent/session identity. Use "
            "this before editing when the room may contain several worktrees."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "source_agent": {
                    "type": "string",
                    "description": "Defaults to this client's agent type.",
                },
                "source_session_id": {
                    "type": "string",
                    "description": "Defaults to this bridge's durable CLI session id.",
                },
            },
            "required": [],
        },
    },
    {
        "name": "disc_workspace_set",
        "description": (
            "Declare or refresh THIS joined CLI session's Git worktree in the "
            "current discussion. `workspace_path` defaults to the bridge's "
            "current directory; Kronn canonicalizes it and reads the real "
            "branch + HEAD from Git, so never guess or send those fields. "
            "Optionally link the workspace to a planning task with `task_ref` "
            "(for example `KT-140`). The path must be a registered worktree of "
            "the discussion project's primary or linked repositories, and one "
            "physical worktree cannot belong to two discussions. The result "
            "always exposes a `blockers` array: dirty/branch-concurrency guards "
            "on success, or a structured missing/repository/ownership blocker "
            "when the declaration is refused."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "workspace_path": {
                    "type": "string",
                    "description": "Defaults to this bridge process's current directory.",
                },
                "task_ref": {
                    "type": "string",
                    "description": "Optional planning task reference or UUID.",
                },
                "source_agent": {
                    "type": "string",
                    "description": "Defaults to this client's agent type.",
                },
                "source_session_id": {
                    "type": "string",
                    "description": "Defaults to this bridge's durable CLI session id.",
                },
            },
            "required": [],
        },
    },
    {
        "name": "agent_list",
        "description": (
            "List the worker identities this principal room can pass verbatim to "
            "task_exec_prepare: native HTTP providers, host CLIs and exact joined CLI "
            "sessions. Reports configured/reachable/available separately with stable, "
            "secret-free reason codes. Call this before choosing a worker; then preflight "
            "the selected `worker` object."
        ),
        "inputSchema": {"type": "object", "properties": {}, "required": []},
    },
    {
        "name": "task_exec_prepare",
        "description": (
            "Preflight a Todo from THIS principal room without mutation. Returns `launchable` "
            "plus stable refusal codes after readiness/worker checks. Never bypass refusal. MUST read "
            "tool_manual({tool: \"task_exec_prepare\"}) before authoring worker, scope or "
            "validations."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_reference": {"type": "string", "description": "KT-### or task UUID."},
                "worker": {
                    "type": "object",
                    "description": "Typed MessageTarget; exact transport shapes are in tool_manual.",
                },
                "worker_scope_intent": {
                    "type": "string",
                    "enum": ["generic", "scoped"],
                    "description": (
                        "Required contract sentinel. Use scoped with worker_scope, or "
                        "generic only when no mechanical scope is intended."
                    ),
                },
                "worker_scope": {
                    "type": "object",
                    "description": "Native-HTTP scope required when worker_scope_intent is scoped.",
                },
            },
            "required": ["task_reference", "worker", "worker_scope_intent"],
        },
    },
    {
        "name": "task_exec_launch",
        "description": (
            "Launch the accepted preflight into its durable child room and SHA-pinned worktree. "
            "Keep worker/scope unchanged; reuse idempotency_key on retry. MUST first read "
            "tool_manual({tool: \"task_exec_prepare\"})."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_reference": {"type": "string"},
                "worker": {
                    "type": "object",
                    "description": "Exact typed worker accepted by preflight.",
                },
                "worker_scope_intent": {
                    "type": "string",
                    "enum": ["generic", "scoped"],
                    "description": "Must exactly match the preflighted scope intent.",
                },
                "worker_scope": {
                    "type": "object",
                    "description": "Exact scope accepted by preflight when intent is scoped.",
                },
                "base_rev": {"type": "string", "description": "Target/base branch; defaults to main."},
                "idempotency_key": {"type": "string", "description": "Stable retry key."},
                "validations": {
                    "type": "array",
                    "description": "Principal-owned gates; exact item shape is in tool_manual.",
                    "items": {"type": "object"},
                },
            },
            "required": ["task_reference", "worker", "worker_scope_intent"],
        },
    },
    {
        "name": "task_exec_status",
        "description": (
            "Read a party-visible TaskExecution and its durable evidence/recovery state. "
            "After reconnect, use its id or task_reference, obey returned `next_action`, "
            "and never infer execution state from chat."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_execution_id": {"type": "string"},
                "task_reference": {"type": "string", "description": "KT-### or task UUID fallback."},
            },
            "required": [],
        },
    },
    {
        "name": "task_exec_cancel",
        "description": (
            "Cancel a non-terminal TaskExecution as its principal. Kronn cancels due/live "
            "dispatches; completed commits and audit history are preserved. The worktree is "
            "kept by default; `remove_if_clean` removes only a proven-clean owned worktree "
            "and refuses dirty or unproven paths."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_execution_id": {"type": "string"},
                "reason": {"type": "string"},
                "cleanup_policy": {"type": "string", "enum": ["preserve", "remove_if_clean"]},
            },
            "required": ["task_execution_id", "reason"],
        },
    },
    {
        "name": "task_exec_resume",
        "description": (
            "Resume only when task_exec_status returns "
            "`next_action.tool=task_exec_resume`; the guarded checkpoint is retry-safe. "
            "See tool_manual({tool: \"task_exec_resume\"})."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_execution_id": {"type": "string"},
            },
            "required": ["task_execution_id"],
        },
    },
    {
        "name": "task_exec_reassign",
        "description": (
            "Reassign an interrupted/blocked execution without losing its room or evidence. "
            "See tool_manual({tool: \"task_exec_reassign\"})."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_execution_id": {"type": "string"},
                "worker": {
                    "type": "object",
                    "description": (
                        "Copy from agent_list. Native HTTP workers use discussion_agent; "
                        "Custom targets require their connection_id."
                    ),
                },
                "reason": {"type": "string"},
            },
            "required": ["task_execution_id", "worker", "reason"],
        },
    },
    {
        "name": "task_exec_accept_worker_offer",
        "description": (
            "Attach THIS joined CLI to a task worker offer using only its opaque "
            "offer_id. The backend verifies the exact session and success rebinds "
            "this bridge to the child room. See "
            "tool_manual({tool: \"task_exec_accept_worker_offer\"})."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "offer_id": {
                    "type": "string",
                    "description": "The opaque offer id from the control-offer message.",
                },
            },
            "required": ["offer_id"],
        },
    },
    {
        "name": "task_exec_deliver",
        "description": (
            "Submit your DeliveryManifest v1 for review when the task's DoD is met "
            "(KT-319). Pass your `task_execution_id` and the `manifest` object: the "
            "backend derives your identity from this bridge's durable session and "
            "verifies you are the execution's EXACT worker (a different session is "
            "refused). On success the manifest is persisted, the execution flips to "
            "AwaitingReview, and a review request wakes the principal in the parent "
            "room — call this BEFORE announcing 'ready for review'. This does not "
            "move your session; you stay in the sub-discussion. A malformed manifest "
            "is refused, not silently accepted."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_execution_id": {
                    "type": "string",
                    "description": "The task execution you are the worker of (from your brief).",
                },
                "manifest": {
                    "type": "object",
                    "description": (
                        "The DeliveryManifest v1 object: version, task_ref, head_sha, "
                        "files_touched, tests, dod_status, docs, migrations, risks, "
                        "limitations, summary (see the brief's delivery format)."
                    ),
                },
            },
            "required": ["task_execution_id", "manifest"],
        },
    },
    {
        "name": "task_exec_review",
        "description": (
            "Decide a delivered task attempt as the principal (KT-319). Pass the "
            "`task_execution_id` and a ReviewDecision v1 `decision` object "
            "(`decision`: 'approve' | 'request_changes'; `comment` is required for "
            "request_changes; optional structured `findings`). The backend derives "
            "your identity from this bridge's durable session and authorizes you as a "
            "party to the execution — the parent-room principal, or the worker only if "
            "the run explicitly enables self-review. approve is REFUSED if the manifest "
            "is missing, the worktree HEAD drifted since delivery, or a DoD is unmet; "
            "request_changes hands your findings to the worker in its sub-discussion "
            "and keeps the worktree. A caller who is not a party gets one opaque refusal."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "task_execution_id": {
                    "type": "string",
                    "description": "The execution whose delivered attempt you are reviewing.",
                },
                "decision": {
                    "type": "object",
                    "description": (
                        "The ReviewDecision v1 object: version, task_ref, decision "
                        "('approve' | 'request_changes'), comment (required for "
                        "request_changes), optional findings [{path?, line?, issue}]."
                    ),
                },
            },
            "required": ["task_execution_id", "decision"],
        },
    },
    {
        "name": "disc_workspace_history_lease",
        "description": (
            "Advisory guard for destructive Git history rewrites in THIS "
            "session's declared worktree. Before rebase/squash/reset/force "
            "push: create a ref under refs/kronn-backup/ at current HEAD, then "
            "acquire with that exact ref. Refusal means another room peer owns "
            "the lease: do not rewrite. Release afterward. Kronn verifies the "
            "ref and arbitrates cooperating agents, but cannot stop a CLI that "
            "runs Git without calling this tool."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["acquire", "release"]},
                "backup_ref": {
                    "type": "string",
                    "description": "Required for acquire; refs/kronn-backup/... at current HEAD.",
                },
            },
            "required": ["action"],
        },
    },
    {
        "name": "disc_find_by_session",
        "description": (
            "Look up the Kronn disc_id bound to a CLI session, or `null` if "
            "none. ⚠ BOTH ARGUMENTS ARE OPTIONAL: called bare, "
            "`disc_find_by_session({})` answers « which room is MY session "
            "bound to? » and actively restores THIS bridge's runtime binding "
            "and durable read cursor after an MCP reload. A legacy/pre-fix "
            "session can still have a server link but no local resume "
            "credential; that exceptional response carries "
            "`rejoin_required: true` instead of falsely claiming the bridge is "
            "ready, and needs one fresh kr-join bootstrap. "
            "Also the way to decide between `disc_create` (no prior thread) "
            "and `disc_append` (resume an existing one)."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "source_agent": {"type": "string", "description": "Defaults to this client's agent type."},
                "source_session_id": {
                    "type": "string",
                    "description": "Defaults to this bridge's durable CLI session id.",
                },
            },
            "required": [],
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
                "include_notes": {"type": "boolean", "description": "Explicit audit opt-in. Default false."},
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
            "only in your own terminal is INVISIBLE to peers. "
            "⚠ JOINING IS NOT THE TASK : joining then going quiet "
            "reads to the human as having left. You are expected to "
            "STAY and FOLLOW the room — loop on "
            "`disc_wait_for_peer` and answer what arrives, and when "
            "the room is quiet pick up the next task from the shared "
            "plan (`plan_get` / `task_list`) instead of idling. You "
            "may read AND update those tasks. BEFORE your first "
            "substantive action, announce the task / scope / next "
            "action in the room with `disc_append`; peers must know "
            "what you are taking before you edit or execute it. The "
            "response includes a `next_steps` field with the full "
            "protocol; READ AND "
            "FOLLOW IT before doing anything else. Tokens are "
            "single-use and expire after 10 min."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "token": {
                    "type": "string",
                    "description": "Invite token (kr-join-… form).",
                },
                "model": {
                    "type": "string",
                    "description": (
                        "Optional: the model you run on (e.g. "
                        "\"claude-opus-4\"). Self-DECLARED, shown in the "
                        "participant header as declared-at-join. Omit if "
                        "unknown — Kronn never guesses a model."
                    ),
                },
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
            "The room has no native Kronn principal by default: messages "
            "posted by joined MCP peers do NOT auto-launch the discussion's "
            "placeholder agent. Only the explicitly joined peers answer.\n\n"
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
            "Wait outside the model loop for peer messages. Omit `since_sort_order`: the "
            "bridge keeps a durable cursor. An override must reuse `latest_sort_order` "
            "from a WAIT, never `last_sort_order` returned by an append. Each item has a "
            "durable `message_id`; reply with its exact `reply_to_message_id`. Items are "
            "context, not your turns. A wait moved to the background is still "
            "active: DO NOT start another wait. Quiet is normal; re-arm until the "
            "task is done, blocked on the human, or stopped. Routing and acknowledgement: "
            "`tool_manual({tool: \"disc_wait_for_peer\"})`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "since_sort_order": {
                    "type": "integer",
                    "description": "Advanced override: highest sort_order actually read. Normally omit it so the bridge uses its durable read cursor. Never pass an append's last_sort_order.",
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Inner poll window in seconds (default 60, capped at 60 so interruptions stay responsive). The OVERALL wait is governed by max_total_secs, not this.",
                },
                "max_total_secs": {
                    "type": "integer",
                    "description": "OPT-IN overall wait budget in seconds before a quiet return (env KRONN_WAIT_TOTAL_SECS). Omit it in normal use: the bridge default is unbounded. A host may still background the tool call; never stack another wait while the original background task is active.",
                },
            },
            "required": [],
        },
    },
    {
        "name": "disc_load_other",
        "description": (
            "Load a slice of messages from a disc OTHER than the current one. "
            "Returns `{disc_id, title, total_messages, from_idx, to_idx, "
            "messages}`. **No range = the LAST ~40 messages, not the whole "
            "disc** (one real thread is 1 427). An explicit `from`/`to` is "
            "honoured at any size."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "disc_id": {"type": "string", "description": (
                    "Discussion id. The user can copy it from the UI: the "
                    "#-prefixed pill in the chat header (click = full UUID)."
                )},
                "from": {"type": "integer", "description": "Inclusive start (0-based)."},
                "to": {"type": "integer", "description": "Exclusive end."},
                "include_notes": {"type": "boolean", "description": "Explicit audit opt-in. Default false."},
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
            "view (id, name, agent, description, variable_names, variables, "
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
            "entries include `{name, label, required, description, source, "
            "source_ref, allow_manual_override, control}`. Pass only user-input "
            "values to `qa_run`; project/context values resolve at dispatch. Use this to (a) "
            "discover the right QA for an action via `qa_run`, (b) "
            "reuse a matching QA via `quick_api_id` in a workflow "
            "`ApiCall` / `BatchApiCall` step, or as a source in "
            "`CollectApiData`, instead of re-specifying the endpoint inline."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "qe_list",
        "description": (
            "List saved Quick Execs — compact view (id, name, command, argv, "
            "output_format, timeout_secs, project_id, variables). Reuse one through "
            "CollectApiData.sources[].quick_exec_id instead of duplicating a CLI command."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "page_list",
        "description": (
            "List every Live Page in Kronn as a compact discovery view: id, "
            "title, slug, project_id, data_revision, updated_at and "
            "last_published_at. Call this before authoring a PublishPageData "
            "step: Pages are shared destinations and several workflows may "
            "publish into the same Page. Reuse a matching page_id instead of "
            "creating a duplicate."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "page_get",
        "description": (
            "Fetch one Live Page by id or slug, including its current HTML "
            "revision, declared datasets, retained points and every saved "
            "workflow step that targets it. Read this before changing the HTML "
            "or wiring another workflow into the Page."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "page_id": {"type": "string", "description": "Page id or slug from page_list."},
            },
            "required": ["page_id"],
        },
    },
    {
        "name": "page_create",
        "description": (
            "Create a sandboxed Live Page and its first immutable HTML revision. "
            "Call `page_list` first. Data arrives through `window.KronnPageData` "
            "and `kronn:page-data`; use the returned id in PublishPageData. Scope "
            "inherits from a bound discussion, while host CLIs may create standalone "
            "Pages. Typed inline QP/QA/QE/Workflow CTAs are documented by "
            "`tool_manual({tool: \"page_create\"})`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Title, 1-200 chars."},
                "slug": {"type": "string", "description": "Optional ASCII slug."},
                "project_id": {"type": "string", "description": "Optional project binding."},
                "discussion_id": {"type": "string", "description": "Optional origin discussion."},
                "html": {"type": "string", "description": "Self-contained HTML, max 1 MB."},
                "datasets": {
                    "type": "array",
                    "description": "Named PublishPageData contracts; may be empty.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "kind": {"type": "string", "enum": ["snapshot", "time_series", "collection"]},
                            "initial": {},
                            "schema": {},
                            "max_points": {"type": "integer"},
                            "max_age_days": {"type": "integer"},
                        },
                        "required": ["name", "kind"],
                    },
                },
            },
            "required": ["title", "html", "datasets"],
        },
    },
    {
        "name": "page_update_html",
        "description": (
            "Replace a Live Page's presentation by creating a new immutable "
            "HTML revision. Dataset values and publication history are kept. "
            "Call page_get first, then send the complete self-contained HTML; "
            "this is a full replacement, not a patch."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "page_id": {"type": "string", "description": "Page id or slug from page_list."},
                "html": {"type": "string", "description": "Complete replacement HTML document (1 MB max)."},
            },
            "required": ["page_id", "html"],
        },
    },
    {
        "name": "page_add_dataset",
        "description": (
            "Attach a dataset to an existing Page so PublishPageData can write "
            "to it. Idempotent on name+kind; reusing a name with another kind conflicts."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "page_id": {"type": "string"},
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": ["snapshot", "time_series", "collection"]},
                "initial": {},
                "schema": {},
                "max_points": {"type": "integer"},
                "max_age_days": {"type": "integer"},
            },
            "required": ["page_id", "name", "kind"],
        },
    },
    {
        "name": "mcp_list",
        "description": (
            "List the current MCP/API configs, REST endpoints, `config_keys` and "
            "readiness hints. Call this in the current session before authoring any "
            "workflow, QA or `api_call` that references a plugin: slugs, config ids "
            "and paths must never be guessed. `${ENV.KEY}` may reference only "
            "non-secret keys where `auth_managed:false`; always obey each returned "
            "`hint`. See `tool_manual({tool: \"mcp_list\"})`."
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
            "Create a disabled draft for explicit user review. Discovery first: "
            "resolve plugin/config ids with `mcp_list`, bindings with their list "
            "tools, Quick APIs with `qa_list`, Quick Execs with `qe_list`, and Pages "
            "with `page_list`; never guess. `step_type` is a tagged object and its "
            "closed set is: **Agent · ApiCall · BatchApiCall · "
            "BatchQuickPrompt · Exec · Gate · Notify · JsonData · "
            "CollectApiData · TransformData · PublishPageData · SubWorkflow**. "
            "Call `workflow_step_schema` before composing steps. Prefer adapting a real "
            "workflow via `workflow_get`/`workflow_clone`. Full authoring contract: "
            "`tool_manual({tool: \"workflow_create_draft\"})`. Returns the created JSON."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Workflow name (1-200 chars)."},
                "trigger": {
                    "type": "object",
                    "description": "Tagged Manual/Cron/Tracker trigger; scheduled runs default to concurrency 1.",
                },
                "steps": {
                    "type": "array",
                    "description": "1-20 top-level typed steps. Call workflow_step_schema for fields and examples.",
                },
                "project_id": {"type": "string", "description": "Optional Kronn project id to bind the workflow to."},
                "variables": {"type": "array", "description": "PromptVariable declarations: `{name,label?,placeholder?,description?,required?,pattern?,source?,source_ref?,allow_manual_override?,control?}`. `source` is `user_input` (default), `project_env` (`source_ref:'<env.NAME>'`, project required), or `kronn_context` (`source_ref:'<context.key>'`). Store references only; values resolve at each launch."},
                "guards": {"type": "object", "description": "Execution limits."},
                "on_failure": {"type": "array", "description": "Rollback steps."},
                "exec_allowlist": {"type": "array", "items": {"type": "string"}, "description": "Allowed Exec binaries."},
                "artifacts": {"type": "object", "description": "Artifact declarations."},
                "concurrency_limit": {"type": "integer", "description": "Max concurrent runs; scheduled default is 1."},
                "safety": {"type": "object", "description": "Optional WorkflowSafety overrides."},
                "actions": {"type": "array", "description": "Optional actions (legacy slot)."},
                "workspace_config": {"type": "object", "description": "Direct or Isolated workspace."},
            },
            "required": ["name", "trigger", "steps"],
        },
    },
    {
        "name": "qp_create_draft",
        "description": (
            "Create a reusable, manual-launch Quick Prompt. Discover every agent, skill, "
            "profile and directive id first; never invent a binding UUID. For improving "
            "an existing QP, use its `qp-improver` flow. Authoring and binding contract: "
            "`tool_manual({tool: \"qp_create_draft\"})`. Returns the complete created QP."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "QP name (1-200 chars, displayed on the QP card)."},
                "prompt_template": {"type": "string", "description": "The template body. Use `{{var}}` for required variables."},
                "agent": {"type": "string", "description": "Default agent: `ClaudeCode` / `Codex` / `Vibe` / `GeminiCli` / `Kiro` / `CopilotCli` / `Ollama` / `Custom`."},
                "variables": {"type": "array", "description": "PromptVariable declarations: `{name,label?,placeholder?,description?,required?,pattern?,source?,source_ref?,allow_manual_override?,control?}`. Use `project_env` + `<env.NAME>` or `kronn_context` + `<context.key>` as references only; never include resolved values."},
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
                "variables": {"type": "array", "description": "Launch-time PromptVariable declarations. Sources: user_input, project_env with `<env.NAME>`, or kronn_context with `<context.key>`; references resolve anew at run start. `label`/`placeholder` auto-default."},
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
                "variables": {"type": "array", "description": "PromptVariable declarations including source/source_ref/allow_manual_override/control; project_env stores `<env.NAME>` only. `label`/`placeholder` auto-default."},
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
            "`qp_update`). IRREVERSIBLE — runs already produced from it are kept."
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
        "description": "Delete a custom skill by id (builtins are protected). IRREVERSIBLE. Past runs keep the prompts they used; only future runs lose it.",
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
        "description": "Delete a custom profile by id (builtins are protected). IRREVERSIBLE. Past runs keep the prompts they used; only future runs lose it.",
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
        "description": "Delete a custom directive by id (builtins are protected). IRREVERSIBLE. Past runs keep the prompts they used; only future runs lose it.",
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
            "truncated, unlike a tool description): the closed 12-set of "
            "`step_type`s, the flat shape, the required + optional fields PER "
            "type, and the RUNTIME CONTRACTS that break a workflow at run time "
            "if missed (e.g. SubWorkflow foreach → the engine writes each item "
            "to the fixed path `.kronn/current_task.json`), plus the complete "
            "run-anchored `time.now` grammar. Zero args. Call this "
            "BEFORE authoring or editing a workflow instead of inferring the "
            "schema from one `workflow_get` sample or from the (possibly "
            "client-truncated) `workflow_create_draft` description."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "qa_create_draft",
        "description": (
            "Save a Quick API for later `qa_run` calls. "
            "Discover `api_plugin_slug`/`api_config_id` with `mcp_list`; the "
            "endpoint remains allow-listed and Kronn owns credentials. String "
            "leaves may use declared `{{var_name}}` values or the generic "
            "run-anchored `{{time.now|...}}` grammar documented by "
            "`workflow_step_schema`. Call "
            "`tool_manual({tool: \"qa_create_draft\"})` first for the required "
            "probe/extract method and variable contract; iterate with `qa_update`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "QA name (1-200 chars, displayed on the QA card)."},
                "api_plugin_slug": {"type": "string", "description": "Plugin slug from `mcp_list` (e.g. `mcp-atlassian`, `api-resend`, `api-custom-foo`)."},
                "api_config_id": {"type": "string", "description": "Plugin config id from `mcp_list.configs[].config_id`. Pin the QA to a specific config (per-project or global)."},
                "api_endpoint_path": {"type": "string", "description": "Endpoint path matching one of the plugin's declared endpoints (e.g. `/rest/api/3/issue/{ticket_id}`). May contain `{{var}}` placeholders OR `{path_param}` segments."},
                "api_method": {"type": "string", "description": "HTTP method override : `GET | POST | PUT | PATCH | DELETE`. Defaults to the plugin endpoint's declared method when omitted."},
                "api_query": {"type": "object", "description": "Query-string parameters as key→value map. Values may contain `{{var}}` placeholders or vendor-neutral `{{time.now|...}}` expressions."},
                "api_path_params": {"type": "object", "description": "Path-segment substitutions for `{name}` segments in the endpoint path."},
                "api_headers": {"type": "object", "description": "Extra request headers. NEVER pass auth — Kronn injects per the plugin spec."},
                "api_body": {"description": "JSON body for POST/PUT/PATCH (object/array). String leaves can contain `{{var}}` placeholders."},
                "api_extract": {"type": "object", "description": "Optional JSONPath extract spec: `{path: \"$.items\", fail_on_empty: false}`."},
                "api_pagination": {"type": "object", "description": "Optional pagination spec (Auto | Offset | Cursor | Page | LinkHeader); shape in tool_manual."},
                "api_timeout_ms": {"type": "integer", "description": "Optional per-call timeout in ms. Defaults to plugin default."},
                "api_max_retries": {"type": "integer", "description": "Optional retry count on transient HTTP errors."},
                "variables": {
                    "type": "array",
                    "description": "PromptVariable declarations; tool_manual documents user_input/project_env/kronn_context sources. Store references, never values.",
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
        "name": "qe_create_draft",
        "description": (
            "Save a reusable shell-free CLI collector. Use one bare binary plus a literal "
            "argv array; shells, paths and command wrappers are rejected. Test with qe_run, "
            "then reference its id from CollectApiData. Call tool_manual first."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "command": {"type": "string", "description": "Bare binary name, e.g. aws or fastly."},
                "args": {"type": "array", "items": {"type": "string"}, "description": "Literal argv values; {{variables}} are allowed."},
                "output_format": {"type": "string", "enum": ["json", "csv", "text", "lines"]},
                "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 1800},
                "variables": {"type": "array", "description": "PromptVariable declarations including source/source_ref/allow_manual_override/control. `project_env` stores `<env.NAME>` only and resolves for the selected project at run start."},
                "description": {"type": "string"},
                "icon": {"type": "string"},
                "project_id": {"type": "string"},
            },
            "required": ["name", "command"],
        },
    },
    {
        "name": "qe_update",
        "description": "Patch a saved Quick Exec field-by-field. Omitted fields are preserved.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "qe_id": {"type": "string"},
                "name": {"type": "string"}, "command": {"type": "string"},
                "args": {"type": "array", "items": {"type": "string"}},
                "output_format": {"type": "string", "enum": ["json", "csv", "text", "lines"]},
                "timeout_secs": {"type": "integer"}, "variables": {"type": "array", "description": "PromptVariable declarations including source/source_ref; project_env values resolve at each run."},
                "description": {"type": "string"}, "icon": {"type": "string"},
                "project_id": {
                    "type": "string",
                    "description": "New project id. Omit to preserve the current binding; pass an empty string to make it global.",
                },
            },
            "required": ["qe_id"],
        },
    },
    {
        "name": "qa_update",
        "description": (
            "Patch a saved Quick API field-by-field. Only `qa_id` is required; "
            "omitted fields are preserved and explicit empty collections clear "
            "them. Returns the complete updated QA for immediate verification "
            "or `qa_run`."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qa_id": {"type": "string", "description": "Quick API id (from `qa_list`)."},
                # Every QA field can be patched. None required beyond qa_id.
                "name": {"type": "string"},
                "icon": {"type": "string"},
                "description": {"type": "string"},
                "api_plugin_slug": {"type": "string", "description": "Re-target to a different plugin (rare)."},
                "api_config_id": {"type": "string"},
                "api_endpoint_path": {"type": "string"},
                "api_method": {"type": "string"},
                "api_query": {"type": "object"},
                "api_path_params": {"type": "object"},
                "api_headers": {"type": "object"},
                "api_body": {"description": "Replace the body JSON."},
                "api_extract": {"type": "object", "description": "Replace the extract spec (the most common patch)."},
                "api_pagination": {"type": "object"},
                "api_timeout_ms": {"type": "integer"},
                "api_max_retries": {"type": "integer"},
                "variables": {"type": "array", "description": "PromptVariable declarations including source/source_ref; project_env stores `<env.NAME>` only."},
                "profile_ids": {"type": "array", "items": {"type": "string"}},
                "directive_ids": {"type": "array", "items": {"type": "string"}},
                "project_id": {"type": "string", "description": "Re-bind to a different project."},
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
            "Invoke a configured API without exposing credentials. Reuse a matching "
            "`qa_list`/`qa_run` first; otherwise discover real plugin/config ids with "
            "`mcp_list` and pass either `api_plugin_slug` + `api_config_id`, or a "
            "`quick_api_id`. Never place auth in path, query, headers or body. Returns "
            "`{success,data,status,summary,http_status,error?}`. Project scope, "
            "`${ENV.KEY}`, time expressions and safe persistence are documented by "
            "`tool_manual({tool: \"api_call\"})`. Suggest `qa_create_draft` for a useful "
            "recurring call instead of rebuilding it."
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
        "name": "media_generate",
        "description": (
            "Generate an image or a video on a configured HTTP connection "
            "(OpenRouter, NVIDIA). Returns `{job_id, status, model}`.\n\n"
            "**You do NOT choose the model.** It comes from the connection's "
            "configured image/video slot, so a generation cannot be billed on "
            "a model the human did not select. A modality with no configured "
            "slot is refused, naming what to configure.\n\n"
            "**Cost is real.** Video is billed per second (~0.07 USD for 5 s "
            "at 480p) and image per picture. Duration and resolution are "
            "capped server-side. Ask for the shortest clip that answers the "
            "need.\n\n"
            "**`wait` defaults to false**, which is almost always right: a "
            "video takes ~100 s and the asset lands in the discussion on its "
            "own, so you can keep working and it will be there. Pass "
            "`wait: true` ONLY when you must use the media inside the answer "
            "you are currently writing.\n\n"
            "The finished asset is attached to the discussion as a context "
            "file, visible to every agent in it."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "connection_id": {
                    "type": "string",
                    "description": "External API connection id (from `mcp_list` or the settings UI).",
                },
                "modality": {
                    "type": "string",
                    "enum": ["image", "video"],
                    "description": "What to produce. The model is taken from the matching configured slot.",
                },
                "prompt": {"type": "string", "description": "What to generate."},
                "discussion_id": {
                    "type": "string",
                    "description": "Discussion the asset is attached to. Defaults to the bound discussion.",
                },
                "duration_secs": {
                    "type": "integer",
                    "description": "Video only. Server-capped; prefer the shortest clip that works.",
                },
                "resolution": {
                    "type": "string",
                    "description": "480p | 720p | 1080p. Higher costs more per second.",
                },
                "aspect_ratio": {"type": "string", "description": "e.g. 16:9, 9:16, 1:1."},
                "generate_audio": {
                    "type": "boolean",
                    "description": "Video only. Audio raises the per-second price.",
                },
                "wait": {
                    "type": "boolean",
                    "description": "Block until the media is delivered. Default false — see the description.",
                },
            },
            "required": ["connection_id", "modality", "prompt"],
        },
    },
    {
        "name": "media_job_status",
        "description": (
            "State of one media generation: `{id, modality, status, model, "
            "context_file_id?, width?, height?, duration_ms?, cost_usd?, "
            "is_byok?, last_error?, attempts}`.\n\n"
            "Absent fields mean NOT MEASURED YET, never zero: a job still "
            "running has no cost and no dimensions because nothing has been "
            "billed or produced, not because they are null.\n\n"
            "Dimensions are read from the produced file, not from the "
            "request — providers do not honour the requested geometry."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"job_id": {"type": "string", "description": "Job id (from `media_generate`)."}},
            "required": ["job_id"],
        },
    },
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
            "`steps[]` has each step's name + status + started_at + duration_ms + "
            "tokens_used (number or null) + tokens_status when measurement is "
            "in progress/partial/unavailable + 200-char output excerpt + step_type. "
            "Never interpret a null token count as zero.\n\n"
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
            "Launch an existing Quick Prompt as one fresh background discussion. "
            "Resolve its id and required variables with `qp_list`; every required "
            "value must be non-empty. The response includes `disc_id` and "
            "`next_check`; read the result with `disc_load_other`. Agent and project "
            "overrides are optional. Tracking details: `tool_manual({tool: \"qp_run\"})`."
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
                    "description": "Optional agent override. Defaults to the QP's declared agent.",
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
            "Launch an existing Quick Prompt into 1–50 child discussions under one "
            "trackable batch. Resolve the id and variables with `qp_list`; each item's "
            "required values must be non-empty. Returns `run_id`, `disc_ids` and "
            "`next_check`; use workflow run tools for progress, then `disc_load_other`. "
            "Full item and timing contract: `tool_manual({tool: \"qp_batch_run\"})`."
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
            "Execute a saved Quick API synchronously. The HTTP call starts no model, "
            "though this invocation and retained output consume normal agent tokens. "
            "Returns `{success,duration_ms,envelope:{data,status,summary},error?}` inline: "
            "NO `next_check`. Discover its id and required vars with `qa_list`; prefer this "
            "repeatable, audited request over a hand-built `api_call`. Variables, time "
            "templates and result semantics: `tool_manual({tool: \"qa_run\"})`."
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
        "name": "qe_run",
        "description": (
            "Execute one saved Quick Exec synchronously. Returns normalized data: JSON as-is, "
            "CSV as an array of objects, text as a string, or lines as an array. The command "
            "runs directly without a shell and is bounded to its saved project cwd or a temp cwd."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "qe_id": {"type": "string", "description": "Quick Exec id from qe_list."},
                "vars": {"type": "object", "description": "Values for declared {{variables}}."},
            },
            "required": ["qe_id"],
        },
    },
    {
        "name": "learning_propose",
        "description": (
            "Propose a durable, human-reviewed learning for future discussions. Evidence "
            "is mandatory and server-verified; nothing reaches a truth file before human "
            "validation. Use `fact`, `preference` or `inference`, scope the claim, and "
            "avoid unsupported absolutes. Evidence rules and examples: "
            "`tool_manual({tool: \"learning_propose\"})`."
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
            "0.9.0 — Step 0 of the audit pipeline (template → audit → "
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
            "Launch a `full` or `partial` project audit and return immediately. This is "
            "NOT detached: closing/reloading this MCP interrupts its SSE-driven run. Check "
            "`audit_status`; only interrupted full/specialized runs resume with "
            "`resume_run_id`, while partial requires 1-based `steps` and is relaunched. "
            "One audit per project. Run `audit_prepare` first. Lifecycle, briefing and "
            "validation-discussion rules: `tool_manual({tool: \"audit_launch\"})`."
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


def _restore_reload_handoff():
    """Restore state consumed by exec without requiring a second initialize."""
    legacy_path = os.environ.pop(_BRIDGE_RELOAD_HANDOFF_ENV, None)
    raw_fd = os.environ.pop(_BRIDGE_RELOAD_HANDOFF_FD_ENV, None)
    expected_nonce = os.environ.pop(_BRIDGE_RELOAD_HANDOFF_NONCE_ENV, None)
    if raw_fd is None:
        if legacy_path:
            raise RuntimeError("bridge reload handoff path transport is no longer accepted")
        return []
    if (not isinstance(raw_fd, str)
            or re.fullmatch(r"[1-9][0-9]*", raw_fd) is None):
        raise RuntimeError("bridge reload handoff descriptor is invalid")
    fd = int(raw_fd)
    if fd <= 2:
        raise RuntimeError("bridge reload handoff descriptor is invalid")
    try:
        if (not isinstance(expected_nonce, str)
                or re.fullmatch(r"[0-9a-f]{64}", expected_nonce) is None):
            raise RuntimeError("bridge reload handoff authenticator is missing or invalid")
        opened = os.fstat(fd)
        if (not stat.S_ISREG(opened.st_mode)
                or stat.S_IMODE(opened.st_mode) != 0o600
                or opened.st_uid != os.geteuid()
                or opened.st_nlink != 0
                or opened.st_size > _BRIDGE_HANDOFF_MAX_BYTES):
            raise RuntimeError("bridge reload handoff descriptor failed validation")
        os.lseek(fd, 0, os.SEEK_SET)
        raw = os.read(fd, _BRIDGE_HANDOFF_MAX_BYTES + 1)
        if len(raw) > _BRIDGE_HANDOFF_MAX_BYTES:
            raise RuntimeError("bridge reload handoff exceeds size limit")
        state = json.loads(raw.decode("utf-8"))
        if not isinstance(state, dict) or set(state) != {
            "version", "client_info", "requests", "cancelled_request_ids",
            "pending_hex", "stdin_eof", "nonce",
        } or state.get("version") != _BRIDGE_HANDOFF_VERSION:
            raise RuntimeError("bridge reload handoff schema is invalid")
        nonce = state["nonce"]
        if (not isinstance(nonce, str)
                or re.fullmatch(r"[0-9a-f]{64}", nonce) is None
                or not hmac.compare_digest(nonce, expected_nonce)):
            raise RuntimeError("bridge reload handoff authentication failed")
        client_info = state["client_info"]
        queued = state["requests"]
        cancelled = state["cancelled_request_ids"]
        pending_hex = state["pending_hex"]
        stdin_eof = state["stdin_eof"]
        if (not isinstance(client_info, dict)
                or set(client_info) != {"name", "version"}
                or any(value is not None and (not isinstance(value, str) or len(value) > 256)
                       for value in client_info.values())
                or not isinstance(queued, list) or len(queued) > 4096
                or not all(isinstance(item, dict) for item in queued)
                or not isinstance(cancelled, list) or len(cancelled) > 4096
                or not all(isinstance(rid, (str, int, float)) and not isinstance(rid, bool)
                           for rid in cancelled)
                or not isinstance(pending_hex, str)
                or len(pending_hex) > _BRIDGE_PENDING_MAX_BYTES * 2
                or not isinstance(stdin_eof, bool)):
            raise RuntimeError("bridge reload handoff payload is invalid")
        try:
            pending = bytes.fromhex(pending_hex)
        except ValueError as exc:
            raise RuntimeError("bridge reload handoff pending buffer is invalid") from exc
    finally:
        with contextlib.suppress(OSError):
            os.close(fd)
    _CLIENT_INFO.update(name=client_info["name"], version=client_info["version"])
    _STDIN_PENDING.extend(pending)
    now = time.monotonic()
    with _CANCELLED_LOCK:
        for rid in cancelled:
            _CANCELLED_REQUEST_IDS[rid] = now
    return queued + ([None] if stdin_eof else [])


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
# NB (0.9.0): this id is NOT relied on to survive an MCP reload anymore.
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
    a logical-session boundary. Codex has no equivalent terminal/project
    scope, but a verified native conversation UUID survives `codex resume`
    across a complete CLI reboot and is therefore stronger than its outer
    process id. Other clients retain the proven outermost-CLI fallback.
    """
    platform = _platform_session_identity()
    if platform is not None:
        return platform
    if _agent_type_for_session() == "Codex":
        conversation_id = _native_conversation_id()
        if conversation_id:
            return ("codex-session", conversation_id)
    return _cli_ancestor_identity()


def _identity_key_from(identity):
    """Hash a stable CLI identity into a short filesystem-safe binding key.
    `None` in → `None` out (fail-closed: no durable identity ⇒ no persisted
    resume). Pure (no I/O) so the stable/distinct invariants are unit-testable:
    a given identity always maps to the same key, distinct identities to
    distinct keys.  Preserve the historical process-key encoding so clients
    without a platform session id keep finding their existing binding file."""
    if identity is None:
        return None
    if identity[0] in ("claude-terminal", "claude-session", "codex-session"):
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


def _native_conversation_id(allow_probe=True):
    """Return the CLI's own resume id when the runtime exposes it.

    This is intentionally separate from `_session_id_for_caller()`: the latter
    identifies the Kronn bridge process, while this value is what the human can
    pass to the CLI's native resume command. Unknown clients and malformed
    values degrade to `None`; Kronn never fabricates a resumable id.

    `allow_probe=False` restricts resolution to the environment and to an
    already-cached probe result. The idle wait runs on every poll and sits on
    the critical path — it must never pay for `ps`/`lsof` walks; those belong
    to join/resume, which happen once.
    """
    agent_type = _agent_type_for_session()
    env_name = {
        "ClaudeCode": "CLAUDE_CODE_SESSION_ID",
        "Codex": "CODEX_THREAD_ID",
    }.get(agent_type)
    if not env_name:
        return None
    raw = os.environ.get(env_name)
    if raw in (None, ""):
        # Codex currently exposes CODEX_THREAD_ID to the interactive shell but
        # some MCP launch paths do not forward it to the stdio server. A
        # resumed Codex process still carries the exact native id in its own
        # `codex resume <uuid>` argv; a FRESH session carries nothing in argv,
        # but keeps its rollout session file open — recover the id from that
        # open descriptor as a last resort (KT-114). Claude has no equivalent
        # stable contract on either path.
        if agent_type != "Codex":
            return None
        if not allow_probe:
            # Cheap path only: reuse a probe that ALREADY ran, never start one.
            return _CODEX_FD_PROBE_CACHE["uuid"] if _CODEX_FD_PROBE_CACHE["done"] else None
        return _codex_resume_id_from_ancestors() or _codex_id_from_open_rollouts()
    if not raw or raw != raw.strip() or len(raw) > 512 or not raw.isprintable():
        return None
    try:
        canonical = str(uuid.UUID(raw))
    except (AttributeError, ValueError):
        return None
    return canonical if raw.lower() == canonical else None


_CODEX_RESUME_CMD_RE = re.compile(
    r"^\s*(?:(?:\S*/)?(?:node|nodejs|npx|pnpm|bun)\s+)?"
    r"(?:\S*/)?codex\s+resume\s+([0-9a-fA-F-]{36})(?:\s|$)"
)


def _codex_resume_id_from_cmdline(cmdline):
    """Extract only an actual `codex resume <uuid>` process invocation.

    The expression is anchored and permits at most one known JS launcher
    prefix. This deliberately rejects the same words appearing later inside a
    `codex exec` prompt, where accepting a UUID would invent provenance.
    """
    if not isinstance(cmdline, str) or len(cmdline) > 64 * 1024:
        return None
    match = _CODEX_RESUME_CMD_RE.match(cmdline)
    if not match:
        return None
    raw = match.group(1)
    try:
        canonical = str(uuid.UUID(raw))
    except (AttributeError, ValueError):
        return None
    return canonical if raw.lower() == canonical else None


def _codex_resume_id_from_ancestors():
    """Find the nearest verified resumed Codex CLI in the MCP process tree."""
    cur = os.getppid()
    seen = set()
    for _ in range(24):
        if cur is None or cur <= 1 or cur in seen:
            break
        seen.add(cur)
        conversation_id = _codex_resume_id_from_cmdline(_cmdline_of(cur))
        if conversation_id:
            return conversation_id
        cur = _ppid_of(cur)
    return None


# ── KT-114 — fresh Codex TUI sessions: recover the native id from open FDs ──
#
# A fresh interactive Codex session has neither CODEX_THREAD_ID in the bridge's
# environment nor a `codex resume <uuid>` argv — so both recoveries above come
# up empty and the participant stays non-resumable. But the CLI itself holds
# its session file open for its whole life: `rollout-<ts>-<uuid>.jsonl`, whose
# FIRST line is a `session_meta` the CLI wrote about itself. Reading which file
# an ANCESTOR process keeps open is a fact, not an inference — verified live on
# 2026-07-29 for both a resumed TUI and a fresh `codex exec`.

_ROLLOUT_NAME_RE = re.compile(r"rollout-[0-9T:.-]+-([0-9a-f-]{36})\.jsonl$")
# Hard cap per subprocess call: the bridge sits on the critical path of every
# tool call, and `lsof` can hang on dead network mounts. Better no Resume
# button than a frozen MCP.
_FD_PROBE_TIMEOUT_SECS = 2
_CODEX_FD_PROBE_CACHE = {"done": False, "uuid": None}
# Discs whose backend row already carries this bridge's conversation_id, so the
# wait loop stops repeating a value that landed.
_CONVERSATION_ID_DELIVERED = {}


def _open_rollout_paths_of(pid):
    """Rollout session files `pid` holds open. Linux/WSL via /proc/<pid>/fd,
    macOS via `lsof -p` — the same platform split `_cmdline_of` uses. Any
    failure degrades to an empty list, never an exception."""
    paths = []
    fd_dir = f"/proc/{pid}/fd"
    if os.path.isdir(fd_dir):
        try:
            for entry in os.listdir(fd_dir):
                try:
                    target = os.readlink(os.path.join(fd_dir, entry))
                except OSError:
                    continue
                if _ROLLOUT_NAME_RE.search(target):
                    paths.append(target)
            return paths
        except OSError:
            return []
    try:
        out = subprocess.check_output(
            ["lsof", "-p", str(pid), "-Fn"],
            stderr=subprocess.DEVNULL, timeout=_FD_PROBE_TIMEOUT_SECS,
        ).decode("utf-8", errors="replace")
    except Exception:
        return []
    for line in out.splitlines():
        if line.startswith("n") and _ROLLOUT_NAME_RE.search(line):
            paths.append(line[1:])
    return paths


def _rollout_session_meta(path):
    """First line of a rollout file, parsed. `None` on any malformation."""
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as fh:
            first = fh.readline(64 * 1024)
        payload = json.loads(first).get("payload")
        return payload if isinstance(payload, dict) else None
    except Exception:
        return None


def _codex_id_from_open_rollouts():
    """UUID of the ONE rollout an ancestor Codex process keeps open.

    Acceptance rule agreed with Codex on 2026-07-29: exactly one distinct
    rollout across the ancestor chain, whose first line is a valid
    `session_meta` with a canonical `session_id` matching the filename,
    `originator == "codex-tui"` and `source == "cli"`. Anything else — zero
    rollouts, two, a malformed first line, or a `codex_exec` run that also
    keeps its file open — returns `None`: Kronn never invents provenance.
    Cached per bridge process; `lsof` is too costly for every tool call.
    """
    if _CODEX_FD_PROBE_CACHE["done"]:
        return _CODEX_FD_PROBE_CACHE["uuid"]
    resolved = None
    found = set()
    cur = os.getppid()
    seen = set()
    for _ in range(24):
        if cur is None or cur <= 1 or cur in seen:
            break
        seen.add(cur)
        for path in _open_rollout_paths_of(cur):
            match = _ROLLOUT_NAME_RE.search(path)
            if match:
                found.add((match.group(1), path))
        cur = _ppid_of(cur)
    if len({uuid_ for uuid_, _ in found}) == 1:
        raw, path = next(iter(found))
        meta = _rollout_session_meta(path)
        try:
            canonical = str(uuid.UUID(raw))
        except (AttributeError, ValueError):
            canonical = None
        if (
            canonical is not None
            and raw.lower() == canonical
            and isinstance(meta, dict)
            and meta.get("session_id") == canonical
            and meta.get("originator") == "codex-tui"
            and meta.get("source") == "cli"
        ):
            resolved = canonical
    _CODEX_FD_PROBE_CACHE["done"] = True
    _CODEX_FD_PROBE_CACHE["uuid"] = resolved
    return resolved


def _parent_process_cmdline():
    """Read the direct parent's cmdline on every supported platform.

    `_cmdline_of` already handles Linux/WSL through `/proc` and macOS through
    `ps`. Keeping a second Linux-only implementation here made host-launched
    Vibe sessions appear as `Unknown` on macOS whenever its MCP `clientInfo`
    name was generic.
    """
    return _cmdline_of(os.getppid())


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
_LAST_RESUME_ERROR = None
_LAST_READ_SORT_ORDER_BY_DISC = {}
_PENDING_READ_SORT_ORDER_BY_DISC = {}
# KT-189 — two-phase awareness acknowledgement, mirroring the read cursor:
# an awareness batch is STAGED when its response is emitted and COMMITTED by
# the model's next tool call; only committed values are echoed to the server
# as `ack_awareness_upto`, which is the sole thing that advances the durable
# per-session awareness cursor. A cancelled call purges its stage, so a
# delivery the model never saw is replayed instead of skipped.
_PENDING_AWARENESS_ACK_BY_DISC = {}
_ACKED_AWARENESS_UPTO_BY_DISC = {}
_RPC_SEQUENCE = 0
_CURRENT_RPC_SEQUENCE = None


def _set_current_disc_id(disc_id):
    """Mutate the disc binding (used by `disc_join`). Pass `None` to
    clear (used by `disc_leave`). Side-effect : invalidates the cached
    disc meta so the next read goes to the new disc."""
    global _CURRENT_DISC_ID
    _CURRENT_DISC_ID = disc_id
    _CURRENT_DISC_META_CACHE["checked"] = False
    _CURRENT_DISC_META_CACHE["value"] = None


def _set_read_cursor(disc_id, sort_order):
    """Remember only a cursor the bridge has actually delivered to its caller."""
    if (
        isinstance(disc_id, str)
        and disc_id
        and isinstance(sort_order, int)
        and not isinstance(sort_order, bool)
        and sort_order >= -1
    ):
        current = _LAST_READ_SORT_ORDER_BY_DISC.get(disc_id)
        if current is None or sort_order > current:
            _LAST_READ_SORT_ORDER_BY_DISC[disc_id] = sort_order


def _read_cursor(disc_id):
    return _LAST_READ_SORT_ORDER_BY_DISC.get(disc_id)


def _stage_read_cursor(disc_id, sort_order):
    """Keep a delivered batch pending until the CLI makes another tool call."""
    if (
        not isinstance(disc_id, str)
        or not disc_id
        or not isinstance(sort_order, int)
        or isinstance(sort_order, bool)
        or sort_order < -1
    ):
        return
    committed = _read_cursor(disc_id)
    if committed is not None and sort_order <= committed:
        return
    pending = _PENDING_READ_SORT_ORDER_BY_DISC.get(disc_id)
    if pending is None or sort_order > pending["sort_order"]:
        _PENDING_READ_SORT_ORDER_BY_DISC[disc_id] = {
            "sort_order": sort_order,
            "rpc_sequence": _CURRENT_RPC_SEQUENCE,
        }


# 0.9.0 — presence root-fix: reload recovery via a persisted resume
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


def _legacy_codex_binding_path():
    """Return the pre-KT-82 PID-keyed binding path for this Codex process.

    The first MCP reload after upgrading still needs to consume the credential
    written by the old bridge. The outer Codex process has not changed during
    that reload, so its historical key remains discoverable. A successful
    resume then writes the rotated credential at the new conversation-keyed
    path, which survives later full CLI reboots. Missing/ambiguous ancestry
    simply disables the compatibility read.
    """
    identity = _binding_identity()
    if not identity or identity[0] != "codex-session":
        return None
    legacy_identity = _cli_ancestor_identity()
    legacy_key = _identity_key_from(legacy_identity)
    if not legacy_key:
        return None
    legacy_path = os.path.join(_BINDING_DIR, f"disc-binding-{legacy_key}.json")
    return None if legacy_path == _binding_path() else legacy_path


def _durable_session_id():
    """The CLI session id that SURVIVES an MCP reload, or `None`.

    KT-76 — `_session_id_for_caller()` is the bridge PROCESS identity and is
    documented to rotate on reconnect, so it cannot anchor a lasting link. The
    resume credential already relies on a stronger key (terminal+project, or
    `CLAUDE_CODE_SESSION_ID`, or the outermost CLI); reuse exactly that one so
    `disc_find_by_session` keeps finding the room after a reload. Fail-closed:
    no durable identity means no link rather than a link that lies.
    """
    key = _identity_key_from(_binding_identity())
    return f"cli-{key}" if key else None


def _bind_session_to_disc(disc_id, agent_type):
    """Link the durable CLI session to `disc_id`. Returns a dict the caller can
    hand back to the agent verbatim.

    Never passes `force_reassign`: stealing a session that another discussion
    owns would silently redirect that other room's reconnects.
    """
    session_id = _durable_session_id()
    if not session_id:
        return {"session_bound": False, "session_bind_skipped": "no durable CLI identity"}
    if not agent_type or agent_type == "Unknown":
        return {"session_bound": False, "session_bind_skipped": "unknown agent type"}
    try:
        _unwrap(_http("POST", "/api/disc/link", {
            "disc_id": disc_id,
            "source_agent": agent_type,
            "source_session_id": session_id,
        }))
    except Exception as exc:
        # A session already owned by ANOTHER disc lands here — reported, not
        # forced, and never fatal to the join itself.
        return {"session_bound": False, "session_bind_error": str(exc)}
    return {"session_bound": True, "session_id": session_id}


def _write_binding(
    disc_id,
    resume_token,
    agent_type=None,
    pending_resume_token=None,
    last_read_sort_order=None,
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
        existing = _read_binding_path(path)
        if (
            last_read_sort_order is None
            and existing
            and existing.get("disc_id") == disc_id
        ):
            last_read_sort_order = existing.get("last_read_sort_order")

        fd, tmp = tempfile.mkstemp(prefix=".disc-binding-", suffix=".tmp", dir=_BINDING_DIR)
        try:
            with os.fdopen(fd, "w") as f:
                state = {"disc_id": disc_id, "resume_token": resume_token}
                if agent_type:
                    state["agent_type"] = agent_type
                if pending_resume_token:
                    state["pending_resume_token"] = pending_resume_token
                if (
                    isinstance(last_read_sort_order, int)
                    and not isinstance(last_read_sort_order, bool)
                    and last_read_sort_order >= -1
                ):
                    state["last_read_sort_order"] = last_read_sort_order
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


def _read_binding_path(path):
    """Securely read one binding path, or return `None`.

    Refuse anything an attacker could have swapped in: a symlink
    (O_NOFOLLOW), a non-regular file, one not owned by us, or one readable by
    group/world.
    """
    import stat as _stat
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


def _read_binding():
    """Read the current credential, with one Codex upgrade bridge.

    A bridge written before Codex conversation-keyed bindings stored its file
    under the outer process identity. During the first MCP-only reload that
    same ancestor is still available, so accept the secure legacy file once.
    The successful resume rotates and writes the credential to the new primary
    path; a later full `codex resume` reboot then finds it by conversation id.
    """
    current = _read_binding_path(_binding_path())
    if current is not None:
        return current
    return _read_binding_path(_legacy_codex_binding_path())


def _clear_binding():
    path = _binding_path()
    if not path:
        return
    try:
        os.remove(path)
    except Exception:
        pass


def _commit_read_cursor(disc_id, sort_order):
    """Advance the durable read cursor without conflating it with a write.

    Memory is authoritative for this bridge process. Persistence is best
    effort: a bridge without a durable identity remains correct until reload,
    while a bound CLI resumes from the exact last wait it consumed.
    """
    _set_read_cursor(disc_id, sort_order)
    cursor = _read_cursor(disc_id)
    if cursor is None:
        return
    with _binding_transaction_lock() as locked:
        if not locked:
            return
        binding = _read_binding()
        if not binding or binding.get("disc_id") != disc_id:
            return
        persisted = binding.get("last_read_sort_order")
        if isinstance(persisted, int) and persisted >= cursor:
            return
        _write_binding(
            disc_id,
            binding["resume_token"],
            agent_type=binding.get("agent_type"),
            pending_resume_token=binding.get("pending_resume_token"),
            last_read_sort_order=cursor,
        )


def _ack_pending_read_cursors(next_rpc_sequence):
    """Commit batches after any subsequent tool call reaches the bridge.

    The sequence is generated locally instead of trusting the JSON-RPC `id`:
    clients may legally reuse an id after receiving a response or omit it.
    This is a transport-consumption acknowledgement, not proof that the model
    semantically processed every delivered message.
    """
    for disc_id, pending in list(_PENDING_READ_SORT_ORDER_BY_DISC.items()):
        if pending.get("rpc_sequence") == next_rpc_sequence:
            continue
        _commit_read_cursor(disc_id, pending.get("sort_order"))
        if _read_cursor(disc_id) >= pending.get("sort_order"):
            _PENDING_READ_SORT_ORDER_BY_DISC.pop(disc_id, None)
    for disc_id, pending in list(_PENDING_AWARENESS_ACK_BY_DISC.items()):
        if pending.get("rpc_sequence") == next_rpc_sequence:
            continue
        committed = _ACKED_AWARENESS_UPTO_BY_DISC.get(disc_id)
        if committed is None or pending["upto"] > committed:
            _ACKED_AWARENESS_UPTO_BY_DISC[disc_id] = pending["upto"]
        _PENDING_AWARENESS_ACK_BY_DISC.pop(disc_id, None)


def _discard_pending_read_cursors(rpc_sequence):
    """Un-stage a cancelled call's deliveries (KT-189 review P0).

    A batch staged during a cancelled call never reached the model — the
    response is discarded — so letting the NEXT call acknowledge it would
    silently mark unseen messages as read. Dropping the stage means they
    are re-delivered on the next wait; a duplicate in context is cheap,
    a lost message is not. Awareness stages follow the same rule.
    """
    if rpc_sequence is None:
        return
    for disc_id, pending in list(_PENDING_READ_SORT_ORDER_BY_DISC.items()):
        if pending.get("rpc_sequence") == rpc_sequence:
            _PENDING_READ_SORT_ORDER_BY_DISC.pop(disc_id, None)
    for disc_id, pending in list(_PENDING_AWARENESS_ACK_BY_DISC.items()):
        if pending.get("rpc_sequence") == rpc_sequence:
            _PENDING_AWARENESS_ACK_BY_DISC.pop(disc_id, None)


def _stage_awareness_ack(disc_id, upto):
    """Stage the highest awareness sort_order emitted in this call."""
    if not isinstance(disc_id, str) or not disc_id:
        return
    if not isinstance(upto, int) or isinstance(upto, bool):
        return
    committed = _ACKED_AWARENESS_UPTO_BY_DISC.get(disc_id)
    if committed is not None and upto <= committed:
        return
    pending = _PENDING_AWARENESS_ACK_BY_DISC.get(disc_id)
    if pending is None or upto > pending["upto"]:
        _PENDING_AWARENESS_ACK_BY_DISC[disc_id] = {
            "upto": upto,
            "rpc_sequence": _CURRENT_RPC_SEQUENCE,
        }


def _clear_promoted_legacy_codex_binding(disc_id, resume_token):
    """Remove the obsolete PID-keyed credential after a proven promotion.

    Match the exact credential that was consumed before unlinking: a malformed,
    weakly protected, replaced, or unrelated legacy path is left untouched.
    """
    path = _legacy_codex_binding_path()
    state = _read_binding_path(path)
    if (
        not path
        or not state
        or state.get("disc_id") != disc_id
        or state.get("resume_token") != resume_token
    ):
        return
    try:
        os.remove(path)
    except OSError:
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
    global _LAST_RESUME_ERROR
    _LAST_RESUME_ERROR = None
    with _binding_transaction_lock() as locked:
        if not locked:
            return None
        b = _read_binding()
        if not b:
            return None
        disc_id_before = b["disc_id"]
        old_token = b["resume_token"]
        stored_agent_type = b.get("agent_type")
        inferred_agent_type = _agent_type_for_session()
        # A pre-fix binding may have persisted the fallback `Unknown` even
        # though a later bridge can identify its host CLI correctly. Preserve
        # every known identity across reloads, but let this one placeholder
        # self-heal. The backend accepts the same narrow Unknown → known
        # promotion when authenticated by this binding's resume credential.
        agent_type = (
            inferred_agent_type
            if stored_agent_type in (None, "Unknown") and inferred_agent_type != "Unknown"
            else stored_agent_type or inferred_agent_type
        )
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
                last_read_sort_order=b.get("last_read_sort_order"),
            ):
                return None  # never mutate the server before pending is durable

        body = {
            "agent_type": agent_type,
            "session_id": _session_id_for_caller(),
            "resume_token": old_token,
            "next_resume_token": next_token,
            "expected_disc_id": disc_id_before,
        }
        conversation_id = _native_conversation_id()
        if conversation_id:
            body["conversation_id"] = conversation_id
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
        # Promotion failure is recoverable: the pending file still contains
        # `(old,next)` and the backend accepts that exact replay.
        promoted = _write_binding(
            disc_id,
            next_token,
            agent_type=agent_type,
            last_read_sort_order=b.get("last_read_sort_order"),
        )
        if promoted:
            _clear_promoted_legacy_codex_binding(disc_id_before, old_token)
        # KT-76 — re-link on resume too. The durable key can legitimately change
        # between reloads (new terminal, moved project), which would leave the
        # old link pointing at a room this identity no longer uses. Rebinding the
        # disc we just re-attached to is safe by definition.
        bind_result = _bind_session_to_disc(disc_id, agent_type)
        if bind_result.get("session_bind_error"):
            _LAST_RESUME_ERROR = bind_result["session_bind_error"]
            return None
        _set_current_disc_id(disc_id)
        _set_read_cursor(disc_id, b.get("last_read_sort_order"))
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
        # Reload recovery (0.9.0): the in-memory binding is lost on every
        # MCP reconnect. Before failing, try to re-attach to the disc we were
        # bound to via the persisted resume credential — the whole point is
        # the human no longer re-pastes a kr-join token after each reload.
        resumed = _attempt_resume()
        if resumed:
            return resumed
        detail = f" Resume refused: {_LAST_RESUME_ERROR}." if _LAST_RESUME_ERROR else ""
        raise RuntimeError(
            "no disc bound — set KRONN_DISCUSSION_ID env (Kronn-launched) "
            "or call disc_join({token: \"kr-join-...\"}) first (host-launched)."
            + detail
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


def _http(method, path, body=None, timeout=180):
    url = f"{_backend_url()}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, method=method, data=data)
    req.add_header("Content-Type", "application/json")
    token = os.environ.get("KRONN_AUTH_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {e.code}: {body[:500]}")


def _http_transport_retry(
    method, path, attempts=6, delays=(2, 4, 8, 12, 16), timeout=180,
    deadline=None, body=None,
):
    """`_http` with a BOUNDED retry on TRANSPORT failures only (connection
    refused/reset, remote disconnect, socket timeout) — the signature of a
    backend restart, e.g. the dev watcher rebuilding for 30-60s. HTTP errors
    (4xx/5xx) are application-level and never retried. Safe only for
    idempotent calls: the caller re-sends the same request verbatim.
    Total worst-case wait ≈ sum(delays) ≈ 42s + in-flight time."""
    last_err = None
    for i in range(attempts):
        attempt_timeout = timeout
        if deadline is not None:
            # The budget bounds the IN-FLIGHT attempt too, not only the
            # backoff between attempts (KT-189 review residual 1).
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                if last_err is None:
                    last_err = TimeoutError("wait budget exhausted before attempt")
                break
            attempt_timeout = max(2, min(timeout, int(remaining) + 1))
        try:
            return _http(method, path, body=body, timeout=attempt_timeout)
        except RuntimeError:
            raise  # HTTPError path from _http — application error, no retry
        except (urllib.error.URLError, ConnectionError, TimeoutError, OSError) as e:
            last_err = e
            if i + 1 < attempts:
                delay = delays[min(i, len(delays) - 1)]
                if deadline is not None and time.monotonic() + delay >= deadline:
                    break  # the caller's budget outranks the retry ladder
                time.sleep(delay)
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


def _disc_append_attachment_paths(raw_paths):
    """Validate and resolve local files an agent wants to publish in a room.

    The bridge reads regular files only and keeps the same 10 MB ceiling as
    Kronn's image/document extractor. Paths are resolved here, in the CLI host
    process, so Docker-backed Kronn never has to understand a host path.
    """
    if raw_paths is None:
        return []
    if not isinstance(raw_paths, list) or not raw_paths:
        raise RuntimeError(
            "disc_append: attachments must be a non-empty array of local file paths"
        )
    if len(raw_paths) > MAX_DISC_APPEND_ATTACHMENTS:
        raise RuntimeError(
            f"disc_append: at most {MAX_DISC_APPEND_ATTACHMENTS} attachments "
            "can be published with one message"
        )

    resolved = []
    names = set()
    for raw_path in raw_paths:
        if not isinstance(raw_path, str) or not raw_path.strip():
            raise RuntimeError(
                "disc_append: every attachment must be a non-empty local file path"
            )
        path = os.path.realpath(os.path.abspath(os.path.expanduser(raw_path.strip())))
        if not os.path.isfile(path):
            raise RuntimeError(
                f"disc_append: attachment is not a readable regular file: {raw_path}"
            )
        size = os.path.getsize(path)
        if size == 0:
            raise RuntimeError(f"disc_append: attachment is empty: {raw_path}")
        if size > MAX_DISC_APPEND_ATTACHMENT_BYTES:
            raise RuntimeError(
                f"disc_append: attachment exceeds the 10 MB limit: {raw_path}"
            )
        filename = os.path.basename(path)
        folded = filename.casefold()
        if folded in names:
            raise RuntimeError(
                "disc_append: attachment filenames must be unique within one "
                f"message (duplicate: {filename})"
            )
        names.add(folded)
        resolved.append(path)
    return resolved


def _http_upload_context_file(disc_id, file_path):
    """Upload one local file through the existing authenticated context route."""
    boundary = f"----kronn-mcp-{secrets.token_hex(16)}"
    filename = os.path.basename(file_path)
    safe_filename = "".join(
        char for char in filename if char not in {'"', '\r', '\n'}
    ) or "attachment"
    with open(file_path, "rb") as handle:
        file_bytes = handle.read()
    prefix = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{safe_filename}"\r\n'
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode("utf-8")
    payload = prefix + file_bytes + f"\r\n--{boundary}--\r\n".encode("ascii")
    disc_segment = urllib.parse.quote(disc_id, safe="")
    req = urllib.request.Request(
        f"{_backend_url()}/api/discussions/{disc_segment}/context-files",
        method="POST",
        data=payload,
    )
    req.add_header("Content-Type", f"multipart/form-data; boundary={boundary}")
    token = os.environ.get("KRONN_AUTH_TOKEN")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=180) as resp:
            return _unwrap(json.load(resp))
    except urllib.error.HTTPError as exc:
        response_body = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {exc.code}: {response_body[:500]}")


def _rollback_uploaded_context_files(disc_id, uploaded):
    """Compensate a failed attachment batch so retry starts from a clean slate."""
    failures = []
    disc_segment = urllib.parse.quote(disc_id, safe="")
    for item in reversed(uploaded):
        file_id = item.get("id")
        if not file_id:
            continue
        try:
            _unwrap(_http(
                "DELETE",
                f"/api/discussions/{disc_segment}/context-files/"
                f"{urllib.parse.quote(file_id, safe='')}",
            ))
        except Exception as exc:  # noqa: BLE001 — report every cleanup failure
            failures.append(f"{file_id}: {type(exc).__name__}: {exc}")
    return failures


def _attach_files_to_appended_message(disc_id, message_id, paths, duplicate):
    """Upload files and atomically pin this batch to the appended message."""
    already_attached = set()
    if duplicate:
        selector = urllib.parse.quote(message_id, safe="")
        existing = _unwrap(_http(
            "GET",
            f"/api/discussions/{urllib.parse.quote(disc_id, safe='')}/message/"
            f"{selector}?before=0&after=0",
        ))
        already_attached = {
            item.get("filename", "").casefold()
            for item in (existing.get("attachments") or [])
            if isinstance(item, dict)
        }

    uploaded = []
    skipped = []
    linked = 0
    try:
        for path in paths:
            filename = os.path.basename(path)
            if filename.casefold() in already_attached:
                skipped.append(filename)
                continue
            response = _http_upload_context_file(disc_id, path)
            file_data = response.get("file") if isinstance(response, dict) else None
            file_id = file_data.get("id") if isinstance(file_data, dict) else None
            if not file_id:
                raise RuntimeError(
                    f"disc_append: upload returned no file id for {filename}"
                )
            uploaded.append({"id": file_id, "filename": filename})

        if uploaded:
            linked = _unwrap(_http_transport_retry(
                "POST",
                f"/api/discussions/{urllib.parse.quote(disc_id, safe='')}/"
                "context-files/link-pending",
                body={
                    "message_id": message_id,
                    "file_ids": [item["id"] for item in uploaded],
                },
            ))
            if linked != len(uploaded):
                raise RuntimeError(
                    "disc_append: the message was posted but Kronn linked only "
                    f"{linked}/{len(uploaded)} uploaded attachments"
                )
    except Exception as exc:  # noqa: BLE001 — compensate the whole new batch
        cleanup_failures = _rollback_uploaded_context_files(disc_id, uploaded)
        cleanup = f"; rolled back {len(uploaded)} uploaded attachment(s)"
        if cleanup_failures:
            cleanup += "; cleanup failed for " + ", ".join(cleanup_failures)
        raise RuntimeError(f"{exc}{cleanup}") from exc

    return {
        "requested": len(paths),
        "uploaded": len(uploaded),
        "already_attached": len(skipped),
        "linked": linked,
        "files": uploaded,
    }


# ─── Tool dispatch ─────────────────────────────────────────────────────────

def call_disc_meta(_args):
    """Room metadata, plus who can actually be addressed in it (KT-372 DoD-4).

    The 2026-08-21 incident was not a resolution failure: `@claude` really is
    the native agent and `@claude-cli-2` really is a joined session. The author
    simply could not see, at the moment of writing, that both existed. Refusing
    afterwards helps; showing the identities beforehand is what prevents it.

    So the cheap call an agent already makes before acting now carries the exact
    aliases — no second HTTP round trip, and no internal session pk to know.
    """
    disc_id = _disc_id()
    meta = _unwrap(_http("GET", f"/api/discussions/{disc_id}/meta"))
    if not isinstance(meta, dict):
        return meta
    try:
        participants = _unwrap(
            _http("GET", f"/api/discussions/{disc_id}/participants")
        )
    except Exception:  # noqa: BLE001
        # Advisory field: metadata stays useful without it. Never fail the call
        # a reader depends on for something that only enriches it.
        return meta
    if not isinstance(participants, list):
        return meta

    addressable = []
    principal = meta.get("agent")
    if principal:
        alias = _ALIAS_BY_AGENT_TYPE.get(principal, str(principal).lower())
        addressable.append({
            "mention": f"@{alias}",
            "kind": "discussion_agent",
            "agent_type": principal,
            "note": "the room's own agent — NOT a joined CLI session",
        })
    for participant in participants:
        agent_type = participant.get("agent_type")
        ordinal = participant.get("cli_ordinal")
        if not agent_type or not ordinal:
            continue
        alias = _ALIAS_BY_AGENT_TYPE.get(agent_type, str(agent_type).lower())
        suffix = "" if int(ordinal) == 1 else f"-{int(ordinal)}"
        addressable.append({
            "mention": f"@{alias}-cli{suffix}",
            "kind": "cli",
            "agent_type": agent_type,
            "note": "a joined CLI session; the bare @alias would reach the native agent instead",
        })
    meta["addressable"] = addressable
    # Named so it can be checked without scanning the list: this is the exact
    # condition under which a bare alias is refused.
    meta["ambiguous_aliases"] = sorted({
        entry["agent_type"] for entry in addressable if entry["kind"] == "cli"
    } & {
        entry["agent_type"] for entry in addressable if entry["kind"] == "discussion_agent"
    })
    return meta


def call_resolve_id(args):
    object_id = (args.get("id") or "").strip()
    if not object_id:
        raise RuntimeError("resolve_id: id is required")
    encoded = urllib.parse.quote(object_id, safe="")
    return _unwrap(_http("GET", f"/api/resolve/{encoded}"))


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

def call_disc_note_list(args):
    params = {}
    if args.get("cursor") is not None:
        params["cursor"] = args["cursor"]
    if args.get("limit") is not None:
        params["limit"] = args["limit"]
    query = urllib.parse.urlencode(params)
    suffix = f"?{query}" if query else ""
    return _unwrap(_http("GET", f"/api/discussions/{_disc_id()}/notes{suffix}"))


def call_disc_summarize(args):
    body = {
        "from": args.get("from"),
        "to": args.get("to"),
        "force_refresh": bool(args.get("force_refresh", False)),
        "include_notes": bool(args.get("include_notes", False)),
    }
    return _unwrap(_http("POST", f"/api/discussions/{_disc_id()}/summarize", body))


# ─── 0.9.1 planning / discussion plans ─────────────────────────────────

def _planning_actor(args):
    actor = {"kind": "agent", "id": _agent_type_for_session()}
    session_id = _durable_session_id()
    if session_id:
        actor["session_id"] = session_id
    source_message_id = args.get("source_message_id")
    if source_message_id:
        actor["source_message_id"] = source_message_id
    return actor


def _planning_discussion_id(args):
    return (args.get("discussion_id") or "").strip() or _disc_id()


def call_plan_get(args):
    discussion_id = urllib.parse.quote(_planning_discussion_id(args), safe="")
    return _unwrap(_http("GET", f"/api/discussions/{discussion_id}/plan"))


def call_task_list(args):
    allowed = (
        "search", "status", "priority", "project_id", "discussion_id", "tag",
        "with_discussion", "cursor", "limit",
    )
    query = {}
    for key in allowed:
        if key in args and args[key] is not None:
            value = args[key]
            query[key] = str(value).lower() if isinstance(value, bool) else value
    suffix = f"?{urllib.parse.urlencode(query)}" if query else ""
    return _unwrap(_http("GET", f"/api/planning/tasks{suffix}"))


def call_task_get(args):
    task_id = (args.get("task_id") or "").strip()
    if not task_id:
        raise RuntimeError("task_get: task_id is required")
    encoded = urllib.parse.quote(task_id, safe="")
    return _unwrap(_http("GET", f"/api/planning/tasks/{encoded}"))


def call_proposal_list(args):
    # 0.9.2-H — read-only inbox of durable Planning proposals. Agents may READ
    # proposals; only a human accepts/rejects (no mutation tool is exposed).
    query = {"discussion_id": _planning_discussion_id(args)}
    if args.get("pending_only") is not None:
        query["pending_only"] = str(args["pending_only"]).lower()
    suffix = f"?{urllib.parse.urlencode(query)}"
    return _unwrap(_http("GET", f"/api/planning/proposals{suffix}"))


def call_proposal_get(args):
    proposal_id = (args.get("proposal_id") or "").strip()
    if not proposal_id:
        raise RuntimeError("proposal_get: proposal_id is required")
    encoded = urllib.parse.quote(proposal_id, safe="")
    return _unwrap(_http("GET", f"/api/planning/proposals/{encoded}"))


def call_task_changes(args):
    discussion_id = urllib.parse.quote(_planning_discussion_id(args), safe="")
    query = {}
    if args.get("since"):
        query["since"] = args["since"]
    suffix = f"?{urllib.parse.urlencode(query)}" if query else ""
    return _unwrap(_http(
        "GET", f"/api/discussions/{discussion_id}/plan/changes{suffix}"
    ))


# KT-250 — a narrow write gets a narrow receipt.
#
# The planning API answers every write with the WHOLE task: description, full
# DoD, and the event log — in which an earlier `updated` event still holds a
# verbatim copy of the previous description. So the reply GROWS with the task's
# history, and ticking the fifth checkbox costs more than the first. Measured on
# KT-249: 11 512 B per call, 6 437 of them the event log, with the description
# shipped twice (3 438 B as the field, 3 400 B again inside an event). Five
# booleans cost about 57 KB.
#
# The UI keeps the full payload — it renders history. An agent ticking a box
# already knows the task; it needs proof the write landed and nothing else.
_TASK_ACK_FIELDS = (
    "id", "reference", "title", "status", "priority",
    "parent_reference", "blocker_count",
)


def _task_ack(task, dod_id=None):
    """What an agent needs to trust a write, and no more.

    `dod_id` names the checklist item this call touched: its own state comes
    back so the agent can confirm the RIGHT item moved, which a bare count
    cannot show.
    """
    if not isinstance(task, dict):
        return task
    ack = {key: task[key] for key in _TASK_ACK_FIELDS if key in task}
    dod = task.get("definition_of_done")
    if isinstance(dod, list) and dod:
        done = sum(1 for item in dod if isinstance(item, dict) and item.get("completed"))
        ack["dod_progress"] = f"{done}/{len(dod)}"
        if dod_id is not None:
            touched = next(
                (item for item in dod if isinstance(item, dict) and item.get("id") == dod_id),
                None,
            )
            if touched is not None:
                ack["dod_item"] = {
                    "id": touched.get("id"),
                    "completed": touched.get("completed"),
                    "sentence": touched.get("sentence"),
                }
    # Named explicitly: silence about a dropped field reads as "there was none".
    ack["omitted"] = "description, full definition_of_done, events — call task_get for them"
    return ack


def call_task_create(args):
    title = (args.get("title") or "").strip()
    if not title:
        raise RuntimeError("task_create: title is required")
    discussion_id = _planning_discussion_id(args)
    body = {
        "title": title,
        "discussion_id": discussion_id,
        "actor": _planning_actor(args),
    }
    for key in (
        "description", "status", "priority", "parent_id", "project_ids",
        "tags", "definition_of_done", "links",
    ):
        if key in args and args[key] is not None:
            body[key] = args[key]
    explicit_key = args.get("idempotency_key")
    source_message_id = args.get("source_message_id")
    if explicit_key is not None:
        explicit_key = str(explicit_key).strip()
        if not explicit_key:
            raise RuntimeError("task_create: idempotency_key cannot be empty")
        stable_provenance = f"explicit\0{explicit_key}"
    elif source_message_id:
        stable_provenance = f"message\0{source_message_id}"
    else:
        stable_provenance = None
    if stable_provenance is not None:
        scoped = f"{discussion_id}\0{stable_provenance}".encode("utf-8")
        body["idempotency_key"] = (
            "mcp-task-create:" + hashlib.sha256(scoped).hexdigest()
        )
    return _task_ack(_unwrap(_http("POST", "/api/planning/tasks", body)))


def call_task_update(args):
    task_id = (args.get("task_id") or "").strip()
    if not task_id:
        raise RuntimeError("task_update: task_id is required")
    body = {"actor": _planning_actor(args)}
    for key in (
        "title", "description", "status", "priority", "parent_id",
        "blocked_reason", "rank", "project_ids", "tags",
        "definition_of_done", "links",
    ):
        if key in args:
            body[key] = args[key]
    encoded = urllib.parse.quote(task_id, safe="")
    return _task_ack(_unwrap(_http("PATCH", f"/api/planning/tasks/{encoded}", body)))


def call_task_link_discussion(args):
    task_id = (args.get("task_id") or "").strip()
    if not task_id:
        raise RuntimeError("task_link_discussion: task_id is required")
    body = {
        "discussion_id": _planning_discussion_id(args),
        "placement": args.get("placement", "active"),
        "is_primary": bool(args.get("is_primary", False)),
        "actor": _planning_actor(args),
    }
    if args.get("position") is not None:
        body["position"] = args["position"]
    encoded = urllib.parse.quote(task_id, safe="")
    return _task_ack(_unwrap(_http(
        "POST", f"/api/planning/tasks/{encoded}/discussions", body
    )))


def call_task_update_dod(args):
    task_id = (args.get("task_id") or "").strip()
    dod_id = (args.get("dod_id") or "").strip()
    if not task_id or not dod_id:
        raise RuntimeError("task_update_dod: task_id and dod_id are required")
    if not isinstance(args.get("completed"), bool):
        raise RuntimeError("task_update_dod: completed must be a boolean")
    encoded_task = urllib.parse.quote(task_id, safe="")
    encoded_dod = urllib.parse.quote(dod_id, safe="")
    return _task_ack(
        _unwrap(_http(
            "PATCH",
            f"/api/planning/tasks/{encoded_task}/dod/{encoded_dod}",
            {
                "completed": args["completed"],
                "actor": _planning_actor(args),
            },
        )),
        dod_id=dod_id,
    )


def call_task_add_blocker(args):
    task_id = (args.get("task_id") or "").strip()
    blocker_task_id = (args.get("blocker_task_id") or "").strip()
    if not task_id or not blocker_task_id:
        raise RuntimeError(
            "task_add_blocker: task_id and blocker_task_id are required"
        )
    encoded = urllib.parse.quote(task_id, safe="")
    return _task_ack(_unwrap(_http(
        "POST",
        f"/api/planning/tasks/{encoded}/blockers",
        {
            "blocker_task_id": blocker_task_id,
            "actor": _planning_actor(args),
        },
    )))


def call_task_remove_blocker(args):
    task_id = (args.get("task_id") or "").strip()
    blocker_task_id = (args.get("blocker_task_id") or "").strip()
    if not task_id or not blocker_task_id:
        raise RuntimeError(
            "task_remove_blocker: task_id and blocker_task_id are required"
        )
    encoded_task = urllib.parse.quote(task_id, safe="")
    encoded_blocker = urllib.parse.quote(blocker_task_id, safe="")
    return _task_ack(_unwrap(_http(
        "DELETE",
        f"/api/planning/tasks/{encoded_task}/blockers/{encoded_blocker}",
        {"actor": _planning_actor(args)},
    )))


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
    for k in (
        "language",
        "project_id",
        "source_agent",
        "source_session_id",
        "no_agent",
    ):
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


_MENTION_TARGETS = {
    "claude": "ClaudeCode",
    "codex": "Codex",
    "vibe": "Vibe",
    "gemini": "GeminiCli",
    "kiro": "Kiro",
    "copilot": "CopilotCli",
    "ollama": "Ollama",
}


def _structured_target_agent(content):
    """Return one unambiguous @agent target from conversational prose.

    Fenced and inline code are removed first so documentation/examples do not
    launch agents. Multiple distinct mentions intentionally return None: the
    append contract carries one durable responder, never an arbitrary first
    choice from a fan-out request.
    """
    if not isinstance(content, str) or not content:
        return None
    prose = re.sub(r"```.*?(?:```|$)", "", content, flags=re.DOTALL)
    prose = re.sub(r"`[^`\n]*`", "", prose)
    pattern = r"(?<![\w@])@(" + "|".join(_MENTION_TARGETS) + r")(?![\w-])"
    targets = {
        _MENTION_TARGETS[match.group(1).lower()]
        for match in re.finditer(pattern, prose, flags=re.IGNORECASE)
    }
    return next(iter(targets)) if len(targets) == 1 else None


def _structured_message_targets(content, disc_id):
    """Resolve ordered native and exact-CLI mentions from prose.

    Canonical mentions (`@codex`) are native identities. A joined CLI must be
    addressed through its stable room alias (`@codex-cli`, then
    `@codex-cli-2`, …), resolved against the participants endpoint whose
    `joined_at` ordering is also used by the UI.
    """
    if not isinstance(content, str) or not content:
        return []
    prose = re.sub(r"```.*?(?:```|$)", "", content, flags=re.DOTALL)
    prose = re.sub(r"`[^`\n]*`", "", prose)
    pattern = (
        r"(?<![\w@])@("
        + "|".join(_MENTION_TARGETS)
        + r")(-cli(?:-(\d+))?)?(?![\w-])"
    )
    matches = list(re.finditer(pattern, prose, flags=re.IGNORECASE))
    if not matches:
        return []

    meta = _current_disc_meta() or {}
    principal = meta.get("agent")
    participants = None
    targets = []
    for match in matches:
        agent_type = _MENTION_TARGETS[match.group(1).lower()]
        cli_suffix = match.group(2)
        if cli_suffix:
            if participants is None:
                participants = _unwrap(
                    _http("GET", f"/api/discussions/{disc_id}/participants")
                )
                if not isinstance(participants, list):
                    participants = []
            matching = [
                participant
                for participant in participants
                if participant.get("agent_type") == agent_type
            ]
            ordinal = int(match.group(3) or "1")
            # KT-247 — resolve through the backend's STABLE `cli_ordinal`, not the
            # position in this list. Position shifts when a session leaves, so the
            # alias the human reads in a message header and the session the MCP
            # targets could drift apart: two truths for one name.
            ranked = [p for p in matching if p.get("cli_ordinal")]
            if ranked:
                if len(ranked) != len(matching):
                    # Mixing both schemes is the one case that silently mis-targets.
                    raise RuntimeError(
                        "disc_append: this room mixes sessions with and without a "
                        "stable ordinal; refusing to guess which one "
                        f"{match.group(0)} means — reconnect the Kronn MCP so every "
                        "participant carries its ordinal"
                    )
                chosen = next(
                    (p for p in ranked if int(p["cli_ordinal"]) == ordinal), None
                )
            elif ordinal <= len(matching):
                # Server predates the ordinal: keep the historical positional
                # behaviour rather than break every alias against an old backend.
                chosen = matching[ordinal - 1]
            else:
                chosen = None
            if chosen is None:
                alias = match.group(0)
                raise RuntimeError(
                    f"disc_append: {alias} does not identify a joined CLI in "
                    "this discussion; use an alias exposed by the room"
                )
            target = {
                "kind": "cli",
                "agent_type": agent_type,
                "cli_session_id": chosen["id"],
            }
        else:
            target = {
                "kind": (
                    "discussion_agent"
                    if agent_type == principal
                    else "agent"
                ),
                "agent_type": agent_type,
            }
        if target not in targets:
            targets.append(target)
    return targets


_ALIAS_BY_AGENT_TYPE = {value: key for key, value in _MENTION_TARGETS.items()}


def _reject_ambiguous_short_alias(disc_id, targets):
    """KT-372 — a bare `@provider` is ambiguous once that provider has joined.

    `@claude` legitimately names the NATIVE identity, and a joined session is
    `@claude-cli-2`. Both are real, so neither resolution is a bug — which is
    exactly why the mistake is silent: the message is delivered, to somebody
    else, and the intended session never wakes. Observed 2026-08-21, and the
    KT-211 guard did not catch it because that one only fires inside a reply
    to a CLI-authored message.

    Refused only when ALL hold: the target came from prose (an explicit
    `targets` argument is the author saying which identity they mean), a
    native mention of provider P is present, P has at least one joined CLI in
    this room, and no exact CLI of P is listed alongside. A deliberate fan-out
    that names both passes untouched, as it already does in the reply guard.

    The bulk `messages` path is deliberately out of scope: it never derives a
    target from prose (`_structured_message_targets` runs only on the `content`
    branch), so every target it carries was supplied explicitly — the same case
    this guard exempts. There is no ambiguity to catch there, only an author
    already naming an identity.
    """
    native = [
        t for t in targets or []
        if isinstance(t, dict) and t.get("kind") in ("agent", "discussion_agent")
    ]
    if not native:
        return
    try:
        participants = _unwrap(
            _http("GET", f"/api/discussions/{disc_id}/participants")
        )
    except Exception as exc:  # noqa: BLE001
        # Cannot prove the alias is unambiguous → refuse rather than deliver to
        # an identity we did not verify. Same posture as the reply guard.
        raise RuntimeError(
            "disc_append: cannot list this room's participants to check whether "
            "a bare @mention is ambiguous; retry, or address the joined CLI with "
            "its exact -cli alias"
        ) from exc
    if not isinstance(participants, list):
        # A 200 with an unexpected shape is not "no participants": it is an
        # answer we cannot read. Passing here would let the ambiguity through on
        # the one path where the server replied — the exception branch above
        # already refuses, and a successful-but-unreadable payload deserves the
        # same treatment, not the opposite one.
        raise RuntimeError(
            "disc_append: this room's participant list came back in a shape this "
            "bridge cannot read, so a bare @mention cannot be proven unambiguous; "
            "reconnect the Kronn MCP, or address the joined CLI with its exact "
            "-cli alias"
        )
    listed_cli = {
        t.get("agent_type") for t in targets or []
        if isinstance(t, dict) and t.get("kind") == "cli"
    }
    for mention in native:
        agent_type = mention.get("agent_type")
        if agent_type in listed_cli:
            continue
        joined = [p for p in participants if p.get("agent_type") == agent_type]
        if not joined:
            continue
        alias = _ALIAS_BY_AGENT_TYPE.get(agent_type, str(agent_type).lower())
        ordinals = sorted(int(p["cli_ordinal"]) for p in joined if p.get("cli_ordinal"))
        if ordinals:
            exact = ", ".join(
                f"@{alias}-cli" + (f"-{o}" if o > 1 else "") for o in ordinals
            )
        else:
            # Server predates stable ordinals. Refuse anyway rather than let the
            # ambiguity through on an old backend, but never invent an ordinal:
            # name the form, not a rank we cannot verify.
            exact = f"@{alias}-cli[-N]"
        raise RuntimeError(
            f"disc_append: @{alias} names the NATIVE {agent_type} agent, but this "
            f"room also has {len(joined)} joined {agent_type} CLI session(s): "
            f"{exact}. Both are real identities, so this would be delivered — to "
            "the wrong one, silently. Use the exact alias for a joined session, "
            "name both for a deliberate fan-out, or pass `targets` explicitly to "
            "mean the native agent."
        )


def _reject_short_alias_reply_to_cli_author(disc_id, reply_to_message_id, targets):
    """KT-211 reply-coherence guard — fail closed on the ONE unambiguous case.

    Observed live: CLI A answers CLI B but types `@b` (short alias); the
    reply then spawns/wakes a native agent while B never wakes. Identities
    are never silently substituted (sealed 2026-08-02), but a reply context
    makes the intent explicit, so the mismatch is refused with the
    corrective alias instead of being delivered wrong.

    Refusal requires ALL of: a reply to a CLI-authored message, a native
    mention (punctual OR discussion agent — the provider may be the room's
    principal) of that same provider, and the replied author's exact CLI
    absent from the targets. A deliberate fan-out that lists the exact CLI
    alongside its native identity passes untouched. When the replied
    author or the corrective alias cannot be resolved, the append is
    refused rather than delivered unverified — fail closed, and never
    with a fabricated ordinal.
    """
    if not reply_to_message_id or not targets:
        return
    native = [
        t for t in targets
        if isinstance(t, dict) and t.get("kind") in ("agent", "discussion_agent")
    ]
    if not native:
        return
    encoded = urllib.parse.quote(str(reply_to_message_id), safe="")
    try:
        replied = _unwrap(_http(
            "GET", f"/api/discussions/{disc_id}/message/{encoded}?before=0&after=0"
        ))
    except Exception as exc:  # noqa: BLE001 — suspicious shape stays fail-closed
        raise RuntimeError(
            "disc_append: cannot verify the replied message's author while a "
            "native @mention is present in a reply; retry, or address the "
            "joined CLI with its exact -cli alias"
        ) from exc
    reply_target = replied.get("reply_target") if isinstance(replied, dict) else None
    if not isinstance(reply_target, dict) or reply_target.get("kind") != "cli":
        return
    author_type = reply_target.get("agent_type")
    mismatch = next((t for t in native if t.get("agent_type") == author_type), None)
    if mismatch is None:
        return
    # Deliberate fan-out: the exact replied author IS listed too — the
    # native mention is then an intentional second responder, not a typo.
    author_listed = any(
        isinstance(t, dict)
        and t.get("kind") == "cli"
        and t.get("agent_type") == author_type
        and t.get("cli_session_id") == reply_target.get("cli_session_id")
        for t in targets
    )
    if author_listed:
        return
    alias = _ALIAS_BY_AGENT_TYPE.get(author_type, str(author_type).lower())
    suggestion = None
    try:
        participants = _unwrap(_http("GET", f"/api/discussions/{disc_id}/participants"))
        if isinstance(participants, list):
            same = [p for p in participants if p.get("agent_type") == author_type]
            for position, participant in enumerate(same, start=1):
                if participant.get("id") == reply_target.get("cli_session_id"):
                    # Prefer the backend ordinal; position is only a fallback for a
                    # server predating it. Suggesting a positional alias against a
                    # ranked room would hand the caller a wrong identity.
                    rank = participant.get("cli_ordinal") or position
                    suggestion = f"@{alias}-cli" + (f"-{rank}" if int(rank) > 1 else "")
                    break
    except Exception:  # noqa: BLE001 — no fabricated ordinal below
        pass
    if suggestion is None:
        raise RuntimeError(
            f"disc_append: this reply answers a message authored by the joined "
            f"{author_type} CLI, but the mention names its native agent — use "
            f"that CLI's exact room alias (@{alias}-cli[-N], as shown in the "
            "room) so the reply wakes the right identity"
        )
    raise RuntimeError(
        f"disc_append: this reply answers a message authored by the joined "
        f"{author_type} CLI, but the mention names its native agent — use "
        f"'{suggestion}' (the replied author) so the reply wakes the right "
        "identity"
    )


def _legacy_agent_target(agent_type):
    """Project the old one-provider override onto a typed native identity."""
    meta = _current_disc_meta() or {}
    return {
        "kind": (
            "discussion_agent"
            if agent_type == meta.get("agent")
            else "agent"
        ),
        "agent_type": agent_type,
    }


def _live_message_id(disc_id, args):
    """Dedup key for a simple-mode append, DERIVED from the call instead of
    random.

    The backend dedups on `(disc_id, source_msg_id)`. A random id per call means
    a host that retries the tool call — after ITS own timeout, while the message
    is already stored — creates a duplicate instead of hitting that dedup. A
    retry replays the same arguments, so deriving the id from them makes the
    replay idempotent. Chaining a long wait inside the append (KT-43) stretched
    the call from ~200 ms to up to 60 s, i.e. it widened exactly that window,
    which is why this is derived now.

    Two identical messages posted DELIBERATELY would also collapse; pass an
    explicit `source_msg_id` to force a distinct one.
    """
    payload = json.dumps(
        {
            "disc": disc_id,
            "role": args.get("role") or "Agent",
            "channel": args.get("channel") or "main",
            "agent": args.get("agent_type") or _agent_type_for_session(),
            "content": args.get("content"),
            "target": args.get("target_agent"),
            "targets": args.get("targets"),
            "reply_to": args.get("reply_to_message_id"),
        },
        sort_keys=True,
        ensure_ascii=False,
    )
    return f"live-{hashlib.sha256(payload.encode()).hexdigest()[:32]}"


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
    attachment_paths = _disc_append_attachment_paths(args.get("attachments"))
    if attachment_paths and messages:
        raise RuntimeError(
            "disc_append: attachments are supported only with the simple "
            "content form, not bulk transcript imports"
        )

    # Light shorthand : an agent passed `content` directly.
    if not messages and args.get("content"):
        message = {
            "source_msg_id": args.get("source_msg_id") or _live_message_id(disc_id, args),
            "role": args.get("role") or "Agent",
            "channel": args.get("channel") or "main",
            "content": args["content"],
            "agent_type": (
                args.get("agent_type")
                or _agent_type_for_session()
                or None
            ),
        }
        if message["channel"] not in ("main", "note"):
            raise RuntimeError("disc_append: channel must be 'main' or 'note'")
        target_agent = args.get("target_agent")
        targets = args.get("targets")
        prose_derived = False
        if message["channel"] == "note":
            target_agent = None
            targets = []
        elif targets is None and message["role"] == "Agent":
            if target_agent:
                targets = [_legacy_agent_target(target_agent)]
            else:
                targets = _structured_message_targets(message["content"], disc_id)
                prose_derived = True
        if targets:
            message["targets"] = targets
            # Compatibility projection for pre-KT-116 servers/readers.
            message["target_agent"] = targets[0]["agent_type"]
        elif target_agent:
            message["target_agent"] = target_agent
        if args.get("reply_to_message_id"):
            message["reply_to_message_id"] = args["reply_to_message_id"]
            _reject_short_alias_reply_to_cli_author(
                disc_id, message["reply_to_message_id"], message.get("targets")
            )
        # After the reply guard on purpose: inside a reply it names the exact
        # session that authored the replied message, which is a better answer
        # than the list of candidates this one can give.
        if prose_derived:
            _reject_ambiguous_short_alias(disc_id, message.get("targets"))
        messages = [message]

    if not isinstance(messages, list) or not messages:
        raise RuntimeError(
            "disc_append: pass either `content: \"...\"` (single message, "
            "easiest for multi-agent chat) OR `messages: [{source_msg_id, "
            "role, content}, …]` (bulk transcript import)"
        )
    is_live_single = bool(args.get("content")) and len(messages) == 1
    # 0.9.0 — carry the caller's session id so the backend scopes the
    # append heartbeat + activity-clear to THIS (possibly resumed) row only,
    # never to every session of the same agent_type (multi-machine / sibling
    # peer safety). A legacy bridge that omits it gets no presence refresh on
    # append — deliberately conservative, matching disc_wait_for_peer.
    append_body = {
        "disc_id": disc_id,
        "messages": messages,
        "session_id": _session_id_for_caller(),
    }
    consumed_cursor = _read_cursor(disc_id)
    if consumed_cursor is not None:
        append_body["since_sort_order"] = consumed_cursor
    appended = _unwrap(_http("POST", "/api/disc/append", append_body))

    if attachment_paths:
        message_id = appended.get("last_message_id") if isinstance(appended, dict) else None
        if not message_id:
            appended["attachment_error"] = (
                "The message was posted, but this backend did not return its "
                "durable message id. Reload Kronn after updating it, then retry "
                "the same disc_append call to publish the attachments."
            )
        else:
            try:
                appended["attachments"] = _attach_files_to_appended_message(
                    disc_id,
                    message_id,
                    attachment_paths,
                    bool(appended.get("skipped_as_duplicates")),
                )
            except Exception as exc:  # noqa: BLE001 — the message already exists
                appended["attachment_error"] = f"{type(exc).__name__}: {exc}"
                appended["attachment_retry"] = (
                    "The text message was posted. Retry the SAME disc_append "
                    "arguments/source_msg_id; the failed attachment batch was "
                    "rolled back so only missing files will be uploaded."
                )

    # KT-43 — post AND listen in ONE tool call.
    #
    # Every append used to hand the turn back, leaving the next wait to the
    # agent's discipline; a runtime that forgets goes `offline` and the human
    # reads it as "it left the room" (measured live on 2026-07-28: a Claude
    # session posted, never reopened a wait, and showed offline for 20 min).
    # Raising the wait ceiling only helps agents that already re-poll, so the
    # fix has to be structural: chaining the long-poll here makes speaking and
    # listening the same call. The wait must resume from the last position
    # actually READ, not this append's write receipt: another message can land
    # between those two positions.
    #
    # Never for a bulk transcript import (nobody is waiting on a backfill),
    # and `wait_for_reply: false` opts out for an agent posting twice in a row.
    if not is_live_single or args.get("wait_for_reply") is False:
        return appended

    cursor = _read_cursor(disc_id)
    try:
        wait_args = {
            "_disc_id": disc_id,
            "timeout_secs": args.get("wait_timeout_secs"),
        }
        if cursor is not None:
            wait_args["since_sort_order"] = cursor
        # Single poll only: an append should come back quickly to let the
        # agent keep working. The bridge-side long wait (KT-189) belongs to
        # an explicit disc_wait_for_peer call.
        waited = _wait_once(wait_args)
    except Exception as exc:  # noqa: BLE001 — the append already succeeded
        # The message IS posted; a failed wait must not read as a failed post.
        appended["wait_error"] = f"{type(exc).__name__}: {exc}"
        appended["hint"] = (
            "Your message was posted, but the chained wait failed. Call "
            "disc_wait_for_peer yourself to get back into the room."
        )
        return appended

    appended["waited"] = waited
    if isinstance(waited, dict) and isinstance(waited.get("latest_sort_order"), int):
        appended["pending_read_cursor"] = waited["latest_sort_order"]
    if isinstance(waited, dict) and waited.get("hint"):
        # Lift it, don't copy it: the same ~400 B of unchanging protocol text was
        # shipping twice per response (here and inside `waited`). Over a long
        # multi-agent session that repetition costs more than the entire
        # documentation bootstrap. One copy, at the level callers read.
        appended["hint"] = waited.pop("hint")
    return appended


def call_disc_link(args):
    # KT-76 — an agent cannot know its own durable key (it is derived inside the
    # bridge), so both identity fields fall back to what this session actually
    # is. `disc_id` still defaults to the bound disc, making the common case
    # `disc_link({})`.
    disc_id = args.get("disc_id") or _disc_id()
    source_agent = args.get("source_agent") or _agent_type_for_session()
    source_session_id = args.get("source_session_id") or _durable_session_id()
    if not source_session_id:
        raise RuntimeError(
            "disc_link: no durable CLI session id for this bridge — pass "
            "source_session_id explicitly"
        )
    if not source_agent or source_agent == "Unknown":
        raise RuntimeError("disc_link: could not infer source_agent — pass it explicitly")
    return _unwrap(_http("POST", "/api/disc/link", {
        "disc_id": disc_id,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
        "force_reassign": bool(args.get("force_reassign", False)),
    }))


def call_disc_transfer_session(args):
    if args.get("confirm_transfer") is not True:
        raise RuntimeError(
            "disc_transfer_session: confirm_transfer=true is required after an "
            "explicit human request to change rooms"
        )
    from_disc_id = (args.get("from_disc_id") or "").strip()
    if not from_disc_id:
        raise RuntimeError(
            "disc_transfer_session: from_disc_id is required; read the current "
            "owner with disc_find_by_session({}) first"
        )

    current_disc_id = _disc_id()
    to_disc_id = (args.get("to_disc_id") or current_disc_id).strip()
    if to_disc_id != current_disc_id:
        raise RuntimeError(
            "disc_transfer_session: to_disc_id must be the room currently "
            "joined by this bridge"
        )
    local_binding = _read_binding()
    if not local_binding or local_binding.get("disc_id") != to_disc_id:
        raise RuntimeError(
            "disc_transfer_session: the joined target has no durable local "
            "resume credential; join it with disc_join before transferring"
        )
    source_agent = _agent_type_for_session()
    source_session_id = _durable_session_id()
    if not source_agent or source_agent == "Unknown" or not source_session_id:
        raise RuntimeError(
            "disc_transfer_session: no durable identity is available for this bridge"
        )

    result = _unwrap(_http("POST", "/api/disc/transfer-session", {
        "from_disc_id": from_disc_id,
        "to_disc_id": to_disc_id,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
        "confirm_transfer": True,
    }))
    if not isinstance(result, dict) or result.get("session_bound") is not True:
        raise RuntimeError(
            "disc_transfer_session: backend did not confirm the new durable binding"
        )
    return result


def _task_exec_identity(tool_name):
    source_agent = _agent_type_for_session()
    # Task-execution authorization is party-scoped against
    # `discussion_sessions`, whose session id is the live bridge identity used
    # by peer-join/peer-resume. The separate `cli-*` durable identity only owns
    # room recovery in `disc_source_history`; sending it here makes a perfectly
    # resumed principal look absent because the active row still carries the
    # `adhoc-*` identity. A resume updates/reuses the active session row before
    # lifecycle tools run, so use the same live identity at both boundaries.
    source_session_id = _session_id_for_caller()
    if not source_agent or source_agent == "Unknown" or not source_session_id:
        raise RuntimeError(
            f"{tool_name}: no active session identity for this bridge — join or resume "
            "the relevant room before using orchestration tools"
        )
    return source_agent, source_session_id


_TASK_WORKER_CONTEXT_ENV = "KRONN_TASK_WORKER_CONTEXT"


def _spawned_task_worker_context(required=False, tool_name="spawned task worker"):
    """Read the runner-injected task capability.

    This JSON is process environment owned by Kronn. It is deliberately absent
    from every MCP input schema, so a model cannot choose an execution, room,
    provider or dispatch trigger. Partial/malformed context fails closed.
    """
    raw = os.environ.get(_TASK_WORKER_CONTEXT_ENV)
    if not raw:
        if required:
            raise RuntimeError(f"{tool_name}: spawned worker context is unavailable")
        return None
    try:
        value = json.loads(raw)
    except (TypeError, ValueError) as error:
        raise RuntimeError(
            f"{tool_name}: spawned worker context is invalid"
        ) from error
    required_fields = (
        "execution_id", "discussion_id", "agent_type", "dispatch_job_id",
        "source_message_id",
    )
    if not isinstance(value, dict) or any(
        not isinstance(value.get(field), str) or not value[field].strip()
        for field in required_fields
    ):
        raise RuntimeError(f"{tool_name}: spawned worker context is incomplete")
    return {field: value[field].strip() for field in required_fields}


def _spawned_task_worker_mode():
    # Presence, rather than successful parsing, narrows the catalogue. A broken
    # capability must fail closed instead of exposing the principal surface.
    return bool(os.environ.get(_TASK_WORKER_CONTEXT_ENV))


def _visible_tools():
    if not _spawned_task_worker_mode():
        return TOOLS
    commit = {
        "name": "task_exec_commit",
        "description": (
            "Commit ONLY the explicitly named files for THIS spawned task worker. "
            "Kronn derives and revalidates the execution, child room, provider, "
            "dispatch and attached managed worktree, then performs Git server-side. "
            "Pass only relative `files` and a concise `message`; there is no amend, "
            "push, branch, ref or repository-path capability. After success, call "
            "`task_exec_deliver` with the semantic delivery assertions."
        ),
        "inputSchema": {
            "type": "object",
            "additionalProperties": False,
            "properties": {
                "files": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 100,
                    "uniqueItems": True,
                    "items": {"type": "string", "minLength": 1},
                    "description": "Explicit relative paths changed for this task.",
                },
                "message": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 500,
                    "description": "Concise commit message.",
                },
            },
            "required": ["files", "message"],
        },
    }
    delivery = next(item for item in TOOLS if item["name"] == "task_exec_deliver")
    semantic_manifest = {
        "type": "object",
        "additionalProperties": False,
        "required": [
            "tests", "dod_status", "docs", "migrations", "risks",
            "limitations", "summary",
        ],
        "properties": {
            "tests": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": False,
                    "required": ["name", "status", "evidence"],
                    "properties": {
                        "name": {"type": "string", "minLength": 1},
                        "status": {
                            "type": "string",
                            "enum": ["pass", "fail", "skipped"],
                        },
                        "evidence": {"type": "string", "minLength": 1},
                    },
                },
            },
            "dod_status": {
                "type": "array",
                "description": (
                    "Exactly one semantic assertion per DoD, in brief order. "
                    "Kronn injects the opaque dod_id."
                ),
                "items": {
                    "type": "object",
                    "additionalProperties": False,
                    "required": ["met", "evidence"],
                    "properties": {
                        "met": {"type": "boolean"},
                        "evidence": {"type": "string", "minLength": 1},
                    },
                },
            },
            "docs": {"type": "array", "items": {"type": "string"}},
            "migrations": {"type": "array", "items": {"type": "string"}},
            "risks": {"type": "array", "items": {"type": "string"}},
            "limitations": {"type": "array", "items": {"type": "string"}},
            "summary": {"type": "string", "minLength": 1},
        },
    }
    status = next(item for item in TOOLS if item["name"] == "task_exec_status")
    status = {
        **status,
        "description": (
            "Read THIS spawned task worker's execution status. Kronn derives "
            "the execution, child room, provider and dispatch from the runner capability; "
            "no execution selector is accepted."
        ),
        "inputSchema": {
            "type": "object",
            "additionalProperties": False,
            "properties": {},
        },
    }
    return [status, commit, {
        **delivery,
        "description": (
            "Submit semantic delivery assertions for THIS spawned task worker. "
            "Kronn derives the execution, child room, provider, dispatch, task, "
            "clean committed HEAD, file inventory and ordered DoD ids; pass only "
            "`manifest`. On success a complete DeliveryManifest v1 is persisted, "
            "the execution moves to AwaitingReview and the principal is woken."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "manifest": semantic_manifest,
            },
            "required": ["manifest"],
        },
    }]


_TASK_EXEC_MANUAL_HINT = (
    'Read tool_manual({tool: "task_exec_prepare"}) for the exact worker, '
    "worker_scope_intent, worker_scope and validations shapes."
)

_TASK_EXEC_WORKER_KINDS = ("discussion_agent", "agent", "cli")


def _validate_task_exec_worker(worker, tool_name):
    """Fail closed with a typed, actionable error instead of the backend's raw
    422 when `worker` is not the flat MessageTarget object `agent_list` hands
    back verbatim. Catches the classic mistake of wrapping it as the internal
    `{target: {...}, model, profile_id}` envelope."""
    if not isinstance(worker, dict):
        raise RuntimeError(
            f"{tool_name}: worker must be the typed MessageTarget object copied "
            f"verbatim from agent_list. {_TASK_EXEC_MANUAL_HINT}"
        )
    if "target" in worker and "kind" not in worker:
        raise RuntimeError(
            f"{tool_name}: worker must be the flat MessageTarget object itself "
            '(kind/agent_type/...), not wrapped as {"target": {...}, "model": ..., '
            '"profile_id": ...}. Copy the `worker` object from agent_list verbatim. '
            f"{_TASK_EXEC_MANUAL_HINT}"
        )
    kind = worker.get("kind")
    if kind not in _TASK_EXEC_WORKER_KINDS:
        raise RuntimeError(
            f"{tool_name}: worker.kind must be one of {_TASK_EXEC_WORKER_KINDS}, "
            f"got {kind!r}. {_TASK_EXEC_MANUAL_HINT}"
        )
    agent_type = worker.get("agent_type")
    if not isinstance(agent_type, str) or not agent_type.strip():
        raise RuntimeError(
            f"{tool_name}: worker.agent_type is required. {_TASK_EXEC_MANUAL_HINT}"
        )
    if kind == "cli" and not isinstance(worker.get("cli_session_id"), int):
        raise RuntimeError(
            f"{tool_name}: worker.kind=cli requires the exact integer cli_session_id "
            f"copied from agent_list — never guess it. {_TASK_EXEC_MANUAL_HINT}"
        )


def _task_exec_request(path, body):
    """Keep compact schemas fail-closed while making shape errors self-repairing."""
    try:
        return _unwrap(_http("POST", path, body))
    except RuntimeError as exc:
        raise RuntimeError(f"{exc} {_TASK_EXEC_MANUAL_HINT}") from exc


def _task_exec_scope_contract(args, tool_name):
    """Prove that the host transported the current scope contract.

    A current bridge process is not sufficient: some MCP hosts cache an older
    tool schema and strip fields they do not know before invoking this process.
    The required intent sentinel makes that loss observable and fail-closed.
    """
    intent = args.get("worker_scope_intent")
    scope = args.get("worker_scope")
    if intent not in ("generic", "scoped"):
        raise RuntimeError(
            f"{tool_name}: worker_scope_intent is required and must be generic or "
            "scoped. The MCP host tool schema may be stale — reconnect the Kronn "
            "MCP before retrying."
        )
    if intent == "scoped" and not isinstance(scope, dict):
        raise RuntimeError(
            f"{tool_name}: worker_scope_intent=scoped requires a worker_scope object. "
            f"{_TASK_EXEC_MANUAL_HINT}"
        )
    if intent == "generic" and scope is not None:
        raise RuntimeError(
            f"{tool_name}: worker_scope_intent=generic forbids worker_scope; use "
            "scoped or remove the scope."
        )
    return intent, scope


def call_agent_list(_args):
    _require_fresh_bridge("agent_list")
    source_agent, source_session_id = _task_exec_identity("agent_list")
    return _unwrap(_http(
        "POST",
        "/api/orchestration/tool/workers",
        {
            "parent_discussion_id": _disc_id(),
            "source_agent": source_agent,
            "source_session_id": source_session_id,
        },
    ))


def call_task_exec_prepare(args):
    _require_fresh_bridge("task_exec_prepare")
    task_reference = (args.get("task_reference") or "").strip()
    worker = args.get("worker")
    if not task_reference or not isinstance(worker, dict):
        raise RuntimeError(
            "task_exec_prepare: task_reference and a typed worker object are required. "
            f"{_TASK_EXEC_MANUAL_HINT}"
        )
    scope_intent, worker_scope = _task_exec_scope_contract(args, "task_exec_prepare")
    source_agent, source_session_id = _task_exec_identity("task_exec_prepare")
    body = {
        "task_reference": task_reference,
        "parent_discussion_id": _disc_id(),
        "worker": worker,
        "worker_scope_intent": scope_intent,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
    }
    if worker_scope is not None:
        body["worker_scope"] = worker_scope
    return _task_exec_request("/api/orchestration/tool/prepare", body)


def call_task_exec_launch(args):
    _require_fresh_bridge("task_exec_launch")
    task_reference = (args.get("task_reference") or "").strip()
    worker = args.get("worker")
    if not task_reference or not isinstance(worker, dict):
        raise RuntimeError(
            "task_exec_launch: task_reference and the preflighted typed worker are required. "
            f"{_TASK_EXEC_MANUAL_HINT}"
        )
    scope_intent, worker_scope = _task_exec_scope_contract(args, "task_exec_launch")
    source_agent, source_session_id = _task_exec_identity("task_exec_launch")
    body = {
        "task_reference": task_reference,
        "parent_discussion_id": _disc_id(),
        "worker": worker,
        "worker_scope_intent": scope_intent,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
    }
    for optional in ("base_rev", "idempotency_key", "validations"):
        if args.get(optional) is not None:
            body[optional] = args[optional]
    if worker_scope is not None:
        body["worker_scope"] = worker_scope
    return _task_exec_request("/api/orchestration/tool/launch", body)


def call_task_exec_status(args):
    if _spawned_task_worker_mode():
        context = _spawned_task_worker_context(
            required=True, tool_name="task_exec_status"
        )
        return _unwrap(_http(
            "POST",
            f"/api/orchestration/tool/executions/{urllib.parse.quote(context['execution_id'], safe='')}/status",
            {
                "spawned_agent": {
                    "discussion_id": context["discussion_id"],
                    "agent_type": context["agent_type"],
                    "dispatch_job_id": context["dispatch_job_id"],
                    "source_message_id": context["source_message_id"],
                },
            },
        ))
    execution_id = (args.get("task_execution_id") or args.get("task_reference") or "").strip()
    if not execution_id:
        raise RuntimeError("task_exec_status: task_execution_id or task_reference is required")
    source_agent, source_session_id = _task_exec_identity("task_exec_status")
    result = _unwrap(_http(
        "POST",
        f"/api/orchestration/tool/executions/{urllib.parse.quote(execution_id, safe='')}/status",
        {"source_agent": source_agent, "source_session_id": source_session_id},
    ))
    execution = ((result.get("lineage") or {}).get("execution") or {})
    status = execution.get("status")
    blocked_from = execution.get("blocked_from_status")
    interrupted_from = execution.get("interrupted_from_status")
    if (
        (status == "Blocked" and blocked_from == "Applying")
        or (
            status == "Interrupted"
            and interrupted_from == "Blocked"
            and blocked_from == "Applying"
        )
    ):
        result["next_action"] = {
            "tool": "task_exec_resume",
            "task_execution_id": execution_id,
            "reason": "Applying-origin checkpoint can be retried after cleaning the parent",
        }
    return result


def call_task_exec_resume(args):
    execution_id = (args.get("task_execution_id") or "").strip()
    if not execution_id:
        raise RuntimeError("task_exec_resume: task_execution_id is required")
    source_agent, source_session_id = _task_exec_identity("task_exec_resume")
    return _unwrap(_http(
        "POST",
        f"/api/orchestration/tool/executions/{urllib.parse.quote(execution_id, safe='')}/resume",
        {"source_agent": source_agent, "source_session_id": source_session_id},
    ))


def call_task_exec_cancel(args):
    execution_id = (args.get("task_execution_id") or "").strip()
    reason = (args.get("reason") or "").strip()
    if not execution_id or not reason:
        raise RuntimeError("task_exec_cancel: task_execution_id and reason are required")
    source_agent, source_session_id = _task_exec_identity("task_exec_cancel")
    body = {
        "source_agent": source_agent,
        "source_session_id": source_session_id,
        "reason": reason,
    }
    if args.get("cleanup_policy") is not None:
        body["cleanup_policy"] = args["cleanup_policy"]
    return _unwrap(_http(
        "POST", f"/api/orchestration/tool/executions/{execution_id}/cancel", body
    ))


def call_task_exec_reassign(args):
    _require_fresh_bridge("task_exec_reassign")
    execution_id = (args.get("task_execution_id") or "").strip()
    worker = args.get("worker")
    reason = (args.get("reason") or "").strip()
    if not execution_id or not isinstance(worker, dict) or not reason:
        raise RuntimeError(
            "task_exec_reassign: task_execution_id, typed worker and reason are required"
        )
    _validate_task_exec_worker(worker, "task_exec_reassign")
    source_agent, source_session_id = _task_exec_identity("task_exec_reassign")
    return _task_exec_request(
        f"/api/orchestration/tool/executions/{execution_id}/reassign",
        {
            "source_agent": source_agent,
            "source_session_id": source_session_id,
            "worker": worker,
            "reason": reason,
        },
    )


def call_task_exec_accept_worker_offer(args):
    """Accept a task-execution worker control offer targeted at THIS session and
    attach to its sub-discussion (KT-328 tranche 2). The caller passes ONLY the
    opaque `offer_id`; both identities are DERIVED by this bridge, and the
    backend verifies that the live session is the exact target before moving
    its separate durable room binding.

    On success the backend moves this session origin -> child (durable source
    binding + `discussion_sessions` membership), posts the work brief in the child,
    and flips the execution to `Working`. This tool then does the LOCAL half of the
    move (DoD-3): follow the session into the child so subsequent calls and
    `disc_wait_for_peer` operate there, and rewrite the durable resume credential to
    the child so an MCP reload re-attaches. The session row is re-homed WITHOUT
    rotating its resume credential, so the `resume_token` we already hold still
    resolves to the session in the child — we reuse it rather than mint a new one."""
    _require_fresh_bridge("task_exec_accept_worker_offer")
    offer_id = (args.get("offer_id") or "").strip()
    if not offer_id:
        raise RuntimeError("task_exec_accept_worker_offer: offer_id is required")
    # Offer acceptance crosses two deliberately distinct identity domains.
    # `source_session_id` identifies the active `discussion_sessions` row and
    # must match the exact target PK. `source_binding_session_id` identifies
    # the reload-stable `disc_source_history` binding that follows that row to
    # the child. Collapsing them made a real resumed CLI impossible to accept:
    # its active identity is `adhoc-*`, while its durable binding is `cli-*`.
    # Both values are bridge-derived and absent from the MCP input schema.
    source_agent, source_session_id = _task_exec_identity(
        "task_exec_accept_worker_offer"
    )
    source_binding_session_id = _durable_session_id()
    if not source_binding_session_id:
        raise RuntimeError(
            "task_exec_accept_worker_offer: no durable room identity for this bridge — "
            "join the origin room (disc_join) before accepting an offer"
        )
    prior_binding = _read_binding()
    # `_unwrap` raises on a refused offer, preserving the backend's opaque message
    # ("not found or not addressed to this session") so no rebind happens on refusal.
    result = _unwrap(_http("POST", "/api/orchestration/accept-offer", {
        "offer_id": offer_id,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
        "source_binding_session_id": source_binding_session_id,
    }))
    child_disc_id = (
        result.get("child_discussion_id") if isinstance(result, dict) else None
    )
    if not child_disc_id:
        raise RuntimeError(
            "task_exec_accept_worker_offer: backend accepted but returned no child "
            "discussion to attach to"
        )
    # ── Local rebind — follow the server-side move into the child room. ──
    _set_current_disc_id(child_disc_id)
    # Seed the child cursor at -1 so the work brief (just posted there, targeted at
    # this session) is delivered on the next wait rather than skipped.
    _set_read_cursor(child_disc_id, -1)
    resume_token = (
        prior_binding.get("resume_token") if isinstance(prior_binding, dict) else None
    )
    if resume_token:
        _write_binding(
            child_disc_id,
            resume_token,
            agent_type=source_agent,
            last_read_sort_order=_read_cursor(child_disc_id),
        )
    result["local_rebound_to"] = child_disc_id
    return result


def call_task_exec_deliver(args):
    """Submit delivery assertions for review (KT-319 tranche 2). Joined CLI
    sessions pass a complete DeliveryManifest v1 plus
    `task_execution_id` and the `manifest` object; identity is DERIVED SERVER-SIDE
    from this bridge's active room session, and the backend verifies you are the
    execution's EXACT worker (a different session is refused). On success the
    manifest is persisted, the execution flips to `AwaitingReview`, and a review
    request is posted to the principal in the parent room. This does NOT move your
    session — you stay in the sub-discussion. A refused delivery surfaces an opaque
    reason (not found / not addressed to you) or a specific state (not deliverable,
    invalid manifest). A spawned host worker passes only the semantic projection;
    Kronn derives the execution/task/Git/DoD mechanics from its runner capability."""
    manifest = args.get("manifest")
    if not isinstance(manifest, dict):
        raise RuntimeError(
            "task_exec_deliver: manifest (a DeliveryManifest v1 object) is required"
        )
    if _spawned_task_worker_mode():
        context = _spawned_task_worker_context(
            required=True, tool_name="task_exec_deliver"
        )
        # A spawned worker never selects its execution. Even if a client sends
        # an out-of-schema task_execution_id, discard it and use only the
        # runner-owned capability. The backend independently revalidates this
        # child room + provider + dispatch trigger tuple.
        return _unwrap(_http("POST", "/api/orchestration/deliver", {
            "task_execution_id": context["execution_id"],
            "manifest": manifest,
            "spawned_agent": {
                "discussion_id": context["discussion_id"],
                "agent_type": context["agent_type"],
                "dispatch_job_id": context["dispatch_job_id"],
                "source_message_id": context["source_message_id"],
            },
        }))

    task_execution_id = (args.get("task_execution_id") or "").strip()
    if not task_execution_id:
        raise RuntimeError("task_exec_deliver: task_execution_id is required")
    source_agent, source_session_id = _task_exec_identity("task_exec_deliver")
    # `_unwrap` raises on a refused delivery, preserving the backend's opaque message
    # ("not found or not addressed to this session") — the caller sends only the
    # execution id + manifest; the bridge derives its active identity itself.
    return _unwrap(_http("POST", "/api/orchestration/deliver", {
        "task_execution_id": task_execution_id,
        "manifest": manifest,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
    }))


def call_task_exec_commit(args):
    """Commit explicit spawned-worker paths through Kronn's trusted Git boundary.

    The model-visible arguments intentionally contain no execution or workspace
    selector. The runner capability supplies those out-of-band and the backend
    independently authenticates the exact child/provider/dispatch tuple before
    resolving its attached managed worktree.
    """
    if not _spawned_task_worker_mode():
        raise RuntimeError(
            "task_exec_commit: available only to a spawned Kronn task worker"
        )
    files = args.get("files")
    if (
        not isinstance(files, list)
        or not files
        or len(files) > 100
        or any(not isinstance(path, str) or not path.strip() for path in files)
    ):
        raise RuntimeError(
            "task_exec_commit: files must contain 1 to 100 non-empty relative paths"
        )
    message = args.get("message")
    if not isinstance(message, str) or not message.strip():
        raise RuntimeError("task_exec_commit: message is required")
    context = _spawned_task_worker_context(
        required=True, tool_name="task_exec_commit"
    )
    return _unwrap(_http("POST", "/api/orchestration/worker-commit", {
        "task_execution_id": context["execution_id"],
        "files": files,
        "message": message,
        "spawned_agent": {
            "discussion_id": context["discussion_id"],
            "agent_type": context["agent_type"],
            "dispatch_job_id": context["dispatch_job_id"],
            "source_message_id": context["source_message_id"],
        },
    }))


def call_task_exec_review(args):
    """Decide a delivered attempt as the principal (KT-319 tranche 3a). Pass the
    `task_execution_id` and a ReviewDecision v1 `decision` object; identity is
    DERIVED SERVER-SIDE from this bridge's active room session, and the backend
    authorizes you as a PARTY to the execution — the parent-room principal, or the
    worker only when the run explicitly allows self-review. approve is refused if the
    manifest is missing, the worktree HEAD drifted, or a DoD is unmet; request_changes
    bumps the round and hands your findings to the worker in its sub-discussion,
    keeping the worktree. A refused decision surfaces an opaque reason (not found /
    not addressed to you) or a specific state (not reviewable, self-review forbidden,
    approve refused, invalid decision)."""
    task_execution_id = (args.get("task_execution_id") or "").strip()
    if not task_execution_id:
        raise RuntimeError("task_exec_review: task_execution_id is required")
    decision = args.get("decision")
    if not isinstance(decision, dict):
        raise RuntimeError(
            "task_exec_review: decision (a ReviewDecision v1 object) is required"
        )
    source_agent, source_session_id = _task_exec_identity("task_exec_review")
    # `_unwrap` raises on a refused decision, preserving the backend's message. The
    # caller sends only the execution id + decision; the bridge derives its active
    # identity itself, so the model can never name a session id.
    return _unwrap(_http("POST", "/api/orchestration/review", {
        "task_execution_id": task_execution_id,
        "decision": decision,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
    }))


def call_disc_unlink(args):
    # KT-85 — release THIS session's binding, never the whole room's. A shared
    # discussion carries one binding per joined CLI, so the unscoped call would
    # detach every other peer as a side effect of one agent letting go.
    # Releasing every binding stays possible, but only as an explicit human
    # action from the UI, not from an agent tool call.
    disc_id = args.get("disc_id") or _disc_id()
    source_agent = args.get("source_agent") or _agent_type_for_session()
    source_session_id = args.get("source_session_id") or _durable_session_id()
    if not source_session_id or not source_agent or source_agent == "Unknown":
        raise RuntimeError(
            "disc_unlink: no durable identity for this bridge, so the binding to "
            "release cannot be identified — pass source_agent and "
            "source_session_id explicitly"
        )
    return _unwrap(_http("POST", "/api/disc/unlink", {
        "disc_id": disc_id,
        "source_agent": source_agent,
        "source_session_id": source_session_id,
    }))


def call_disc_find_by_session(args):
    # KT-76 — symmetric with `disc_link`: bare, this answers "which room is MY
    # session bound to?", which is what an agent actually needs after a reload.
    src_agent = args.get("source_agent") or _agent_type_for_session()
    src_sess = args.get("source_session_id") or _durable_session_id()
    if not src_agent or src_agent == "Unknown" or not src_sess:
        raise RuntimeError(
            "disc_find_by_session: no durable identity for this bridge — pass "
            "source_agent and source_session_id explicitly"
        )
    qs = urllib.parse.urlencode({
        "source_agent": src_agent,
        "source_session_id": src_sess,
    })
    found = _unwrap(_http("GET", f"/api/disc/find_by_session?{qs}"))

    # KT-76 follow-up, found by using it: the link is written by JOIN and by
    # resume, and resume is LAZY — it only runs when a tool needs a disc id.
    # Looking up our own durable link is NOT enough: after a process reload the
    # server can still return a disc_id while this bridge has neither restored
    # `_CURRENT_DISC_ID` nor its durable read cursor. Returning immediately in
    # that state produced a dangerous false positive ("room found") followed by
    # `disc_append: no disc bound`, and could tempt callers to continue without
    # replaying the unread gap. Always resume our own unbound runtime, whether
    # the durable link already exists or not. Explicit third-party lookups stay
    # pure reads.
    asked_about_self = not args.get("source_agent") and not args.get("source_session_id")
    if not asked_about_self:
        return found

    found_disc_id = found.get("disc_id") if isinstance(found, dict) else None
    local_binding = _read_binding()
    resume_disc_id = (
        local_binding.get("disc_id") if isinstance(local_binding, dict) else None
    )
    runtime_disc_id = _CURRENT_DISC_ID
    expected_disc_id = runtime_disc_id or resume_disc_id
    if found_disc_id and expected_disc_id and found_disc_id != expected_disc_id:
        return {
            "disc_id": found_disc_id,
            "runtime_bound": False,
            "binding_conflict": True,
            "runtime_disc_id": runtime_disc_id,
            "resume_disc_id": resume_disc_id,
            "rejoin_required": True,
            "hint": (
                "Kronn refused to choose between conflicting room bindings: "
                f"the durable session link points to {found_disc_id}, while "
                f"this bridge is bound or can resume {expected_disc_id}. "
                "Open the intended room and join it once with a fresh kr-join "
                "token; do not append until the conflict is resolved."
            ),
        }
    if runtime_disc_id:
        return found
    resumed = _attempt_resume()
    if resumed:
        # The resume path re-links the possibly-evolved durable identity. Read
        # once more so the response reflects that authoritative server state.
        return _unwrap(_http("GET", f"/api/disc/find_by_session?{qs}"))

    if isinstance(found, dict) and found.get("disc_id"):
        # Legacy/pre-fix sessions can have a server-side link but no local
        # resume credential (and therefore no durable read cursor). Do not
        # pretend that the runtime is ready: one fresh join bootstraps the
        # credential, after which reload recovery is automatic.
        found["runtime_bound"] = False
        found["rejoin_required"] = True
        found["hint"] = (
            "The durable session link still points to this room, but this "
            "bridge could not restore its resume credential/read cursor. "
            "Join once with a fresh kr-join token before appending."
        )
        return found
    return found


def _workspace_identity(args, tool_name):
    source_agent = args.get("source_agent") or _agent_type_for_session()
    source_session_id = args.get("source_session_id") or _durable_session_id()
    if not source_agent or source_agent == "Unknown" or not source_session_id:
        raise RuntimeError(
            f"{tool_name}: no durable identity for this bridge — pass "
            "source_agent and source_session_id explicitly"
        )
    return source_agent, source_session_id


def call_disc_workspace_get(args):
    source_agent, source_session_id = _workspace_identity(
        args, "disc_workspace_get"
    )
    query = urllib.parse.urlencode({
        "source_agent": source_agent,
        "source_session_id": source_session_id,
    })
    return _unwrap(_http("GET", f"/api/disc/workspace?{query}"))


def _unwrap_disc_workspace_set(envelope):
    if not isinstance(envelope, dict):
        raise RuntimeError(f"unexpected response shape: {envelope!r}")
    if envelope.get("success", False):
        return envelope.get("data")

    message = envelope.get("error") or "workspace declaration failed"
    error_code = envelope.get("error_code") or "internal"
    lowered = message.lower()
    if error_code == "conflict":
        kind = "ownership_ambiguous"
    elif "does not exist" in lowered:
        kind = "missing"
    elif "detached head" in lowered:
        kind = "detached_head"
    elif "does not belong" in lowered or "registered git worktree" in lowered:
        kind = "repository_scope"
    else:
        kind = error_code
    return {
        "workspace": None,
        "blockers": [{"kind": kind, "message": message}],
        "error_code": error_code,
    }


def call_disc_workspace_set(args):
    source_agent, source_session_id = _workspace_identity(
        args, "disc_workspace_set"
    )
    workspace_path = args.get("workspace_path")
    if workspace_path is None:
        workspace_path = os.getcwd()
    if not isinstance(workspace_path, str) or not workspace_path.strip():
        raise RuntimeError("disc_workspace_set: workspace_path must be a non-empty string")
    body = {
        "source_agent": source_agent,
        "source_session_id": source_session_id,
        "workspace_path": workspace_path,
    }
    task_ref = args.get("task_ref")
    if task_ref is not None:
        if not isinstance(task_ref, str) or not task_ref.strip():
            raise RuntimeError("disc_workspace_set: task_ref must be a non-empty string")
        body["task_ref"] = task_ref.strip()
    return _unwrap_disc_workspace_set(_http("POST", "/api/disc/workspace", body))


def call_disc_workspace_history_lease(args):
    source_agent, source_session_id = _workspace_identity(
        args, "disc_workspace_history_lease"
    )
    action = args.get("action")
    if action not in ("acquire", "release"):
        raise RuntimeError(
            "disc_workspace_history_lease: action must be acquire or release"
        )
    body = {
        "source_agent": source_agent,
        "source_session_id": source_session_id,
        "action": action,
    }
    backup_ref = args.get("backup_ref")
    if action == "acquire":
        if not isinstance(backup_ref, str) or not backup_ref.startswith(
            "refs/kronn-backup/"
        ):
            raise RuntimeError(
                "disc_workspace_history_lease: acquire requires backup_ref "
                "under refs/kronn-backup/"
            )
        body["backup_ref"] = backup_ref
    return _unwrap(_http("POST", "/api/disc/workspace/history-lease", body))


def call_disc_search(args):
    # Accept `query` as well: the rest of the surface spells the search term that
    # way, so callers reach for it first and paid a round-trip on a hard refusal.
    # The schema still advertises `q`; this only stops a naming detail from
    # costing a retry.
    q = args.get("q") or args.get("query")
    if not q:
        raise RuntimeError("disc_search: missing required 'q' (alias: 'query')")
    params = {"q": q}
    if args.get("limit") is not None:
        params["limit"] = args["limit"]
    if args.get("include_notes"):
        params["include_notes"] = "true"
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
    conversation_id = _native_conversation_id()
    if conversation_id:
        body["conversation_id"] = conversation_id
    # KT-37 — optional self-DECLARED model. Explicit arg wins; else an
    # env-declared default (KRONN_AGENT_MODEL). Never inferred: if neither is
    # set we omit it and the backend leaves any prior declaration untouched.
    model = args.get("model")
    if model is None:
        model = os.environ.get("KRONN_AGENT_MODEL")
    if isinstance(model, str) and model.strip():
        body["model"] = model.strip()
    prior_binding = _read_binding()
    result = _unwrap(_http("POST", "/api/discussions/peer-join", body))

    # Bind THIS process to the joined disc so subsequent tool calls
    # operate on it without the agent having to thread the disc_id
    # through every call.
    disc_id = result.get("disc_id") if isinstance(result, dict) else None
    if disc_id:
        _set_current_disc_id(disc_id)
        recent_messages = result.get("recent_messages") or []
        recent_orders = [
            message.get("sort_order")
            for message in recent_messages
            if isinstance(message, dict)
            and isinstance(message.get("sort_order"), int)
            and not isinstance(message.get("sort_order"), bool)
        ]
        join_read_cursor = max(recent_orders, default=-1)
        prior_same_disc = bool(
            prior_binding and prior_binding.get("disc_id") == disc_id
        )
        prior_read_cursor = (
            prior_binding.get("last_read_sort_order")
            if prior_same_disc
            and isinstance(prior_binding.get("last_read_sort_order"), int)
            and not isinstance(prior_binding.get("last_read_sort_order"), bool)
            else None
        )
        if prior_read_cursor is not None:
            _set_read_cursor(disc_id, prior_read_cursor)
        elif _read_cursor(disc_id) is None:
            _set_read_cursor(disc_id, -1)
        if not prior_same_disc:
            _stage_read_cursor(disc_id, join_read_cursor)
        # 0.9.0 — stash the resume credential so a later MCP reload can
        # re-attach to THIS disc via `/peer-resume` without a fresh token.
        resume_token = result.get("resume_token") if isinstance(result, dict) else None
        if resume_token:
            _write_binding(
                disc_id,
                resume_token,
                agent_type=agent_type,
                last_read_sort_order=_read_cursor(disc_id),
            )
        # KT-76 — link the DURABLE session here rather than asking the agent to
        # remember a step: a protocol that depends on the model's discipline is
        # the failure mode this room documented all day.
        if isinstance(result, dict):
            result.update(_bind_session_to_disc(disc_id, agent_type))

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
    create_args = {
        "title": title,
        # The persisted Discussion model still requires an agent value, but a
        # collaboration room must not have a native responder: joined MCP peers
        # are the only actors expected to answer there.
        "agent": _agent_type_for_session() or "Unknown",
        "no_agent": True,
    }
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
    # 0.9.0 — a deliberate leave drops the resume capability: the next
    # session must join explicitly, not silently reclaim this row.
    _clear_binding()
    return result


def _wait_once(args):
    """One server long-poll for new peer messages (≤170 s).

    Hits `GET /api/discussions/:id/wait` server-side. Excludes the
    caller's own `agent_type` (env-derived, same way as `disc_join`)
    so an agent doesn't wake itself on its own `disc_append`.
    `call_disc_wait_for_peer` chains these polls bridge-side (KT-189)
    so inner quiet polls do not return to the caller; chained append waits
    still use a single poll. Host-imposed tool-call background notifications
    remain outside this bridge's control.
    """
    disc_id = args.get("_disc_id") or _disc_id()
    since = args.get("since_sort_order")
    if since is None:
        since = _read_cursor(disc_id)
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
    # KT-114 — late capture: a fresh Codex TUI has no native id at join time,
    # but the FD probe can resolve it once the CLI is up. The idle loop calls
    # this every ≤170 s anyway, so piggyback the id here instead of adding a
    # round trip; sent until a successful wait confirms delivery, then stopped.
    if not _CONVERSATION_ID_DELIVERED.get(disc_id):
        late_conversation_id = _native_conversation_id(allow_probe=False)
        if late_conversation_id:
            params["conversation_id"] = late_conversation_id
    # KT-189 — echo the COMMITTED awareness acknowledgement. The server only
    # advances its durable per-session awareness cursor on this value.
    acked_awareness = _ACKED_AWARENESS_UPTO_BY_DISC.get(disc_id)
    if acked_awareness is not None:
        params["ack_awareness_upto"] = acked_awareness
    qs = urllib.parse.urlencode(params)
    sep = "?" if qs else ""
    # Transport-level retry (bounded): a backend restart mid-poll must not
    # surface as a tool error — the wait is idempotent on since_sort_order.
    # Socket timeout tracks the requested window (+margin) instead of the
    # generic 180 s, the caller's budget bounds every in-flight attempt and
    # backoff, and the whole transport runs in a WORKER thread so the main
    # thread keeps serving ping/cancel/progress even while the socket or a
    # retry backoff blocks (KT-189 review residuals 1+2). An abandoned
    # worker's late result is discarded here, BEFORE any read-cursor
    # staging, so an interrupted delivery is replayed, never half-consumed.
    socket_timeout = min(180, int(timeout_secs or 60) + 30)
    result = _unwrap(_transport_in_worker(
        "GET", f"/api/discussions/{disc_id}/wait{sep}{qs}", timeout=socket_timeout,
        deadline=args.get("_retry_deadline"),
    ))
    if isinstance(result, dict) and params.get("conversation_id"):
        _CONVERSATION_ID_DELIVERED[disc_id] = True
    if isinstance(result, dict):
        if result.get("messages"):
            _stage_read_cursor(disc_id, result.get("latest_sort_order"))
            delivered_message_ids = [
                message.get("message_id")
                for message in result["messages"]
                if isinstance(message, dict)
                and isinstance(message.get("message_id"), str)
                and message.get("message_id")
            ]
            if delivered_message_ids:
                result["delivered_message_ids"] = delivered_message_ids
                result["delivery_ack_hint"] = (
                    "Acknowledge or answer the exact delivered message id(s): "
                    + ", ".join(delivered_message_ids)
                    + ". For one transcript message, pass its id as "
                    "`reply_to_message_id` when replying."
                )
        else:
            _commit_read_cursor(disc_id, result.get("latest_sort_order"))
        withheld = result.get("withheld_by_routing")
        if isinstance(withheld, int) and withheld > 0:
            result["routing_visibility"] = _routing_visibility_hint(withheld)
        # KT-189 — awareness turns are room CONTEXT attached to this wake:
        # untargeted traffic or turns owned by another responder. The agent
        # reads them silently and never answers them individually.
        awareness_count = sum(
            1
            for message in result.get("messages", [])
            if isinstance(message, dict) and message.get("awareness")
        )
        if awareness_count:
            _stage_awareness_ack(disc_id, result.get("awareness_delivered_upto"))
            omitted = result.get("awareness_omitted") or 0
            hint = (
                f"{awareness_count} message(s) flagged `awareness: true` are "
                "room turns that did not target you (or targeted another "
                "responder, who owns the reply). Read them as context — do "
                "NOT answer them. If one still requires action from you "
                "specifically, address it in a single consolidated reply."
            )
            if isinstance(omitted, int) and omitted > 0:
                hint += (
                    f" {omitted} older unseen turn(s) exceeded the attach cap; "
                    "they will arrive with your next wake."
                )
            result["awareness_hint"] = hint
    # A timed-out wait (no peer activity in the window) is NORMAL in an ongoing
    # collaboration — but literal agents (notably Codex) otherwise read the empty
    # result as "conversation over" and STOP after ~60s. Surface an explicit
    # next-action hint so the agent keeps waiting instead of leaving.
    if isinstance(result, dict):
        addressed_here = False
        other_targets = set()
        for message in result.get("messages", []):
            if not isinstance(message, dict):
                continue
            typed_targets = message.get("targets") or []
            if typed_targets:
                message_addressed_here = bool(message.get("addressed_to_caller"))
                addressed_here = addressed_here or message_addressed_here
                for target in typed_targets:
                    if not isinstance(target, dict):
                        continue
                    agent = target.get("agent_type")
                    kind = target.get("kind")
                    if not agent:
                        continue
                    label = (
                        f"{agent} CLI"
                        if kind == "cli"
                        else f"{agent} discussion agent"
                        if kind == "discussion_agent"
                        else f"{agent} punctual agent"
                    )
                    if not (message_addressed_here and kind == "cli"):
                        other_targets.add(label)
                continue
            targets = message.get("target_agents") or []
            if not targets and message.get("target_agent"):
                targets = [message["target_agent"]]
            if exclude in targets:
                addressed_here = True
            other_targets.update(target for target in targets if target != exclude)
        other_targets = sorted(other_targets)
        if addressed_here:
            result["routing_hint"] = (
                f"This exact {exclude} CLI session is explicitly listed and addressed: "
                "answer this turn once. Other listed agents own their own replies; do not "
                "synthesize or wait for them unless the human asks."
            )
        if other_targets:
            awareness_hint = (
                "Read targeted messages for room awareness, but do NOT answer "
                f"messages addressed to {', '.join(other_targets)} while that "
                "target owns the turn. Step in only after an explicit target "
                "failure/interruption or a new message asks you to."
            )
            if addressed_here:
                result["routing_hint"] += " " + awareness_hint
            else:
                result["routing_hint"] = awareness_hint
    if isinstance(result, dict) and result.get("timed_out"):
        result["hint"] = (
            "No peer posted during this window. This is NORMAL — the other "
            "agent may still be thinking. Re-arm disc_wait_for_peer when you "
            "are ready to listen: it now blocks bridge-side until a real "
            "message arrives. If your host moved the prior call to a background "
            "task, wait for that task to finish before re-arming. Do "
            "NOT stop or disc_leave() just because a window was quiet — only "
            "leave when the task is done or the user explicitly says stop."
        )
    return result


# KT-189 — hold quiet waits bridge-side instead of returning each server
# timeout to the model: every such return replays the CLI's full native
# context (measured 2026-08: 1 341 disc_wait turns / 804.6M tokens in 30
# days on this machine alone). The bridge wait is UNBOUNDED by default so it
# does not create periodic model wakes itself, with an opt-in budget for callers
# that want one. A client can still impose its own tool-call backgrounding and
# model-visible notifications; that host limitation is documented above.
# Short poll slices: the blocking urllib call is the only moment the bridge
# cannot service control traffic (ping/cancel/progress), so each slice is
# kept small and everything is serviced between slices.
_WAIT_POLL_SECS = 15
_WAIT_PROGRESS_SLICE_SECS = 10


class _WaitAborted(RuntimeError):
    """Raised when an in-flight wait transport is abandoned (cancellation,
    queued tools/call, stdin EOF, or the opt-in budget). The poll's result,
    if it ever lands, is discarded before any cursor staging."""

    def __init__(self, reason):
        super().__init__(f"wait transport abandoned: {reason}")
        self.reason = reason


def _transport_in_worker(method, path, timeout=180, deadline=None):
    """Run a blocking transport call in a daemon worker while the main
    thread keeps the control plane alive (ping/tools_list/notifications,
    cancellation checks, progress heartbeats). Raises `_WaitAborted` when
    the wait must hand back before the transport returns."""
    box = {}

    def run():
        try:
            box["result"] = _http_transport_retry(
                method, path, timeout=timeout, deadline=deadline
            )
        except Exception as exc:  # noqa: BLE001 — re-raised on the main thread
            box["error"] = exc

    worker = threading.Thread(target=run, daemon=True, name="wait-transport")
    worker.start()
    next_progress = time.monotonic() + _WAIT_PROGRESS_SLICE_SECS
    while True:
        worker.join(0.5)
        if not worker.is_alive():
            break
        reason = _wait_abort_reason()
        if reason:
            raise _WaitAborted(reason)
        if deadline is not None and time.monotonic() >= deadline:
            raise _WaitAborted("budget")
        if time.monotonic() >= next_progress:
            _emit_wait_progress(0, 0)
            next_progress = time.monotonic() + _WAIT_PROGRESS_SLICE_SECS
    if "error" in box:
        raise box["error"]
    return box.get("result")


def _wait_total_budget(args):
    """Opt-in overall wait budget in seconds; None = unbounded (default)."""
    raw = args.get("max_total_secs")
    if raw is None:
        raw = os.environ.get("KRONN_WAIT_TOTAL_SECS")
    if raw is None:
        return None
    try:
        total = int(raw)
    except (TypeError, ValueError):
        return None
    if total <= 0:
        return None
    return max(total, 1)


def _emit_wait_progress(polls, waited_secs):
    """Progress notification keeps the client's tool-call timeout alive."""
    token = _CURRENT_PROGRESS_TOKEN.get("token")
    if token is None:
        return
    _send({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": {
            "progressToken": token,
            "progress": polls,
            "message": f"still listening (quiet for ~{waited_secs}s, no model turn spent)",
        },
    })


def _wait_abort_reason():
    """Why the bridge-side wait loop must hand control back NOW.

    Control-plane traffic (ping, tools/list, notifications) is served
    inline by `_service_control_traffic` and never aborts the wait; only
    this call's own cancellation, a queued tools/call, or stdin EOF do.
    """
    rid = _CURRENT_PROGRESS_TOKEN.get("rid")
    if rid is not None and _is_cancelled(rid):
        return "cancelled"
    return _service_control_traffic()


def _wait_sleep(delay, polls, started):
    """Sleep `delay` seconds in short slices; True if aborted mid-sleep.

    Emits a progress heartbeat every ~10 s so a client never sees a
    silent gap approaching its tool-call timeout during long pacing.
    """
    end = time.monotonic() + max(0, delay)
    next_progress = time.monotonic() + _WAIT_PROGRESS_SLICE_SECS
    while time.monotonic() < end:
        if _wait_abort_reason():
            return True
        if time.monotonic() >= next_progress:
            _emit_wait_progress(polls, int(time.monotonic() - started))
            next_progress = time.monotonic() + _WAIT_PROGRESS_SLICE_SECS
        time.sleep(min(1.0, max(0.0, end - time.monotonic())))
    return _wait_abort_reason() is not None


# KT-190 — this bridge reports its OWN token cost.
#
# Kronn knows what the agents it spawns cost. A CLI that joined a room was never
# spawned, so everything it posts is stored `tokens_used = 0` — measured on one
# real session, 4 143 787 451 tokens recorded as zero. Only this machine holds
# the vendor's transcript and only this process knows which session it is, so the
# measuring has to happen here.
#
# Three rules, because telemetry must never become the thing that breaks the
# room: it runs on a daemon thread so it cannot add a millisecond to a wait, it
# adds nothing to the wait's response, and any failure is logged and dropped.
# A session whose cost is unknown is a nuisance; a wait that hangs is an outage.
_TELEMETRY_INTERVAL_SECS = 300
_TELEMETRY_STATE = {"last_run": 0.0, "in_flight": False}


def _telemetry_vendor():
    """Which collector applies to this CLI, or None when there is none.

    Codex and Copilot deliberately return None: their counters are not readable
    yet, and reporting zero for them is the blind spot this exists to remove.
    """
    return {"ClaudeCode": "claude-code", "Vibe": "vibe"}.get(_agent_type_for_session())


def _collect_and_report_telemetry(disc_id, session_id, vendor):
    """Measure this session and POST it. Best effort, silent on failure."""
    try:
        spec = importlib.util.spec_from_file_location(
            "cli_token_collector",
            os.path.join(os.path.dirname(_BRIDGE_SOURCE_PATH),
                         "cli_token_collector.py"),
        )
        collector = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(collector)

        if vendor == "claude-code":
            session_key = _native_conversation_id(allow_probe=False)
        else:
            # Vibe exports nothing and keeps no file open, so its session is
            # resolved from the recorded cwd — and refuses when ambiguous.
            resolved = collector.resolve_vibe_session_id(os.getcwd())
            if resolved["status"] != "resolved":
                print(f"kronn-internal: vibe telemetry {resolved['status']}: "
                      f"{resolved.get('reason')}", file=sys.stderr)
                return
            session_key = resolved["session_id"]
        if not session_key:
            return

        # Resume where the last report stopped, so a 61 MB transcript is not
        # re-parsed every time. The cursor is the SERVER's, echoed back by the
        # previous POST — see the end of this function.
        offset = _TELEMETRY_STATE.get(f"offset:{session_key}", 0)

        # The timeline is what lets the backend stamp each message with the
        # SESSION's running total at that instant — @user's UX call, and the only
        # honest one: a CLI's spend cannot be cut into per-message costs, since
        # between two room messages it also reads files and runs tests.
        result = collector.collect_for_session(
            vendor, session_key, since_offset=offset, with_timeline=True
        )
        if result.get("status") != "measured":
            print(f"kronn-internal: telemetry not measured: {result.get('reason')}",
                  file=sys.stderr)
            return

        counters = result.get("counters") or {}
        body = {
            "session_id": session_id,
            "vendor": result["vendor"],
            "provenance": result["provenance"],
            # A counter this vendor does not publish is sent as null, never 0.
            "input_tokens": counters.get("input"),
            "cache_creation_tokens": counters.get("cache_creation"),
            "cache_read_tokens": counters.get("cache_read"),
            "output_tokens": counters.get("output"),
            "measured_responses": result.get("measured_responses"),
            "models_json": json.dumps(result.get("models") or {}, ensure_ascii=False),
            "window_start": result.get("window_start"),
            "window_end": result.get("window_end"),
            "vendor_cost_usd": result.get("vendor_cost_usd"),
            "read_offset": result.get("next_offset", 0),
            # Absent for a snapshot vendor (Vibe): it reports session totals with
            # no per-response instants, so there is nothing to place against a
            # message. Sending an empty list is correct — the backend then stamps
            # nothing rather than guessing.
            "timeline": result.get("timeline") or [],
        }
        encoded = urllib.parse.quote(disc_id, safe="")
        response = _http("POST", f"/api/discussions/{encoded}/telemetry", body)
        stored_offset = ((response or {}).get("data") or {}).get("read_offset")
        if isinstance(stored_offset, int):
            # Trust the SERVER's cursor: it may be ahead of ours if another
            # report landed, and resuming from a stale one would double-count.
            _TELEMETRY_STATE[f"offset:{session_key}"] = stored_offset
    except Exception as error:  # noqa: BLE001 — telemetry must never break a wait
        print(f"kronn-internal: telemetry report failed: {error}", file=sys.stderr)
    finally:
        _TELEMETRY_STATE["in_flight"] = False
        _TELEMETRY_STATE["last_run"] = time.monotonic()


def _maybe_report_telemetry():
    """Throttled, non-blocking trigger. Never raises into the caller."""
    try:
        vendor = _telemetry_vendor()
        if not vendor:
            return
        if _TELEMETRY_STATE["in_flight"]:
            return
        elapsed = time.monotonic() - _TELEMETRY_STATE["last_run"]
        if _TELEMETRY_STATE["last_run"] and elapsed < _TELEMETRY_INTERVAL_SECS:
            return
        disc_id = _disc_id()
        session_id = _session_id_for_caller()
        if not disc_id or not session_id:
            return
        _TELEMETRY_STATE["in_flight"] = True
        threading.Thread(
            target=_collect_and_report_telemetry,
            args=(disc_id, session_id, vendor),
            daemon=True,
        ).start()
    except Exception as error:  # noqa: BLE001
        _TELEMETRY_STATE["in_flight"] = False
        print(f"kronn-internal: telemetry trigger failed: {error}", file=sys.stderr)


def _routing_visibility_hint(withheld):
    """The caller-facing note explaining a read-cursor jump caused by routing.
    Shared by `_wait_once` (a single poll) and the bridge loop's accumulated
    total so the count and its prose can never disagree."""
    return (
        f"{withheld} newer peer turn(s) were intentionally withheld "
        "because they target another identity. The read cursor moved "
        "past them by design; do not claim to have read their content."
    )


def _carry_withheld_total(result, total):
    """KT-330 DoD-3 — accumulate-until-report. A quiet inner poll that saw peer
    turns withheld by routing advances the read cursor past them and is then
    discarded by the bridge loop; without this its count would vanish on "an
    internal poll the caller never sees". Stamp the running total (and matching
    prose) onto whatever result the caller actually receives, so a withheld
    count is reported at least once and never silently dropped."""
    if isinstance(result, dict) and total > 0:
        result["withheld_by_routing"] = total
        result["routing_visibility"] = _routing_visibility_hint(total)
    return result


def call_disc_wait_for_peer(args):
    """KT-189 — wait OUTSIDE the LLM loop.

    Chains server long-polls bridge-side and returns to the model only on
    a real message, a terminal error, an interruption (client cancelled
    the call / a queued tools/call / stdin EOF) or the opt-in budget. A
    quiet inner polls do not return to the model. A host may still background
    the long-running tool call and surface its own notification.
    """
    # KT-190 — measure this session's own cost. Fires a daemon thread at most
    # every few minutes and returns immediately: the wait's latency and its
    # response are untouched.
    _maybe_report_telemetry()
    budget = _wait_total_budget(args)
    started = time.monotonic()
    deadline = started + budget if budget is not None else None
    poll_args = dict(args)
    polls = 0
    # Running total of peer turns withheld by routing across every inner poll
    # of this one logical wait (KT-330 DoD-3). Each quiet poll's own result is
    # discarded, so the count must be carried here, not left on the poll.
    withheld_total = 0
    interrupted_hint = (
        "Wait interrupted: another request reached the bridge. The room "
        "stayed quiet; re-arm disc_wait_for_peer after handling the new "
        "activity."
    )
    while True:
        # Inner polls stay short so a cancellation is honored within
        # ~_WAIT_POLL_SECS even though the overall wait can span hours —
        # and never longer than the remaining opt-in budget.
        requested = poll_args.get("timeout_secs")
        poll_secs = min(int(requested or _WAIT_POLL_SECS), _WAIT_POLL_SECS)
        if deadline is not None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return _carry_withheld_total({
                    "timed_out": True,
                    "messages": [],
                    "bridge_polls": polls,
                    "hint": _wait_budget_hint(started, polls),
                }, withheld_total)
            poll_secs = max(1, min(poll_secs, int(remaining)))
        _emit_wait_progress(polls, int(time.monotonic() - started))
        poll_args["timeout_secs"] = poll_secs
        poll_args["_retry_deadline"] = deadline
        try:
            result = _wait_once(poll_args)
        except _WaitAborted as aborted:
            quiet = {"timed_out": True, "messages": [], "bridge_polls": polls + 1}
            if aborted.reason == "budget":
                quiet["hint"] = _wait_budget_hint(started, polls + 1)
            elif aborted.reason != "cancelled":
                quiet["hint"] = interrupted_hint
            return _carry_withheld_total(quiet, withheld_total)
        polls += 1
        if not isinstance(result, dict):
            return result
        withheld = result.get("withheld_by_routing")
        if isinstance(withheld, int) and withheld > 0:
            withheld_total += withheld
        result["bridge_polls"] = polls
        # Carry the accumulated withheld total onto every dict the caller can
        # receive — the delivered poll below AND each interrupt/quiet exit that
        # returns `result` further down — so an intermediate poll's count is
        # reported, never dropped on a cursor advance (KT-330 DoD-3).
        _carry_withheld_total(result, withheld_total)
        if result.get("messages") or not result.get("timed_out"):
            return result
        # Quiet poll — keep waiting bridge-side unless something changed.
        reason = _wait_abort_reason()
        if reason == "cancelled":
            # The client abandoned this call; any response is discarded.
            return result
        if reason is not None:
            result["hint"] = interrupted_hint
            return result
        if deadline is not None and time.monotonic() >= deadline:
            result["hint"] = _wait_budget_hint(started, polls)
            return result
        # Follow the server's pacing between polls; presence eligibility is
        # derived from next_poll_at + grace, so honoring it keeps the
        # session an eligible responder while it sleeps.
        pacing = result.get("pacing") or {}
        delay = pacing.get("next_delay_seconds")
        try:
            delay = min(max(int(delay), 0), 480) if delay is not None else 0
        except (TypeError, ValueError):
            delay = 0
        if deadline is not None:
            delay = min(delay, max(0, int(deadline - time.monotonic())))
        if delay and _wait_sleep(delay, polls, started):
            if _wait_abort_reason() != "cancelled":
                result["hint"] = interrupted_hint
            return result
        # Resume from what this poll actually observed so replays stay exact.
        latest = result.get("latest_sort_order")
        if isinstance(latest, int):
            poll_args["since_sort_order"] = latest


def _wait_budget_hint(started, polls):
    return (
        f"Quiet for the whole opt-in wait budget "
        f"({int(time.monotonic() - started)}s, {polls} bridge polls). "
        "This is NORMAL — re-arm disc_wait_for_peer to keep "
        "listening, or take the next actionable task from the plan. Do NOT "
        "stop or disc_leave() just because the room is quiet."
    )


def call_disc_load_other(args):
    disc_id = args.get("disc_id")
    if not disc_id:
        raise RuntimeError("disc_load_other: missing required 'disc_id'")
    params = {"disc_id": disc_id}
    if args.get("from") is not None:
        params["from"] = args["from"]
    if args.get("to") is not None:
        params["to"] = args["to"]
    if args.get("include_notes"):
        params["include_notes"] = "true"
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


def _compact_variable_contract(variable):
    """Display-safe PromptVariable metadata shared by QP/QA/QE catalogues.

    Source references are declarations, never resolved values. Returning them
    lets an agent reuse or propose the contract without asking for a secret.
    """
    return {
        "name": variable.get("name"),
        "label": variable.get("label"),
        "required": bool(variable.get("required", True)),
        "description": variable.get("description") or None,
        "source": variable.get("source") or "user_input",
        "source_ref": variable.get("source_ref") or None,
        "allow_manual_override": bool(variable.get("allow_manual_override", False)),
        "control": variable.get("control") or {"type": "text"},
    }


def call_qp_list(_args):
    # 0.8.5 — compact list. Keeps variable names so the agent can decide
    # if an existing QP fits the user's use case before drafting a new
    # one. Drops the full `prompt_template` body — call `qp_get(qp_id)` to
    # read it (understand what the QP does / run it yourself / pre-edit).
    data = _unwrap(_http("GET", "/api/quick-prompts")) or []
    out = []
    for q in data:
        variables = [_compact_variable_contract(v) for v in (q.get("variables") or [])]
        var_names = [v.get("name") for v in variables]
        out.append({
            "id": q.get("id"),
            "name": q.get("name"),
            "agent": q.get("agent"),
            "description": q.get("description"),
            "variable_names": var_names,
            "variables": variables,
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
        variables = [_compact_variable_contract(v) for v in (q.get("variables") or [])]
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


def call_qe_list(_args):
    """Compact saved Quick Exec discovery."""
    data = _unwrap(_http("GET", "/api/quick-execs")) or []
    return [
        {
            "id": item.get("id"),
            "name": item.get("name"),
            "description": item.get("description"),
            "command": item.get("command"),
            "args": item.get("args") or [],
            "output_format": item.get("output_format"),
            "timeout_secs": item.get("timeout_secs"),
            "project_id": item.get("project_id"),
            "variables": [_compact_variable_contract(variable) for variable in (item.get("variables") or [])],
        }
        for item in data
    ]


def call_page_list(_args):
    """Compact Page discovery for workflow composition."""
    data = _unwrap(_http("GET", "/api/pages")) or []
    return [
        {
            "id": page.get("id"),
            "title": page.get("title"),
            "slug": page.get("slug"),
            "project_id": page.get("project_id"),
            "data_revision": page.get("data_revision"),
            "updated_at": page.get("updated_at"),
            "last_published_at": page.get("last_published_at"),
            "pinned": page.get("pinned", False),
            "archived": page.get("archived", False),
        }
        for page in data
    ]


def _page_selector(args, tool_name):
    selector = args.get("page_id") or args.get("id")
    if not isinstance(selector, str) or not selector.strip():
        raise RuntimeError(f"{tool_name}: missing required 'page_id'")
    return urllib.parse.quote(selector.strip(), safe="")


def call_page_get(args):
    """Full Page definition plus its workflow and discussion links."""
    encoded = _page_selector(args, "page_get")
    detail = _unwrap(_http("GET", f"/api/pages/{encoded}"))
    workflows = _unwrap(_http("GET", f"/api/pages/{encoded}/workflows")) or []
    discussions = _unwrap(_http("GET", f"/api/pages/{encoded}/discussions")) or []
    result = dict(detail or {})
    result["workflows"] = workflows
    result["discussions"] = discussions
    return result


def call_page_create(args):
    """Create the Page destination before a PublishPageData workflow step."""
    title = args.get("title")
    html = args.get("html")
    datasets = args.get("datasets")
    if not isinstance(title, str) or not title.strip():
        raise RuntimeError("page_create: missing required 'title'")
    if not isinstance(html, str) or not html.strip():
        raise RuntimeError("page_create: missing required 'html'")
    if not isinstance(datasets, list):
        raise RuntimeError("page_create: 'datasets' must be an array")

    body = {
        "title": title,
        "html": html,
        "datasets": datasets,
    }
    for field in ("slug", "project_id", "discussion_id"):
        if field in args:
            body[field] = args[field]
    if not body.get("discussion_id"):
        try:
            body["discussion_id"] = _disc_id()
        except RuntimeError:
            # A Page is a shared destination, not a discussion child. Host
            # CLIs must be able to create standalone/mock Pages before they
            # have joined any room.
            body.pop("discussion_id", None)
    if body.get("project_id") is None:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    actor = _agent_type_for_session()
    body["created_by_agent"] = None if actor == "Unknown" else actor
    return _unwrap(_http("POST", "/api/pages", body))


def call_page_update_html(args):
    """Create an immutable replacement HTML revision for a Page."""
    encoded = _page_selector(args, "page_update_html")
    html = args.get("html")
    if not isinstance(html, str) or not html.strip():
        raise RuntimeError("page_update_html: missing required 'html'")
    actor = _agent_type_for_session()
    body = {
        "html": html,
        "created_by_agent": None if actor == "Unknown" else actor,
    }
    return _unwrap(_http("PUT", f"/api/pages/{encoded}/html", body))


def call_page_add_dataset(args):
    """Attach one dataset to an already-created Page (idempotent on name+kind)."""
    encoded = _page_selector(args, "page_add_dataset")
    name = args.get("name")
    kind = args.get("kind")
    if not isinstance(name, str) or not name.strip():
        raise RuntimeError("page_add_dataset: missing required 'name'")
    if kind not in ("snapshot", "time_series", "collection"):
        raise RuntimeError(
            "page_add_dataset: 'kind' must be snapshot, time_series or collection"
        )
    body = {"name": name, "kind": kind}
    for field in ("initial", "schema", "max_points", "max_age_days"):
        if field in args:
            body[field] = args[field]
    return _unwrap(_http("POST", f"/api/pages/{encoded}/datasets", body))


def call_mcp_list(_args):
    # 0.8.5 — wired MCP configs (the API plugin slug + config id the
    # workflow ApiCall steps need). Drops env values (secrets) and
    # scan diagnostics; keeps only what the agent needs to compose an
    # ApiCall step (slug + config_id + project scoping).
    data = _unwrap(_http("GET", "/api/mcps")) or {}
    captured_at = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    out_configs = []
    for c in data.get("configs") or []:
        out_configs.append({
            "config_id": c.get("id"),
            "server_id": c.get("server_id"),
            "is_global": c.get("is_global"),
            "project_ids": c.get("project_ids") or [],
            "label": c.get("label"),
            # Names are safe authoring metadata. Values remain encrypted and
            # are deliberately absent; `<env.NAME>` is resolved server-side
            # only when a QP/QA/QE/Workflow run starts.
            "env_keys": c.get("env_keys") or [],
            "secrets_broken": bool(c.get("secrets_broken", False)),
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
    return {
        "captured_at": captured_at,
        "configs": out_configs,
        "servers_with_api": out_servers,
        "api_call_contract": (
            "servers_with_api is the discovery snapshot of server registry "
            "entries that currently expose api_spec. ApiCall requires BOTH "
            "an exact config_id active globally or on the workflow project "
            "AND its matching server entry with api_spec and decryptable "
            "configuration. Presence is necessary but not sufficient; absence "
            "at captured_at means ApiCall cannot use that server in this "
            "snapshot. Re-run mcp_list before diagnosing a later observation."
        ),
        "execution_variable_contract": (
            "configs[].env_keys exposes names only, never values. A project-bound "
            "QP/QA/QE/Workflow may declare source:'project_env' with "
            "source_ref:'<env.NAME>'; Kronn resolves the current encrypted value "
            "again at each launch. The key must belong to exactly one configuration "
            "active for that project, and secrets_broken must be false. Never copy a "
            "masked or resolved value into a template."
        ),
    }


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
    # Send an explicit empty object for older Kronn backends whose
    # Option<Json<T>> extractor rejects an empty JSON body. Current backends
    # accept both forms, so this remains backward-compatible.
    return _unwrap(_http("POST", f"/api/workflow-runs/{rid}/resume", {}))


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


def call_qe_create_draft(args):
    for field in ("name", "command"):
        if not args.get(field):
            raise RuntimeError(f"qe_create_draft: missing required '{field}'")
    body = dict(args)
    if "project_id" not in body or body.get("project_id") is None:
        inherited = _current_project_id()
        if inherited:
            body["project_id"] = inherited
    body.setdefault("args", [])
    body.setdefault("variables", [])
    body.setdefault("output_format", "json")
    body.setdefault("timeout_secs", 60)
    return _unwrap(_http("POST", "/api/quick-execs", body))


def call_qe_update(args):
    qe_id = args.get("qe_id")
    if not qe_id:
        raise RuntimeError("qe_update: missing required 'qe_id'")
    existing_list = _unwrap(_http("GET", "/api/quick-execs")) or []
    existing = next((item for item in existing_list if item.get("id") == qe_id), None)
    if not existing:
        raise RuntimeError(f"qe_update: Quick Exec {qe_id!r} not found — call qe_list")
    fields = (
        "name", "icon", "description", "project_id", "command", "args",
        "timeout_secs", "output_format", "variables",
    )
    body = {
        field: args[field] if field in args else existing.get(field)
        for field in fields
    }
    return _unwrap(_http("PUT", f"/api/quick-execs/{qe_id}", body))


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

def call_media_generate(args):
    """Queue an image/video generation, optionally waiting for delivery.

    The model is never taken from the caller: the backend resolves it from the
    connection's configured slot, so an agent cannot dispatch — and bill — a
    model the human did not choose.

    `wait` defaults to false because a video takes ~100 s and the asset lands
    in the discussion by itself. Waiting is for the case where the media must
    appear in the answer being written right now.
    """
    for field in ("connection_id", "modality", "prompt"):
        if not args.get(field):
            raise RuntimeError(f"media_generate: missing required '{field}'")
    modality = args["modality"]
    if modality not in ("image", "video"):
        raise RuntimeError("media_generate: modality must be 'image' or 'video'")

    # `_disc_id()` is the shared entry point and already raises a clear error
    # when nothing is bound, so an explicit id simply short-circuits it.
    discussion_id = args.get("discussion_id") or _disc_id()

    body = {
        "connection_id": args["connection_id"],
        "modality": modality,
        "prompt": args["prompt"],
        "discussion_id": discussion_id,
    }
    for key in ("duration_secs", "resolution", "aspect_ratio", "generate_audio"):
        if args.get(key) is not None:
            body[key] = args[key]

    queued = _unwrap(_http("POST", "/api/media/generate", body))
    if not args.get("wait"):
        return queued

    # Waiting path. The ceiling matches the server-side deadline so this can
    # never outlive the job it is watching.
    job_id = queued.get("job_id")
    deadline = time.time() + 20 * 60
    delay = 5
    while time.time() < deadline:
        time.sleep(delay)
        delay = min(delay + 5, 30)
        state = _unwrap(_http("GET", f"/api/media/jobs/{job_id}"))
        if state.get("status") in ("completed", "failed", "cancelled", "timed_out"):
            return state
    # Timed out on OUR side only: the job keeps running and will still deliver.
    return {**queued, "waited": False, "note": "still running; poll media_job_status"}


def call_media_job_status(args):
    """Read one media job. Absent fields mean not measured yet, never zero."""
    job_id = args.get("job_id")
    if not job_id:
        raise RuntimeError("media_job_status: missing required 'job_id'")
    return _unwrap(_http("GET", f"/api/media/jobs/{job_id}"))


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


def call_qe_run(args):
    qe_id = args.get("qe_id")
    if not qe_id:
        raise RuntimeError("qe_run: missing required 'qe_id'")
    vars_obj = args.get("vars")
    variables = (
        {str(key): str(value) for key, value in vars_obj.items()}
        if isinstance(vars_obj, dict)
        else {}
    )
    return _unwrap(_http("POST", f"/api/quick-execs/{qe_id}/run", {"variables": variables}))


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
    # 0.10.0 — Continual Learning. Propose a durable fact/preference/inference,
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
            "CollectApiData",
            "TransformData",
            "PublishPageData",
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
            "PublishPageData": {
                "required": ["page_publish.page_id", "page_publish.writes"],
                "optional": [],
                "note": (
                    "Typed, zero-token sink for Kronn Live Pages. Each write requires "
                    "dataset, operation (replace|append|upsert), and value_from as one "
                    "typed context path such as steps.fetch.data.series. Upsert also "
                    "requires key_field; append is idempotent per workflow run by default."
                ),
                "example": {
                    "name": "publish-report",
                    "step_type": {"type": "PublishPageData"},
                    "page_publish": {
                        "page_id": "<id from page_list or page_create>",
                        "writes": [{
                            "dataset": "summary",
                            "operation": "replace",
                            "value_from": "steps.shape-report.data",
                        }],
                    },
                },
            },
            "CollectApiData": {
                "required": ["collect_api_data.sources"],
                "optional": ["collect_api_data.concurrent_limit (default 5, max 20)"],
                "note": (
                    "Runs independent Quick APIs and shell-free CLI commands concurrently. Every "
                    "source requires a unique alias, optional required flag/variables map, and "
                    "exactly one of quick_api_id, quick_exec_id or inline quick_exec. Prefer a "
                    "saved quick_exec_id from qe_list. Inline quick_exec is "
                    "{command,args,timeout_secs?,output_format}; command must be a bare binary in "
                    "workflow exec_allowlist, shell binaries are rejected, args stay literal, and "
                    "output_format is json|csv|text|lines; CSV becomes an array of objects and "
                    "each stream is capped at 1 MiB. Output "
                    "is {sources:{alias:<typed extracted data>},meta:{...}}. Source variables "
                    "may use run-anchored time expressions such as "
                    "{{time.now|shift:-24h|floor:hour|fmt:rfc3339}}; every parallel source "
                    "shares the same anchor. Optional failures yield PARTIAL; required failures "
                    "stop the workflow."
                ),
                "example": {
                    "name": "collect-sources",
                    "step_type": {"type": "CollectApiData"},
                    "collect_api_data": {
                        "concurrent_limit": 5,
                        "sources": [
                            {
                                "alias": "analytics",
                                "quick_api_id": "<id from qa_list>",
                                "required": True,
                                "variables": {"host": "fr.example.com"},
                            },
                            {
                                "alias": "cloudwatch",
                                "quick_api_id": "",
                                "quick_exec_id": "<id from qe_list>",
                                "required": False,
                            },
                        ],
                    },
                },
            },
            "TransformData": {
                "required": ["transform_data.input_from", "transform_data.fields"],
                "optional": [],
                "note": (
                    "Deterministic zero-token JSON shaping. input_from is one typed context path; "
                    "each field has target, RFC 9535 JSONPath source, operation "
                    "(copy|count|sum|average|min|max|first|last), optional fallback and optional "
                    "value_type (string|number|boolean)."
                ),
                "example": {
                    "name": "shape-report",
                    "step_type": {"type": "TransformData"},
                    "transform_data": {
                        "input_from": "steps.collect-sources.data",
                        "fields": [
                            {
                                "target": "metrics.total",
                                "source": "$.sources.analytics.total",
                                "operation": "copy",
                                "fallback": 0,
                                "value_type": "number",
                            }
                        ],
                    },
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
                "optional": [
                    "batch_wait_for_completion",
                    "batch_max_items",
                    "batch_concurrent_limit",
                    "batch_workspace_mode",
                    "batch_chain_prompt_ids",
                ],
                "OUTPUT_PIPING": (
                    "When batch_wait_for_completion=true, `{{steps.<name>.data.results}}` "
                    "is an ordered array of `{index, discussion_id, item, output, "
                    "tokens_used, tokens_status}`. `output` is the complete final Agent "
                    "message from that child discussion (not truncated), so an Exec/ApiCall "
                    "can consume the fan-out deterministically without an Agent collector. "
                    "The data envelope also carries aggregate `tokens_used` plus "
                    "`tokens_status` (`measured`, `partial`, or "
                    "`unavailable_children_not_measured`). Fire-and-forget mode returns "
                    "results=[] and tokens_status=pending because children are still running."
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
                "note": "fan one ApiCall over a list without starting a model.",
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
                "note": "deterministic data source that starts no model — feeds {{steps.<name>.data}}.",
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
                "FOREACH_CONCURRENCY": (
                    "SubWorkflow foreach is intentionally SEQUENTIAL in the shared parent "
                    "worktree. Workflow-level `concurrency_limit` controls overlapping FULL "
                    "workflow runs (Cron/Tracker); it does not parallelize foreach items, and "
                    "values above 1 are rejected when foreach is present so this cannot be "
                    "mistaken for an ignored worker count. "
                    "Use BatchQuickPrompt for parallel agent fan-out. Parallel SubWorkflow "
                    "children would require isolated worktrees plus deterministic merge and "
                    "is not silently enabled by any current field."
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
            "it may use only Agent steps. This 12-set IS the whole taxonomy. For a "
            "rich real example to adapt, workflow_get/workflow_clone the AutoPilot "
            "workflow (multi-step), not a single-Agent one."
        ),
        "template_vars": {
            "syntax": (
                "`{{namespace.path}}` in any string field (prompt_template, "
                "api_endpoint_path, api_query/api_body values, exec_args/exec_stdin, "
                "gate_message, notify_config, …). Dotted nested access works incl. "
                "array index: `{{steps.plan.data.subtasks.0.title}}`. An UNKNOWN ref "
                "is left VERBATIM in previews and rejected before execution, so a typo "
                "never reaches an agent, command or external API."
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
                "time.now / now": "one timestamp captured at run start and reused by all steps and parallel CollectApiData sources, including after resume. `now` is shorthand unless a static/launch variable named `now` exists.",
                "<launch_var>": "any `variables[].name` is referenced bare as `{{name}}`. Its source is user_input (default), project_env with source_ref `<env.NAME>`, or kronn_context with `<context.key>`. References resolve at run start; never store the resolved value.",
            },
            "time_expressions": {
                "canonical": "{{time.now|shift:-24h|tz:Europe/Paris|floor:hour|fmt:local_iso_ms}}",
                "shorthand": "{{now-24h|floor:hour}} (UTC + rfc3339 defaults)",
                "filters": {
                    "shift": "+/- fixed duration; units s, m, h, d, w; 10-year safety limit",
                    "tz": "IANA timezone such as Europe/Paris; UTC by default",
                    "floor": "minute | hour | day; day boundaries use the selected timezone",
                    "fmt": "rfc3339 | local_iso_ms | date | unix | unix_ms",
                },
                "vendor_neutrality": "Do not use plugin/vendor names such as fmt:adobe. Adobe's YYYY-MM-DDTHH:mm:ss.SSS without Z is fmt:local_iso_ms; GitHub uses rfc3339; Jira day parameters use date; epoch APIs use unix or unix_ms.",
                "ordering": "Filter order is declarative: Kronn applies the shift to the run anchor, converts to the requested timezone, floors there, then formats.",
            },
            "batch_quick_prompt_results": (
                "`steps.<name>.data.results` is the ordered, complete BatchQuickPrompt "
                "payload list after completion; use `data.results` / `data_json.results` "
                "for deterministic downstream piping."
            ),
        },
        "data_pipeline_contract": {
            "normal_path": "CollectApiData -> TransformData -> PublishPageData",
            "discovery_order": [
                "qa_list/qe_list: prefer and resolve saved Quick APIs or Quick Execs; keep inline quick_exec for one-offs and add each CLI binary to exec_allowlist",
                "page_list: reuse a matching shared Page; otherwise page_create",
                "workflow_step_schema: compose the three typed configs",
                "workflow_create_draft: save disabled for human review",
            ],
            "direct_publish": (
                "TransformData is optional when the Page intentionally consumes the complete "
                "lossless collector envelope. Then bind value_from directly to "
                "steps.<collect-name>.data."
            ),
            "page_contract": (
                "A Page is shared and is not owned by one workflow. page_publish.page_id "
                "stores the configured link; several workflows may target the same Page."
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
            "Kronn centralise les discussions, plans, automatisations et configurations. "
            "Depuis une CLI compatible reliée au MCP kronn-internal, je peux piloter les "
            "capacités exposées ci-dessous ; certaines validations et la saisie des secrets "
            "restent dans l'interface lorsqu'elles sont requises.\n\n"
            "## 💬 Discussions sauvegardées — ta mémoire partagée\n"
            "Chaque conversation vit dans Kronn, cherchable et rechargeable par N'IMPORTE quel agent.\n"
            "→ 'Retrouve ce qu'on a décidé sur l'auth le mois dernier' (disc_search + disc_load_other)\n"
            "→ 'Crée une disc pour ce sujet et notes-y nos conclusions' (disc_create + disc_append)\n\n"
            "## 🖼️ Rendus enrichis dans les rooms\n"
            "Les messages sont en Markdown. Un bloc `mermaid` devient un diagramme ; "
            "`kronn-doc-preview` affiche un HTML isolé avec export PDF/DOCX ; "
            "`kronn-doc-data` prépare un export CSV/XLSX/PPTX. Un simple bloc `html` "
            "reste du code : j'utilise les balises Kronn uniquement quand le rendu aide vraiment.\n\n"
            "## 🎬 Génération d'images et de vidéos\n"
            "Si l'humain a configuré un modèle image ou vidéo sur une connexion HTTP "
            "(OpenRouter, NVIDIA), je peux en générer un et l'attacher à la discussion. "
            "Je ne choisis pas le modèle : il vient du slot configuré, donc rien ne peut "
            "être facturé sur un modèle que l'humain n'a pas retenu. Une modalité sans slot "
            "est refusée en nommant ce qu'il faut configurer — je n'ai donc pas à deviner la "
            "disponibilité, je peux demander et lire la réponse.\n"
            "Le coût est réel (~0,07 USD pour 5 s de vidéo en 480p) : je demande le plus court "
            "clip qui répond à la question, et j'annonce la dépense.\n"
            "→ 'Génère une image de X dans cette disc' (media_generate puis media_job_status)\n\n"
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
            "(workflow_create_draft — je connais le schéma canonique des 12 types de steps)\n"
            "→ 'Lance le PR-review sur la 123' (workflow_trigger) · 'Qu'est-ce qui tourne ?' (workflow_active_runs)\n\n"
            "## 📄 Pages vivantes — des rapports HTML alimentés par les workflows\n"
            "Je peux réutiliser ou créer une Page, puis composer CollectApiData → TransformData → "
            "PublishPageData avec des Quick APIs déterministes.\n"
            "→ 'Crée une Page qui suit mes métriques Adobe toutes les heures' "
            "(qa_list + page_list/page_create + workflow_create_draft)\n\n"
            "## 🌐 APIs déclarées dans Kronn — sans exposer le secret au modèle\n"
            "mcp_list indique les plugins qui exposent une interface API et leurs endpoints autorisés. "
            "Le broker refuse les chemins non déclarés et applique l'authentification côté serveur. "
            "Tous les plugins MCP ne disposent pas forcément d'une interface API.\n"
            "→ 'Combien de tickets ouverts sur le projet EW ?' (mcp_list → api_call, auth injectée)\n"
            "→ Un appel que tu referas ? Je le sauvegarde en Quick API rejouable (qa_create_draft).\n\n"
            "## 🧠 La désagentification (LE concept clé pour bien commencer)\n"
            "Une étape mécanique ApiCall, Exec, JsonData, CollectApiData, TransformData, "
            "PublishPageData ou Notify s'exécute sans lancer de modèle. "
            "Si un agent déclenche cette étape ou consomme son résultat, sa conversation conserve "
            "toutefois son coût normal. Kronn réserve les agents aux étapes qui demandent du raisonnement. "
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


# KT-192 — reference material for the heaviest tools, moved OUT of the catalogue.
# The catalogue is injected into every session before any work is exchanged; these
# guides are read by the few callers that author one of these payloads. Splitting
# them is the trade the measurements argued for: pay once, on demand, instead of
# always. What stays in a tool's own description is what fails at RUN time if
# guessed — traps, closed sets, binding rules — never the methodology.
TOOL_MANUALS = {
    "agent_list": (
        "Use `agent_list()` before `task_exec_prepare` when the worker identity is not "
        "already known. Copy one entry's `worker` object unchanged; it is the same typed "
        "MessageTarget accepted by preflight. Native HTTP providers use "
        "`kind: discussion_agent`, punctual host processes use `kind: agent`, and an "
        "already joined CLI uses `kind: cli` plus its exact `cli_session_id`. Joined CLI "
        "tiers are empty because that transport ignores per-turn tier overrides.\n\n"
        "`configured` and `reachable` are independent observations. The strict invariant is "
        "`available => configured && reachable`; a configured provider can be temporarily "
        "down, while a public endpoint such as NVIDIA's catalogue can answer before an "
        "account key is configured. `available` proves only that Kronn's transport is ready "
        "for preflight — never that the selected model is entitled, that the task fits the "
        "worker, or that execution will succeed. NVIDIA is not completion-probed by this "
        "read-only discovery call. For Ollama, discovery also does not prove that the exact "
        "resolved tag is already pulled locally; a missing tag fails explicitly at `/api/chat`, "
        "so treat catalogue availability and model presence as separate facts.\n\n"
        "Unavailable entries remain visible with stable `reasons[].code`; details are fixed "
        "backend phrases and never contain keys, endpoints, hostnames or raw upstream errors. "
        "Provider probes run in parallel under a short global bound. After choosing an "
        "available identity, call `task_exec_prepare` and obey its task-specific refusal "
        "codes before `task_exec_launch`."
    ),
    "task_exec_prepare": (
        "**One durable lifecycle, two roles.** A principal starts by reading the room plan, "
        "selecting one Todo task and a typed worker. Native HTTP providers use "
        "`{kind: \"discussion_agent\", agent_type: \"Ollama\"|\"LiteLlm\"|\"Nvidia\", "
        "tier?: ...}`; a punctual host process uses `{kind: \"agent\", "
        "agent_type: \"ClaudeCode\"|\"Codex\"|...}`; an already joined CLI uses "
        "`{kind: \"cli\", agent_type, cli_session_id}` with the exact active id. Provider "
        "equality is not identity, and a transport fallback MUST change `kind`.\n\n"
        "**Local-worker contract.** Ollama preflight proves transport readiness, not task "
        "fitness. Use it only for one atomic, prelocalised mutation with principal-owned checks. "
        "For replacement use `worker_scope: {mode: \"prelocalized_edit\", path, start_line, "
        "end_line}`. For a pure insertion, prefer "
        "`{mode: \"prelocalized_insert_after\", path, anchor_line}`: Kronn exposes only the "
        "new text and preserves the anchor mechanically. Always choose the narrowest verified "
        "anchor (`start_line == end_line` for a one-line replacement). Kronn validates the "
        "closed shape against the SHA-pinned worktree and gives the worker one bounded read "
        "followed by one CAS mutation. Use a stronger worker immediately for trust or "
        "protocol boundaries, concurrency, migrations, architecture or cross-layer parity.\n\n"
        "Every prepare and launch MUST pass `worker_scope_intent: \"scoped\"` together with "
        "that exact `worker_scope`, or `worker_scope_intent: \"generic\"` with no scope when a "
        "general worker is deliberately intended. The sentinel proves the MCP host transported "
        "the current schema; omitting it fails closed with a reconnect diagnostic instead of "
        "silently launching a generic worker. Only `launchable:true` permits a matching launch; "
        "never create the child by hand or alter the worker, intent or scope without preflighting again. "
        "Launch may persist principal-owned `validations: [{command, quick_exec_id?, "
        "timeout_secs?}]`; never copy gates from the worker manifest. Reuse one idempotency key "
        "if the launch response is lost.\n\n"
        "**Worker handoff.** The child room contains the immutable brief, execution id, pinned "
        "worktree/branch and DeliveryManifest v1 shape. Work only in that checkout. The worker "
        "does not merge, approve or close the Planning task. When the DoD is evidenced, call "
        "`task_exec_deliver({task_execution_id, manifest})`; prose saying 'done' is not a durable "
        "delivery. The principal reads `task_exec_status`, then calls `task_exec_review` with a "
        "ReviewDecision v1. `request_changes` needs a non-empty actionable comment and preserves "
        "the same execution/worktree. An Ollama result is never its own quality gate: the "
        "principal reviews the exact SHA and runs the persisted validations. Allow at most one "
        "targeted local rework; structural misunderstanding or missing durable delivery keeps "
        "the trace and is reassigned to a stronger worker.\n\n"
        "**Reconnect and recovery.** After an MCP reload, restore the room with "
        "`disc_find_by_session({})`, refresh `plan_get`, and call `task_exec_status` on the known "
        "execution. Do not launch again merely because chat history is incomplete. Status is "
        "party-scoped; cancel/reassign are parent-principal-only; delivery is exact-worker-only. "
        "If a joined runtime lacks these tools after reconnect, reconnect the Kronn MCP and report "
        "the capability gap instead of fabricating a handoff."
    ),
    "task_exec_resume": (
        "Call resume only when `task_exec_status` returns the exact "
        "`next_action.tool: task_exec_resume`. The backend rechecks parent "
        "cleanliness and checkpoint SHAs, cannot skip provisioning or review, "
        "and returns the existing terminal result when an Applying-origin "
        "resume already succeeded. After reconnect, recover with status rather "
        "than replaying launch."
    ),
    "task_exec_reassign": (
        "Reassignment is principal-only and preserves the execution room, "
        "worktree and evidence. Pass the flat typed MessageTarget copied from "
        "`agent_list` (`kind`, `agent_type`, optional exact `cli_session_id` and "
        "tier), never the internal `{target, model, profile_id}` envelope. A "
        "transport change must change `worker.kind`. Native HTTP targets do not "
        "need an internal connection id; a dynamic Custom target does."
    ),
    "task_exec_accept_worker_offer": (
        "Pass only the opaque `offer_id` from the control message. The backend "
        "derives identity from this bridge's durable session and refuses another "
        "session even when it uses the same provider. Success moves the session "
        "into the child discussion, exposes the work brief and rebinds subsequent "
        "calls and `disc_wait_for_peer`. Refusals distinguish expired/already "
        "accepted state from an offer that is absent or not addressed to you."
    ),
    "disc_append": (
        "**BULK transcript import** (cross-agent memory, since 0.8.4) — pass "
        "`messages: [{source_msg_id, role, content, agent_type}, …]` to push a "
        "whole conversation history in one call instead of appending turn by "
        "turn. Idempotent on `(disc_id, source_msg_id)`: re-pushing the same "
        "transcript does not duplicate it, so a retry after a partial failure is "
        "safe.\n\n"
        "`diverged: true` in the response means the discussion was edited in the "
        "Kronn UI after a previous import. Warn the user before pushing more: "
        "further updates would be layered onto a transcript a human has already "
        "changed by hand.\n\n"
        "Everything else about this tool — mentions, targets, the fact that "
        "posting also listens — is in its own description, because getting it "
        "wrong means a peer never hears you."
    ),
    "disc_wait_for_peer": (
        "**Cursor and acknowledgement.** The bridge keeps a durable read cursor, "
        "so omitting `since_sort_order` is the normal case. A delivered batch is "
        "acknowledged only when the CLI makes its NEXT tool call, tracked by a "
        "bridge-local sequence rather than the client's JSON-RPC id (clients may "
        "legally reuse or omit that id). An unacknowledged batch is replayed after "
        "a reconnect rather than skipped: a duplicate in context is cheap, a lost "
        "message is not.\n\n"
        "When you do override the cursor, only reuse `latest_sort_order` returned "
        "by a WAIT. `last_sort_order` from an append is a different counter, and "
        "passing it skips every turn between the two.\n\n"
        "**Who a turn wakes.** Messages carry typed `targets` distinguishing three "
        "identities: the configured discussion agent, a punctual native invocation, "
        "and one exact joined CLI. Provider equality is not identity — a joined "
        "Codex must not answer a turn addressed to punctual Codex. A joined CLI "
        "message also carries `reply_target` when Kronn knows its exact author "
        "session, so a reply stays attached to that CLI even when several peers run "
        "the same provider.\n\n"
        "A joined CLI is WOKEN only by turns selecting its own identity. Everything "
        "else in the room arrives attached to its next wake as `awareness` context, "
        "so nothing is lost and nothing wakes it needlessly. `withheld_by_routing` "
        "counts turns deliberately omitted because they target someone else — that "
        "is why a jumping cursor is not evidence of a dropped message.\n\n"
        "`target_agents` / `target_agent` remain compatibility projections of "
        "`targets`; do not reason from them."
    ),
    "mcp_list": (
        "The result is a timestamped discovery snapshot: `configs` carries instance "
        "ids and project scope; `servers_with_api` carries descriptions, docs URLs, "
        "endpoints, side-effect flags, readiness `hint`s and `config_keys`. Re-list in "
        "the current session before creating or updating a workflow, QA or direct API "
        "call. Kronn allow-lists the returned slugs, ids and paths, so remembered or "
        "fabricated values fail only when executed.\n\n"
        "A config key is `{env_key,label,auth_managed}`. `${ENV.<env_key>}` works in "
        "endpoint paths, path params, query, headers and body only for non-secret "
        "identifiers where `auth_managed:false`. Authentication keys remain server-side; "
        "never reference or request their values. Read `hint` before acting: it states "
        "whether ApiCall is ready, documentation must be fetched, or user configuration "
        "is still required."
    ),
    "page_create": (
        "Live Page HTML is a complete, self-contained document rendered without network "
        "access. Declare named datasets as snapshot, time_series or collection contracts; "
        "an empty array creates standalone HTML, while `initial` values support mock design "
        "validation. Retention may be bounded with max_points or max_age_days.\n\n"
        "A bound discussion supplies project scope and is linked as the origin. A host CLI "
        "may omit discussion_id and create a standalone Page. The returned id is the exact "
        "PublishPageData.page_publish.page_id. HTML reads the initial value from "
        "window.KronnPageData and listens for the `kronn:page-data` CustomEvent."
        "\n\nA human-gated inline action pairs a visible element carrying "
        "`data-kronn-action=\"stable-ref\"` with an inert "
        "`<script type=\"application/kronn-action\" data-action-id=\"stable-ref\">` "
        "JSON block. The shared reference is 1–256 URL-safe characters "
        "(`[A-Za-z0-9._~-]`). Its shape is `{kind,target_id,project_id?,values?}` and kind is "
        "quick_prompt, quick_api, quick_exec or workflow; discover and use a real "
        "target id. A Page-only value may use provenance `dynamic_binding` plus a "
        "source_ref such as `<page.title>`, `<page.dataset.summary.owner>` or "
        "`<page.dataset.tickets.find(key).id>`. For the last form, "
        "`data-kronn-bindings` carries only a JSON selector map keyed by variable name. "
        "Never put secrets or resolved environment values in HTML. The sandbox emits "
        "an intention; only the native card's explicit human launch can execute it."
    ),
    "workflow_create_draft": (
        "The workflow always lands with `enabled:false`; no cron fires until the user "
        "reviews and enables it. Use autonomous creation only after the conversation has "
        "converged. The payload mirrors CreateWorkflowRequest: required name, tagged trigger "
        "and 1-20 steps; optional project, variables, guards, failure chain, allowlist, "
        "artifacts, concurrency, safety, actions and workspace config.\n\n"
        "Each PromptVariable is `{name,label?,placeholder?,description?,required?,pattern?,"
        "source?,source_ref?,allow_manual_override?,control?}`. Omitted source means "
        "`user_input`. Use `project_env` with a declarative `<env.NAME>` reference only "
        "when the workflow is project-bound and `mcp_list.configs` proves that exactly one "
        "active project config exposes NAME. Use `kronn_context` with `<context.key>` for "
        "allowlisted runtime metadata. Kronn resolves current values at every launch, then "
        "keeps one encrypted snapshot for that run; never copy a masked/resolved value.\n\n"
        "Type-specific step fields live at the top level. `step_type`, trigger and output "
        "format use tagged objects. `workflow_step_schema` is the canonical on-demand source "
        "for every field, example, output-piping rule and template namespace. In particular, "
        "SubWorkflow foreach uses `current_task.*` while batch fan-out uses `batch.item.*`. "
        "Every referenced plugin, binding, Quick API, Quick Exec and Page must come from its "
        "current list tool; unresolved bindings require asking the user, never guessing."
    ),
    "qp_run": (
        "Call `qp_list` first to resolve the QP id and its required variables. Pass values "
        "as `vars: {name: value}`; rendering uses the same `{{var}}` substitution as the UI "
        "and every required value must be non-empty. `qp_run` launches an existing QP — it "
        "does not create or update the definition.\n\n"
        "The QP's declared agent and project are used by default. `agent` and `project_id` "
        "are explicit overrides; omitting project scope preserves the QP's project or no "
        "project. The returned `next_check` is based on the QP's weighted-average first-reply "
        "duration across versions. After that delay, read the fresh discussion through "
        "`disc_load_other(disc_id)`; the agent already runs in the background."
    ),
    "qp_batch_run": (
        "Call `qp_list` first, then pass `items: [{title?, vars?}, ...]`. Each item renders "
        "the QP's `{{var}}` placeholders independently; every required variable must be "
        "non-empty on every item. Titles default to `<qp_name> #<n>` and the batch is capped "
        "at 50 children.\n\n"
        "All children share the returned `run_id`. Poll `workflow_run_status({run_id})` for "
        "`batch_completed / batch_total`, or enumerate them with "
        "`workflow_run_discussions({run_id})`, then read selected results with "
        "`disc_load_other`. `next_check` is the single-launch per-item baseline, so it is a "
        "floor for the whole batch. Use `qp_run` when exactly one discussion is needed."
    ),
    "qp_create_draft": (
        "Quick Prompts are reusable manual-launch templates and have no enabled flag. Create "
        "one only after the conversation has converged on a prompt worth keeping. Variables "
        "use `{{name}}`; declare each name and whether it is required. A variable may use "
        "`source:'project_env'` + `source_ref:'<env.NAME>'` when the QP is project-bound, "
        "or `source:'kronn_context'` + `<context.key>`; those references resolve anew per "
        "run and never carry the real value. Return the created id "
        "to the user so the QP can be opened or compared.\n\n"
        "Bindings are durable ids, not labels. Read the current QP/binding catalog first and "
        "use only real agent, skill, profile and directive ids. If a requested binding cannot "
        "be enumerated, ask instead of fabricating a UUID: unknown ids may be stripped at run "
        "time. Use the qp-improver flow for an existing QP rather than creating a duplicate."
    ),
    "api_call": (
        "**Project scope** — resolved server-side from three sources, in "
        "priority order: (1) an explicit `project_id` argument, (2) the disc "
        "context when Kronn spawned you from a disc (auto-injected), (3) the "
        "chosen `api_config_id`'s first linked project. Host-CLI sessions "
        "launched outside Kronn work natively through source 3 — no env var, no "
        "argument — as long as the config you pick is project-scoped. Pass "
        "`project_id` explicitly only when the config is global and you want the "
        "call attributed to one project.\n\n"
        "**Non-secret config values via `${ENV.KEY}`** — when a plugin config "
        "holds an identifier that is not a secret (Didomi's `organization_id`, an "
        "account_id, a workspace_slug), reference it instead of hardcoding it. "
        "Take the key from `mcp_list.servers_with_api[].config_keys`, then write "
        "e.g. `query: {organization_id: '${ENV.ORGANIZATION_ID}'}`. Kronn "
        "substitutes it server-side, so you never see the value. Accepted in "
        "`endpoint_path`, `path_params`, `query`, `headers` and `body` — string "
        "leaves only.\n\n"
        "**Run-anchored time** — the same fields accept vendor-neutral "
        "`{{time.now|...}}` expressions before `${ENV.KEY}` substitution. "
        "One timestamp is captured for the complete call. Use `shift:+1d|-24h`, "
        "`tz:Europe/Paris`, `floor:minute|hour|day`, and "
        "`fmt:rfc3339|local_iso_ms|date|unix|unix_ms`; the compact "
        "`{{now-24h|floor:hour}}` alias defaults to UTC/RFC 3339. "
        "Formats are generic: never write `fmt:adobe`; its no-zone ISO shape "
        "is `fmt:local_iso_ms`. `workflow_step_schema` is the canonical spec.\n\n"
        "This is NOT the mechanism for credentials. Secrets are injected by the "
        "plugin's auth spec and never appear in a call you compose."
    ),
    "qa_create_draft": (
        "**PROBE then PERSIST — the workflow that saves the most tokens.**\n"
        "  1. **Probe**: one `api_call` to the endpoint with NO `extract` (or "
        "`extract: null`). Read the real response shape. Many vendors (JIRA, "
        "Confluence, AWS, GitHub) return `changelog`, `renderedFields`, ADF nodes "
        "or ARN-heavy refs — 10-40k tokens for a single ticket.\n"
        "  2. **Decide**: pick the JSONPath that keeps only what downstream "
        "agents need (often `$.fields`, `$.data`, `$.items[*].{id,title,status}`). "
        "When in doubt, ask the user what they care about.\n"
        "  3. **Persist**: create the QA with that `api_extract` AND vendor-side "
        "filters in `api_query` (`fields=summary,status` for JIRA, `expand=` knobs "
        "for Confluence). Both stack: server-side filtering and client-side "
        "extraction.\n\n"
        "Persisting without `api_extract` is fine for small-payload vendors "
        "(Resend, Mailjet, simple webhooks) — but measure first. Adding it later "
        "with `qa_update` is friction the user never had to pay.\n\n"
        "**Variables**: each entry is `{name,label?,placeholder?,description?,required?,"
        "pattern?,source?,source_ref?,allow_manual_override?,control?}`; `required` defaults "
        "to true and `source` to `user_input`. A project-bound QA may use "
        "`source:'project_env'` + `source_ref:'<env.NAME>'`; runtime metadata uses "
        "`kronn_context` + `<context.key>`. Resolve available key names through `mcp_list`, "
        "never copy masked/resolved values, and let Kronn read the current value per run. "
        "`name` must match the `{{var_name}}` placeholders.\n\n"
        "**Pagination**: `api_pagination` is internally tagged `{\"type\": ...}`; "
        "Auto | Offset | Cursor | Page | LinkHeader (LinkHeader = GitHub-style bare "
        "array + `Link: rel=next` header; fields page_size_param/page_size/max_pages).\n\n"
        "**Rolling windows**: endpoint/query/header/body string leaves may also "
        "embed the server-side `{{time.now|...}}` grammar documented by "
        "`workflow_step_schema`. For an existing variable-based QA used by "
        "`CollectApiData`, put those expressions in the source `variables` map; "
        "parallel sources then share one workflow-run anchor.\n\n"
        "**Safety**: a QA has no `enabled` flag and cannot auto-fire — every QA is "
        "launched on demand, and the user reviews it in the Quick APIs page first. "
        "Same profile as `qp_create_draft`.\n\n"
        "**Iteration**: `qa_update({qa_id, ...patch})` merges onto the existing QA, "
        "so specify only what changed. Heavier payload than expected, missing query "
        "param, wrong extract path — all fixable without sending the user through "
        "the UI."
    ),
    "qa_run": (
        "Resolve a saved Quick API with `qa_list`, including its exact variable names. Pass "
        "only those values in the flat `vars` object; missing required values are rejected. "
        "The saved QA owns endpoint, method, query, headers, body, extraction and pagination, "
        "so every agent executes the same mechanical request.\n\n"
        "Execution is synchronous and audited in `api_call_logs`. Time templates are expanded "
        "server-side from one run anchor. Use `workflow_step_schema` for the canonical time "
        "grammar. If no QA matches, fall back to `api_call`, probe the response shape, and "
        "suggest persisting a reusable call with `qa_create_draft`."
    ),
    "learning_propose": (
        "A learning is a candidate, never an immediate truth-file write. State one scoped claim "
        "and classify it as `fact`, `preference` or `inference`. Supply at least one resolvable "
        "evidence item: file refs should include a line, URLs identify a source, discussion refs "
        "identify the relevant room, commands identify reproducible output, and user evidence "
        "records an explicit confirmation. A short quote should expose the premise.\n\n"
        "The server checks evidence resolution and may run a faithfulness gate; a human still "
        "accepts or rejects the candidate. Avoid `always`/`never` unless the evidence truly "
        "supports that scope. Discussion, project and agent provenance are inherited."
    ),
    "audit_launch": (
        "Call `audit_prepare` first and read its briefing status. A full audit runs the complete "
        "pipeline; a partial audit requires explicit 1-based step indices. The bridge consumes "
        "the SSE in a background thread, but the execution remains owned by this MCP process: "
        "closing or reloading it interrupts the run. Only one audit may run per project.\n\n"
        "Observe durable truth with `audit_status`. An interrupted full or specialized run may "
        "resume by its reported `resume_run_id`; an interrupted partial is relaunched for its "
        "stale scope. Successful full audits and fully successful partial audits create a "
        "validation discussion. Missing briefing context lowers audit quality and should be "
        "addressed in the UI when relevant."
    ),
    "qe_create_draft": (
        "**DISCOVER → TEST → PERSIST.** Call `qe_list` first. A Quick Exec is a "
        "saved CLI data collector, not a shell script: `command` is one bare binary "
        "and every `args[]` entry is one literal argv value. Never use `sh -c`, pipes, "
        "redirections, globs or command substitution. Shells and executable paths are "
        "rejected server-side.\n\n"
        "Declare every `{{variable}}` used in argv. PromptVariable supports `user_input` "
        "(default), or `project_env` with `source_ref:'<env.NAME>'` for a project-bound "
        "collector; `kronn_context` uses `<context.key>`. Discover safe key names through "
        "`mcp_list.configs[].env_keys`; never copy a masked/resolved value. Kronn resolves "
        "the current value at each run. `output_format` is `json`, `csv`, "
        "`text` or `lines`; CSV is normalized into an array of objects using its header "
        "row so TransformData and Pages receive ordinary JSON. Test the saved resource "
        "with `qe_run`. In a workflow, use `CollectApiData.sources[].quick_exec_id` and "
        "add the saved binary to the workflow `exec_allowlist`. Keep inline `quick_exec` "
        "only for a truly one-off command.\n\n"
        "A project-bound Quick Exec uses that project as cwd when tested standalone. A "
        "workflow uses its own run/worktree cwd, which makes the saved command portable."
    ),
}

# Launch deliberately shares the prepare guide: both calls form one typed,
# preflight-bound contract and duplicating the text would invite drift.
TOOL_MANUALS["task_exec_launch"] = TOOL_MANUALS["task_exec_prepare"]


def call_tool_manual(args):
    """Return one tool's authoring guide, or the list of what exists."""
    name = (args or {}).get("tool")
    if not isinstance(name, str) or not name.strip():
        return {
            "available": sorted(TOOL_MANUALS),
            "hint": "Pass `tool` to read one of these.",
        }
    name = name.strip()
    manual = TOOL_MANUALS.get(name)
    if manual is None:
        # Naming the alternatives beats a bare "unknown": a caller that guessed
        # the name is one hop from the right one.
        return {
            "error": f"No manual for {name!r}.",
            "available": sorted(TOOL_MANUALS),
            "hint": (
                "Only tools whose description points at `tool_manual` have one. "
                "For everything else the description IS the whole contract."
            ),
        }
    return {"tool": name, "manual": manual}


def _bridge_freshness():
    mtime_now, sha256_now = _bridge_script_snapshot()
    stale = (
        _BRIDGE_SCRIPT_SHA256_AT_LOAD is None
        or sha256_now is None
        or sha256_now != _BRIDGE_SCRIPT_SHA256_AT_LOAD
    )
    return {
        "mtime_now": mtime_now,
        "sha256_now": sha256_now,
        "stale": stale,
    }


def _require_fresh_bridge(tool_name):
    freshness = _bridge_freshness()
    if freshness["stale"]:
        raise BridgeStaleError(
            f"{tool_name}: refused before changing orchestration state because this "
            "kronn-internal bridge is stale; its loaded tool contract differs from "
            "the script on disk. Reconnect the Kronn MCP, recover the task with "
            "task_exec_status, then retry once with the same idempotency key."
        )


# One classification point for every orchestration action that may mutate
# execution state. Status is intentionally absent because recovery reads must
# remain available while a stale bridge is fail-closed.
_GUARDED_ORCHESTRATION_TOOLS = frozenset({
    "agent_list",
    "task_exec_prepare",
    "task_exec_launch",
    "task_exec_resume",
    "task_exec_cancel",
    "task_exec_reassign",
    "task_exec_accept_worker_offer",
    "task_exec_commit",
    "task_exec_deliver",
    "task_exec_review",
})


def call_bridge_info(_args):
    freshness = _bridge_freshness()
    mtime_now = freshness["mtime_now"]
    return {
        "script_path": _BRIDGE_SOURCE_PATH,
        "loaded_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(_BRIDGE_LOADED_AT)),
        "script_mtime_at_load": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(_BRIDGE_SCRIPT_MTIME_AT_LOAD)),
        "script_mtime_now": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(mtime_now)),
        "script_sha256_at_load": _BRIDGE_SCRIPT_SHA256_AT_LOAD,
        "script_sha256_now": freshness["sha256_now"],
        "stale": freshness["stale"],
        "reload": dict(_BRIDGE_RELOAD_STATE),
        "hint": (
            "stale=true: the on-disk bridge differs from this process. The next "
            "guarded mutation schedules one transparent reload; retry it once with "
            "the same idempotency key. If reload.status is failed, reconnect the "
            "Kronn MCP manually once."
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


# ─── KT-374 — the room reaches the agent, instead of waiting to be asked ────
#
# `disc_wait_for_peer` has to be CALLED. An agent three minutes into a
# `cargo test` does not call it, and that is not a discipline problem: the
# instruction was written into this very file and its author kept failing it
# the same day. So the room is attached to the return value of whatever tool
# the agent was already using — it arrives instead of having to be fetched.

# Tools that already put the room in front of the agent. Peeking on top of
# these would fetch what they just delivered, or re-ask what they are about
# to ask better.
_TOOLS_THAT_ALREADY_SHOW_THE_ROOM = frozenset({
    "disc_wait_for_peer",  # its whole job, and it waits properly
    "disc_join",           # hands back recent_messages on the way in
    "disc_leave",          # leaving; the room is no longer the agent's problem
})

# A burst of tool calls must not become a burst of requests. The peek is a
# single indexed query against a local backend, but "cheap" is not "free" and
# an agent can call ten tools in one turn. One peek per window is enough to
# make a peer's message visible within seconds.
_ROOM_PEEK_MIN_INTERVAL_SECS = 5.0
_LAST_ROOM_PEEK_AT = {"monotonic": None}

# An agent that was away for an hour can come back to a dozen turns. Attaching
# all of them to a `task_get` would bury the answer it asked for, so the batch
# is capped and the rest is announced as `remaining`.
#
# The cap is applied in SORT ORDER, never by priority, and that is a
# correctness constraint rather than a style choice: the read cursor is a
# single position. Showing a late addressed turn while hiding an earlier one
# would push the cursor past a message the agent never saw, and the next wait
# would consider it read. Oldest-first keeps "what was shown" and "what the
# cursor covers" the same set.
_ROOM_PEEK_MAX_MESSAGES = 8


def _hold_cursor_at_what_was_actually_shown(disc_id, shown_upto):
    """Never let the staged cursor cover a message that was withheld.

    `_wait_once` stages everything the server returned. When the batch is
    capped, that stage is a lie by exactly the messages left out — and a lie in
    the one direction that loses them. Pulling the stage back down is always
    safe: the worst case is re-delivering a turn, which costs a few tokens,
    against dropping one, which is the whole incident this ticket exists for.
    """
    pending = _PENDING_READ_SORT_ORDER_BY_DISC.get(disc_id)
    if pending is None:
        return
    if not isinstance(shown_upto, int) or isinstance(shown_upto, bool):
        _PENDING_READ_SORT_ORDER_BY_DISC.pop(disc_id, None)
        return
    if pending["sort_order"] > shown_upto:
        pending["sort_order"] = shown_upto


def _room_peek_for_tool_result(tool_name):
    """Unread room messages to attach to an unrelated tool's result, or None.

    Returns None far more often than not, and that is the point: nothing is
    injected when nothing is new (a peek that comes back quiet adds no key at
    all), when the caller is not in a room, or when a peek already ran inside
    the current window.

    This never raises. A peek is a courtesy attached to someone else's call;
    a room that cannot be reached must not turn `task_get` into a failure.
    """
    if tool_name in _TOOLS_THAT_ALREADY_SHOW_THE_ROOM:
        return None
    try:
        disc_id = _disc_id()
    except Exception:  # noqa: BLE001 — no room bound is the normal case here
        return None
    if not disc_id:
        return None

    now = time.monotonic()
    last = _LAST_ROOM_PEEK_AT["monotonic"]
    if last is not None and (now - last) < _ROOM_PEEK_MIN_INTERVAL_SECS:
        return None
    _LAST_ROOM_PEEK_AT["monotonic"] = now

    try:
        peek_args = {"_disc_id": disc_id, "timeout_secs": 0}
        cursor = _read_cursor(disc_id)
        if cursor is not None:
            peek_args["since_sort_order"] = cursor
        # `_wait_once` stages the read cursor for whatever it delivers, exactly
        # as an explicit wait does. That is what keeps this from duplicating:
        # the durable cursor stays the single source of truth, so a message
        # surfaced here is not surfaced again by the next `disc_wait_for_peer`,
        # and a cancelled turn un-stages it rather than losing it.
        waited = _wait_once(peek_args)
    except Exception:  # noqa: BLE001 — see docstring: never break the host call
        return None

    if not isinstance(waited, dict):
        return None
    messages = waited.get("messages")
    if not isinstance(messages, list) or not messages:
        return None

    shown = messages[:_ROOM_PEEK_MAX_MESSAGES]
    remaining = len(messages) - len(shown)
    if remaining:
        shown_orders = [
            message.get("sort_order") for message in shown
            if isinstance(message, dict) and isinstance(message.get("sort_order"), int)
        ]
        _hold_cursor_at_what_was_actually_shown(
            disc_id, max(shown_orders) if shown_orders else None,
        )

    # Two lists, not one list and a counter. A turn addressed to THIS session is
    # a debt someone is waiting on; ambient room traffic is background. Merging
    # them into a homogeneous array leaves the agent to re-derive that
    # distinction on every read, which is exactly the step it skips when busy.
    attention_required = [
        message for message in shown
        if isinstance(message, dict) and message.get("addressed_to_caller")
    ]
    context = [
        message for message in shown
        if not (isinstance(message, dict) and message.get("addressed_to_caller"))
    ]
    room = {"unread": len(messages)}
    if attention_required:
        room["attention_required"] = attention_required
    if context:
        room["context"] = context
    if remaining:
        room["remaining"] = remaining
    withheld = waited.get("withheld_by_routing")
    if isinstance(withheld, int) and withheld > 0:
        room["withheld_by_routing"] = withheld

    if attention_required:
        room["hint"] = (
            f"{len(attention_required)} message(s) in `attention_required` are addressed to "
            "YOU and arrived while you were working; they rode along on this result because "
            "you did not ask for them. Read them BEFORE continuing — a peer routinely "
            "announces a scope you are about to duplicate. Answer with disc_append."
        )
    else:
        room["hint"] = (
            "Room context that arrived while you were working, attached to this result. "
            "Nothing here is addressed to you specifically: read it, do not answer it turn "
            "by turn."
        )
    if remaining:
        room["hint"] += (
            f" {remaining} older turn(s) are held back to keep this batch small; they are "
            "still unread and come back on your next call."
        )
    return room


DISPATCH = {
    # 0.8.12 PR A — audit surface
    "audit_prepare": call_audit_prepare,
    "audit_install_template": call_audit_install_template,
    "tool_manual": call_tool_manual,
    "bridge_info": call_bridge_info,
    "kronn_intro": call_kronn_intro,
    "resolve_id": call_resolve_id,
    "audit_launch": call_audit_launch,
    "audit_status": call_audit_status,
    "disc_meta": call_disc_meta,
    "disc_get_message": call_disc_get_message,
    "disc_note_list": call_disc_note_list,
    "disc_summarize": call_disc_summarize,
    # 0.9.1 — planning and discussion plans.
    "plan_get": call_plan_get,
    "task_list": call_task_list,
    "task_get": call_task_get,
    "proposal_list": call_proposal_list,
    "proposal_get": call_proposal_get,
    "task_changes": call_task_changes,
    "task_create": call_task_create,
    "task_update": call_task_update,
    "task_update_dod": call_task_update_dod,
    "task_link_discussion": call_task_link_discussion,
    "task_add_blocker": call_task_add_blocker,
    "task_remove_blocker": call_task_remove_blocker,
    # 0.8.4 (#294) cross-agent memory
    "disc_create": call_disc_create,
    "disc_append": call_disc_append,
    "disc_link": call_disc_link,
    "disc_transfer_session": call_disc_transfer_session,
    "agent_list": call_agent_list,
    "task_exec_prepare": call_task_exec_prepare,
    "task_exec_launch": call_task_exec_launch,
    "task_exec_status": call_task_exec_status,
    "task_exec_resume": call_task_exec_resume,
    "task_exec_cancel": call_task_exec_cancel,
    "task_exec_reassign": call_task_exec_reassign,
    "task_exec_accept_worker_offer": call_task_exec_accept_worker_offer,
    "task_exec_commit": call_task_exec_commit,
    "task_exec_deliver": call_task_exec_deliver,
    "task_exec_review": call_task_exec_review,
    "disc_unlink": call_disc_unlink,
    "disc_workspace_get": call_disc_workspace_get,
    "disc_workspace_set": call_disc_workspace_set,
    "disc_workspace_history_lease": call_disc_workspace_history_lease,
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
    "qe_list": call_qe_list,
    "page_list": call_page_list,
    "page_get": call_page_get,
    "page_create": call_page_create,
    "page_update_html": call_page_update_html,
    "page_add_dataset": call_page_add_dataset,
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
    "qe_create_draft": call_qe_create_draft,
    # 0.8.6 phase 4 — partial-update for QAs (load-merge-write).
    # Closes the post-test iteration loop : agent probes, persists,
    # tests, then patches `api_extract` / `api_query` without UI clicks.
    "qa_update": call_qa_update,
    "qe_update": call_qe_update,
    # 0.8.6 — Agent API broker. Lets the agent invoke a configured
    # plugin without ever seeing the credentials (cf.
    # [[project_agent_api_broker_0_8_6]]).
    "api_call": call_api_call,
    # 0.8.6 phase 4 — MCP remote control. Launches + tracks workflows
    # and Quick Prompts from MCP, with smart-polling next_check hints
    # to cut mobile token cost ~80% (cf. [[project_mcp_remote_control_0_8_6]]).
    "media_generate": call_media_generate,
    "media_job_status": call_media_job_status,
    "workflow_trigger": call_workflow_trigger,
    "workflow_run_status": call_workflow_run_status,
    "qp_run": call_qp_run,
    # 0.8.7 phase 4 PR2/PR3 — batch fan-out, child-disc listing, long-poll
    # wait. Completes the mobile remote-control surface.
    "qp_batch_run": call_qp_batch_run,
    "workflow_run_discussions": call_workflow_run_discussions,
    "workflow_wait_for_completion": call_workflow_wait_for_completion,
    # 0.8.6 phase 4 — synchronous QA execution. The deagentified twin
    # of `api_call`: same end-result without starting another model for
    # the HTTP call. Always prefer when a matching QA exists.
    "qa_run": call_qa_run,
    "qe_run": call_qe_run,
    # 0.10.0 — Continual Learning. Propose a durable learning (typed, evidence
    # mandatory). Server gates it (existence + faithfulness) + a human validates
    # before it's ever written to a truth file. Free-form fences are NOT used.
    "learning_propose": call_learning_propose,
}


# ─── MCP JSON-RPC loop ─────────────────────────────────────────────────────

def _send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def _schedule_bridge_reload():
    """Preflight and schedule at most one self-reexec for this loaded process."""
    if _BRIDGE_RELOAD_STATE["status"] not in ("idle", "deferred_active_audit"):
        return dict(_BRIDGE_RELOAD_STATE)
    with _AUDIT_LOCK:
        active_audits = sorted(project_id for project_id, entry in _AUDIT_STREAMS.items()
                               if entry.get("state") in ("launching", "running"))
    if active_audits:
        _BRIDGE_RELOAD_STATE.update(
            status="deferred_active_audit",
            error="active audit SSE stream(s): " + ", ".join(active_audits),
        )
        return dict(_BRIDGE_RELOAD_STATE)
    global _BRIDGE_ARTIFACT_FD
    try:
        artifact_fd, artifact_path = tempfile.mkstemp(
            prefix="kronn-mcp-artifact-", suffix=".py"
        )
        try:
            # Remove the name before the first write.  From this point on the
            # artifact is fd-only, so a concurrent replacement of the old
            # pathname cannot influence preflight or exec.
            os.unlink(artifact_path)
            artifact_path = None
            if os.fstat(artifact_fd).st_nlink != 0:
                raise RuntimeError("bridge reload artifact could not be unlinked")
            with open(_BRIDGE_SOURCE_PATH, "rb") as source:
                artifact_bytes = source.read()
            with os.fdopen(os.dup(artifact_fd), "wb") as artifact:
                artifact.write(artifact_bytes)
                artifact.flush()
            os.fsync(artifact_fd)
            os.lseek(artifact_fd, 0, os.SEEK_SET)
            os.set_inheritable(artifact_fd, True)
        except BaseException:
            with contextlib.suppress(OSError):
                os.close(artifact_fd)
            if artifact_path is not None:
                with contextlib.suppress(OSError):
                    os.unlink(artifact_path)
            raise
        artifact_sha256 = hashlib.sha256(artifact_bytes).hexdigest()
        artifact_exec_path = f"/dev/fd/{artifact_fd}"
        checked = subprocess.run(
            [sys.executable, artifact_exec_path],
            env={**os.environ, _BRIDGE_PREFLIGHT_ENV: "1",
                 _BRIDGE_SOURCE_ENV: _BRIDGE_SOURCE_PATH,
                 _BRIDGE_ARTIFACT_SHA_ENV: artifact_sha256},
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE, text=True, timeout=10,
            pass_fds=(artifact_fd,),
        )
        if checked.returncode != 0:
            raise RuntimeError((checked.stderr or "bridge preflight failed")[-1000:])
    except (OSError, subprocess.SubprocessError, RuntimeError) as exc:
        if locals().get("artifact_path"):
            with contextlib.suppress(OSError):
                os.unlink(artifact_path)
        if "artifact_fd" in locals():
            with contextlib.suppress(OSError):
                os.close(artifact_fd)
        _BRIDGE_RELOAD_STATE.update(status="failed", error=str(exc))
    else:
        _BRIDGE_ARTIFACT_FD = artifact_fd
        _BRIDGE_RELOAD_STATE.update(
            status="scheduled", error=None, artifact_sha256=artifact_sha256,
        )
    return dict(_BRIDGE_RELOAD_STATE)


def _perform_scheduled_bridge_reload():
    """Replace this process while preserving its inherited stdio transport."""
    if _BRIDGE_RELOAD_STATE["status"] != "scheduled":
        return False
    global _BRIDGE_ARTIFACT_FD
    handoff_path = None
    handoff_fd = None
    try:
        _STDIN_READ_LOCK.acquire()
        queued = []
        stdin_eof = False
        while True:
            try:
                item = _REQUEST_QUEUE.get_nowait()
            except queue.Empty:
                break
            if item is None:
                stdin_eof = True
            else:
                queued.append(item)
        queued_ids = {
            rid for rid in (item.get("id") for item in queued)
            if isinstance(rid, (str, int, float)) and not isinstance(rid, bool)
        }
        with _CANCELLED_LOCK:
            cancelled = [
                rid for rid, ts in _CANCELLED_REQUEST_IDS.items()
                if rid in queued_ids and time.monotonic() - ts <= _CANCELLATION_TTL_SECS
            ]
        handoff_fd, handoff_path = tempfile.mkstemp(prefix="kronn-mcp-reload-", suffix=".json")
        try:
            # Same fd-only rule as the executable artifact: never write the
            # handoff while a pathname can still name its inode.
            os.unlink(handoff_path)
            handoff_path = None
            if os.fstat(handoff_fd).st_nlink != 0:
                raise RuntimeError("bridge reload handoff could not be unlinked")
            handoff_nonce = secrets.token_hex(32)
            with os.fdopen(os.dup(handoff_fd), "w", encoding="utf-8") as handoff:
                json.dump({
                    "version": _BRIDGE_HANDOFF_VERSION,
                    "nonce": handoff_nonce,
                    "client_info": dict(_CLIENT_INFO),
                    "requests": queued,
                    "cancelled_request_ids": cancelled,
                    "pending_hex": bytes(_STDIN_PENDING).hex(),
                    "stdin_eof": stdin_eof,
                }, handoff, separators=(",", ":"))
                handoff.flush()
                os.fsync(handoff.fileno())
                if os.fstat(handoff.fileno()).st_size > _BRIDGE_HANDOFF_MAX_BYTES:
                    raise RuntimeError("bridge reload handoff exceeds size limit")
        except BaseException:
            with contextlib.suppress(OSError):
                os.close(handoff_fd)
            raise
        os.lseek(handoff_fd, 0, os.SEEK_SET)
        os.set_inheritable(handoff_fd, True)
        # The replacement receives this already-open, unlinked inode.  A
        # pathname handoff would leave a replacement race between validation
        # and read, even with O_NOFOLLOW and a nonce.
        os.environ[_BRIDGE_RELOAD_HANDOFF_FD_ENV] = str(handoff_fd)
        os.environ[_BRIDGE_RELOAD_HANDOFF_NONCE_ENV] = handoff_nonce
        os.environ[_BRIDGE_RELOAD_READY_ENV] = "1"
        expected_sha256 = _BRIDGE_RELOAD_STATE.get("artifact_sha256")
        if _BRIDGE_ARTIFACT_FD is None:
            raise RuntimeError("preflighted bridge artifact descriptor is unavailable")
        os.lseek(_BRIDGE_ARTIFACT_FD, 0, os.SEEK_SET)
        with os.fdopen(os.dup(_BRIDGE_ARTIFACT_FD), "rb") as artifact:
            actual_sha256 = hashlib.sha256(artifact.read()).hexdigest()
        if actual_sha256 != expected_sha256:
            raise RuntimeError("preflighted bridge artifact changed before exec")
        os.lseek(_BRIDGE_ARTIFACT_FD, 0, os.SEEK_SET)
        os.environ[_BRIDGE_SOURCE_ENV] = _BRIDGE_SOURCE_PATH
        os.environ[_BRIDGE_ARTIFACT_FD_ENV] = str(_BRIDGE_ARTIFACT_FD)
        os.environ[_BRIDGE_ARTIFACT_SHA_ENV] = expected_sha256
        artifact_exec_path = f"/dev/fd/{_BRIDGE_ARTIFACT_FD}"
        os.execv(sys.executable, [sys.executable, artifact_exec_path])
    except (OSError, TypeError, ValueError, RuntimeError) as exc:
        os.environ.pop(_BRIDGE_RELOAD_READY_ENV, None)
        os.environ.pop(_BRIDGE_RELOAD_HANDOFF_ENV, None)
        os.environ.pop(_BRIDGE_RELOAD_HANDOFF_FD_ENV, None)
        os.environ.pop(_BRIDGE_RELOAD_HANDOFF_NONCE_ENV, None)
        os.environ.pop(_BRIDGE_ARTIFACT_FD_ENV, None)
        if handoff_fd is not None:
            with contextlib.suppress(OSError):
                os.close(handoff_fd)
        if handoff_path:
            with contextlib.suppress(OSError):
                os.unlink(handoff_path)
        _BRIDGE_RELOAD_STATE.pop("artifact_sha256", None)
        if _BRIDGE_ARTIFACT_FD is not None:
            with contextlib.suppress(OSError):
                os.close(_BRIDGE_ARTIFACT_FD)
            _BRIDGE_ARTIFACT_FD = None
        for item in queued:
            _REQUEST_QUEUE.put(item)
        if stdin_eof:
            _REQUEST_QUEUE.put(None)
        if _STDIN_READ_LOCK.locked():
            _STDIN_READ_LOCK.release()
        _BRIDGE_RELOAD_STATE.update(status="failed", error=str(exc))
        return False
    return True


def _close_inherited_reload_artifact():
    """Close the descriptor used only to bootstrap this process image.

    Python has already read `/dev/fd/<n>` before it reaches `main`, so retaining
    the inheritable descriptor would serve no purpose and would leak one fd on
    every hot reload.
    """
    raw_fd = os.environ.pop(_BRIDGE_ARTIFACT_FD_ENV, None)
    if raw_fd is None:
        return
    if (not isinstance(raw_fd, str)
            or re.fullmatch(r"[1-9][0-9]*", raw_fd) is None
            or int(raw_fd) <= 2):
        raise RuntimeError("bridge reload artifact descriptor is invalid")
    fd = int(raw_fd)
    try:
        descriptor = os.fstat(fd)
        if not stat.S_ISREG(descriptor.st_mode):
            raise RuntimeError("bridge reload artifact descriptor is not a regular file")
        if descriptor.st_nlink != 0:
            raise RuntimeError("bridge reload artifact descriptor is still linked")
    finally:
        with contextlib.suppress(OSError):
            os.close(fd)


def _bridge_stale_result(rid, tool_name, message):
    reload_state = _schedule_bridge_reload()
    failed = reload_state["status"] in ("failed", "deferred_active_audit")
    payload = {
        "error_code": "bridge_stale",
        "tool": tool_name,
        "mutation_applied": False,
        "reload": reload_state,
        "retry": {
            "allowed": not failed,
            "same_idempotency_key_required": True,
            "max_attempts": 1,
        },
        "action": (
            "Wait for the active audit to finish (or stop it explicitly), then retry; "
            "the bridge will reload without interrupting its SSE stream."
            if reload_state["status"] == "deferred_active_audit" else
            "Reconnect the Kronn MCP manually once, recover with task_exec_status, "
            "then retry once with the same idempotency key."
            if failed else
            "The bridge will reload over the existing transport. Retry once with "
            "the same idempotency key after the tool list refreshes."
        ),
        "detail": str(message),
    }
    return {
        "jsonrpc": "2.0",
        "id": rid,
        "result": {
            "isError": True,
            "content": [{
                "type": "text",
                "text": json.dumps(payload, ensure_ascii=False, indent=2),
            }],
        },
    }


# KT-189 — requests flow through a queue fed by a reader thread so the
# bridge can notice `notifications/cancelled` (and any follow-up request)
# while a tool call is blocked in the bridge-side wait loop. Only the main
# thread writes to stdout; the reader thread only parses and enqueues.
_REQUEST_QUEUE: "queue.Queue[dict | None]" = queue.Queue()
_STDIN_READ_LOCK = threading.Lock()
_STDIN_PENDING = bytearray()
# id → monotonic arrival time. Entries expire so a cancellation landing
# AFTER its response was sent can never poison a later reuse of the id.
# Guarded by _CANCELLED_LOCK: the reader thread inserts/prunes while the
# main thread checks/consumes.
_CANCELLED_REQUEST_IDS: dict = {}
_CANCELLED_LOCK = threading.Lock()
_CANCELLATION_TTL_SECS = 600
# Per-call context for the wait loop: the JSON-RPC id (to spot its own
# cancellation) and the client's progressToken (to keep the call alive).
_CURRENT_PROGRESS_TOKEN: dict = {"rid": None, "token": None}
_QUEUE_EMPTY = object()


def _is_cancelled(rid):
    with _CANCELLED_LOCK:
        ts = _CANCELLED_REQUEST_IDS.get(rid)
        if ts is None:
            return False
        if time.monotonic() - ts > _CANCELLATION_TTL_SECS:
            _CANCELLED_REQUEST_IDS.pop(rid, None)
            return False
        return True


def _consume_cancellation(rid):
    """Check-and-clear: a cancellation applies to exactly one request."""
    if rid is None:
        return False
    with _CANCELLED_LOCK:
        ts = _CANCELLED_REQUEST_IDS.pop(rid, None)
        return ts is not None and time.monotonic() - ts <= _CANCELLATION_TTL_SECS


def _stdin_reader():
    while True:
        try:
            ready, _, _ = select.select([sys.stdin], [], [], 0.1)
        except (TypeError, ValueError, OSError):
            # In-memory streams are used by unit tests and embedders. They do
            # not participate in reexec, so the ordinary iterator is enough.
            for raw in sys.stdin:
                with _STDIN_READ_LOCK:
                    _consume_stdin_chunk(
                        raw.encode("utf-8") if isinstance(raw, str) else raw
                    )
            _REQUEST_QUEUE.put(None)
            return
        if not ready:
            continue
        with _STDIN_READ_LOCK:
            chunk = os.read(sys.stdin.fileno(), 65536)
            if not chunk:
                _REQUEST_QUEUE.put(None)
                return
            _consume_stdin_chunk(chunk)


def _consume_stdin_chunk(chunk):
    """Consume complete lines while retaining a reexec-safe partial line."""
    _STDIN_PENDING.extend(chunk)
    while True:
        newline = _STDIN_PENDING.find(b"\n")
        if newline < 0:
            if len(_STDIN_PENDING) > _BRIDGE_PENDING_MAX_BYTES:
                raise RuntimeError("partial JSON-RPC line exceeds bridge limit")
            return
        raw_line = bytes(_STDIN_PENDING[:newline])
        del _STDIN_PENDING[:newline + 1]
        _enqueue_stdin_line(raw_line.decode("utf-8", errors="replace"))


def _enqueue_stdin_line(raw):
    line = raw.strip()
    if not line:
        return
    try:
        req = json.loads(line)
    except json.JSONDecodeError:
        print(f"kronn-internal: bad JSON-RPC line ignored: {line[:120]}", file=sys.stderr)
        return
    if not isinstance(req, dict):
        return
    if (req.get("method") or "") == "notifications/cancelled":
        params = req.get("params") or {}
        rid = params.get("requestId")
        # Generation guard (KT-189 review): a cancellation landing AFTER
        # its request completed must not poison a legal reuse of the same
        # JSON-RPC id. Only requests still in flight or still queued can
        # be cancelled; anything else is a spec-sanctioned no-op.
        if rid is not None and _cancellation_applies(rid):
            now = time.monotonic()
            with _CANCELLED_LOCK:
                # Lazy pruning keeps the dict bounded without a timer.
                for stale in [k for k, ts in _CANCELLED_REQUEST_IDS.items()
                              if now - ts > _CANCELLATION_TTL_SECS]:
                    _CANCELLED_REQUEST_IDS.pop(stale, None)
                _CANCELLED_REQUEST_IDS[rid] = now
            return
        return
    _REQUEST_QUEUE.put(req)


def _cancellation_applies(rid):
    if _CURRENT_PROGRESS_TOKEN.get("rid") == rid:
        return True
    with _REQUEST_QUEUE.mutex:
        return any(
            isinstance(item, dict) and item.get("id") == rid
            for item in _REQUEST_QUEUE.queue
        )


def _peek_request():
    with _REQUEST_QUEUE.mutex:
        return _REQUEST_QUEUE.queue[0] if _REQUEST_QUEUE.queue else _QUEUE_EMPTY


def _service_control_traffic():
    """Serve control-plane traffic inline during a bridge-side wait.

    `ping`, `tools/list` and protocol notifications are answered right
    here so a long wait never looks dead to the client and never costs a
    model turn. Only a queued `tools/call` ("new_request") or stdin EOF
    ("eof") preempts the wait. Returns the preemption reason or None.
    """
    while True:
        head = _peek_request()
        if head is _QUEUE_EMPTY:
            return None
        if head is None:
            return "eof"
        if (head.get("method") or "") == "tools/call":
            return "new_request"
        _REQUEST_QUEUE.get_nowait()
        resp = _handle(head)
        if resp is not None:
            _send(resp)


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
        if _spawned_task_worker_mode():
            return {
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {"listChanged": True}},
                    "serverInfo": {
                        "name": "kronn-internal",
                        "version": BRIDGE_TOOL_SURFACE_VERSION,
                    },
                    "instructions": (
                        "You are a spawned Kronn task worker. Use your native CLI "
                        "file and shell tools to complete the bounded task in the "
                        "current worktree. Do not run `git commit`: shared Git storage "
                        "is outside your sandbox by design. Call `task_exec_commit` with "
                        "only the explicit changed files and message; Kronn commits them "
                        "through its authenticated server boundary. Then call "
                        "`task_exec_deliver` with semantic `manifest` assertions only "
                        "(tests, ordered DoD "
                        "evidence, docs/migrations/risks/limitations and summary). "
                        "Kronn injects and verifies execution/task identity, clean HEAD, "
                        "file inventory and opaque DoD ids; never invent them."
                    ),
                },
            }
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
                "capabilities": {"tools": {"listChanged": True}},
                # Tool-surface version, intentionally distinct from the Kronn
                # app release. Bumping it tells clients that cache tools/list
                # to refresh after the Planning contract was added.
                "serverInfo": {
                    "name": "kronn-internal",
                    "version": BRIDGE_TOOL_SURFACE_VERSION,
                },
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
                    "discussions, multi-step workflows and external APIs. "
                    "Kronn stores configured credentials encrypted and injects API-broker "
                    "authentication server-side. MCP host sync may also write environment "
                    "values required by the local CLI configuration. Never paste secrets into prompts. "
                    "Your tools, by area:\n"
                    "• Opaque IDs: when the user pastes an ID without naming its type, call `resolve_id` FIRST; it returns compact routing context and the object-specific tool to use next.\n"
                    "• Discussions (multi-agent threads): `disc_meta`/`disc_get_message`/`disc_search`/`disc_load_other`/`disc_create`/`disc_append`/`disc_join`/`disc_invite_peer`…\n"
                    "• **Working in a room:** a room is not a mailbox you empty at the end. After each real step — a commit, a green test run, a background task that finished, a milestone — call `disc_wait_for_peer` BEFORE starting the next one. Its cursor is durable, so re-checking never re-delivers what you already read, and a quiet return costs nothing. A peer's message routinely changes what you are about to build: a design review of the choice you just made, a file boundary, a decision the human already took. Long silent stretches of work are how two agents duplicate each other, or how one keeps building on a choice the other has already overturned. Messages flagged `awareness: true` are context to read, never turns to answer. Any tool result may also carry a `kronn_room` block: turns that arrived while you were working, attached to an answer you asked for. `attention_required` holds turns addressed to YOU — read them before continuing, because a peer announcing a scope is how duplicate work gets prevented; `context` is background you read without answering turn by turn. Seeing it does not replace calling `disc_wait_for_peer`: it appears only when you happen to call something else.\n"
                    "• Rich room output: messages are Markdown. A `mermaid` fence renders a diagram; `kronn-doc-preview` renders sandboxed HTML with PDF/DOCX actions (a plain `html` fence is only code); `kronn-doc-data` exposes CSV/XLSX/PPTX export. Use visual output only when it materially helps.\n"
                    "• Planning: a discussion may have a shared plan made of prioritized, editable tasks. The user may refer to it naturally as “the plan”, “the tasks”, “what remains”, “the priority”, and similar wording. Use `plan_get` (compact current objective/plan) · `task_list` (compact filtered backlog) · `task_get` (FULL task) · `task_changes` (deltas) · `proposal_list`/`proposal_get` (durable proposals, read-only) · narrow writes `task_create`/`task_update`/`task_update_dod`/`task_link_discussion`/`task_add_blocker`/`task_remove_blocker`. Read the relevant plan first. Immediately before any direct `task_create`, call `plan_get` again so a peer's recent write is visible. Apply unambiguous intent directly; otherwise propose a human-gated `kronn-plan-action` fence (`create`, `create_many`, `status`, `complete`, `unblock`, `open`). You may read and propose, but only a human accepts, rejects or decides a durable proposal. Never replace a requested plan update with a prose-only summary. Whenever tracked work starts or materially changes, keep its status, DoD and priority honest in the plan. Write only on a real change: never reload or rewrite an unchanged task merely to report progress. If the announced Planning tools are missing from your MCP surface, use the read-only `plan_snapshot` from `disc_join`, ask @user to reconnect the Kronn MCP, and never fabricate an update.\n"
                    "• Human-gated Automation proposals: after resolving a real QP/QA/QE/Workflow id and its declared variables through the catalogue/get tools, an agent may emit one `kronn-action` fence with `{\"kind\":\"quick_prompt|quick_api|quick_exec|workflow\",\"target_id\":\"<real id>\",\"project_id\":\"<optional id>\",\"values\":[{\"name\":\"<declared variable>\",\"value\":\"<editable suggestion>\",\"provenance\":\"agent_suggestion\",\"suggested_by\":\"<your alias>\"}]}`. This proposes only: Kronn validates and persists the card, and the human click launches it. Never invent ids/variables or include secret/resolved values.\n"
                    "• Workflows (multi-step pipelines): `workflow_list` (compact) · `workflow_get` (FULL, every step) · `workflow_step_schema` (CANONICAL step schema as an untruncatable result — the closed 12 `step_type`s, per-type fields, runtime contracts; call before authoring) · `workflow_create_draft` · `workflow_clone`/`workflow_update`/`workflow_set_enabled` · `workflow_trigger`/`workflow_run_status` · run history `workflow_runs`/`workflow_run_get` · `workflow_active_runs`/`workflow_cancel_run`. Agent-step bindings (full CRUD): `skills_list`/`profiles_list`/`directives_list` enumerate valid ids; `skill_get`/`profile_get`/`directive_get` read FULL bodies; `skill_create`/`skill_update`/`skill_delete` (+ `profile_*`/`directive_*`) author & edit custom ones.\n"
                    "• Quick Prompts (reusable prompt templates): `qp_list` (no body) · `qp_get` (FULL incl `prompt_template` — read this to know what a QP does, or to run it yourself) · `qp_create_draft`/`qp_update`/`qp_delete` · `qp_run`/`qp_batch_run`.\n"
                    "• Quick APIs + API broker: `qa_list`/`qa_run`/`qa_create_draft`/`qa_update` · `mcp_list` → `api_call` (configured plugins, auth injected). Quick Execs: `qe_list`/`qe_run`/`qe_create_draft`/`qe_update` for saved shell-free CLI collectors.\n"
                    "• Live Pages (shared HTML reports): `page_list` · `page_get` · `page_create` · `page_update_html` · `page_add_dataset`. Resolve or create the Page before authoring a `PublishPageData` step.\n"
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
    if method == "ping":
        # MCP liveness probe — answered inline even mid-wait (KT-189).
        return {"jsonrpc": "2.0", "id": rid, "result": {}}
    if method == "tools/list":
        return {"jsonrpc": "2.0", "id": rid, "result": {"tools": _visible_tools()}}
    if method == "tools/call":
        params = req.get("params") or {}
        name = params.get("name") or ""
        args = params.get("arguments") or {}
        if _spawned_task_worker_mode() and name not in {
            "task_exec_status", "task_exec_commit", "task_exec_deliver",
        }:
            return {
                "jsonrpc": "2.0",
                "id": rid,
                "error": {
                    "code": -32601,
                    "message": f"Tool unavailable in spawned task-worker mode: {name}",
                },
            }
        fn = DISPATCH.get(name)
        if not fn:
            return {
                "jsonrpc": "2.0",
                "id": rid,
                "error": {"code": -32601, "message": f"Unknown tool: {name}"},
            }
        if name in _GUARDED_ORCHESTRATION_TOOLS:
            try:
                _require_fresh_bridge(name)
            except BridgeStaleError as exc:
                return _bridge_stale_result(rid, name, exc)
        # Cancelled BEFORE dispatch: never execute — this is the only safe
        # moment to drop a mutation, and a cancelled request gets no reply.
        if rid is not None and _consume_cancellation(rid):
            return None
        # Cancellation AFTER dispatch is tool-dependent: the read-only wait
        # suppresses its response (nothing happened), but a mutation that
        # already ran MUST keep its terminal receipt — silently dropping it
        # would invite a duplicating retry.
        suppress_response_on_cancel = name == "disc_wait_for_peer"
        this_call_sequence = None
        was_cancelled = False
        try:
            global _CURRENT_RPC_SEQUENCE, _RPC_SEQUENCE
            previous_rpc_sequence = _CURRENT_RPC_SEQUENCE
            _RPC_SEQUENCE += 1
            this_call_sequence = _RPC_SEQUENCE
            _CURRENT_RPC_SEQUENCE = this_call_sequence
            _ack_pending_read_cursors(_CURRENT_RPC_SEQUENCE)
            # KT-189 — expose this call's id + progressToken to the
            # bridge-side wait loop (cancellation + keep-alive).
            meta = params.get("_meta") or {}
            _CURRENT_PROGRESS_TOKEN["rid"] = rid
            _CURRENT_PROGRESS_TOKEN["token"] = (
                meta.get("progressToken") if isinstance(meta, dict) else None
            )
            try:
                data = fn(args)
            finally:
                _CURRENT_RPC_SEQUENCE = previous_rpc_sequence
                _CURRENT_PROGRESS_TOKEN["rid"] = None
                _CURRENT_PROGRESS_TOKEN["token"] = None
                was_cancelled = rid is not None and _consume_cancellation(rid)
                if was_cancelled:
                    # Whatever this call staged never reached the model:
                    # un-stage it so the next call cannot silently ack it.
                    _discard_pending_read_cursors(this_call_sequence)
            if was_cancelled and suppress_response_on_cancel:
                return None
            # KT-374 — every tool result passes through here, so this is the one
            # place where the room can reach an agent that never thought to ask
            # for it. Attached only to a dict result: wrapping a list or a
            # string would change a shape callers already parse, and a silent
            # room adds no key at all.
            if isinstance(data, dict) and "kronn_room" not in data:
                room = _room_peek_for_tool_result(name)
                if room:
                    data["kronn_room"] = room
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
        except BridgeStaleError as e:
            if was_cancelled and suppress_response_on_cancel:
                return None
            return _bridge_stale_result(rid, name, e)
        except Exception as e:
            # The inner finally already consumed the cancellation and purged
            # this call's staged cursors; only the response decision remains.
            if was_cancelled and suppress_response_on_cancel:
                return None
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
    _close_inherited_reload_artifact()
    for request in _restore_reload_handoff():
        _REQUEST_QUEUE.put(request)
    reader = threading.Thread(target=_stdin_reader, daemon=True, name="stdin-reader")
    reader.start()
    if os.environ.pop(_BRIDGE_RELOAD_READY_ENV, None) == "1":
        # Emitted by the NEW process, not the stale one. This is a readiness
        # barrier: an eager host cannot race a tools/list request into the old
        # reader thread and lose it when exec replaces that process image.
        _send({
            "jsonrpc": "2.0",
            "method": "notifications/tools/list_changed",
        })
    while True:
        req = _REQUEST_QUEUE.get()
        if req is None:  # EOF — client closed our stdin
            return
        resp = _handle(req)
        if resp is not None:
            _send(resp)
        if _BRIDGE_RELOAD_STATE["status"] == "scheduled":
            _perform_scheduled_bridge_reload()


if __name__ == "__main__":
    if os.environ.pop(_BRIDGE_PREFLIGHT_ENV, None) != "1":
        main()
