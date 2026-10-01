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

These launch paths reuse `model_catalog::preflight_check`, but their refusal
presentation differs. Discussions and Quick Prompts return an API diagnostic;
workflow execution records a failed preflight result and emits a run error.
The shared gate does not imply an identical UI card on every surface.

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
  backend/src/api/discussions/orchestration.rs:60-130] [src: file:
  backend/src/api/discussions/orchestration.rs:650-1035] [src: file:
  backend/src/workflows/steps.rs:241-330] [src: file:
  backend/src/workflows/steps.rs:888-930]
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
- **HTTP 402 / billing error during a connection test:** check the provider's
  API account balance and billing, then test again. This is handled for every
  preset during catalogue discovery, OpenRouter key validation and model
  invocation. It stops the test immediately: later model errors cannot hide
  the billing failure. The form and saved connection card show a translated
  message; provider response bodies and credentials remain private.
  [src: file: backend/src/api/external_api_connections.rs:464]
  [src: file: frontend/src/components/settings/ExternalApiSection.tsx:334]
  Xiaomi MiMo returns HTTP 402 (`insufficient_balance`) when generation lacks
  credit, even when the key can load `/v1/models`. Previously the generic
  fallback could hide this behind a later HTTP 400 from a TTS model.
  [src: url: https://mimo.mi.com/docs/en-US/api/guidance/error-codes]


## Discussion attachments (KT-946)

Ordinary HTTP discussion turns load the room's image attachments separately
from its text context. A model with the catalogue capability `vision` receives
OpenAI `image_url` data-URL parts (LiteLLM, NVIDIA and named connections), or
Ollama `images` base64 values. Settings → model catalogue exposes **Vision
(read images)** independently of the `image` generation capability. Provider
image-input declarations also populate it; without a catalogue declaration,
Ollama `/api/show` and LiteLLM `/model/info` may confirm vision for the exact
model. A missing declaration never causes an image to be sent speculatively.
[src: file: backend/src/agents/vision.rs:1]
[src: file: backend/src/api/discussions/streaming.rs:1757]

Every withheld image is named in the model's context with an explicit instruction
not to describe its contents, and a way for the user to supply the information.
PNG, JPEG, GIF and WebP are supported. The eight most recent attachments are
eligible, bounded to 5 MiB per transmitted image and 12 MiB in total before
base64. Uploads up to 10 MiB that exceed the transmission limit are decoded
under allocation/dimension limits and downscaled to PNG, preserving the original
file. Failed decoding or remaining limits produce the same explicit withholding
notice. Transmission/downscaling counts are logged without the image payload.
Prompt estimates count a bounded image allowance instead of base64 text bytes.
[src: file: backend/src/agents/vision.rs:1]

`read_file` accepts only file paths registered to the current discussion, even
without a project. This exception grants no directory listing or writing rights;
`.kronn/context-files` remains read-only inside a project too. Symlink
attachments are refused. Unrelated files outside the workspace keep the existing
refusal. Images are never returned as lossy text by `read_file`.
[src: file: backend/src/api/agent_tools.rs:3159]
[src: file: backend/src/api/agent_workspace_tools.rs:1226]

CLI agents retain the existing attachment prompt: an image's path and an
instruction to inspect it with their own image/file tools before describing it.
Kronn does not claim that every CLI/model can decode every format. The HTTP
image parts described here apply to ordinary discussion turns; workflow Agent
steps do not implicitly import attachments from an unrelated discussion.
[src: file: backend/src/core/context_files.rs:544]
