- **ID**: TD-20260510-codex-mcp-sandbox-block
- **Area**: Backend / agents
- **Severity**: Medium (Codex sees the MCP, attempts the call, but the call is killed before the bridge runs — no introspection on Codex despite the wiring landing)
- **Status**: 🟡 Identified — verified empirically 2026-05-10

## Problem (fact)

Phase B+ wired `kronn-internal` into Codex's global config:

```toml
# ~/.codex/config.toml
[mcp_servers.kronn-internal]
args = ["/.../disc-introspection-mcp.py"]
command = "python3"
startup_timeout_sec = 30
```

`codex mcp list` confirms Codex sees the entry as `enabled`. When prompted to call the tool, Codex 0.121's runtime *attempts* the call:

```
codex
Running `disc_meta` now and I'll return only `message_count`.
mcp: kronn-internal/disc_meta started
mcp: kronn-internal/disc_meta (failed)
user cancelled MCP tool call
```

The call dies with `(failed)` and `user cancelled MCP tool call` before the bridge process gets HTTP traffic — `introspection_call_count` stays at 0.

Despite the banner reading `approval: never` and the runner passing `--sandbox=workspace-write` (or `danger-full-access` when the agent has `full_access`), Codex's exec-mode sandbox/approval layer blocks the bridge subprocess spawn.

## Why we can't fix now

The blocker is inside Codex 0.121's exec-mode policy enforcement, not Kronn. Kronn already passes the right approval flag (`approval: never` is shown in Codex's banner). We need either:

1. A Codex flag that explicitly authorises subprocess MCP servers in `exec` mode (the documented `-a never` is supposed to do this, but doesn't in 0.121).
2. A per-MCP `allowed = true` / `auto_approve = true` field in `[mcp_servers.kronn-internal]` (not currently in Codex's TOML schema as far as we can tell).
3. Force `--sandbox=danger-full-access` for *every* Codex spawn that has kronn-internal injected — works but undermines Kronn's existing `full_access` opt-in for Codex.

Each path needs Codex-side investigation and likely a CLI/CLI-config feature request to OpenAI's codex-cli repo. Phase B+ already lands the *configuration* fix (which unblocks Claude Code, Kiro, Gemini, Copilot — see TD-20260510-introspection-vibe for Vibe/Ollama). Closing the Codex gap is bounded extra work but waiting on either an upstream change or a careful sandbox-flag toggle.

## Workaround

Today, Codex users in Kronn don't get the introspection tools. They fall back to the raw transcript in their context window — same behaviour as before Phase B+. Recommend `summary_strategy=Auto` for Codex discussions so the auto-summary at least gives them compressed history past the context budget (UI hint already present from Phase B++ for Vibe/Ollama; we should extend it to Codex when this TD ships its fix).

## Where (pointers)

- `backend/src/agents/runner.rs:846-883` — Codex spawn args; `--sandbox` is passed only when `KRONN_HOST_HOME` is set and based on the `full_access` flag.
- `backend/src/core/mcp_scanner.rs:1085-1175` — `CodexSync` writes `[mcp_servers.kronn-internal]` to `~/.codex/config.toml`. Verified working — config is read.
- `backend/scripts/disc-introspection-mcp.py` — bridge stdio loop. Verified independently working when invoked with proper env.
- Manual repro: `KRONN_DISCUSSION_ID=<any> KRONN_BACKEND_URL=http://127.0.0.1:3140 codex exec --skip-git-repo-check "Call kronn-internal.disc_meta()"` shows the `(failed) user cancelled` pattern even with full creds.

## Suggested direction (non-binding)

Three options, in order of preference:

1. **Track upstream**: file an issue on `openai/codex-cli` describing the failure and ask whether `-a never` is supposed to pass MCP calls. If yes → bug fix lands. If no → ask for a per-MCP auto-approve.
2. **Per-spawn config override**: pass `-c approval_policy.mcp_servers.kronn-internal.auto_approve=true` (or whatever the right key turns out to be) from `runner.rs` when Codex is spawned with the bridge.
3. **Extend `frontend/src/lib/constants.ts::agentSupportsIntrospection`** to add Codex to the unsupported list temporarily, with a UI warning matching the existing Vibe/Ollama note. Lets the user know Codex won't introspect today. Reverse when (1) or (2) lands.

## Next step

Option 3 first (1 h, ships a clear UX warning), in parallel with filing option 1 upstream. Option 2 if the upstream issue stalls > 1 month.
