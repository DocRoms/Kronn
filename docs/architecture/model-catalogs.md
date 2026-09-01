# Dynamic model catalogs

Kronn resolves selectable models from one durable catalog contract. The
catalog is independent from the transport used to execute the model.

## Stable identity

The canonical key is `(runtime_target_id, model_id)`:

- a CLI family uses `agent:<canonical-slug>` (`agent:codex`,
  `agent:opencode`, …);
- a named OpenAI-compatible connection uses its immutable database id as
  `http:<connection-id>`;
- `agent_type` is projection metadata for existing UI/runner code, never a
  namespace;
- direct CLI and ACP are routes to the same `agent:*` identity.

Consequently two HTTP connections may expose the same provider `model_id`
while keeping independent freshness, capabilities and tier assignments.
[src: file: backend/src/db/sql/162_model_catalog.sql:1-42]
[src: file: backend/src/db/model_catalog.rs:20-59]

## Resolution and reconciliation

The resolution order is live discovery, last valid cached snapshot, manual
operator entry, then the one-time migrated seed. A successful refresh merges
on the canonical identity. It preserves the operator alias and tier choice,
updates technical capabilities from the live source, marks disappeared models
`cached` + `unavailable`, and reactivates the same row when it reappears.
Failed refreshes keep every last-known row intact, downgrade `live` provenance
to visibly stale `cached`, and record a normalized target-level error. A
provider failure does not claim that an individual model disappeared.
[src: file: backend/src/db/model_catalog.rs:390-499]

The former runtime literals are inserted once during startup and the runner's
hot-path tier lookup is then projected from catalog rows. Editing the manual
catalog refreshes that projection; removing a migrated/manual row therefore
does not make a hidden hard-coded fallback reappear.
[src: file: backend/src/core/model_catalog/mod.rs:38-181]
[src: file: backend/src/agents/runner.rs:2407-2488]

## Discovery boundaries

- ACP runtimes negotiate a throwaway session and read its configuration
  options without sending a prompt.
- Codex uses `codex app-server` and its machine-readable `model/list` result,
  including supported reasoning efforts.
- Named HTTP connections reuse the bounded authenticated connection test and
  capability metadata from the provider. OpenRouter's chat, image and video
  catalogs are deliberately separate (`/v1/models`, `/v1/images/models`,
  `/v1/videos/models`); they are merged by exact model id and persisted under
  `http:<connection-id>`. NVIDIA capability metadata is read from its model
  records. A saved media slot never invents a capability.
[src: file: backend/src/core/model_catalog/acp_discovery.rs:1-104]
[src: file: backend/src/core/model_catalog/codex_discovery.rs:1-166]
[src: file: backend/src/api/external_api_connections.rs:536-638]

## Launch safety and UI

Discussion dispatch is the shared boundary for ordinary discussions, Quick
Prompts, comparisons and judges. It checks the exact durable target before
marking the provider as started. Workflows check every statically reachable
agent step before the first step and check each step again immediately before
dispatch. A known unavailable model or a recent target refresh failure returns
a structured diagnostic and zero agent tokens are consumed. A stale CLI target
first receives one bounded refresh attempt; the preflight consumes its
normalized auth/timeout/missing-runtime result.
[src: file: backend/src/api/discussions/streaming.rs:2168-2220]
[src: file: backend/src/workflows/runner.rs:738-858]
[src: file: backend/src/workflows/steps.rs:220-286]

`AgentSwitchPicker` remains the common selector on discussion, QP/compare and
workflow surfaces. It reads the shared snapshot, shows provenance, and leaves
known unavailable choices visible but disabled. Settings exposes refresh plus
manual create/update/delete, including chat/image/video capabilities and
reasoning modes.
[src: file: frontend/src/components/AgentSwitchPicker.tsx:1-300]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:1-236]

## Migration and test invariants

Migration 162 creates both catalog tables and keys the refresh log by
`runtime_target_id`. Its upgrade test always starts from the immediately
preceding registered migration (161 once the ACP session migration is
integrated). Tests pin manual/live reconciliation, idempotency,
disappearance/reappearance, refresh errors, cache staleness, provider media
capability isolation and same-model isolation across HTTP connections.
Generated TypeScript remains owned by `make typegen`.
[src: file: backend/src/db/model_catalog.rs:675-917]
[src: file: docs/testing-quality.md:1-38]
