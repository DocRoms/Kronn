# ADR-003 — ACP control-plane boundary

- **Status:** Accepted for the KT-368 foundation; extended 2026-09-01 (KT-542) with adapters/broker, and 2026-09-14 (KT-652) with default adapters and worker isolation.
- **Date:** 2026-08-30.
- **Scope:** ACP transport ownership, runtime identity, and the boundary with MCP and HTTP model providers.

## Decision

The ACP foundation makes Kronn the future ACP client/host. ACP is reserved for the control-plane connection between Kronn and a compatible coding-agent runtime. The shared host contract negotiates the protocol version and concrete runtime capabilities before it creates or resumes a session. It retains an opaque, non-empty session target; it has no `Custom` fallback. [src: file: backend/src/acp.rs:935-956] [src: file: backend/src/acp.rs:207-221]

MCP remains the agent-to-tool protocol. A scoped MCP server list belongs to `session/new`, so it is bound to the workspace/session rather than the process. Kronn derives that list from the project's canonical `.mcp.json`, injects only command/argument declarations, and drops an entry wholesale when it carries an `env` map or credential-like argument. The permission broker independently reconstructs the same declaration and requires exact server/command/argument equality, so a caller cannot reuse an authorized name with another executable. [src: file: backend/src/agents/runner.rs] [src: file: backend/src/acp/permission_broker.rs] Codex gets a complete project-only `mcp_servers={...}` override plus the trusted `kronn-internal` bridge instead of inheriting its global, multi-project registry. Claude gets `--mcp-config`/`--strict-mcp-config` only when the complete project file equals the authorized safe set; a mixed or credential-bearing file is omitted rather than partially escaping the broker through the CLI. Both adapters pass prompt content on stdin, preserve `KRONN_DISCUSSION_ID` in the child environment, and expose only non-secret correlation fields in events/audit records. [src: file: backend/src/acp/claude_adapter.rs] [src: file: backend/src/acp/codex_adapter.rs]

The runner advertises no file or terminal client capabilities until Kronn can bind them to its scoped executor (`clientCapabilities: {}` at `initialize`). Every incoming agent→client request is routed through one `AcpPermissionBroker`, scoped and audited. `session/request_permission` is deny-by-default: in addition to its operation kind, a production request must match the protocol session bound at `session/new`/`session/load` and identify either an authorized server/tool or non-empty locations contained by the canonical project root. Missing/malformed locations, another session id, an unregistered server/tool, `..`, and symlink escapes all fail closed, including under `full_access`. `fs/*`/`terminal/*` and unknown methods get spec-correct JSON-RPC errors. Audit entries correlate the discussion/session label, protocol session, server, tool and paths without retaining raw input. [src: file: backend/src/acp/permission_broker.rs] [src: file: backend/src/acp.rs]

OpenAI-compatible HTTP remains a separate runtime-to-model-provider transport in this design. It does not become an ACP runtime or an MCP server merely because it can stream model output. [src: inferred: boundary required by KT-368 objective]

## Runtime policy

OpenCode, Gemini CLI, GitHub Copilot CLI, Kiro, and Vibe run over the same native ACP transport through their vendor-documented subprocess command: `opencode acp`, `gemini --acp`, `copilot --acp`, `kiro-cli acp`, and `vibe-acp`. Those commands are the single source of truth, kept pure so they are unit-tested without spawning a process; an agent with no verified command returns `None` and stays on the observable direct-CLI migration route rather than guessing a flag. [src: file: backend/src/acp.rs:135-148]

KT-542 initially delivered opt-in Claude/Codex adapters. The human-approved
KT-652 scope completes the default route in 0.13.0: both use `AdaptedAcp`
behind the shared `AcpHost`, including task workers. An explicit false
`KRONN_ACP_ADAPTER_CODEX` / `KRONN_ACP_ADAPTER_CLAUDE` override selects the
direct compatibility route; unset uses the adapter. Both choices are logged.
An adapter failure never automatically replays a submitted prompt in the
direct runner. Worker arguments reuse the narrow direct worker builder and
the same process launcher: fresh session, isolated settings/MCP/tools,
worktree-local temporary files and per-execution delivery context. A worker's
policy takes precedence over `full_access`. Neither adapter claims vendor
ACP wire support: they continue to drive the existing non-interactive CLI
protocols. HTTP providers remain a distinct route.
[src: user: 2026-09-14: proposal:d2b26053-d297-48e3-8c08-1024053142ec:0]
[src: file: backend/src/acp.rs:103]
[src: file: backend/src/agents/runner.rs:3297]
[src: file: backend/tests/adapter_worker_policy.rs:1]

KT-652 also tightens Claude's MCP boundary described above: the authorized
project registry is frozen as safe inline JSON, never a mutable file path.
Absent/invalid/mixed configurations contribute no project servers; only
Kronn's independently located trusted bridge is retained, including for
project-less discussions. Strict mode never restores the account's global
registry. Duplicate copies of the same authorized declaration do not remove
that bridge. Both adapters share cancellable child ownership, including
cancellation during startup and while awaiting process exit.
[src: file: backend/src/acp/claude_adapter.rs:112]
[src: file: backend/src/acp/adapter_process.rs:1]

### Codex/Claude adapter session lifecycle

Each adapter drives its CLI's own documented, stable, non-interactive flags instead of ACP JSON-RPC — the wire is not ACP, only the Rust-level contract (`initialize`/`create_session`/`resume_session`/`prompt`/`cancel`/`shutdown`) is:

- **Claude** lets the client pin a session id up front: `create_session` allocates a UUID with no subprocess spawn, and the first `prompt` turn passes `--session-id <uuid>`; every later turn (including one immediately preceded by `resume_session`, covering a cross-restart resume) passes `--resume <uuid>` instead [src: file: backend/src/acp/claude_adapter.rs:108-177]. Each turn is its own `claude --print --output-format stream-json --verbose --include-partial-messages …` subprocess, parsed by the existing, already-tested `parse_claude_stream_line`/`StreamJsonEvent` (reused, not forked) [src: file: backend/src/agents/runner.rs:8800] [src: file: backend/src/acp/claude_adapter.rs:195-230].
- **Codex** cannot hand out a session id before a turn runs — `codex exec` has no "create an empty thread" mode — so `create_session` allocates Kronn's own correlation UUID as the opaque `AcpSessionTarget`. The real `thread_id` from `thread.started` is emitted immediately while the turn is streaming and persisted before later output is processed. A fresh adapter is seeded with that id after a backend restart and its first subprocess uses `codex exec resume <thread_id>`. `codex exec resume` does not accept `--sandbox`, so the adapter only applies sandbox policy on a thread's first turn. [src: file: backend/src/acp/codex_adapter.rs] [src: file: backend/src/agents/runner.rs]
- **Durability:** normal discussion starts receive an `AcpSessionStore` backed by the dedicated `acp_runtime_sessions` table. It is intentionally separate from `discussion_sessions`, whose rows represent presence/routing leases rather than CLI conversation state. The key contains discussion id, agent identity, adapter runtime and canonical project scope; switching projects cannot resume an old identifier. Claude's client-selected UUID is persisted before prompting, and Codex's runtime-selected thread is persisted as soon as `thread.started` arrives. [src: file: backend/src/db/acp_runtime_sessions.rs] [src: file: backend/src/db/sql/161_acp_runtime_sessions.sql] [src: file: backend/src/agents/runner.rs]
- Kronn's current adapters do not implement live mid-turn permission callbacks. They apply `AcpPermissionBroker::session_policy` as static launch flags instead of negotiating each tool call through the native ACP request path. Claude applies the permission bypass under `full_access`; Codex adds its sandbox override only for a fresh non-worker thread. This describes the current integration, not an exhaustive claim about vendor callback interfaces. [src: file: backend/src/acp/permission_broker.rs:449-469] [src: file: backend/src/acp/claude_adapter.rs:282-284] [src: file: backend/src/acp/codex_adapter.rs:389-396]
- Model selection bypasses the ACP session-config-options dance entirely for both adapters: `--model` is a direct CLI flag baked in at construction, `config_options()` returns empty, and `start_adapted_acp` passes `model_flag: None` into the shared session-run core so its (native-agent-oriented) `select_model` step is a correctly-skipped no-op rather than a misleading "no matching option" log line [src: file: backend/src/agents/runner.rs:3221-3250].

The product defaults only identify the candidate transport. Once connected, the runtime's ACP initialize response is authoritative. ACP v1 baseline methods — `session/new`, `session/prompt`, the `session/cancel` notification, and stdio MCP servers — are always available on a conformant agent and are never gated behind an optional-capability sub-object; only session loading (`loadSession`) and scoped permission negotiation are treated as advertised `initialize` extras. A model/mode catalogue is deliberately not derived from `initialize`: per the ACP session-config-options contract it is returned in the `session/new`/`session/load` *response*, so a fabricated `modelCapabilities`/`models` object at initialize is never trusted. [src: file: backend/src/acp.rs:622-660] [src: file: backend/src/acp.rs:264-267]

## Consequences

The backend ACP host exposes one trait for initialize, session create/resume, prompt streaming, cancellation, and shutdown. It rejects capabilities that were not negotiated and rejects protocol versions newer than the host supports. Runtime-specific process management and wire adaptation stay behind that trait. [src: file: backend/src/acp.rs:351-391] [src: file: backend/src/acp.rs:935-951] [src: file: backend/src/acp.rs:1014-1028]

The implementation starts every native ACP session over stdin/stdout ND-JSON JSON-RPC with the process working directory set to the worktree; `session/new` carries the same `cwd`. ACP framing, request correlation, incoming client requests and notifications are decoded in the ACP transport, never through the Claude stream-json parser. The runner routes any agent whose resolved route is `NativeAcp` through `start_native_acp`, and `AdaptedAcp` through `start_adapted_acp` (KT-542); both are thin wrappers that construct their transport, then hand off to one shared `run_acp_session` core, which creates an `AcpHost`, negotiates capabilities, creates the session, then applies the resolved tier/model by matching it against the options that session actually returned and calling `session/set_config_option` — a deliberate no-op when no matching option exists, so a catalogue-less agent (or either adapter, which always exposes none) keeps its default — and forwards normalized events into the existing agent stream lifecycle. `session/resume` restates the workspace scope (`cwd` + `mcpServers`), not only the opaque session id, for the native JSON-RPC transport. [src: file: backend/src/agents/runner.rs:3186-3205] [src: file: backend/src/agents/runner.rs:3221-3250] [src: file: backend/src/agents/runner.rs:3257-3412] [src: file: backend/src/acp.rs:834-850]

OpenCode is persisted independently from `Custom`, including its per-agent access and model-tier settings; the frontend has a canonical `@opencode` mention and label. [src: file: backend/src/models/setup.rs:425-445] [src: file: backend/src/models/setup.rs:716-737] [src: file: frontend/src/lib/constants.ts:11-57]

## Delegation delivery-summary contract

A delegated worker never holds a technical conversation in the discussion while it works. After the orchestrator has privately accepted its structured result, exactly one concise report is published, and its structure is owned by Kronn, not the model. The worker supplies only semantic fields (`DeliverySummaryInput`); Kronn stamps `status = accepted`, the schema version, the task reference, the execution id and the timestamp, then validates the payload before anything is published. A missing required field, an unjustified commit absence, a validation without evidence, an over-long summary or a non-RFC-3339 timestamp is refused rather than silently accepted. The canonical JSON is retained for audit and the Markdown is rendered by Kronn in a single fixed section order (summary → changes → commit → validations → documentation → attention points → metrics), so two equal deliveries render byte-for-byte identically. This report is deliberately distinct from an orchestrator/human `important` steering card. [src: file: backend/src/delivery.rs:80] [src: file: backend/src/delivery.rs:153] [src: file: backend/src/delivery.rs:183] [src: file: backend/src/delivery.rs:265]

## Sources

- ACP defines the client-to-agent interface and supports local subprocess communication over JSON-RPC; remote transport standardization remains in progress. [src: url: https://agentclientprotocol.com/get-started/introduction]
- ACP initialization exchanges a protocol version, and the official Rust runtime crate is the intended higher-level integration entry point. [src: url: https://github.com/agentclientprotocol/agent-client-protocol/blob/main/README.md]
- The ACP v1 protocol flow is initialize → session/new or session/load → session/prompt, with session/update notifications and session/cancel as a notification; baseline session, prompt, cancellation and stdio MCP support are not gated behind optional capability flags. [src: url: https://agentclientprotocol.com/protocol/v1/initialization] [src: url: https://agentclientprotocol.com/protocol/v1/prompt-turn]
- Session config options (models/modes) are returned in the `session/new`/`session/load` response and selected by the client via `session/set_config_option`; a resumed session restates `cwd` and `mcpServers`. [src: url: https://agentclientprotocol.com/rfds/session-config-options] [src: url: https://agentclientprotocol.com/rfds/session-resume]
- The existing direct CLI runner starts agents with `AgentStartConfig` and separately constructs MCP context. [src: file: backend/src/agents/runner.rs:2473-2484]
- `session/request_permission` params are `sessionId`/`toolCall`/`options` (each option an `optionId`/`name`/`kind` of `allow_once`/`allow_always`/`reject_once`/`reject_always`); the result is `{"outcome": {"outcome": "selected", "optionId": …} | {"outcome": "cancelled"}}`. `ToolCall.kind` is one of `read`/`edit`/`delete`/`move`/`search`/`execute`/`think`/`fetch`/`other`. `fs/read_text_file` (`{"content": …}`) and `fs/write_text_file` (`null`) require the client to have advertised `clientCapabilities.fs`; `terminal/create`/`output`/`wait_for_exit`/`kill`/`release` are separate methods. [src: url: https://agentclientprotocol.com/protocol/v1/tool-calls] [src: url: https://agentclientprotocol.com/protocol/v1/file-system] [src: url: https://agentclientprotocol.com/protocol/v1/terminals]
- The adapter argument builders, rather than an absence of options in a CLI help listing, define the integration qualified here. Their session, MCP, model/effort and permission arguments are implemented in the two adapters; their fixtures exercise the expected subprocess contract. Vendor interfaces outside those launch paths are not evaluated by this ADR. [src: file: backend/src/acp/claude_adapter.rs] [src: file: backend/src/acp/codex_adapter.rs] [src: file: backend/tests/adapter_worker_policy.rs]
- `codex exec --json` emits JSONL `ThreadEvent`s (`thread.started{thread_id}`, `turn.started`, `turn.completed{usage}`, `turn.failed{error}`, `item.started`/`item.updated`/`item.completed{item}` with `item.type` ∈ `agent_message`/`reasoning`/`command_execution`/`file_change`/`mcp_tool_call`/`collab_tool_call`/`web_search`/`todo_list`/`error`, `error{message}`). [src: url: https://github.com/openai/codex/blob/main/codex-rs/exec/src/exec_events.rs]
