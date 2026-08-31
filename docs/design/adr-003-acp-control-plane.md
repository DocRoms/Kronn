# ADR-003 — ACP control-plane boundary

- **Status:** Accepted for the KT-368 foundation.
- **Date:** 2026-08-30.
- **Scope:** ACP transport ownership, runtime identity, and the boundary with MCP and HTTP model providers.

## Decision

The ACP foundation makes Kronn the future ACP client/host. ACP is reserved for the control-plane connection between Kronn and a compatible coding-agent runtime. The shared host contract negotiates the protocol version and concrete runtime capabilities before it creates or resumes a session. It retains an opaque, non-empty session target; it has no `Custom` fallback. [src: file: backend/src/acp.rs:75-89] [src: file: backend/src/acp.rs:147-214]

MCP remains the agent-to-tool protocol. A scoped MCP server list belongs to `session/new`, so it is bound to the workspace/session rather than the process. Kronn derives that list from the project's canonical `.mcp.json`, but injects only command-only server declarations: any entry carrying an `env` map is dropped so credentials never cross into the ACP payload, and no credential field exists in Kronn's ACP request model. The runner advertises no file or terminal client capabilities until Kronn can bind them to its scoped executor; incoming callbacks are denied explicitly rather than silently granted. [src: file: backend/src/agents/runner.rs:3295] [src: file: backend/src/acp.rs:561] [src: file: backend/src/acp.rs:413]

OpenAI-compatible HTTP remains a separate runtime-to-model-provider transport in this design. It does not become an ACP runtime or an MCP server merely because it can stream model output. [src: inferred: boundary required by KT-368 objective]

## Runtime policy

OpenCode, Gemini CLI, GitHub Copilot CLI, Kiro, and Vibe run over the same native ACP transport through their vendor-documented subprocess command: `opencode acp`, `gemini --acp`, `copilot --acp`, `kiro-cli acp`, and `vibe-acp`. Those commands are the single source of truth, kept pure so they are unit-tested without spawning a process; an agent with no verified command returns `None` and stays on the observable direct-CLI migration route rather than guessing a flag. [src: file: backend/src/acp.rs:68] [src: file: backend/src/acp.rs:252]

Codex and Claude Code remain explicit `DirectCliMigration` routes until an evaluated adapter exists; HTTP model providers remain a distinct route. This prevents an unwired adapter from being presented as active ACP and never maps an unknown identity to `Custom`. [src: file: backend/src/acp.rs:50]

The product defaults only identify the candidate transport. Once connected, the runtime's ACP initialize response is authoritative. ACP v1 baseline methods — `session/new`, `session/prompt`, the `session/cancel` notification, and stdio MCP servers — are always available on a conformant agent and are never gated behind an optional-capability sub-object; only session loading (`loadSession`) and scoped permission negotiation are treated as advertised `initialize` extras. A model/mode catalogue is deliberately not derived from `initialize`: per the ACP session-config-options contract it is returned in the `session/new`/`session/load` *response*, so a fabricated `modelCapabilities`/`models` object at initialize is never trusted. [src: file: backend/src/acp.rs:526] [src: file: backend/src/acp.rs:196]

## Consequences

The backend ACP host exposes one trait for initialize, session create/resume, prompt streaming, cancellation, and shutdown. It rejects capabilities that were not negotiated and rejects protocol versions newer than the host supports. Runtime-specific process management and wire adaptation stay behind that trait. [src: file: backend/src/acp.rs:130-145] [src: file: backend/src/acp.rs:162-214]

The implementation starts every native ACP session over stdin/stdout ND-JSON JSON-RPC with the process working directory set to the worktree; `session/new` carries the same `cwd`. ACP framing, request correlation, incoming client requests and notifications are decoded in the ACP transport, never through the Claude stream-json parser. The runner routes any agent whose production route is `NativeAcp` through `start_native_acp`, which creates an `AcpHost`, negotiates capabilities, creates the session, then applies the resolved tier/model by matching it against the options that session actually returned and calling `session/set_config_option` — a deliberate no-op when no matching option exists, so a catalogue-less agent keeps its default — and forwards normalized events into the existing agent stream lifecycle. `session/resume` restates the workspace scope (`cwd` + `mcpServers`), not only the opaque session id. [src: file: backend/src/agents/runner.rs:3141] [src: file: backend/src/agents/runner.rs:3187] [src: file: backend/src/acp.rs:859] [src: file: backend/src/acp.rs:880]

OpenCode is persisted independently from `Custom`, including its per-agent access and model-tier settings; the frontend has a canonical `@opencode` mention and label. [src: file: backend/src/models/setup.rs:425-445] [src: file: backend/src/models/setup.rs:716-737] [src: file: frontend/src/lib/constants.ts:11-57]

## Delegation delivery-summary contract

A delegated worker never holds a technical conversation in the discussion while it works. After the orchestrator has privately accepted its structured result, exactly one concise report is published, and its structure is owned by Kronn, not the model. The worker supplies only semantic fields (`DeliverySummaryInput`); Kronn stamps `status = accepted`, the schema version, the task reference, the execution id and the timestamp, then validates the payload before anything is published. A missing required field, an unjustified commit absence, a validation without evidence, an over-long summary or a non-RFC-3339 timestamp is refused rather than silently accepted. The canonical JSON is retained for audit and the Markdown is rendered by Kronn in a single fixed section order (summary → changes → commit → validations → documentation → attention points → metrics), so two equal deliveries render byte-for-byte identically. This report is deliberately distinct from an orchestrator/human `important` steering card. [src: file: backend/src/delivery.rs:80] [src: file: backend/src/delivery.rs:153] [src: file: backend/src/delivery.rs:183] [src: file: backend/src/delivery.rs:265]

## Sources

- ACP defines the client-to-agent interface and supports local subprocess communication over JSON-RPC; remote transport standardization remains in progress. [src: url: https://agentclientprotocol.com/get-started/introduction]
- ACP initialization exchanges a protocol version, and the official Rust runtime crate is the intended higher-level integration entry point. [src: url: https://github.com/agentclientprotocol/agent-client-protocol/blob/main/README.md]
- The ACP v1 protocol flow is initialize → session/new or session/load → session/prompt, with session/update notifications and session/cancel as a notification; baseline session, prompt, cancellation and stdio MCP support are not gated behind optional capability flags. [src: url: https://agentclientprotocol.com/protocol/v1/initialization] [src: url: https://agentclientprotocol.com/protocol/v1/prompt-turn]
- Session config options (models/modes) are returned in the `session/new`/`session/load` response and selected by the client via `session/set_config_option`; a resumed session restates `cwd` and `mcpServers`. [src: url: https://agentclientprotocol.com/rfds/session-config-options] [src: url: https://agentclientprotocol.com/rfds/session-resume]
- The existing direct CLI runner starts agents with `AgentStartConfig` and separately constructs MCP context. [src: file: backend/src/agents/runner.rs:2473-2484]
