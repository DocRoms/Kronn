# Native catalogue growth check, 2026-09-22

Adding the five workflow tools increased the full catalogue from 39,841 to
42,861 serialized bytes (+3,020, 54 to 59 tools). On the two smallest local
models, all fourteen scenario outcomes matched their earlier full-catalogue
observations: qwen3.5:2b passed 6/7 and gemma4:e2b passed 5/7. This bounded check
found no lost action success; it does not establish general non-regression.
[src: file: docs/research/native-catalogue-growth-2026-09-22.json:1]
[src: file: docs/research/native-tool-catalogue-2026-09-22.json:1]

## Protocol

The same seven prompts, real native executor, temporary databases and exact
persisted checks were retained. Only full mode was run, using
`KRONN_QP_BENCH=1`, `KRONN_BENCH_FULL_ONLY=1`, and
`KRONN_OLLAMA_NUM_CTX_CAP=24576`. The frozen libtest executable has SHA-256
`9b4a6918098fe2e51e9d02ca6efea041384c9cf70dcd5750475bb08819106ba7`.
Its production source is commit `70763655`; the additional test-only selector
skips progressive-mode cells. The host run began at 2026-09-22T11:52:56.537222+00:00
and finished at 2026-09-22T11:54:26.689256+00:00. The initial sandbox attempt
failed to connect to Ollama before producing any inference observation. Both
host benchmark processes exit 101 because their final assertion requires every
scenario to pass; all fourteen measurements were nevertheless completed.
[src: file: backend/src/api/agent_quick_prompt_bench.rs:250-335]
[src: commit: 70763655]
[src: file: docs/research/native-catalogue-growth-2026-09-22.json:1]

This is one observation per cell, in a fixed order, against a historical build.
The new build also includes the intervening fixes, notably saved QP generation
settings. It is not a same-build isolation of declaration cost or a randomized
latency trial. No compilation or test suite launched by this agent overlapped
the host replay; other host activity was not controlled. Child QP dispatchers
and image generation workers remain unstarted: those cells verify job creation,
not completed inference or generated media. The prior campaign's other scoring
limits still apply.
[src: file: backend/src/api/agent_quick_prompt_bench.rs:285-323]
[src: file: docs/research/native-tool-catalogue-2026-09-22.md:1]

## Observations

| Model | Scenario | Result | First prompt tokens | All prompt tokens | Turns | Seconds |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| qwen3.5:2b | list | pass | 12224 | 24508 | 2 | 3.89 |
| qwen3.5:2b | run | pass | 12228 | 36968 | 3 | 1.68 |
| qwen3.5:2b | create | pass | 12250 | 37039 | 3 | 1.57 |
| qwen3.5:2b | update | pass | 12229 | 36936 | 3 | 1.07 |
| qwen3.5:2b | media | pass | 12241 | 24692 | 2 | 1.48 |
| qwen3.5:2b | edit | pass | 12241 | 37320 | 3 | 1.84 |
| qwen3.5:2b | resume | fail | 12232 | 24512 | 2 | 0.85 |
| gemma4:e2b | list | pass | 11556 | 23168 | 2 | 9.02 |
| gemma4:e2b | run | pass | 11560 | 34931 | 3 | 8.92 |
| gemma4:e2b | create | pass | 11581 | 23323 | 2 | 7.66 |
| gemma4:e2b | update | fail | 11562 | 34919 | 3 | 10.49 |
| gemma4:e2b | media | pass | 11573 | 23351 | 2 | 16.29 |
| gemma4:e2b | edit | fail | 11574 | 35313 | 3 | 17.42 |
| gemma4:e2b | resume | pass | 11566 | 23164 | 2 | 6.31 |

Every first request grew by 841 reported tokens on qwen3.5:2b and 793 on
gemma4:e2b compared with the corresponding earlier full-mode cell. These are
provider-reported request counts; the 3,020-byte declaration increase is a
separate measurement. No token or latency saving is claimed.
[src: file: docs/research/native-catalogue-growth-2026-09-22.json:1]
[src: file: docs/research/native-tool-catalogue-2026-09-22.json:1]

The failures are retained, not repaired in scoring. Qwen inspected blocked tasks
with `task_list` rather than calling `agent_resume_status`. Gemma changed
`prompt_template` rather than `description` in the update cell, then claimed
success. In the edit cell it passed a literal backslash-n in `new_string`, so
the resulting file did not have the requested bytes. Both tool calls succeeded;
the persisted-result assertions correctly rejected the outcomes.
[src: file: docs/research/native-catalogue-growth-2026-09-22.json:1]

These observations support including the new authoring tools with their measured
cost and the documented local-model limitations. They do not replace the still
missing live LiteLLM validation.
