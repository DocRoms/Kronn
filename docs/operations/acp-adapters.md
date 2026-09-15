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

**ACP adapters are the production default for both agents (KT-652).** Direct
CLI remains an explicit, per-agent compatibility route. This changes transport
selection, not the number of CLI processes: both still spawn once per turn.
[src: file: backend/src/acp.rs:103]

## Explicit compatibility override

```bash
# Codex direct CLI only
KRONN_ACP_ADAPTER_CODEX=0

# Claude Code direct CLI only
KRONN_ACP_ADAPTER_CLAUDE=0
```

Set either (or both) in the backend's environment before starting Kronn.
Each toggle only affects its own agent. Unset means the default adapter;
`1`/`true` explicitly enables it. Other explicit values, including empty or
malformed ones, retain the previous strict false interpretation and select
direct CLI. The agent's identity/model
selectors are unaffected either way (`AgentType::Codex`/`AgentType::ClaudeCode`
stay exactly what they were).

Task workers follow the same transport choice. Their adapter arguments reuse
the direct worker builder: isolated settings, workspace sandbox, restricted
tools and only the internal delivery bridge. Workers always start fresh, even
with a resume hint, and `full_access` cannot override their worker policy.
The common process launcher supplies delivery context and worktree-local
temporary files. It removes inherited worker context from ordinary turns and
the permissive container marker from workers.
[src: file: backend/src/agents/runner.rs:3333]
[src: file: backend/tests/adapter_worker_policy.rs:1]

Each adapter permits one active prompt at a time; sequential turns still resume
the session. The prompt owns a guard across its stdin writes, streaming and
process wait. Cancellation during startup remains effective when the child is
registered; abandoning the prompt releases its owned process group. Explicit
cancellation reaps the child. Cleanup failures are reported through the same
redacted runner diagnostic as prompt failures, never as a clean success.
[src: file: backend/src/acp/adapter_process.rs:1]
[src: file: backend/src/agents/runner.rs:4056]

## Observability

Claude's SDK model catalogue is discovered independently of these execution
toggles. It uses an initialization-only, no-prompt CLI process, not the ACP
adapter's empty configuration options. See [catalogue discovery and selector
freshness](../gotchas/claude-catalogue-discovery.md).

When the adapter route is taken, the backend logs an `info`-level line
(`"Starting shared ACP adapter session…"`) naming the agent and worker mode;
the direct compatibility override is also logged. The actual route is visible
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
  names. Claude freezes a safe inline snapshot only when the entire project
  file matches the broker-authorized commands and arguments. An absent,
  malformed or refused file contributes no project servers. Kronn adds only
  its own trusted internal bridge, so a project-less discussion can still use
  room tools without falling back to the account's global MCP servers. The CLI
  cannot reload a replacement file after authorization. Worker registries
  remain separately narrowed to the internal bridge. Prompts are written on
  stdin for both adapters, never argv. Secret
  values therefore enter neither adapter argv, ACP payloads, events, nor
  audit records.

## Known limitations

- **No live permission negotiation for the adapters.** Permission policy is
  computed once per session, not per tool call, because neither CLI's
  non-interactive mode offers a live callback.
- **No automatic prompt replay.** Adapter failures do not retry the submitted
  prompt through the direct runner. Switching the compatibility override is
  an operator action, not an error-recovery guess.
- **Credential-bearing project MCP entries are omitted.** Secure credential
  injection without putting values in adapter argv/payloads is not implemented
  yet. A project mixing safe and credential-bearing entries is therefore
  denied as a whole by Claude's strict config path; Codex/native ACP retain
  only the independently reconstructed safe entries.
- **Kronn's normalized ACP usage event retains token counts, not cost.** This
  is a limit of Kronn's current event mapping, not a claim that the protocol
  prohibits cost metadata. The separate global spend report reads local
  Claude, Codex and Gemini logs; it does not derive spend from the ACP event.
  A missing cost remains unknown, never free. See the
  [compatibility matrix](agents-v2-matrix.md#known-asymmetries) for the
  distinction between ACP normalization, token statistics and log collection.

- **`codex exec resume` cannot change the sandbox mode** — verified absent
  from `codex exec resume --help` though present on `codex exec` — so a
  resumed Codex adapter session keeps whatever sandbox policy its first turn
  set.

## Troubleshooting

- **"no verified production ACP command" / adapter never engages:** check the
  exact toggle name (`KRONN_ACP_ADAPTER_CODEX` / `KRONN_ACP_ADAPTER_CLAUDE`,
  case-sensitive). Unset enables the adapter; `1`/`true` also enables it,
  ignoring case/outer whitespace. Inspect the backend process's environment,
  not just the shell you're inspecting logs from.
- **A run using the adapter behaves differently from the direct-CLI path
  (e.g. permission prompts, MCP tool availability):** compare against the
  security model above — the adapters intentionally use `--strict-mcp-config`
  and a broker-derived static permission policy, which can be narrower than
  an ad hoc local `claude`/`codex` invocation.
- **Rolling back to direct CLI:** set the relevant toggle to `0`. Unsetting it
  restores the default adapter. The change takes effect on the next
  agent start. The additive `acp_runtime_sessions` table can remain in place;
  direct CLI migration does not read it.
