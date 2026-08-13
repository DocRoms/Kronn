# TD-20260808-http-agents-no-tool-calling

- **ID**: TD-20260808-http-agents-no-tool-calling
- **Area**: Backend / Agents

- **Status**: **Core shipped 2026-08-09; workflow parity added in 0.9.7.** Agents on the HTTP path (Ollama, LiteLLM) call Kronn primitives through a native tool loop in discussions and Workflow Agent steps. Calls are executed, projected for small windows, and rendered as bounded, secret-free receipts. The row stays open for the remaining per-model capability probe and an explicit tool-exposure policy.

- **Problem (fact, as filed)**: agents running over Kronn's HTTP path could call **no tools at all** — not Kronn's APIs, not MCP servers, not the deagentified primitives. CLI agents got all of it through the `kronn-internal` stdio bridge; HTTP agents got nothing, which left a local model as a read-only advisor.

  This was deliberate rather than an oversight: describing MCP tools in the prompt had been tried and taught the model to **hallucinate** calling them — it presented `fastly_execute` as its own capability (2026-07-01), so the code told the model the truth instead [src: file: backend/src/agents/runner.rs:820]. The request body carried no tool channel either [src: file: backend/src/agents/runner.rs:1537].

- **Why native, not a textual convention** — decided 2026-08-09 on measurements, superseding the earlier `KRONN_CALL:` proposal:
  - A declared tool is **enforced** by the API: the provider reports `finish_reason: "tool_calls"` and returns a structured object. A textual syntax only *asks*, and re-creates the exact failure mode of the 2026-07-01 incident — except now Kronn would execute the hallucinated call.
  - Parsing: the proposed `\(([^)]*)\)` breaks on any argument containing `)`. There is nothing to parse on the native path.
  - Context: describing a syntax plus a catalogue in prose costs tokens on precisely the models with the tightest window.
  - Reusing Kronn's display format was considered and **rejected**: `[kronn-internal: tool(args) → result]` is what Kronn renders *back* into the transcript, so making it also *trigger* execution would let an agent quoting history re-fire an action.
  - **Measured**: all seven models on the dev machine advertise `tools` (`/api/show` → `capabilities`), and both transports carry them — Ollama returns the call on the message, LiteLLM forwards it as OpenAI `tool_calls`. The population needing a textual fallback was empty.

- **What shipped**:
  - `backend/src/agents/tools.rs` — `ToolCall`/`ToolOutcome`, the `ToolExecutor` trait (so `runner.rs` never depends on `AppState`; tests supply a fake), a fragment accumulator, and the message shapes both providers require.
  - Fragment merging is index-keyed because the formats disagree: Ollama emits a whole call in one frame, a true OpenAI upstream splits one across frames (name first, then argument text in slices that are not valid JSON alone).
  - The loop in `start_ollama_http` [src: file: backend/src/agents/runner.rs:1668]: stream a turn, execute any calls, append the assistant + tool messages, re-POST, repeat — capped by `MAX_TOOL_ITERATIONS` (8), which fails the step loudly rather than truncating silently. Token counts accumulate across turns, so a three-tool run reports the sum.
  - `backend/src/api/agent_tools.rs` — the executor, invoking existing handlers in-process (`mcps::overview`, `quick_apis::list`/`run_qa`, `agent_api::agent_api_call`). Tool failures come back as data (`{"error": …}`) instead of killing the turn, so the model can correct itself.
  - **Results are projected, not dumped.** Measured on the real instance, the raw handler payloads are unusable for a local model: `/api/mcps` is 52 KB (~13 000 tokens, almost all of it `api_spec`) and `/api/quick-apis` is 17 KB (~4 400). Injecting either as a tool result would consume a small model's whole window on the first call. The executor therefore returns decision-shaped views — and `mcp_list` was split, listing plugins only, with a new `api_endpoints(slug)` fetching one plugin's paths on demand:

    | Tool | Raw | Projected |
    |---|---|---|
    | `mcp_list` | ~13 000 tokens | **~543** (10 plugins) |
    | `api_endpoints` | — | ~291 for one plugin |
    | `qa_list` | ~4 400 tokens | **~1 161** (19 entries) |
  - Initially wired only on the discussion reply path. In 0.9.7, Workflow Agent steps receive a project/run-scoped executor with API and Quick API tools plus read-only Planning (`task_list`, `task_get`) when a project exists. Planning mutations remain excluded; CLI agents still use the bridge, and internal summarisation passes do not gain tools.

- **Verified in three layers**, because each caught what the previous could not:
  1. *Does the loop work?* Mock-based tests (happy path + iteration cap), plus an `#[ignore]`d live test (`live_tool_loop_against_a_real_model`).
  2. *Can a model use the real data?* Fed the **actual projected** `mcp_list` from a running instance and asked which plugin sends email: it answered `mcp-resend` and chained `api_endpoints(api_plugin_slug: "mcp-resend")` correctly, on a 907-token prompt. Would have failed silently before the projections.
  3. *Does it work in a real Kronn discussion?* A throwaway instance (isolated `KRONN_DATA_DIR`, own port), a real project, a real discussion, the real `/run` endpoint. **This layer found two bugs the first two could not**, both of which made the feature silently useless:
     - **Ollama 400s on the tool round-trip.** OpenAI wants `arguments` as a JSON-encoded *string*; Ollama requires a real *object* and rejects the string with `Value looks like object, but can't find closing '}' symbol`. The tool ran, then the loop died feeding the result back. Message rendering is now codec-specific (`assistant_tool_call_message(calls, string_arguments)`), and round-trip errors now include the provider's body instead of a bare status.
     - **The prompt told the model it had no tools.** The injected `=== TOOLS ===` block still said "You have NO executable tools", written when that was true. With `tools_declared=5` on the wire, the model answered *"je ne peux pas exécuter d'outils ici"* — prose beat the declaration. The block is now conditional on whether an executor is attached.

     After both fixes, in a real discussion: `backend="Ollama" tools_declared=5 → tool call mcp_list ok=true turn=1`, and the same for `backend="LiteLLM" model=local-fast`. Both answered from the tool result instead of the earlier confabulation ("Ollama et LiteLLM sont des plugins API", lifted from `AGENTS.md`).
  4. *Does it work against a real third-party API?* Same instance, real SpeedCurve credentials. This found two more:
     - **`api_call` needs an `api_config_id`**, which the `mcp_list` projection had dropped — every call failed with "Either (api_plugin_slug + api_config_id) OR quick_api_id is required" and the model had no way to discover the value.
     - **Making the model carry that UUID was the wrong fix.** With the id exposed in the list, a 4 B model paired `api-speedcurve` with Resend's config id. Kronn owns that mapping, so the executor now resolves it from the slug and treats an explicit value as a disambiguation hint. That single change is what made the 4 B model succeed.

     Final result, both agents, real data: `@ollama` (qwen3:32b, 3 turns) and `@litellm` (qwen3:4b via the proxy, 2 turns) each returned the same eight SpeedCurve site names, with the call chain visible in the transcript as `[kronn-internal: …]` System messages.

- **Residual**:
  - ~~**Traces are not rendered in the UI.**~~ **Done 2026-08-09.** The HTTP loop's trace lines are lifted out of the run's stderr capture into the same `kronn_tool_calls` list the CLI path fills, so they persist as `[kronn-internal: …]` System messages and render through `ToolCallsGroup` like every other agent's [src: file: backend/src/api/discussions/streaming.rs:166]. Verified in a real discussion.
  - **The catalogue is five tools** (`mcp_list`, `api_endpoints`, `qa_list`, `qa_run`, `api_call`). Every entry costs context on a small model and a long list degrades selection, so widening it should follow evidence, not completeness.
  - **`qa_list` still costs ~1 161 tokens** at 19 entries and grows linearly with the library. It will need the same list/detail split as `mcp_list` once a user has a few dozen.
  - **`api_call` results are passed through unprojected.** A plugin returning a large body lands whole in the model's context; a size guard belongs there too.
  - **No per-model capability gate.** Every HTTP agent is offered tools; a model without support may error or ignore them. Ollama exposes `capabilities` via `/api/show` so this is detectable, but LiteLLM does not expose it for an `ollama_chat` backend — the proxy path needs try-and-recover or an explicit setting.
  - **No dedicated tool-exposure policy.** Project/API scope is enforced and workflow Planning mutations are excluded, but operators cannot independently disable native HTTP tools for an otherwise enabled model.
  - ~~**Workflow steps get no tools.**~~ **Done in 0.9.7.** Ollama and LiteLLM Workflow Agent steps receive the bounded project/run-scoped catalogue. Both wire shapes are exercised through complete step execution; run details persist only tool name + success, and an empty receipt explicitly points to a tool-capable model or deterministic ApiCall fallback when external data was expected.
  - ~~**The `=== TOOLS ===` prose has no regression test.**~~ **Done.** `tools_notice_matches_whether_tools_were_actually_declared` pins both the executable-tool and no-tool variants, including the configured REST API discovery route.

- **Next step**: add the per-model capability probe and an operator-facing native-tool exposure policy. Both are correctness guards on a feature that now works end to end in discussions and workflows.
