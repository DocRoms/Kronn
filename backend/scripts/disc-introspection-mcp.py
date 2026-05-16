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
            "Append messages to an existing Kronn discussion. Idempotent "
            "on (disc_id, source_msg_id) — re-pushing the same exported "
            "transcript does NOT duplicate. Returns `{appended, "
            "skipped_as_duplicates, diverged}`. `diverged=true` means the "
            "Kronn UI was edited after a previous import; the agent "
            "should warn the user before applying more updates."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "disc_id": {"type": "string"},
                "messages": {
                    "type": "array",
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
            "required": ["disc_id", "messages"],
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
]


# ─── HTTP plumbing ─────────────────────────────────────────────────────────

def _backend_url():
    return os.environ.get("KRONN_BACKEND_URL", "http://127.0.0.1:3140").rstrip("/")


def _disc_id():
    did = os.environ.get("KRONN_DISCUSSION_ID")
    if not did:
        raise RuntimeError("KRONN_DISCUSSION_ID env var not set — Kronn auto-injection misconfigured")
    return did


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
    return _unwrap(_http("POST", "/api/disc/create", body))


def call_disc_append(args):
    disc_id = args.get("disc_id")
    messages = args.get("messages")
    if not disc_id:
        raise RuntimeError("disc_append: missing required 'disc_id'")
    if not isinstance(messages, list) or not messages:
        raise RuntimeError("disc_append: 'messages' must be a non-empty array")
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
}


# ─── MCP JSON-RPC loop ─────────────────────────────────────────────────────

def _send(payload):
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def _handle(req):
    method = req.get("method") or ""
    rid = req.get("id")
    if method == "initialize":
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
