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
recorded by the shared `AcpPermissionBroker` with a normalized reason and
non-secret correlation fields (discussion/session label, ACP protocol session,
server, tool, and normalized locations), retrievable via
`permission_audit_log()` on the transport/adapter. There is no HTTP endpoint
exposing this log yet; it is available to Rust callers and to tests.

Discussion-bound Codex and Claude adapter sessions persist their native
conversation identifier in `acp_runtime_sessions`, keyed by discussion,
agent, adapter runtime, and project scope. Codex records `thread.started`
immediately while a turn is still streaming; Claude records the UUID Kronn
passes with `--session-id` before the turn. A backend restart therefore
re-seeds the adapter and resumes the same CLI conversation, but a different
project path never reuses that identifier.

## Security model

- **Permissions:** deny-by-default. A live `session/request_permission`
  request (native ACP agents only) is auto-approved without `full_access`
  only for conservative, non-mutating tool-call kinds (`read`, `search`,
  `think`, `fetch`); everything else is refused. Neither Claude's `--print`
  mode nor `codex exec` exposes a live permission callback at all, so the
  adapters compute the same policy once per session and apply it as static
  flags (`--dangerously-skip-permissions` / `--sandbox=danger-full-access`
  under `full_access`, the CLI's own restrictive default otherwise).
  A scoped live request must also match the bound ACP protocol session and
  identify either an authorized MCP server/tool or at least one path wholly
  contained by the canonical project root. Missing/malformed locations and
  symlink escapes are denied, including under `full_access`.
- **Filesystem / terminal:** Kronn has not bound a scoped executor for
  `fs/*`/`terminal/*` yet. Every such request — from any agent — gets a
  spec-correct JSON-RPC error (`-32001` "capability not granted"), never a
  fabricated "result" object and never a silent grant.
- **MCP / secrets:** every production broker is scoped to one project and
  reconstructs the canonical `.mcp.json` declaration itself. A matching
  server name never authorizes caller-supplied replacement commands or
  arguments. Entries carrying `env` values or credential-like arguments are
  dropped wholesale. Native ACP agents receive only the remaining command
  declarations. Codex receives a complete `mcp_servers={...}` override, so
  its global multi-project configuration cannot bleed into this discussion;
  the trusted `kronn-internal` bridge forwards only a fixed list of env-var
  names. Claude loads the project `.mcp.json` with `--strict-mcp-config` only
  when the whole file exactly matches the broker-authorized set; otherwise
  the config is omitted rather than partially authorizing a secret-bearing
  file. Prompts are written on stdin for both adapters, never argv. Secret
  values therefore enter neither adapter argv, ACP payloads, events, nor
  audit records.

## Known limitations

- **No live permission negotiation for the adapters.** Permission policy is
  computed once per session, not per tool call, because neither CLI's
  non-interactive mode offers a live callback.
- **Task workers are excluded** (see above) — they stay on direct CLI
  migration unconditionally.
- **Credential-bearing project MCP entries are omitted.** Secure credential
  injection without putting values in adapter argv/payloads is not implemented
  yet. A project mixing safe and credential-bearing entries is therefore
  denied as a whole by Claude's strict config path; Codex/native ACP retain
  only the independently reconstructed safe entries.
- **`codex exec resume` cannot change the sandbox mode** — verified absent
  from `codex exec resume --help` though present on `codex exec` — so a
  resumed Codex adapter session keeps whatever sandbox policy its first turn
  set.

## Troubleshooting

- **"no verified production ACP command" / adapter never engages:** check the
  exact toggle name (`KRONN_ACP_ADAPTER_CODEX` / `KRONN_ACP_ADAPTER_CLAUDE`,
  case-sensitive; only `1` or `true`, ignoring case/outer whitespace, enables
  it) and that it's set in the backend
  process's environment, not just the shell you're inspecting logs from.
- **A run using the adapter behaves differently from the direct-CLI path
  (e.g. permission prompts, MCP tool availability):** compare against the
  security model above — the adapters intentionally use `--strict-mcp-config`
  and a broker-derived static permission policy, which can be narrower than
  an ad hoc local `claude`/`codex` invocation.
- **Rolling back:** unset the toggle. The change takes effect on the next
  agent start. The additive `acp_runtime_sessions` table can remain in place;
  direct CLI migration does not read it.
