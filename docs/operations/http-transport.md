# HTTP model-provider transport (KT-545)

Companion to [`docs/design/adr-004-http-transport.md`](../design/adr-004-http-transport.md),
which is the source of truth for the design rationale. This page is the
operator-facing "how does a target actually resolve / what changed / how do
I debug it" view.

## What this is

LiteLLM, NVIDIA and any number of named Custom OpenAI-compatible connections
(configured in Settings → Agents → External API, KT-339) all dispatch over
the same shared HTTP chat path, owned by `backend/src/http_transport.rs`.
This is a distinct transport from ACP (`docs/operations/acp-adapters.md`) —
it never becomes an ACP runtime or an MCP server just because it streams
model output.

## Codec

Every HTTP-chat agent (`LiteLlm`, `Nvidia`, `Custom`) speaks the
OpenAI-compatible `/v1/chat/completions` wire (`OpenAiCodec`); `Ollama`
speaks its own native `/api/chat`. The choice is made once, explicitly, by
`http_transport::resolve_chat_codec` — there is no per-connection codec
override today.

## Pre-dispatch capability check

If a named connection's live catalogue (Settings → test a connection) tags a
model with capabilities that exclude `"chat"` — for example a video-only
model like `bytedance/seedance-2.0-mini` — a launch targeting that model is
refused *before* any request reaches the provider, with a structured
diagnostic (`CatalogPreflightFailure`, `reason: "unsupported"`). A model the
catalog has never seen (never tested, or a hand-typed override) is not
blocked — only a positive capability mismatch refuses.

This reuses the same `model_catalog::preflight_check` gate every launch
surface already calls, so the refusal shows the same card everywhere
(Discussions, Quick Prompts, Workflow steps).

## The discussion's sticky connection

Every discussion whose primary agent is a named connection persists that
connection on the `discussions.connection_id` column (migration 165), set at
creation and updated whenever the discussion's agent is switched via the
ChatHeader picker. This is what an *ordinary reply with no explicit
`@mention`* resolves through — before this column existed, only the very
first message of the discussion reliably carried its connection; every
plain follow-up reply lost it and failed to dispatch.

Switching a discussion's agent to a different `AgentType` (not `Custom`, not
`LiteLlm`/`Nvidia`) automatically clears the stored connection unless the
same request also sets a new one, so a stale connection id never lingers
under an unrelated agent.

## Surfaces with connection-aware targets

- **Discussions** — New Discussion, mid-thread `@mention` (already worked —
  the backend resolves aliases server-side regardless of frontend
  autocomplete), and the ChatHeader picker (now `availableTargets`-aware).
- **Quick Prompts** — connection picker already existed (KT-339/486).
- **Compare** — the AI judge and prompt-improver launch now accept an
  optional `connection_id`, validated against the chosen `agent` the same
  way every other surface is.
- **Orchestration (multi-agent debate)** — each participant is now an
  `OrchestrationParticipant { agent_type, connection_id }` instead of a bare
  `AgentType`, so two different named connections can both appear in one
  debate without colliding on `AgentType::Custom`.

## Known limitations

- **Preflight remains caller-owned.** Each launch surface must pass the actual
  runtime target to `model_catalog::preflight_check`. Orchestration and
  Workflow Agent steps fail closed when their named connection cannot be
  resolved. Orchestration resolves and checks a fresh connection/model snapshot
  immediately before each launch. A future
  centralized guard can make this invariant structural once the agent runner
  is available for that refactor. [src: file:
  backend/src/api/discussions/orchestration.rs:235-385] [src: file:
  backend/src/workflows/steps.rs:241-310]
- **Mid-thread `@mention` autocomplete doesn't suggest connection aliases.**
  The composer's autocomplete only lists built-in agents; typing a
  connection's exact alias (e.g. `@groq`) still dispatches correctly — the
  backend resolves it independently of what the UI suggested — but nothing
  prompts the user to type it.
- **Stored provider API keys are not AES-encrypted.** Several docstrings
  (including on `ExternalApiConnection` itself) describe the credential as
  living in "Kronn's encrypted credential store." In the current
  implementation a connection's `credential_slug` looks up a plaintext
  `ApiKey.value` in `TokensConfig.keys`, serialized as-is into
  `~/.config/kronn/config.toml` and protected only by file permissions
  (`0600`) — not by the AES-256-GCM `core::crypto` module MCP secrets use.
  This predates KT-545 and is not changed by it; see
  `docs/inconsistencies-tech-debt.md`.

## Troubleshooting

- **"The selected external API connection is unavailable" on an otherwise
  working discussion:** check the discussion's agent — if it's `Custom` and
  was switched away from and back without re-selecting the connection in the
  picker, `discussions.connection_id` may be unset. Re-select the connection
  from the ChatHeader picker.
- **A model that used to work now gets refused pre-dispatch:** re-test the
  connection (Settings → External API → Test). If the provider's catalogue
  now tags that model as image/video-only, the refusal is correct — pick a
  chat-capable model for that connection's tier.
