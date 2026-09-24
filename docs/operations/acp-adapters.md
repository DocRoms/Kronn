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

## Packaged desktop bridge

The ACP host and adapters are Rust code shared with the desktop backend. Agent
tools still use the separate `kronn-internal` MCP bridge. Desktop installers
include `kronn-mcp`, a frozen copy of the shared Python bridge and its runtime;
users do not need a separate Python installation for this bridge.
The desktop resolves it through Tauri's resource directory and sets
`KRONN_INTERNAL_MCP_EXECUTABLE` before starting the backend. An explicitly
configured but missing executable fails closed instead of using a developer's
source checkout. Docker and source development keep the existing script route.
[src: file: desktop/src-tauri/src/main.rs:898]
[src: file: backend/src/agents/runner.rs:4498]

The embedded backend exports its actual loopback port as `KRONN_BACKEND_URL`
before starting agent dispatch and MCP configuration sync. Generated desktop
MCP configurations also carry this URL for host CLIs launched outside Kronn.
This overrides an inherited URL belonging to another installation. Turning off
the ACP adapter does not repair a missing bridge or an incorrect backend URL.
[src: file: desktop/src-tauri/src/main.rs:559]

Claude task workers require a sandbox. Native Windows launches are rejected
with a specific diagnostic; this does not disable ordinary Claude discussions
or relax worker isolation. WSL routing remains separate and must be qualified
with a working Linux agent and bridge configuration.
[src: file: backend/src/agents/runner.rs:9382]
[Claude sandbox platform support](https://code.claude.com/docs/en/sandboxing)

After replacing a frozen bridge, reconnect the MCP session. Frozen and Windows
runtimes report that a restart is required instead of attempting the Unix
file-descriptor script reload used during source development.
[src: file: backend/scripts/disc-introspection-mcp.py:10384]

The desktop sidecar build runs an MCP smoke test from a relocated directory
with spaces and non-ASCII characters, an empty PATH and an ephemeral HTTP
backend. It verifies initialization, a fresh source fingerprint and a real tool
call to that backend. This test does not authenticate Claude, open the app UI,
or prove that an installer passes Gatekeeper; those require installed-app
qualification on each target OS.
[src: file: backend/sidecars/mcp/smoke_bundle.py:15]

## Observability

Native ACP updates distinguish `user_message_chunk` (including Vibe's echo
of the injected prompt) from `agent_message_chunk`. Only the latter contributes
to the displayed reply. Thought, tool-content and unknown labelled updates do
not become answer text; tool and usage events still follow their own paths.
Older runtimes with unlabelled content retain their compatibility path.
An echo without an agent message therefore supplies no answer text.
[src: file: backend/src/acp.rs:940]
[ACP session updates](https://agentclientprotocol.com/protocol/v1/prompt-turn#session-updates)

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
  `think`, `fetch`); everything else is refused. Kronn's current Claude/Codex
  adapters do not implement live permission callbacks. They apply the broker's
  static session policy through CLI flags: Claude's permission bypass when
  `full_access` is set; Codex's sandbox override on a fresh non-worker thread.
  Resumes and workers retain their separately scoped launch rules. This is an
  implementation limit, not a claim that no vendor interface supports callbacks.
  [src: file: backend/src/acp/permission_broker.rs:449-469]
  [src: file: backend/src/acp/claude_adapter.rs:282-284]
  [src: file: backend/src/acp/codex_adapter.rs:389-396]
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

- **No live permission negotiation in the current adapters.** Kronn applies
  a static launch policy instead of consulting its broker for each tool call.
  The native ACP permission-request path remains separate.
  [src: file: backend/src/acp/permission_broker.rs:449-469]
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
