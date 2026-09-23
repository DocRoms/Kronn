# Native tool catalogue campaign — 2026-09-22

The full catalogue remains the default. Across seven bounded scenarios on each
of six installed local models, it passed 35/42 cells; progressive loading passed
17/42. Every model lost successful actions with progressive loading. Fewer tokens
on a failed action are not a performance improvement at comparable quality.
`KRONN_TIERED_TOOLS=1` keeps progressive loading available as an opt-in experiment.
[src: file: docs/research/native-tool-catalogue-2026-09-22.json]
[src: file: backend/src/api/agent_tools.rs]

## Protocol and reproducibility

All 84 cells used one frozen executable, SHA-256
`7d4a867636c87d4c90ca2afa0666e679944255f6289ee03fade4806999c42d30`.
It was copied before commit `4930224`; the only subsequent change in that commit
was an explanatory comment in `tool_dispatch`. The runner and scenario source are
versioned in that commit. Models ran sequentially, full then tiered for each
scenario, with `KRONN_OLLAMA_NUM_CTX_CAP=24576`. The elapsed campaign was
06:59:01–07:23:25 UTC. Raw calls, emitted text, persisted checks, token counts,
turn counts and seconds are retained in the adjacent JSON artifact.
[src: commit: 4930224]
[src: file: docs/research/native-tool-catalogue-2026-09-22.json]

The benchmark delegates every call to `KronnToolExecutor::arc`; it does not
substitute a mock dispatcher. Each cell starts with an isolated database and
workspace. Quick Prompt and media launch checks prove job creation only: their
child dispatchers are deliberately not started. They do not prove downstream
inference or an actual rendered image. The edit scenario checks the exact file
bytes. List and resume checks prove the intended handler was reached, not the
semantic completeness of the final prose. Create/update checks inspect persisted
values, so a model claiming success after changing the wrong field fails.
[src: file: backend/src/api/agent_quick_prompt_bench.rs]

Scenarios, in order: list accessible Quick Prompts and required variables; launch
“Résumé” once for “été”; create “Synthèse test” with a required `topic`; update
only “Résumé”'s description; enqueue an image using the saved connection; read and
edit `status.txt` while preserving its newline; inspect pending background work
and resumptions. Exact prompts are in the benchmark source.
[src: file: backend/src/api/agent_quick_prompt_bench.rs]

## Observed action success

| Model | Full | Progressive |
|---|---:|---:|
| qwen3.5:2b | 6/7 | 1/7 |
| qwen3.5:4b | 6/7 | 3/7 |
| gemma4:e2b | 5/7 | 2/7 |
| gemma4:e4b | 6/7 | 2/7 |
| gemma4:12b-mlx | 5/7 | 3/7 |
| qwen3.8:27b-mlx | 7/7 | 6/7 |

A recurring progressive-mode failure was `qa_list` instead of loading the
`automations` family, followed by a claim that the Quick Prompt did not exist.
Full mode also has failures: for example, gemma4:e4b changed the prompt's `name`
instead of its `description`, then claimed the description had changed.
These are recorded model errors, not silently repaired responses.
[src: file: docs/research/native-tool-catalogue-2026-09-22.json]

## Per-cell measurements

Each cell below is `pass/fail · prompt tokens for all turns · turns · seconds`.
Failed cells retain their cost but cannot establish a same-quality saving.
[src: file: docs/research/native-tool-catalogue-2026-09-22.json]

| Model | Scenario | Full | Progressive |
|---|---|---|---|
| qwen3.5:2b | list | pass · 22826 · 2 · 3.58 | fail · 10410 · 2 · 0.85 |
| qwen3.5:2b | run | pass · 34445 · 3 · 1.71 | fail · 15728 · 3 · 1.50 |
| qwen3.5:2b | create | pass · 34526 · 3 · 1.64 | fail · 15769 · 3 · 1.36 |
| qwen3.5:2b | update | pass · 34438 · 3 · 1.12 | fail · 10420 · 2 · 0.75 |
| qwen3.5:2b | media | pass · 23009 · 2 · 1.59 | pass · 39677 · 6 · 8.52 |
| qwen3.5:2b | edit | pass · 34797 · 3 · 1.81 | fail · 21713 · 4 · 2.48 |
| qwen3.5:2b | resume | fail · 22830 · 2 · 0.83 | fail · 15699 · 3 · 0.91 |
| qwen3.5:4b | list | pass · 22826 · 2 · 6.16 | fail · 10410 · 2 · 2.22 |
| qwen3.5:4b | run | pass · 34449 · 3 · 2.65 | fail · 10418 · 2 · 1.71 |
| qwen3.5:4b | create | pass · 23083 · 2 · 3.34 | pass · 20239 · 3 · 6.39 |
| qwen3.5:4b | update | pass · 34438 · 3 · 1.99 | fail · 10420 · 2 · 3.14 |
| qwen3.5:4b | media | pass · 23027 · 2 · 2.88 | pass · 26529 · 4 · 11.01 |
| qwen3.5:4b | edit | pass · 34797 · 3 · 2.72 | pass · 30897 · 5 · 7.41 |
| qwen3.5:4b | resume | fail · 22828 · 2 · 2.34 | fail · 10429 · 2 · 3.01 |
| gemma4:e2b | list | pass · 21582 · 2 · 10.75 | fail · 9582 · 2 · 5.15 |
| gemma4:e2b | run | pass · 32548 · 3 · 10.66 | fail · 9590 · 2 · 6.83 |
| gemma4:e2b | create | pass · 21868 · 2 · 10.08 | fail · 4801 · 1 · 12.92 |
| gemma4:e2b | update | fail · 32565 · 3 · 11.42 | fail · 9594 · 2 · 16.21 |
| gemma4:e2b | media | pass · 21754 · 2 · 11.41 | pass · 16516 · 3 · 14.65 |
| gemma4:e2b | edit | fail · 32902 · 3 · 20.50 | pass · 28878 · 5 · 50.79 |
| gemma4:e2b | resume | pass · 21578 · 2 · 11.51 | fail · 9606 · 2 · 9.95 |
| gemma4:e4b | list | pass · 21582 · 2 · 28.87 | fail · 9582 · 2 · 17.71 |
| gemma4:e4b | run | pass · 32555 · 3 · 34.65 | fail · 9590 · 2 · 7.73 |
| gemma4:e4b | create | pass · 21833 · 2 · 13.58 | pass · 18727 · 3 · 33.11 |
| gemma4:e4b | update | fail · 32563 · 3 · 26.17 | fail · 9594 · 2 · 11.07 |
| gemma4:e4b | media | pass · 21762 · 2 · 27.98 | pass · 16521 · 3 · 30.13 |
| gemma4:e4b | edit | pass · 33010 · 3 · 50.99 | fail · 14832 · 3 · 86.31 |
| gemma4:e4b | resume | pass · 21578 · 2 · 22.60 | fail · 9606 · 2 · 13.53 |
| gemma4:12b-mlx | list | pass · 21590 · 2 · 43.93 | fail · 9590 · 2 · 23.89 |
| gemma4:12b-mlx | run | pass · 32571 · 3 · 22.24 | pass · 30337 · 5 · 42.71 |
| gemma4:12b-mlx | create | pass · 21879 · 2 · 20.67 | pass · 25937 · 4 · 14.05 |
| gemma4:12b-mlx | update | fail · 43637 · 4 · 16.76 | fail · 30438 · 5 · 18.61 |
| gemma4:12b-mlx | media | pass · 32917 · 3 · 29.93 | pass · 25009 · 4 · 16.87 |
| gemma4:12b-mlx | edit | fail · 45433 · 5 · 107.13 | fail · 38152 · 7 · 113.44 |
| gemma4:12b-mlx | resume | pass · 21586 · 2 · 14.97 | fail · 9620 · 2 · 2.54 |
| qwen3.8:27b-mlx | list | pass · 22842 · 2 · 45.86 | pass · 19900 · 3 · 35.57 |
| qwen3.8:27b-mlx | run | pass · 34476 · 3 · 20.17 | pass · 27473 · 4 · 22.46 |
| qwen3.8:27b-mlx | create | pass · 34682 · 3 · 14.06 | pass · 27970 · 4 · 14.68 |
| qwen3.8:27b-mlx | update | pass · 34468 · 3 · 7.85 | pass · 27551 · 4 · 10.22 |
| qwen3.8:27b-mlx | media | pass · 23045 · 2 · 17.69 | pass · 26597 · 4 · 32.61 |
| qwen3.8:27b-mlx | edit | pass · 34827 · 3 · 10.65 | pass · 40885 · 6 · 25.03 |
| qwen3.8:27b-mlx | resume | pass · 69214 · 6 · 10.24 | fail · 15775 · 3 · 5.97 |

## Limits and follow-up

This is one observation per cell, with no randomized order or repeated samples.
Compilation and independent frontend tests overlapped part of the campaign;
wall-clock values therefore are observations, not isolated latency estimates.
MLX observations are not evidence of determinism and do not justify enabling
progressive loading for a model based on one apparently faster cell. GGUF uses
the runner's greedy settings and fixed seed; those do not guarantee bitwise
reproducibility on Metal. No aggregate latency or token saving is claimed.
[src: file: backend/src/agents/runner.rs]
[src: file: docs/research/native-tool-catalogue-2026-09-22.json]

The frozen executable predates `b580bc2`, which fixes missing JSON commas in the
reachable-catalogue size estimate. That small undercount was caught by the full
backend suite; the reinforced test now checks exact bytes across all four family
loads. The campaign was not selectively rerun after this fix. It also predates
the separate own-room `disc_read` fix; neither change is retroactively claimed
as part of the measured executable.
[src: commit: b580bc2] [src: commit: 66cab1c7]

The initial declared surface changed from 15,161 to 15,203 bytes with Quick Prompt
support (+42); the full surface changed from 37,914 to 39,841 (+1,927), and the
`automations` family from 4,804 to 6,731 bytes (+1,927). These are serialized JSON
bytes, not token estimates. The pre-Quick-Prompt baseline is reconstructed from
`bf4ad66e` by removing its four new declarations and restoring the old family
index text; the ignored static measurement test makes that procedure explicit.
[src: file: backend/src/api/agent_quick_prompt_bench.rs]

The earlier tiered benchmark used a substitute dispatcher and could not catch
native `tools_load` being unreachable. Its positive performance claims are
withdrawn as evidence for the production path. The native dispatcher regression
now executes every real family load. At the time of this campaign, LiteLLM
validation remained pending: sandboxed and host-side attempts failed in DNS.
After the VPN was restarted later that day, a separate final-commit replay
passed seven native scenarios on LiteLLM and seven on Ollama. Those new
observations do not alter the historical cells above.
[src: file: docs/research/native-qp-litellm-ollama-2026-09-22.md]
[src: file: backend/src/api/agent_quick_prompt_tests.rs]
[src: file: docs/research/local-agent-mechanisms-2026-09-21.md]
