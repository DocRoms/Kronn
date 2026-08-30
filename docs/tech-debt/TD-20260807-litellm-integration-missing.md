# TD-20260807-litellm-integration-missing — resolved history

- **ID**: TD-20260807-litellm-integration-missing
- **Area**: Backend / Agents
- **Status**: **Resolved 2026-08-30**
- **Historical record**: the original investigation and incremental resolution
  were recorded in commit
  `c1976c37e80b031ec82df6b8ad98bf0b15ddad0c`.
  [src: commit: c1976c37e80b031ec82df6b8ad98bf0b15ddad0c]

## Original gap

This item tracked the absence of a complete operator path for using a LiteLLM
proxy as a selectable Kronn agent: installation and execution needed to be
joined by connection testing, model-tier configuration, discussion routing and
operator documentation.

## Resolution

The gap is closed end to end:

| Original concern | Resolved implementation |
|---|---|
| Select and install LiteLLM | `LiteLlm` is a known agent and the installer provides the supported `uv tool` command with its FastAPI compatibility bound. [src: file: backend/src/agents/mod.rs:92-105] |
| Execute an OpenAI-compatible proxy | LiteLLM uses the shared HTTP chat transport with an OpenAI-compatible codec for `/v1/chat/completions`, SSE frames and usage reporting. [src: file: backend/src/agents/chat_codec.rs:1-11] [src: file: backend/src/agents/chat_codec.rs:87-105] [src: file: backend/src/agents/runner.rs:4555-4583] |
| Configure endpoints and credentials | Settings manages named LiteLLM, NVIDIA and other OpenAI-compatible connections. Each row persists endpoint and credential metadata; key handling is documented separately. [src: file: backend/src/models/external_api_connection.rs:5-30] [src: file: docs/operations/external-api-connections.md:1] |
| Map models to runtime tiers | Every connection has explicit optional Economy, Default and Reasoning model fields. This resolves the former product decision without hardcoding a proxy catalogue. [src: file: backend/src/models/external_api_connection.rs:16-18] |
| Prove a connection is usable | The bounded probe reads `/v1/models` and, when a key and model are present, performs a minimal authenticated chat request. [src: file: backend/src/api/external_api_connections.rs:130-171] |
| Route a named proxy in discussions | A unique mention alias resolves to a target carrying the exact connection identifier, so multiple connections of the same provider remain distinct. [src: file: backend/src/db/external_api_connections.rs:75-173] |
| Preserve legacy configuration | Startup backfills the former LiteLLM and NVIDIA settings idempotently into named connections; the canonical NVIDIA row also receives its hosted default endpoint when missing. [src: file: backend/src/db/external_api_connections.rs:223-285] |
| Give HTTP agents useful tools | HTTP agents receive Kronn's bounded native catalogue rather than an MCP configuration or a textual tool-call convention. [src: file: docs/architecture/http-agent-capabilities.md:8-31] |

The original user impact—no supported route from Kronn to a LiteLLM-proxied
model—is therefore resolved. Operators can create and test a connection, map
its three tiers and invoke it through its discussion alias. The complete setup,
credential and migration behavior is recorded in the
[external API connection guide](../operations/external-api-connections.md).
[src: file: backend/src/api/external_api_connections.rs:410-464]
[src: file: backend/src/db/external_api_connections.rs:103-173]

## Retained operational constraint

Kronn pins `fastapi<0.140` in the LiteLLM proxy install command. That bound is
part of the currently supported installer definition and should only move with
an independently verified upstream compatibility update.
[src: file: backend/src/agents/mod.rs:92-105]

## Separate follow-up

Per-model native-tool capability detection and an operator-facing tool-exposure
policy remain tracked by
[`TD-20260808-http-agents-no-tool-calling`](TD-20260808-http-agents-no-tool-calling.md).
They do not reopen this completed LiteLLM integration: the shared native tool
loop already works, while that row tracks correctness guards for heterogeneous
models. [src: file: docs/tech-debt/TD-20260808-http-agents-no-tool-calling.md:1-6]

No further action remains on this TD.
