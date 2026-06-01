# NLI faithfulness proto — empirical findings (Gate 2 / RFC-6 de-risk)

> **2026-05-31.** Throwaway proto answering the only blocking question for 0.9.0's
> Gate 2 (`claim ⊨ evidence` faithfulness): **can a local NLI model discriminate
> faithfulness well enough to be the `FaithfulnessChecker`?** Answer below, on a
> real, hard, judge-labeled eval set.

## Method

- **Eval set: 255 pairs.** 173 base + 82 hard-adversarial.
  - *Base (173)*: mined from **32 real Kronn conversations** across 3 projects
    (front_euronews, DOCROMS_WEB, Kronn). Premise = the **real content** of the
    cited file slice; hypothesis = the agent's claim. Labeled by **3 independent
    LLM judges + majority vote** (87% keep-decision unanimity, 84% label
    unanimity → trustworthy ground truth). Junk (pointers, process-notes) filtered.
  - *Hard adversarial (82)*: subtle plausible-but-unsupported variants of real
    entailment claims (one fact changed: a method/route name, a value, a
    behavior — never blunt negation), generated then **independently verified**
    by a separate judge. This is the dangerous class — what Gate 1 cannot see.
  - *Gold (4)*: hand-built, incl. the real `nth-of-type(even)` hallucination
    found in the persona re-pass (premise CSS has no such rule).
- **Models** (both multilingual — the claims are French): a fast
  `multilingual-MiniLMv2-L6-xnli` and an accurate `mDeBERTa-v3-base-xnli`.
- **Headline metric**: *flag-recall* (when gold ≠ entailment, does the model
  avoid a green "entailment"?) + *entail-kept-green* (does it keep true
  entailments green, i.e. not false-alarm legitimate claims?).

## Results

| Model | acc(3-class) | flag-recall | entail-kept-green@0.5 | gold caught | latency (CPU) |
|---|---|---|---|---|---|
| MiniLMv2-L6 (fast) | 0.34 | 0.95* | **0.02** | nth-of-type **MISS** (ent_p 0.408) | ~1.9 s/pair |
| mDeBERTa-v3-base | 0.42 | 0.83 | **0.15** | **4/4** (nth-of-type ent_p 0.004) | ~3.9 s/pair |

\* MiniLM's 0.95 flag-recall is largely an **artifact**: it under-predicts
entailment for almost everything, so it "flags" by default. The 0.02
entail-kept-green exposes it — it would amber-flag ~98% of *legitimate* claims.

## What this means (the de-risk verdict)

1. **Local NLI is NOT reliable enough as a standalone auto-gate** on this domain.
   Both models **under-recognize loose-but-valid entailments** — agents write
   descriptive claims ("this file is the page's main content") that aren't tight
   logical entailments, so NLI returns neutral → it would false-alarm the
   majority of honest claims. acc3 ≈ 0.34–0.42 (around/below the majority
   baseline). **This kills posture A (auto-blocking) decisively.**
2. **But there IS real signal at the extremes.** The stronger model (mDeBERTa)
   caught **all 4 gold hallucinations** — crucially the `nth-of-type` case that
   Gate 1 (existence) is structurally blind to, at ent_p **0.004** vs **0.972**
   for a true match. Clear contradiction/clear support separate cleanly; the
   noise is in the loose middle.
3. **The distribution-mismatch caveat (RFC-6) is confirmed on code premises.**
   The fast MiniLM was fooled by the subtle code hallucination (`nth-of-type`
   0.408); only the heavier model handled it. NLI on code is unreliable unless
   the model is strong — and the strong model is **slow on CPU (≈4 s/pair)**,
   impractical for per-message real-time use without GPU/quantization.

## Architecture consequences (feed back into the 0.9.0 spec)

- **Posture B is the right call** (validated, not just chosen): an *informative*
  faithfulness signal a human reads, never an auto-block. The data shows an
  auto-gate would be both noisy (false-flags valid claims) and incomplete.
- **Default checker = `llm` (LLM-judge), not `nli`.** The 3-judge LLM labels hit
  84% unanimity (the quality bar); local NLI hit acc 0.34–0.42. The
  `FaithfulnessChecker` trait (`llm` | `nli` | `off`) is vindicated — ship with
  **`off` by default** (until tuned), **`llm`** for quality when enabled, and
  **`nli`** only as an optional *cheap pre-filter / confidence hint* on the
  extremes, never the authority. CPU latency makes `nli` async/opt-in anyway.
- **Use `ent_p` as a soft hint, not the argmax label.** Separation lives at the
  tails: a very low `ent_p` (<~0.1) against a resolved source is a strong
  "double-check this" signal; the mushy middle should stay silent rather than
  amber-spam.

## Reproduce

`/home/priol/nli_backup/` holds the eval (`nli_eval.jsonl`, `nli_eval_hard.jsonl`),
the mining/labeling/assembly scripts, and the recovered judge votes. Runner:
`nli_proto_v2.py` (CPU venv, `UV_TORCH_BACKEND=cpu`). Models via HuggingFace,
`HF_HUB_DISABLE_XET=1` (the xet backend hangs in this WSL env).
