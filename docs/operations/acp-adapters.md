# Codex/Claude ACP adapters (KT-542)

Companion to [`docs/design/adr-003-acp-control-plane.md`](../design/adr-003-acp-control-plane.md),
which is the source of truth for the design rationale. This page is the
operator-facing "how do I turn it on / what changed / how do I debug it" view.

## What this is

Codex and Claude Code have no native ACP subcommand (no `codex acp` /
`claude acp` exists in the CLIs Kronn ships against — verified against
codex-cli 0.151.0 and Claude Code 2.1.207). `ClaudeAcpAdapter` and
`CodexAcpAdapter` (`backend/src/acp/claude_adapter.rs`,
`backend/src/acp/codex_adapter.rs`) make them speak the same
create/resume/stream/cancel/close contract (`AcpTransport`) as the native ACP
agents (OpenCode, Gemini CLI, Copilot CLI, Kiro, Vibe) by driving each CLI's
own documented, stable, non-interactive flags underneath — `claude --print
--output-format stream-json --session-id/--resume …` and `codex exec [--json]`
/ `codex exec resume <thread_id> [--json]` — instead of ACP JSON-RPC.

**Direct CLI migration remains the production default for both agents.** The
adapters are an explicit, per-agent, off-by-default opt-in.

## Enabling it

```bash
# Codex only
KRONN_ACP_ADAPTER_CODEX=1

# Claude Code only
KRONN_ACP_ADAPTER_CLAUDE=1
```

Set either (or both) in the backend's environment before starting Kronn.
Each toggle only affects its own agent — enabling Codex's adapter never
changes Claude's route, and vice versa (`crate::acp::resolve_acp_route`,
`backend/src/acp.rs`). Unset the variable to fall back to direct CLI
migration immediately; no other state changes, and the agent's identity/model
selectors are unaffected either way (`AgentType::Codex`/`AgentType::ClaudeCode`
stay exactly what they were).

Task workers (durable delegated executions with an isolated worktree) always
use direct CLI migration, regardless of the toggle — the adapters do not yet
carry the task-worker-specific sandbox/tool-allowlist policy
(`backend/src/agents/runner.rs`, the `AdaptedAcp if !task_worker` dispatch
guard).

## Observability

When the adapter route is taken, the backend logs an `info`-level line
(`"KRONN_ACP_ADAPTER_* opt-in active: starting an isolated ACP adapter
session…"`) naming the agent, so which transport a given run used is visible
in the logs without inspecting code.

Every permission decision — live, for a native ACP agent's
`session/request_permission`, or the adapters' static pre-session policy — is
recorded by the shared `AcpPermissionBroker` with a normalized reason
(`AcpAuditEntry { method, verdict, reason }`), retrievable via
`permission_audit_log()` on the transport/adapter. There is no HTTP endpoint
exposing this log yet; it is available to Rust callers and to tests.

## Security model

- **Permissions:** deny-by-default. A live `session/request_permission`
  request (native ACP agents only) is auto-approved without `full_access`
  only for conservative, non-mutating tool-call kinds (`read`, `search`,
  `think`, `fetch`); everything else is refused. Neither Claude's `--print`
  mode nor `codex exec` exposes a live permission callback at all, so the
  adapters compute the same policy once per session and apply it as static
  flags (`--dangerously-skip-permissions` / `--sandbox=danger-full-access`
  under `full_access`, the CLI's own restrictive default otherwise).
- **Filesystem / terminal:** Kronn has not bound a scoped executor for
  `fs/*`/`terminal/*` yet. Every such request — from any agent — gets a
  spec-correct JSON-RPC error (`-32001` "capability not granted"), never a
  fabricated "result" object and never a silent grant.
- **MCP / secrets:** ACP sessions only ever see project-authorized MCP
  servers. Native ACP agents get a command-only list inlined into the
  `session/new` payload (any server needing an `env` value is dropped
  wholesale, not partially redacted). The Codex/Claude adapters take a
  stricter path: they never inline `mcpServers` into any Kronn-controlled
  payload at all. Claude's adapter points `--mcp-config`/`--strict-mcp-config`
  at the project's already-synced `.mcp.json` (the same 0600, secret-bearing
  file today's direct-CLI Claude invocation reads); Codex reads its own
  already-synced `~/.codex/config.toml` automatically, and the adapter only
  narrows the `kronn-internal` server's forwarded env var *names* (never
  values). No secret value is read into the adapter's own process, so none
  can leak into a prompt, a session event, or a client payload.

## Known limitations

- **No cross-restart persistence for Codex yet.** Codex only assigns a real
  `thread_id` after a turn runs; the adapter tracks it in memory and exposes
  it via `AcpTransport::native_session_id`, and the constructor accepts a
  seed id to resume a thread known from before a restart — both are unit
  tested — but nothing in `start_agent_with_config`/`AgentStartConfig` yet
  reads or writes that id to `discussion_sessions.conversation_id`
  (`AgentStartConfig` carries no DB handle). A Kronn restart mid-Codex-adapter
  session starts a fresh thread rather than resuming the old one. Claude does
  not have this gap: `--session-id` lets Kronn choose the id up front, so it
  survives a restart as long as the caller already has it.
- **No live permission negotiation for the adapters.** Permission policy is
  computed once per session, not per tool call, because neither CLI's
  non-interactive mode offers a live callback.
- **Task workers are excluded** (see above) — they stay on direct CLI
  migration unconditionally.
- **`codex exec resume` cannot change the sandbox mode** — verified absent
  from `codex exec resume --help` though present on `codex exec` — so a
  resumed Codex adapter session keeps whatever sandbox policy its first turn
  set.

## Troubleshooting

- **"no verified production ACP command" / adapter never engages:** check the
  exact toggle name (`KRONN_ACP_ADAPTER_CODEX` / `KRONN_ACP_ADAPTER_CLAUDE`,
  case-sensitive, any value counts as set) and that it's set in the backend
  process's environment, not just the shell you're inspecting logs from.
- **A run using the adapter behaves differently from the direct-CLI path
  (e.g. permission prompts, MCP tool availability):** compare against the
  security model above — the adapters intentionally use `--strict-mcp-config`
  and a broker-derived static permission policy, which can be narrower than
  an ad hoc local `claude`/`codex` invocation.
- **Rolling back:** unset the toggle. The change takes effect on the next
  agent start; no migration or cleanup step is required.
