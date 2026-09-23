# Final native Quick Prompt checks on LiteLLM and Ollama — 2026-09-22

The seven versioned native scenarios passed on both `claude-sonnet-4-6` through
the configured LiteLLM proxy and `qwen3.8:27b-mlx` through Ollama. Both test
processes exited successfully. This removes the previously observed network
obstacle to a live LiteLLM check; it does not establish a general model success
rate or repeat the unavailable historical `automations_probe.py` campaign.
[src: file: docs/research/native-qp-litellm-ollama-2026-09-22.json]

## Method and provenance

Both runs used the same frozen executable, built from
`5c439de0b04b3c66d40ee4f4639c5b6340229cab`, SHA-256
`2e47ff501371d6c24923056b75ca8d1a590c19fdd8542edb2079a4c4158f01e2`.
LiteLLM ran first, 15:08:26–15:09:08 UTC; Ollama followed,
15:09:27–15:10:46 UTC. The shipping full catalogue was selected explicitly.
`KRONN_OLLAMA_NUM_CTX_CAP=24576` applied to the Ollama run. No compilation or
second model campaign was launched during these two sequential runs.
[src: file: docs/research/native-qp-litellm-ollama-2026-09-22.json]

The native runner used the real `KronnToolExecutor`, isolated in-memory SQLite
fixtures and a temporary workspace. Scenarios list Quick Prompts, enqueue one
saved Quick Prompt, create a template with its required variable, update only
its description, enqueue an image job, edit exact file bytes, and inspect
background resumptions. Calls and persistence are checked, not only emitted
prose. Quick Prompt and media dispatch workers remain stopped: those two cells
prove the queued job, not child inference or image generation. Provider
credentials are read through the existing runner configuration and are absent
from the retained reports.
[src: file: backend/src/api/agent_quick_prompt_bench.rs]

Reproduce from the measured commit, using an existing configured provider:

```sh
# Both providers: seven scenarios, full catalogue only.
export KRONN_QP_BENCH=1 KRONN_BENCH_FULL_ONLY=1
export KRONN_OLLAMA_NUM_CTX_CAP=24576
# LiteLLM only: point to the operator's existing config, never copy its keys.
export KRONN_BENCH_CONFIG=/absolute/path/to/config.toml
export KRONN_BENCH_MODEL=claude-sonnet-4-6
export KRONN_BENCH_REPORT=/private/tmp/litellm-native-results.json
rtk proxy cargo test --manifest-path backend/Cargo.toml --lib \
  api::agent_tools::quick_prompt_bench::bench_quick_prompt_native_catalogue \
  -- --exact --ignored --nocapture --test-threads=1
# Ollama: repeat after unsetting KRONN_BENCH_CONFIG, using model
# qwen3.8:27b-mlx and a separate KRONN_BENCH_REPORT destination.
```

## Input-token observations

These are provider-reported prompt tokens, including the system prompt,
declarations, request and any accumulated history. Mean input tokens per turn
is total prompt tokens divided by observed turns. It is neither the incremental
cost of the QP declarations nor a monetary charge. Output tokens, cache discounts
and billed currency are not reported by this benchmark. Tokenizers differ;
these numbers do not justify a price or efficiency ranking between providers.
[src: file: backend/src/api/agent_quick_prompt_bench.rs]

| Provider / model | Passed | Turns | Total input tokens | Mean input tokens/turn |
|---|---:|---:|---:|---:|
| litellm / `claude-sonnet-4-6` | 7/7 | 17 | 234,943 | 13,820.2 |
| ollama / `qwen3.8:27b-mlx` | 7/7 | 20 | 246,811 | 12,340.5 |

| Provider | Scenario | Result | First-turn input | Total input | Turns | Seconds |
|---|---|---|---:|---:|---:|---:|
| litellm | list | pass | 13,663 | 27,416 | 2 | 4.98 |
| litellm | run | pass | 13,672 | 41,415 | 3 | 5.66 |
| litellm | create | pass | 13,698 | 27,874 | 2 | 6.32 |
| litellm | update | pass | 13,673 | 41,478 | 3 | 6.05 |
| litellm | media | pass | 13,685 | 27,630 | 2 | 5.17 |
| litellm | edit | pass | 13,681 | 41,626 | 3 | 6.32 |
| litellm | resume | pass | 13,674 | 27,504 | 2 | 4.47 |
| ollama | list | pass | 12,219 | 24,502 | 2 | 35.62 |
| ollama | run | pass | 12,223 | 36,963 | 3 | 13.41 |
| ollama | create | pass | 12,245 | 37,172 | 3 | 8.20 |
| ollama | update | pass | 12,224 | 36,933 | 3 | 3.99 |
| ollama | media | pass | 12,236 | 24,705 | 2 | 5.42 |
| ollama | edit | pass | 12,236 | 37,317 | 3 | 6.03 |
| ollama | resume | pass | 12,227 | 49,219 | 4 | 5.33 |

All per-cell values, calls, arguments, emitted text and persistence checks are
retained in the adjacent JSON. Original logs, individual JSON reports and
metadata are also attached to room message
`68cdb507-ab5b-44fa-96c6-6638552c50f8`.
[src: file: docs/research/native-qp-litellm-ollama-2026-09-22.json]

## Historical reference and limits

KT-670 named `automations_probe.py` and a 0.13.0 baseline of 6/6 scenarios and
3/3 well-formed authoring calls. That script was not recovered from the searched
repository, history or temporary paths. The ticket's author confirmed those
figures cannot be substantiated. They are not a baseline for this report, and
this is not claimed to be an exact replay of that missing instrument. The
replacement is the named, versioned seven-scenario native benchmark above.
An untracked historical script might have existed; failed searches do not
prove otherwise.

There is one observation per cell, no randomization and no repeat sample.
Successful Sonnet observations do not prove small-model reliability. The
previous six-model campaign and its failures remain evidence in their own
right. The full catalogue was used here; progressive loading was not retested.
No downstream provider override or saved-QP settings propagation is proved by
these job-only cells; those contracts have separate regression tests.
[src: file: docs/research/native-tool-catalogue-2026-09-22.md]
[src: file: backend/src/api/agent_quick_prompt_tests.rs]

Before the VPN restart, DNS failed in both sandboxed and host-side probes.
After Romuald restarted the VPN, the same configured endpoint answered HTTP 200
and the real campaign above passed. No application setting was rewritten. A
separate observation, named-connection `reachable: true` without a live probe,
is recorded as follow-up KT-697 and was not changed in this lot.
[src: user: 2026-09-22: litellm-live-access-20260922]
[src: file: backend/src/api/orchestration.rs:8420]
