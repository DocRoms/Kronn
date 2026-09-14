# ADR-004 — HTTP transport boundary

- **Status:** Accepted for KT-545.
- **Date:** 2026-09-01.
- **Scope:** the OpenAI-compatible HTTP model-provider transport (LiteLLM,
  NVIDIA, named Custom connections), its boundary with ACP, and the stable
  target identity shared across launch surfaces.

## Decision

OpenAI-compatible HTTP is a distinct runtime-to-model-provider transport,
never an ACP runtime and never an MCP server, per ADR-003's boundary
statement. `backend/src/http_transport.rs` is the module that owns this seam
explicitly: `acp::production_route`/`resolve_acp_route` already return
`AcpProductionRoute::HttpModelProvider` for every agent type it covers
(`Ollama`, `LiteLlm`, `Nvidia`, `Custom`), and this module is what those
routes hand off to. [src: file: backend/src/acp.rs:94-106]
[src: file: backend/src/http_transport.rs:1-16]

### Codec selection is explicit

`http_transport::resolve_chat_codec(agent_type)` is the single decision point
for which wire format a chat dispatch speaks — `OllamaCodec` for `Ollama`,
`OpenAiCodec` (the explicit OpenAI Chat codec) for every other HTTP-chat
agent, regardless of which named connection backs it. It replaces an inline
ternary that used to live inside `start_ollama_http`, mirroring
`acp::resolve_acp_route` being the sole ACP route decision point.
[src: file: backend/src/http_transport.rs:28-41]
[src: file: backend/src/agents/runner.rs:5111] (call site)

### Pre-dispatch capability gate

`model_catalog::preflight_check` — already the single gate shared by
discussion dispatch, Quick Prompt runs and workflow steps — now also refuses
a model the catalog positively tags with capabilities that exclude `"chat"`
(for example an image/video-only entry from a connection's live catalogue),
before any request reaches the provider. An entry with no recorded
capabilities (every CLI-native discovery, or a model never covered by a
connection test) is neither confirmed nor denied, consistent with the
existing "unknown is not blocked" policy — only a positive mismatch refuses.
[src: file: backend/src/core/model_catalog/mod.rs] (capability check inside
`preflight_check`) [src: file: backend/src/http_transport.rs:43-51]
(`entry_supports_capability`)

### Presets keep their identity, credential slot, color and concurrency

LiteLLM and NVIDIA are `ExternalApiConnectionPreset` variants of the same
named-connection model Custom connections use (`external_api_connections`,
KT-339), but keep their own dedicated `AgentType` — so their fixed UI color
(`AGENT_COLORS.LiteLlm` / `.Nvidia`) and their per-agent-type concurrency
limit (`agent_concurrency_limits`'s `REMOTE` array) are untouched by this
work. [src: file: backend/src/agents/runner.rs:2355-2407]
[src: file: frontend/src/lib/constants.ts:11-24] Their credential is resolved
through `credential_slug` → `TokensConfig.active_key_for`, the same
indirection every named connection already uses.
[src: file: backend/src/models/setup.rs:346-353]

### Stable target: connection + runtime + model + tier

The durable execution-target namespace stays exactly what KT-531 defined:
`agent:<canonical-slug>` for CLI families, `http:<connection-id>` for named
HTTP connections (`db::model_catalog::agent_runtime_target_id` /
`http_runtime_target_id`). This ADR does not introduce a second identity
scheme; it makes every launch surface *resolve into* that identity the same
way:

- `http_transport::connection_tier_model(connection, tier)` is the single
  rule for "which model does this connection's tier resolve to," including
  the fallback-to-Default-tier-model behavior. It replaced three independent,
  subtly inconsistent copies (discussion dispatch, Quick Prompt, workflow/
  batch-compare retry) — the copies disagreed on whether an empty
  Economy/Reasoning slot fell back to Default. [src: file:
  backend/src/http_transport.rs:53-71]
- `http_transport::validate_connection_target` is the single "does this
  connection exist and match the requested agent type" guard, shared by
  Compare's judge/improve launch, multi-agent orchestration, and discussion
  create/update — previously each surface either lacked the check or
  reimplemented it. [src: file: backend/src/http_transport.rs:95-126]
- `discussions.connection_id` (migration 165) is the durable "sticky"
  connection a discussion's *implicit* target resolves to. Before this
  column existed, `MessageTarget::discussion_agent()` — the canonical target
  for an ordinary reply with no explicit `@mention` — always constructed
  `connection_id: None`, and the empty-target dispatch fallback
  (`native_dispatch_agents_for_targets`) did the same. A discussion whose
  primary agent was a named Custom connection could dispatch its *first*
  message (an explicit `Agent`-kind target from New Discussion) but lost the
  connection on every ordinary follow-up reply. Both resolution points now
  read the discussion's sticky `connection_id` instead. [src: file:
  backend/src/api/discussions/messaging.rs] (`canonical_targets`'s
  `DiscussionAgent` branch, `native_dispatch_agents_for_targets`'s
  empty-target fallback) [src: file:
  backend/src/db/sql/165_discussion_connection_id.sql]
- `target_tier` in both `orchestration.rs` and the frontend
  `AgentSwitchPicker`/`MessageTarget` model matches on `(agent_type,
  connection_id)`, not `agent_type` alone — two different connections both
  project onto `AgentType::Custom`, so a match on the enum alone could pick
  whichever target happened to be first. [src: file:
  backend/src/api/discussions/orchestration.rs] (`target_tier`)

### Surfaces that now carry a `connection_id`

`StartBatchCompareJudgeRequest`, `StartBatchCompareImprovementRequest` and
the new `OrchestrationParticipant` type (replacing the bare
`Vec<AgentType>` on `OrchestrationRequest`) all carry an optional
`connection_id`, validated the same way and dispatched through the same
`enqueue_with_connection`/`external_http_runtime` construction discussion
dispatch already used. [src: file: backend/src/models/compare.rs]
[src: file: backend/src/models/discussions.rs] (`OrchestrationParticipant`)

## Consequences

`http_transport.rs` does not replace `external_api_connections` (connection
identity, credentials, endpoints) or `core::model_catalog` (per-model
capability/tier data) — it is the seam between them: codec choice and the
capability gate on one side, tier/connection resolution shared across launch
surfaces on the other. Nothing in this module talks to a provider directly or
owns persistence beyond the `discussions.connection_id` column.

Orchestration resolves and checks the named connection snapshot immediately
before each summary, participant-round, and synthesis launch. Workflow Agent
steps fail closed when their named connection cannot be resolved; their initial
and immediate pre-dispatch checks use its `http:<connection-id>` target and
effective model. The guard remains caller-owned; centralizing it
at the runner boundary is a possible follow-up once overlapping runner work is
integrated. [src: file: backend/src/api/discussions/orchestration.rs:235-385]
[src: file: backend/src/workflows/steps.rs:241-310]
[src: file: backend/src/workflows/runner.rs:815-850]

## Sources

- ADR-003 — ACP control-plane boundary: the "OpenAI-compatible HTTP remains a
  separate runtime-to-model-provider transport" statement this ADR
  implements. [src: file: docs/design/adr-003-acp-control-plane.md]
- KT-531 (dynamic runtime catalog foundation, `b2971152`) — durable
  `runtime_target_id` scheme and `CatalogModelEntry.capabilities` this ADR
  builds the capability gate on top of.
- KT-339 (0.12.0) — named `external_api_connections` (LiteLLM/NVIDIA/
  OpenRouter/Other presets, mention aliases, per-connection tiers) this ADR
  extends with a discussion-level sticky target and cross-surface
  `connection_id` support.
