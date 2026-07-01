# Ollama local models — deterministic offload (0.8.10)

Run **deterministic, well-defined workflow steps** (summarize, reformat,
extract, classify) on **local** models via Ollama to cut API cost, keeping the
paid tier (Claude API/CLI) for reasoning. This doc covers how the Ollama agent
path behaves and the gotchas that make local inference reliable.

## Model resolution (precedence)

`runner::effective_model_flag` [src: file: backend/src/agents/runner.rs] resolves
the model for every run:

1. **Explicit model** (`AgentStartConfig.model_override`) — wins outright.
   Fed from a workflow step's `agent_settings.model` (`steps.rs`) or a
   discussion's `model` (`streaming.rs`, e.g. inherited from the launching QP).
2. **Tier** → `resolve_model_flag(agent, tier, model_tiers)` — the OllamaCard
   overrides (global `ModelTiers`) or the built-in fallbacks.

Built-in Ollama fallbacks are **portability-first** (fit almost any machine),
NOT tuned for a big box: Economy `qwen3:4b`, Default `qwen3:8b`, Reasoning
`qwen3:30b-a3b` [src: file: backend/src/agents/runner.rs]. A powerful machine
sets a bigger model per-tier (OllamaCard) or per-step/QP; small machines are
safe by default. The old `llama3.2` / bare `qwen3` fallbacks (not pulled / not
a pullable tag) that produced opaque Ollama 404s are gone.

Per-step model: workflow Agent step → Advanced → *Modèle* (WorkflowStep
`agent_settings.model`). Per-QP model: QuickPrompt form → *Modèle* field
(persisted in `quick_prompts.agent_settings_json`, migration 070). A QP model
reaches execution via three paths: workflow hydration
(`quick_prompt_hydrate`), batch launch (`create_batch_run`), and standalone
launch (`crud.rs` stamps `discussions.model` from the QP).

## "Stable output", NOT bit-exact determinism

`build_ollama_chat_body` [src: file: backend/src/agents/runner.rs] sends
`temperature:0, top_k:1, seed:42` as INTERNAL constants (no per-step knob).
On Apple Metal the float reduction order isn't guaranteed, so two logits within
epsilon can flip the argmax (more so under Q4 quant) → output is *greedy-stable*,
**not bit-exact reproducible**. **Never** build logic (output hash-caching,
strict text-equality tests) that presumes reproducibility — especially not
cross-machine (Mac/Metal vs a CPU/CUDA peer). Ordered pillars: fixed num_ctx >
temp=0/top_k=1 > same model+quant > seed (near-inert under greedy).

Tests assert on the constructed request BODY, never on generated text.

## Two gotchas (both handled, both verified empirically)

1. **num_ctx** — Ollama's default context window is huge (up to 256K for some
   qwen3 tags). An oversized KV cache balloons memory and spills onto the CPU:
   `llama3.3:70b` measured **0.2 tok/s** at 128K ctx vs **12.5 tok/s** at 8K
   (100% GPU). `ollama_num_ctx` sizes to the prompt and caps at 8192. Diagnose
   with `ollama ps` (PROCESSOR column → want 100% GPU) and `kronn logs | grep
   ollama` (the effective num_ctx is logged at run start on target
   `kronn::ollama`).
2. **qwen3 reasoning** — qwen3 are hybrid-reasoning. The Ollama `think:false`
   API flag is **NOT honored** on `/api/chat` (verified: `message.content`
   still carries reasoning, untagged). The only reliable switch is the qwen
   `/no_think` control token in a dedicated system message
   (`ollama_disables_thinking`, applied to any `qwen3*` tag), which routes
   reasoning into a separate `thinking` field and keeps `content` clean. The
   `strip_thinking_leaks` regex is only a secondary net (widened to catch the
   short `<think>` tag; it can't catch untagged reasoning).

## TypedSchema → constrained JSON + quality escalation

For an Agent step with `output_format: TypedSchema`, `steps.rs::ollama_envelope_format`
wraps the author's `data` schema in the canonical envelope shape
`{data, status, summary}` and passes it as Ollama's `format` param — decoding is
grammar-constrained, so the output is a structurally-valid bare envelope object
that `extract_step_envelope` strategy-2 recovers. `stream:false` is used in this
case (one validated blob, not chunks). Post-extract schema validation + the
repair / `on_invalid` flow are unchanged.

**Quality escalation** (`steps.rs::escalation_step`): if a LOCAL (Ollama)
TypedSchema step still fails validation after the repair attempt, it retries
ONCE on the paid reasoning tier (Claude) before falling through to `on_invalid`.
This is a loop POLICY (derived from the step having run on Ollama), not a knob.
The escalation RATE — logged on `kronn::ollama::escalation` — is the health
metric that reveals which steps are too hard for the chosen local model.

## Bench snapshot (M5 Max 64 GB, Q4_K_M, informational)

| Model | tok/s (warm) | Note |
|---|---|---|
| qwen3:4b | ~114 | needs `/no_think`; economy |
| qwen3:8b | ~80 | clean, portable default |
| qwen3:30b-a3b | ~110 | MoE, best speed/quality |
| qwen3:32b | ~23 | dense, no advantage over 30b-a3b |
| llama3.3:70b | ~12.5 | excellent, heavy; cap num_ctx |

Numbers are machine-specific; the mechanisms above are hardware-agnostic.
