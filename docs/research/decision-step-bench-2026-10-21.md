# Decide step — `/v1/systemone` contract and confidence thresholds (2026-10-21)

Measured for KT-1367 on macOS, Ollama 0.40.6, model `clef-flash:9b`, with the
script next to this note (`decision-step-bench-2026-10-21.py`, run against a
local Ollama). Nothing here was simulated: every number is a real call.

## Contract

`POST /v1/systemone` with `{"model", "state", "questions": {<id>: {...}}}`.
Several questions go in one call.

| kind | request | answer |
|------|---------|--------|
| `noul` | `{"type":"noul","instructions"}`; optional `"criteria": {"true": …, "false": …}` | `{"noul": <probability of true>}` |
| `choice` | `"criteria": {<option>: <meaning>}` (an object, 2–26) | `{"choice", "probabilities": {<option>: p}, "confidence"}` |
| `score` | `"criteria": [<level>, …]` (an array, 2–10, lowest first) | `{"score": <expected level, 0-based>, "legend", "probabilities": {"0": p, …}, "confidence"}` |

Wrong shapes answer HTTP 400 with a sentence (`choice criteria must be an
object`, `score criteria must be an array of 2–10 levels`).

Latency: 8.3 s on the first call (model load; 25 s seen when Ollama was cold),
then 0.13–0.34 s per call.

## Bench (30 real Kronn cases)

Cases: test and lint outputs (merge or not), agent reports (continue or not),
tier routing of a task, review verdicts, risk of a change. Expected answers
written before the run.

- Accuracy: **28/30**.
- Misses: a green vitest run with lint warnings under the cap was read as "not
  mergeable" with a high confidence (noul p = 0.104, confidence 0.79); a typo
  fix in a translation routed to `economy` instead of `local` (choice
  confidence 0.25).
- Confidence scales differ per kind, so one global threshold cannot work:
  - `noul` has no confidence of its own; Kronn uses `|2p − 1|`. Correct answers:
    0.88–0.98.
  - `choice`: correct 0.38–0.90, the miss 0.25.
  - `score`: correct answers only reach 0.32–0.46.

## Defaults chosen

| kind | default `min_confidence` | effect on this bench |
|------|--------------------------|----------------------|
| `noul` | 0.85 | keeps every correct answer, escalates the 0.79 miss |
| `choice` | 0.35 | keeps every correct answer, escalates the 0.25 miss |
| `score` | 0.30 | keeps every answer (all correct) |

With these defaults the 2 misses become `<id>_unsure` + `low_confidence`
(escalated) and the 28 correct answers stand. 30 cases is a small sample: the
thresholds are a starting point, overridable per step (`decide.min_confidence`),
and worth re-measuring on each new decision model.

## Re-run 2026-11-10 (after merging into integ/0.15.3)

Same script, same model (`clef-flash:9b`, local Ollama): 28 / 30 correct, the
same two misses, thresholds table unchanged. Cold first call 4.66 s, warm calls
0.43-0.58 s. The defaults above stand.

## HTTP decision provider (OpenRouter): deferred

The step calls the local Ollama decision model only. No OpenRouter adapter is
written: the ticket requires its call contract to be read and measured with one
real call first, and that call has not been made (the worker sandbox of the
2026-11-10 merge cannot reach openrouter.ai). Writing the adapter before the
measurement would be a guess. Next step: one real call to a decision model
(Perplexity Decider, Jev or Clef) through a connection's `decision_model` slot,
then one adapter behind the same `Decide` step.
