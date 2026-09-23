# Native media launch without a supplied connection id

On 2026-09-22, six local models each created a media job through the real
native tool executor from the same request, with no connection identifier:

> Lance la génération d'une image de phare au lever du jour. Confirme seulement son lancement, sans attendre le résultat.

All seven emitted `media_generate` calls omitted `connection_id`. None of the
six responses asked the operator to supply it. Each successful cell required
both reaching the handler and finding the persisted job in the isolated DB.

| Model | Job persisted | Turns | Recorded seconds | First / total input tokens |
|---|---|---:|---:|---:|
| qwen3.5:2b | yes, after correcting an invalid resolution | 3 | 6.145 | 12,217 / 37,176 |
| qwen3.5:4b | yes | 2 | 8.405 | 12,217 / 24,661 |
| gemma4:e2b | yes | 2 | 11.123 | 11,548 / 23,284 |
| gemma4:e4b | yes | 2 | 15.958 | 11,548 / 23,292 |
| gemma4:12b-mlx | yes | 2 | 16.093 | 11,552 / 23,302 |
| qwen3.8:27b-mlx | yes | 2 | 51.136 | 12,223 / 24,663 |

The first 2b request used `resolution: "1024x768"`; the API refused it with
`resolution must be one of 480p, 720p, 1080p`. The model retried with `720p`
and created the job. The raw result retains this refusal: the outer tool's
`ok: true` means the handler returned an API envelope, not that its inner
request succeeded. Persistence is the success criterion.

## Scope and reproducibility

- Source: `7d1656f187040996f768d01e0dab7acd6dbb4d1f` (test-only extension
  above the delivered implementation and release-note corrections).
- Frozen test binary SHA-256:
  `fa4e8863e000f44a35222178db6309d2fa75a4c2d58a80e5dd10bb0b57ca41f7`.
- UTC interval: 16:07:41–16:09:33; six sequential calls, one observation per
  model. This is not a success-rate estimate or a before/after comparison.
- Real Ollama inference, full native catalogue, `num_ctx` capped at 24,576.
  The fixture has one configured image provider. Media workers are stopped:
  these are pending jobs, with no paid media generation or image-quality test.
- Input counts include system instructions, tools and accumulated history;
  they are not an incremental tool cost or a monetary estimate.
- No retained pre-fix campaign exists. The initial six-model campaign did not
  include the 35b; the follow-up below covers that model.
  The earlier seven-scenario campaign explicitly supplied `saved-provider`
  and cannot establish the identifier-free behavior tested here.

Reproduce each model with `KRONN_QP_BENCH=1`, `KRONN_BENCH_FULL_ONLY=1`,
`KRONN_BENCH_SCENARIO=media`, `KRONN_BENCH_MEDIA_WITHOUT_ID=1`,
`KRONN_OLLAMA_NUM_CTX_CAP=24576`, `KRONN_BENCH_MODEL=<model>`, and a fresh
`KRONN_BENCH_REPORT=<path>`; leave `KRONN_BENCH_CONFIG` unset. Run:

```sh
cargo test --manifest-path backend/Cargo.toml --lib \
  api::agent_tools::quick_prompt_bench::bench_quick_prompt_native_catalogue \
  -- --exact --ignored --nocapture --test-threads=1
```

[Retained raw observations](media-without-connection-id-2026-09-22.json)
include the exact prompt, emitted arguments, responses, inner errors, timing,
token counts, exit statuses and hashes of the six execution logs.

## 35b follow-up — 2026-09-23

The operator waived the before/after requirement and requested a current-state
check on `qwen3.6:35b-mlx` (room message
`6833b732-f8bc-4ce1-a4fd-d946885dc4a7`). The same frozen binary, prompt,
full catalogue and 24,576-token context cap were used, changing only the model
and report path in the reproduction command above.

The first attempt, at 08:14:51 UTC, could not connect to Ollama from the sandbox
and exited with code 101 before producing an inference cell. The rerun with
authorized network access ran from 08:15:59 to 08:16:35 UTC and passed:

| Model | Job persisted | Turns | Recorded seconds | First / total input tokens |
|---|---|---:|---:|---:|
| qwen3.6:35b-mlx | yes | 2 | 35.851 | 12,223 / 24,644 |

Its only tool call was `media_generate` with
`{"modality":"image","prompt":"phare au lever du jour"}`. The native handler
returned no error and the fixture found exactly one image job for the sole
configured provider. The final response confirmed the launch without asking
the operator for a connection id. Media workers remained stopped.

This extends the retained current-state coverage to seven models, including
the 35b. It does not add a before/after comparison, an image-generation test or
a success-rate estimate. [Raw follow-up evidence](media-without-connection-id-35b-2026-09-23.json)
retains both attempts, the successful response, exact arguments, timestamps,
exit statuses and log hashes.

[src: file: backend/src/api/agent_quick_prompt_bench.rs:1]
