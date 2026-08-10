# TD-20260807-litellm-integration-missing

- **ID**: TD-20260807-litellm-integration-missing
- **Area**: Backend / Agents

- **Status**: **shipped 2026-08-09** — agent type, install, connect-then-select card, per-tier models, execution, and tool calling (item 4 landed the same day; see `TD-20260808-http-agents-no-tool-calling`). Residual on this row: the operations doc (item 5).

- **Problem (fact, as originally filed)**: LiteLLM (OpenAI-compatible proxy) was not selectable as a Kronn agent. It is a **server**, not a CLI binary, and it had no `AgentType` variant, no `KNOWN_AGENTS` entry, no tier configuration and no `/api/lite-llm/*` routes — versus the Ollama pair already mounted [src: file: backend/src/lib.rs:979].

- **Scope correction (2026-08-08)**: the first version of this TD scoped a three-phase project including a new HTTP execution engine and a tool-calling convention for non-MCP agents. **Both already exist**, built for Ollama, and are the natural reuse target:

  | Previously listed as missing | Actual state |
  |---|---|
  | HTTP execution path (runner assumes CLI subprocess) | `start_ollama_http` is a complete HTTP streaming path with a dispatch bypass before any CLI spawn [src: file: backend/src/agents/runner.rs:1668] [src: file: backend/src/agents/runner.rs:904] |
  | HTTP health + model-listing endpoints | `api::ollama::health` and `api::ollama::models`, already routed [src: file: backend/src/lib.rs:979] |
  | Schema-constrained decoding + failure escalation | Shipped for Ollama: envelope-constrained `format` [src: file: backend/src/agents/runner.rs:453] and escalation to a cloud agent on repeated failure [src: file: backend/src/workflows/steps.rs:371] |
  | Token-fragment streaming (vs line-based) | Already handled as an Ollama-specific case [src: file: backend/src/agents/runner.rs:109] |

  LiteLLM is therefore **not a new execution engine**. It is the Ollama HTTP path pointed at an OpenAI-compatible base URL.

- **Why we can't fix now (constraint)**:
  - ~~**Wire format differs.**~~ **Resolved 2026-08-08.** Ollama posts to `/api/chat` with newline-delimited JSON [src: file: backend/src/agents/chat_codec.rs:52]; OpenAI-compatible proxies use `/v1/chat/completions` with SSE and a separate usage frame [src: file: backend/src/agents/chat_codec.rs:86]. Both now sit behind `ChatCodec`, so the transport is shared.
  - ~~**Detection needs a new path.**~~ **Wrong — corrected 2026-08-08.** Ollama is *already* a server agent and is detected by binary like every CLI agent (`binary: "ollama"`) [src: file: backend/src/agents/mod.rs:87]; its server nature is handled separately by a health endpoint and a dedicated UI card (`OllamaCard`). `uv tool install 'litellm[proxy]'` puts a `litellm` binary on `PATH`, so LiteLLM detects identically. The split to respect is **installed** (binary present, `KNOWN_AGENTS`) vs **reachable** (proxy running, HTTP health) — Ollama already models both.
  - ~~**Tools are the blocking gap.**~~ **Resolved 2026-08-09.** An MCP config file was never the answer: LiteLLM is a proxy serving completions, and in Kronn's design the *orchestrator* owns the tool loop. That loop now exists on the shared HTTP path, so LiteLLM inherits it — no MCP file, no textual convention. Prose descriptions stay banned here [src: file: backend/src/agents/runner.rs:820]; tools are declared natively.
  - **Model/tier mapping is a product decision**, not a mechanical one: LiteLLM fronts many models, so which one backs which tier needs a config surface rather than a hardcoded table.

- **Impact**: feature gap — no unified access to LiteLLM-proxied models (local or remote) from Kronn; users wanting a model Kronn does not integrate directly have no route in.

- **Where (pointers)**:
  - `backend/src/models/setup.rs:543` — `AgentType` enum
  - `backend/src/agents/mod.rs:40` — `KNOWN_AGENTS` (binary-based detection)
  - `backend/src/agents/runner.rs:1668` — `start_ollama_http`, the path to generalise
  - `backend/src/agents/runner.rs:904` — the pre-CLI dispatch bypass
  - `backend/src/api/ollama.rs` — health/models endpoints to mirror
  - `backend/src/core/config.rs:350` — per-agent tier config

- **Suggested direction (non-binding)**:
  1. ~~**Split `start_ollama_http` into transport + codec.**~~ **Done 2026-08-08.** `backend/src/agents/chat_codec.rs` holds the wire-format seam: `ChatCodec` (endpoint + line decoding), `OllamaCodec` (NDJSON), `OpenAiCodec` (SSE, `[DONE]` sentinel, usage frame) and `build_openai_chat_body`. The transport in `start_ollama_http` is now codec-driven — endpoint via `codec.endpoint()`, decoding via `forward_chat_line` with a stream-scoped `TokenTally` so OpenAI's separate usage frame survives to the sentinel. The old `forward_ollama_line` was removed rather than kept as a wrapper: leaving it would have meant its four regression tests exercised a wrapper no longer on the production path. They now drive `forward_chat_line` directly through a test shim, so they cover what actually runs. No behaviour change for Ollama.
  2. ~~**Follow the Ollama shape end to end.**~~ **Done 2026-08-09**, tested against a live proxy fronting local Ollama:
     - `AgentType::LiteLlm` + `KNOWN_AGENTS` entry (`binary: "litellm"`), `install_prerequisite` → `uv` like Vibe, and an uninstall arm. Kronn's existing install button drives it.
     - `/api/lite-llm/{health,models,test}` and a `LiteLlmCard`. The card is **two-step by necessity**: nothing about a proxy is auto-detectable, so the user declares endpoint + optional key, `test` proves them, and only a proven connection unlocks the per-tier model pickers. `test` persists only on success — saving an endpoint that does not answer would strand the card.
     - Endpoint stored in `AgentConfig.base_url`; the key goes to the encrypted token store under the `litellm` provider (`#[ts(skip)]`, so it is never serialised back to the browser).
     - The runner threads both through the shared HTTP path and sends the key as bearer auth for LiteLLM only.
  3. ~~Expose the tier→model mapping as configuration.~~ **Done** — `ModelTiersConfig.lite_llm`, fed by whatever `/v1/models` declares. There is deliberately no built-in default. A proxy catalogue is not a health guarantee: corporate LiteLLM can list a partner model that its underlying project/region cannot invoke. Non-transient discussion model-start failures are therefore persisted per endpoint/model, rendered as compact actionable System messages, and listed on the LiteLLM card with a real completion-based retry that clears a recovered model. `[src: file: backend/src/db/lite_llm_model_failures.rs:17-70]` `[src: file: backend/src/api/lite_llm.rs:352-443]`
  4. ~~Land the tool loop on the shared HTTP path.~~ **Done 2026-08-09** — Ollama and LiteLLM gained it together, verified against a live proxy. Detail in `TD-20260808-http-agents-no-tool-calling`.
  5. Document setup in `docs/operations/` next to the Ollama guide.

- **Upstream trap (verified 2026-08-09)**: `uv tool install 'litellm[proxy]'` alone installs a **broken** proxy. litellm 1.95.0 declares `fastapi<1.0,>=0.136.3`, but fastapi 0.140 removed `get_flat_dependant`, which the proxy imports — so the newest allowed version crashes on startup with `ImportError`. Kronn's `install_cmd` pins `--with 'fastapi<0.140'` (0.139.2 confirmed working). Revisit when upstream tightens its own range.

- **Next step**: item 5 — document the setup (install, `config.yaml`, the fastapi pin) in `docs/operations/` next to the Ollama guide. Everything else on this row is done.
