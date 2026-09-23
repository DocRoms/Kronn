# Native workflow authoring, 2026-09-22

KT-673 uses the real `KronnToolExecutor` against a temporary database. The fixed
French request asks for a disabled manual workflow containing exactly three
steps: emit `{"items":[1,2,3]}`, count those items, then copy the count into
`total`. Success requires the saved workflow's three deterministic steps to
execute and produce `{"total":3}`. A successful create call alone does not pass.
The harness calls the real JsonData and TransformData executors, while refusing
to execute any other generated step type. It does not launch the full workflow
scheduler or a child model. [src: file: backend/src/api/agent_quick_prompt_bench.rs:6]

Each observation uses full native declarations, tiering disabled, and
`KRONN_OLLAMA_NUM_CTX_CAP=32768`. These are development observations across
changing implementations, not randomized comparative trials or evidence of
reliability across prompts. Token totals sum provider-reported prompt tokens
across turns. Raw reports retain tool arguments, emitted text, persisted workflow
and deterministic execution results.
[src: file: docs/research/native-workflow-2026-09-22-attempt-1.json:1]
[src: file: docs/research/native-workflow-2026-09-22-attempt-2.json:1]
[src: file: docs/research/native-workflow-2026-09-22-attempt-3.json:1]
[src: file: docs/research/native-workflow-2026-09-22-attempt-4.json:1]

| Attempt | Local model | Schema behavior | Turns | Prompt tokens | Seconds | Result |
| --- | --- | --- | ---: | ---: | ---: | --- |
| 1 | qwen3.8:27b-mlx | Index by default | 15 | 188346 | 69.44 | No saved draft |
| 2 | qwen3.8:27b-mlx | Complete contract by default; generic result clamp | 15 | 214754 | 162.92 | Invalid top-level fields; no saved draft |
| 3 | qwen3.5:4b | Complete contract protected from clamping | 3 | 44284 | 14.22 | Disabled draft saved; final total 1 instead of 3 |
| 4 | gemma4:e4b | Protected contract with clarified shape and fallback rules | 4 | 48858 | 30.07 | Disabled draft saved; all three steps pass; final total 3 |

Attempt 3 replaced the requested object with an array, then read its missing
`items` property. A fallback of 0 followed by `count` produced 1 because scalar
count is 1. Its successful tool calls and confident prose therefore did not
constitute functional success. The canonical contract now documents that
fallback precedes the operation, preserves the requested payload shape, and
requires listing existing workflows before choosing an example id.
[src: file: docs/research/native-workflow-2026-09-22-attempt-3.json:1]
[src: file: backend/src/workflows/transform_data_step.rs:77]
[src: file: backend/src/api/workflow_step_schema.json:1]

The first two traces record calls at the executor boundary, not the complete
provider request after clamping. Production already raises a tooled run to its
effective context cap before sending the first request; the initial fit alone
is not the actual production window. A separate regression reproduces that
full window plus accumulated tool history, then the clamp-before-resize order.
It verifies that the schema stays complete while ordinary output is trimmed.
The model failures do not establish whether or when their schema was truncated,
or whether clamping caused a particular mistaken argument.
[src: file: backend/src/agents/runner_test.rs:1480]

Attempt 4 read the JsonData and TransformData contracts separately, created the
correct disabled draft and produced the requested result through all three
real deterministic executors. This is one successful local-model fixture, not
a cross-model success rate or evidence that the contract changes caused the
improvement: both the implementation and the model changed.
[src: file: docs/research/native-workflow-2026-09-22-attempt-4.json:1]

Frozen libtest executable SHA-256 values:

- Attempt 1: `25868ad9262ac26de5df19587bbbbbe1b0b8687f0daa97f355702d594004fff0`
- Attempt 2: `6a23fe84caf45057c9fd1fbbf53c1cced2c475e323139cbaece11a67c60998c9`
- Attempt 3: `c609364b62b6f2a018e68fd3e333c90425040bb120455942a05a8a71210166bc`
- Attempt 4: `0dc7b80c84ca0bdd1d38da7cb400e20295e9d9b39cf6d059de123e29b2dc8e22`

These development binaries predate this report. Retained JSON reports are the
portable artifacts; the executable copies remain local temporary artifacts.

## Declaration cost and validation

The static measurement reconstructs the pre-workflow surface at `39e8a4fb` by
removing the five workflow declarations and restoring the earlier family-index
description. It reports serialized UTF-8 bytes, not tokenizer estimates:

| Surface | Before | After | Increase |
| --- | ---: | ---: | ---: |
| Full native catalogue | 39841 bytes, 54 tools | 42861 bytes, 59 tools | 3020 bytes |
| Automations family | 6731 bytes, 10 tools | 9751 bytes, 15 tools | 3020 bytes |
| Experimental tiered initial catalogue | 15203 bytes, 27 tools | 15322 bytes, 27 tools | 119 bytes |

The existing six-model report records the earlier Quick Prompt increment; this
measurement does not reconstruct or replace that historical experiment.
[src: file: backend/src/api/agent_quick_prompt_bench.rs:199]
[src: file: docs/research/native-tool-catalogue-2026-09-22.md:1]

The MCP declaration surface stays at 111 tools and 86618 bytes; moving its
schema implementation into the shared backend endpoint adds no declaration.
The 11 MCP budget tests and 637 bridge tests pass. Backend checks include 113
workflow-filtered regressions, 6 clamping regressions, and all-target Clippy
with warnings denied. [src: file: backend/scripts/ci/mcp_surface_budget.py:1]
[src: file: backend/scripts/test_disc_introspection_mcp.py:6069]
[src: file: backend/src/api/agent_workflow_tests.rs:1]
[src: file: backend/src/agents/runner_test.rs:1480]

The [two-small-model growth check](native-catalogue-growth-2026-09-22.md)
replays the seven existing non-workflow scenarios with the enlarged catalogue.
All fourteen pass/fail outcomes match the earlier full-mode observations; the
report preserves the added prompt-token cost and the historical-build limits.
[src: file: docs/research/native-catalogue-growth-2026-09-22.json:1]
