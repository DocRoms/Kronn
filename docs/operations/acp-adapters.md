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
[src: file: backend/src/acp.rs:197-242#production_route]

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
[src: file: backend/src/agents/runner.rs:4889-4917#adapter_task_worker_args]
[src: file: backend/src/agents/runner.rs:3751-3769#task_worker_route_policy]
[src: file: backend/src/agents/runner.rs:4348#acp_resume_id]
[src: file: backend/src/agents/runner.rs:13913-13923#CLAUDE_CODE_BUBBLEWRAP]
[src: file: backend/tests/adapter_worker_policy.rs:335-446#worker]

Each adapter permits one active prompt at a time; sequential turns still resume
the session. The prompt owns a guard across its stdin writes, streaming and
process wait. Cancellation during startup remains effective when the child is
registered; abandoning the prompt releases its owned process group. Explicit
cancellation reaps the child. Cleanup failures are reported through the same
redacted runner diagnostic as prompt failures, never as a clean success.
[src: file: backend/src/acp/adapter_process.rs:171-232#AdapterTurn]
[src: file: backend/src/acp/adapter_process.rs:235-254#install]
[src: file: backend/src/acp/adapter_process.rs:292-302#cancel]
[src: file: backend/src/agents/runner.rs:5799-5815#acp_failure_diagnostic]
[src: file: backend/src/agents/runner.rs:5863-5873#acp_failure_diagnostic]

## Packaged desktop bridge

The ACP host and adapters are Rust code shared with the desktop backend. Agent
tools still use the separate `kronn-internal` MCP bridge. Desktop installers
include `kronn-mcp`, a frozen copy of the shared Python bridge and its runtime;
users do not need a separate Python installation for this bridge.
The desktop resolves it through Tauri's resource directory and sets
`KRONN_INTERNAL_MCP_EXECUTABLE` before starting the backend. An explicitly
configured but missing executable fails closed instead of using a developer's
source checkout. Docker and source development keep the existing script route.
[src: file: desktop/src-tauri/src/main.rs:1131-1148#KRONN_INTERNAL_MCP_EXECUTABLE]
[src: file: backend/src/agents/runner.rs:6191-6210#resolve_internal_mcp_command]

The embedded backend exports its actual loopback port as `KRONN_BACKEND_URL`
before starting agent dispatch and MCP configuration sync. Generated desktop
MCP configurations also carry this URL for host CLIs launched outside Kronn.
This overrides an inherited URL belonging to another installation. Turning off
the ACP adapter does not repair a missing bridge or an incorrect backend URL.
[src: file: desktop/src-tauri/src/main.rs:617-623#KRONN_BACKEND_URL]

Claude task workers require a sandbox. Native Windows launches are rejected
with a specific diagnostic; this does not disable ordinary Claude discussions
or relax worker isolation. Workers run through WSL instead; the boundary
(`WSLENV`, translated paths, backend URL in NAT mode, native ACP refusal) is in
[`windows-wsl-agents.md`](windows-wsl-agents.md).
[src: file: backend/src/agents/runner.rs:12474-12482#claude_task_worker_platform_check]
[Claude sandbox platform support](https://code.claude.com/docs/en/sandboxing)

After replacing a frozen bridge, reconnect the MCP session. Frozen and Windows
runtimes report that a restart is required instead of attempting the Unix
file-descriptor script reload used during source development.
[src: file: backend/scripts/disc-introspection-mcp.py:11426-11436#_schedule_bridge_reload]

The desktop sidecar build runs an MCP smoke test from a relocated directory
with spaces and non-ASCII characters, an empty PATH and an ephemeral HTTP
backend. It verifies initialization, a fresh source fingerprint and a real tool
call to that backend. This test does not authenticate Claude, open the app UI,
or prove that an installer passes Gatekeeper; those require installed-app
qualification on each target OS.
[src: file: backend/sidecars/mcp/smoke_bundle.py:15-81#smoke_bundle]

## Observability

Native ACP updates distinguish `user_message_chunk` (including Vibe's echo
of the injected prompt) from `agent_message_chunk`. Only the latter contributes
to the displayed reply. Thought, tool-content and unknown labelled updates do
not become answer text; tool and usage events still follow their own paths.
Older runtimes with unlabelled content retain their compatibility path.
An echo without an agent message therefore supplies no answer text.
[src: file: backend/src/acp.rs:1453-1460#is_answer]
[ACP session updates](https://agentclientprotocol.com/protocol/v1/prompt-turn#session-updates)

Native tool banners retain the reported tool name, a bounded redacted input
excerpt, and the last observed status. ACP updates correlate by `toolCallId`,
Codex exec items by `id`, and Claude stream snapshots by `tool_use_id`. Repeated
calls with the same tool name remain separate. Completion is recorded only
when the runtime reports it; missing metadata remains `unknown` or
`arguments not reported`. Command output, MCP results and file patch bodies
are not copied into these records. The one thing read out of them is a list of
URLs (KT-1308, at most 256 per call and 24 KiB): the pages a call opened (read from
its whole input, before the excerpt cuts it) and the pages a Context7 result named,
which an audit holds its `url:` citations against, and a `result_seen` flag (a result
that reached Kronn, possibly naming no page). The URLs are stored in one canonical
safe form (no scheme, no userinfo, credential-style query values masked as `name=***`
after percent-decoding the name); a `curl` / `wget` URL is recorded only when it is a destination
argument of a real, fully understood invocation, never for text that merely mentions one
(a comment, a heredoc, an option value such as `--proxy <url>`, a `--version` / `--help` call, a
wrapper option value such as `xargs -I curl`). Input masking covers sensitive JSON keys
and the shared credential patterns before truncation; it is heuristic, not a
guarantee that arbitrary user content is secret-free.
[src: file: backend/src/agents/tool_trace.rs:10-24#MARKER]
[src: file: backend/src/agents/tool_trace.rs:52-109#safe_excerpt]
[src: file: backend/src/agents/tool_trace.rs:163-196#from_acp]
[src: file: backend/src/agents/tool_trace.rs:198-262#from_codex]
[src: file: backend/src/agents/tool_trace.rs:264-339#from_claude_line]
[src: file: backend/src/agents/tool_trace.rs:341-380#collect]
[src: file: backend/src/api/discussions/streaming.rs:807-855#lift_acp_tool_calls]

Native ACP sessions receive Kronn's bridge even when no project is attached.
The runtime reconstructs the reserved `kronn-internal` entry from its own
bridge resolver; a project declaration cannot replace or duplicate it. This
entry bypasses the project registry filter, while other MCP servers remain
subject to that filter. A permission request for a runtime-registered Kronn
tool may read or write with `full_access=false`, but must still identify the
bound protocol session and an authorized server/tool. Missing or ambiguous
identity remains refused; registration does not grant filesystem access.
[src: file: backend/src/acp.rs:1531-1570#native_session_mcp_servers]
[src: file: backend/src/acp/permission_broker.rs:577-585#trusted_internal_call]

A native agent's `disc_append` can carry its runtime session id without having
joined the room as a CLI peer. Such a post does not implicitly queue another
answer from the discussion's principal. An actual joined CLI may still hand
the turn back, and explicit recipients keep their existing routing.
[src: file: backend/src/api/disc_source.rs:788-800#route_joined_peer_turn]

Claude's SDK model catalogue is discovered independently of these execution
toggles. It uses an initialization-only, no-prompt CLI process, not the ACP
adapter's empty configuration options. See [catalogue discovery and selector
freshness](../gotchas/claude-catalogue-discovery.md).

When the adapter route is taken, the backend logs an `info`-level line
(`"Starting shared ACP adapter session…"`) naming the agent and worker mode;
the direct compatibility override is also logged. The actual route is visible
in the logs without inspecting code.
[src: file: backend/src/agents/runner.rs:4335-4339#acp_route]
[src: file: backend/src/agents/runner.rs:4392-4398#AdaptedAcp]

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
[src: file: backend/src/db/acp_runtime_sessions.rs:7-13#SessionKey]
[src: file: backend/src/agents/runner.rs:2745-2761#session_key]
[src: file: backend/src/agents/runner.rs:5494-5503#persist]
[src: file: backend/src/acp/codex_adapter.rs:561-565#NativeSessionId]

## Security model

- **Permissions:** deny-by-default. A live `session/request_permission`
  request (native ACP agents only) is auto-approved without `full_access`
  only for conservative, non-mutating tool-call kinds (`read`, `search`,
  `think`, `fetch`); everything else is refused. Kronn's current Claude/Codex
  adapters do not implement live permission callbacks. They apply the broker's
  static session policy through CLI flags: Claude's permission bypass when
  `full_access` is set; Codex's sandbox override on a non-worker turn.
  Resumes and workers retain their separately scoped launch rules. This is an
  implementation limit, not a claim that no vendor interface supports callbacks.
  [src: file: backend/src/acp/permission_broker.rs:491-505#decide_tool_call_permission]
  [src: file: backend/src/acp/permission_broker.rs:731-758#session_policy]
  [src: file: backend/src/acp/claude_adapter.rs:335-337#claude_skip_permissions]
  [src: file: backend/src/acp/codex_adapter.rs:502-513#codex_sandbox]
  A scoped live request must also match the bound ACP protocol session and
  identify either an authorized MCP server/tool or at least one path wholly
  contained by the canonical project root. Missing/malformed locations and
  symlink escapes are denied, including under `full_access`. A read-like call
  that names a real secret file (`.env`, `.env.local`, a key) is denied under
  `full_access` too, while a versioned template (`.env.dist`, `.env.example`,
  `.env.sample`, `.env.template`) is readable. A refusal is returned as the
  agent's own reject option, an answer to the tool call. OpenCode asks about a
  `read` without the path, so Kronn starts it with an inline configuration
  (`OPENCODE_CONFIG_CONTENT`) that denies the real environment files, allows the
  templates and keeps a refusal from ending the turn. See
  [auditing with an ACP agent](../architecture/audit-acp-agents.md).
  [src: file: backend/src/acp/secret_files.rs:51-86#is_secret_file] [src: file: backend/src/acp/secret_files.rs:205-227#opencode_config_content]
- **Filesystem / terminal:** Kronn has not bound a scoped executor for
  `fs/*`/`terminal/*` yet. Every such request — from any agent — gets a
  spec-correct JSON-RPC error (`-32001` "capability not granted"), never a
  fabricated "result" object and never a silent grant.
- **MCP / secrets:** every production broker is scoped to one project and
  reconstructs the canonical `.mcp.json` declaration itself. A matching
  server name never authorizes caller-supplied replacement commands or
  arguments. Entries carrying credential-like arguments are dropped wholesale,
  and so are entries carrying literal `env` values for every adapter except
  Claude. Native ACP agents receive only the remaining command declarations. Codex receives a complete `mcp_servers={...}` override, so
  its global multi-project configuration cannot bleed into this discussion;
  the trusted `kronn-internal` bridge forwards only a fixed list of env-var
  names. Claude freezes a safe inline snapshot of the project entries the broker
  authorized, each kept or left out on its own (KT-1003), and carries an
  entry's `env` values as `${KRONN_MCP_…}` references resolved from its own
  process environment. An absent or malformed registry contributes no project
  servers. Kronn adds its own trusted internal bridge, so a project-less
  discussion can still use room tools without falling back to the account's
  global MCP servers. The CLI cannot reload a replacement file after
  authorization. Worker registries
  remain separately narrowed to the internal bridge. Prompts are written on
  stdin for both adapters, never argv. Secret
  values therefore enter neither adapter argv, ACP payloads, events, nor
  audit records.
- **Vibe's bridge environment (KT-1082):** Vibe starts stdio MCP servers
  without its own environment, only with the `env` declared in `session/new`.
  Vibe saves that declaration in its session metadata, and it can log ACP
  messages (`VIBE_ACP_LOGGING_ENABLED`). So Kronn declares only non-secret
  values for its bridge: `KRONN_DISCUSSION_ID`, `KRONN_BACKEND_URL`, and the
  room/worker contexts. `KRONN_BRIDGE_TOKEN` and `KRONN_WORKFLOW_STEP_CONTEXT`
  go in a per-launch file instead, mode 0600 in a 0700 directory, which the
  declaration names through `KRONN_BRIDGE_SECRET_ENV_FILE`. The bridge loads
  just those two variables, and only from an owner-only regular file. The file
  stays as long as the session's transport, because Vibe restarts the bridge
  for each tool call, and is deleted when the transport is dropped.
  [src: file: backend/src/acp/bridge_env.rs:19-32#PLAIN] [src: file: backend/src/acp/bridge_env.rs:39-83#SecretEnvFile] [src: file: backend/src/acp/bridge_env.rs:94-130#for_launch] [src: file: backend/scripts/disc-introspection-mcp.py:82-118#_load_bridge_secret_env]
- **Vibe's tool identity (KT-1158):** Vibe's `session/request_permission`
  carries only `toolCallId`: no kind, server or arguments. The `tool_call`
  update it sends first names the tool it resolved in `_meta.tool_name`
  (`mcp_<server normalized>.<tool>`, a digest suffix when two servers
  normalize alike). For Vibe sessions only, the broker remembers that name per
  call id (same protocol session, 256 ids, a conflicting name voids the id).
  A name in the exact group of the registered bridge identifies a
  kronn-internal call, then checked against the step's tool list. Any other
  name stays unidentified and is governed as before. Every refusal carries
  `_meta.kronn.refusal` (what, why, what the user can change). Vibe 2.26 does
  not read it: its model sees "User rejected the tool call".
  [src: file: backend/src/acp/permission_broker.rs:392-436#observe_session_update] [src: file: backend/src/acp/permission_broker.rs:449-466#bridge_tool_from_runtime_name] [src: file: backend/src/acp/permission_broker.rs:509-518#permission_response]

## Known limitations

- **Vibe secrets file, partly qualified.** The 0600/0700 POSIX permissions are
  tested. On Windows Kronn relies on the per-user temp directory and does not
  check its ACLs. A real `vibe-acp` was shown to pass declared values, not its
  own environment, to an MCP server. No end-to-end run of Kronn with Vibe has
  been done yet.
- **No live permission negotiation in the current adapters.** Kronn applies
  a static launch policy instead of consulting its broker for each tool call.
  The native ACP permission-request path remains separate.
  [src: file: backend/src/acp/permission_broker.rs:731-758#session_policy]
- **No automatic prompt replay.** Adapter failures do not retry the submitted
  prompt through the direct runner. Switching the compatibility override is
  an operator action, not an error-recovery guess.
- **Credential-bearing project MCP entries are omitted.** Only Claude carries
  an entry's `env` values, as references resolved from its own process
  environment; Codex and the native ACP agents have no secure credential
  injection without putting values in adapter argv/payloads yet, so they omit
  such an entry. A credential in any entry's arguments omits that entry. The
  omission is per entry: every other authorized entry of the project stays.
  [src: file: backend/src/agents/runner.rs:5944-6017#acp_project_mcp_servers]
  [src: file: backend/src/acp/claude_adapter.rs:123-247#initialize]
  [src: file: backend/src/acp/codex_adapter.rs:139-245#codex_project_mcp_override]
- **Kronn's normalized ACP usage event retains token counts, not cost.** This
  is a limit of Kronn's current event mapping, not a claim that the protocol
  prohibits cost metadata. The separate global spend report reads local
  Claude, Codex and Gemini logs; it does not derive spend from the ACP event.
  A missing cost remains unknown, never free. See the
  [compatibility matrix](agents-v2-matrix.md#known-asymmetries) for the
  distinction between ACP normalization, token statistics and log collection.

- **`codex exec resume` rejects `--sandbox`** — verified absent from
  `codex exec resume --help` though present on `codex exec` — and does not keep
  the first turn's mode, so a resumed Codex adapter turn passes the sandbox as
  a `sandbox_mode` config override instead.
  [src: file: backend/src/acp/codex_adapter.rs:502-513#sandbox_mode]

## Declared tools of a workflow Agent step (KT-908)

`agent_settings.tools` (`{cli: [...], kronn_internal: [...]}`) replaces what a
workflow Agent step is launched with. Undeclared, the argv is unchanged.
[src: file: backend/src/models/workflows.rs:1601-1612#StepTools]

- Claude adapter: `--tools <cli list>` (empty string = no built-in tool),
  `--disable-slash-commands` (no skill listing), and an MCP config holding
  only `kronn-internal` started with `--step-tools=<list>`, with one
  `--allowedTools mcp__kronn-internal__<tool>` per tool and
  `ENABLE_TOOL_SEARCH=false` in the CLI's environment; no Kronn tool means
  `{"mcpServers":{}}`. [src: file: backend/src/acp/claude_adapter.rs:174-206#bridge_declared] [src: file: backend/src/acp/claude_adapter.rs:343-352#ENABLE_TOOL_SEARCH] [src: file: backend/src/acp/claude_adapter.rs:220-233#allowed_tools]
- Codex adapter: the MCP override holds only `kronn-internal` with
  `enabled_tools`, or `mcp_servers={}`; `cli` is refused at save.
  [src: file: backend/src/acp/codex_adapter.rs:139-168#codex_project_mcp_override]
  [src: file: backend/src/models/workflows.rs:1613-1634#validate]
- Native ACP agents: `session/new` offers only the narrowed bridge, or no
  server. [src: file: backend/src/agents/runner.rs:5921-5942#declared_mcp_servers]
- The bridge, given `--step-tools=`, lists and dispatches only those tools
  and sends no catalogue instructions on `initialize` (only the decision-card bullet when `disc_append` is declared).
  [src: file: backend/scripts/disc-introspection-mcp.py:5893-5908#_declared_step_tools]
  [src: file: backend/scripts/disc-introspection-mcp.py:12027-12042#_declared_step_tools]
  [src: file: backend/scripts/disc-introspection-mcp.py:12130-12139#_declared_step_tools]
- The direct CLI fallback (`KRONN_ACP_ADAPTER_*=0`) refuses a declared step
  rather than launch it with every tool.

Not measured yet: the context size at the first call of a `tools: none`
step, and whether `--disable-slash-commands` still adds tokens when combined
with an empty MCP config (it did alone, KT-908). `ENABLE_TOOL_SEARCH` is
Claude Code's documented switch for deferred MCP tools; check the transcript
of a declared step shows no `ToolSearch` before relying on it.

## Troubleshooting

- **Codex exits immediately for a project but works in a project-less room:**
  check the project MCP override for a duplicate `kronn-internal` key. Project
  sync may already declare the bridge in `.mcp.json`. The adapter must exclude
  that reserved entry before adding its trusted bridge declaration exactly
  once, or Codex refuses the invalid TOML during bootstrap (exit 1). Other
  authorized project servers remain in the override; no global configuration
  change or MCP permission bypass is needed.
  [src: file: backend/src/acp/codex_adapter.rs:139-245#codex_project_mcp_override]

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
