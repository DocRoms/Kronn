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
[src: file: backend/src/core/model_catalog/mod.rs:38-219]
[src: file: backend/src/agents/runner.rs:2650-2725]

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
dispatch. A known unavailable model or a blocking target refresh failure returns
a structured diagnostic and zero agent tokens are consumed. A stale CLI target
first receives one bounded refresh attempt; the preflight consumes its
normalized auth/timeout/missing-runtime result.
For Claude only, a discovery `Timeout` or `ProviderError` does not block an exact
model still recorded `Available`. The cached provenance and error remain visible;
this is permission to attempt execution, not proof of account access. Missing CLI,
authentication failures and unavailable/unknown identities are not covered by
that exception, and other runtimes keep their existing failure policy.
[src: file: backend/src/core/model_catalog/mod.rs:653]
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

## Reasoning-effort presets and transmission (KT-646)

`ModelTierConfig` (per-agent Economy/Default/Reasoning model tiers, stored in
`config.toml` under `[agents.model_tiers]`) carries an optional effort string
alongside each tier's model (`economy_effort`/`default_effort`/
`reasoning_effort`), additive and backward compatible: an untouched config
deserializes every new field to `None`. Without an execution override or a
preset, no effort flag is sent: the runtime's own default applies, never a
silently assigned `high`/`max`.
[src: file: backend/src/models/setup.rs:1]

Resolution is a single precedence, mirrored on `effective_model_flag`:
`runner::resolve_reasoning_effort` — explicit per-step/per-QP
`AgentSettings.reasoning_effort` (the execution override) wins outright; else
the tier's configured preset applies only when the final model equals that
tier's resolved model; else `None`. This retains a preset when an internal
caller pre-resolves the same model, but never carries it onto a different
model pin. The candidate is sent only when that exact available catalog entry
advertises it in `reasoning_modes`. `runner::agent_supports_reasoning_effort`
gates the whole precedence to agents with a proven contract: Codex via its
documented per-run
`-c model_reasoning_effort=<value>` TOML override
([config reference](https://learn.chatgpt.com/docs/config-file/config-reference),
[developer commands](https://learn.chatgpt.com/docs/developer-commands?surface=cli)),
and Claude Code via its installed `--effort <level>` CLI flag. Direct CLI and
the optional Claude/Codex ACP adapters carry the same resolved value; other
ACP and HTTP routes receive no guessed parameter.
[src: file: backend/src/agents/runner.rs:2746-2850]
[src: file: backend/src/acp/claude_adapter.rs:200-225]
[src: file: backend/src/acp/codex_adapter.rs:358-374]

A Quick Prompt's explicit `agent_settings.reasoning_effort` is copied onto a
hydrated workflow step the same way its `agent_settings.model` already is
(step wins if it sets its own), so a QP-driven step carries the effort its
author picked.
[src: file: backend/src/workflows/quick_prompt_hydrate.rs:1]

Standalone QP launches additionally capture their explicit effort alongside
the resolved model, agent and tier in `discussion_effort_snapshots` (migration
176). The template, model settings and source version come from one QP read.
Creation returns the persisted model, and inserts the discussion and override
atomically. Resumes read this immutable discussion snapshot, never the current
QP or a version that may have been removed. Existing discussions are not
backfilled. A changed model/agent/tier, HTTP connection or per-message tier
override does not inherit this QP override; the new run's own preset resolution
still applies. Failure to read the snapshot stops the run explicitly.
[src: file: backend/src/db/discussion_effort.rs:1]
[src: file: backend/src/api/discussions/crud.rs:201]
[src: file: backend/src/api/discussions/streaming.rs:2535]

These snapshots are local execution state, not a portable QP setting. Exporting
or importing a QP preserves its editable `agent_settings`, while it does not
reconstruct historic discussion launch overrides.
[src: file: backend/src/db/discussion_effort.rs:1]

Settings → Agents exposes an effort selector per tier for Claude and Codex.
It derives choices from the selected model's catalog entry, retains an invalid saved value only as an
explained disabled option, and clears an incompatible effort in the same save
that changes the model. Even without available modes, the control remains
usable to clear an invalid saved effort. Other runtime cards state that effort
is unsupported.
[src: file: frontend/src/components/settings/AgentsSection.tsx:150-205]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1245-1310]

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

## OpenCode Zen: cost hint and privacy note (KT-543)

OpenCode's own docs qualify every model routed through its hosted gateway
("OpenCode Zen") as `opencode/<model-id>` inside its multi-provider config.
Neither ACP's `session/new` config options nor Zen's own `/v1/models`
endpoint (verified: only `id`/`object`/`created`/`owned_by`) expose pricing,
so migration 164 adds a generic, provider-agnostic overlay —
`cost_hint` (`free`/`paid`/`unknown`) and `privacy_note` — to every catalog
entry, not a Zen-specific table. `reconcile_live` auto-tags a first-seen
model as `Unknown` with a structural (non-per-model) privacy note purely by
checking the `opencode/` provider-namespace prefix on `runtime_target_id ==
"agent:opencode"` — never a hardcoded model id. Both fields are
operator-overridable through the same manual-entry path as
`display_alias`/`tier_assignment`, but unlike `tier_assignment` a `None` in
`UpsertManualModelRequest` preserves the existing value (COALESCE) instead
of clearing it, so an unrelated edit (e.g. a rename) cannot silently wipe an
auto-detected or previously-confirmed assessment.
[src: file: backend/src/db/model_catalog.rs (derive_opencode_zen_overlay, reconcile_live)]
[src: file: backend/src/db/sql/164_model_catalog_cost_privacy.sql]
[src: file: backend/src/models/model_catalog.rs (ModelCostHint)]
[src: url: https://opencode.ai/docs/zen/]
[src: url: https://opencode.ai/zen/v1/models]
