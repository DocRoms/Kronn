# Local Ollama offload and escalation routing

> Status: **DEFERRED DESIGN NOTE** — revisit when Claude-plan pressure or local
> offload becomes a product priority. This note records the agreed direction;
> it does not authorize workflow changes. [src: user: 2026-07-22: request to
> record the Ollama offload plan for later]

## Goal

Reduce paid-agent calls without weakening review, triage, or implementation
quality. Existing deterministic work should remain `ApiCall`, `Exec`, or
`JsonData`; Ollama is for bounded semantic work, not for replacing mechanics
with another model. Kronn already supports explicit per-step Ollama models and
grammar-constrained `TypedSchema` output. [src: file:
docs/operations/ollama-local-models.md:8-31] [src: file:
docs/operations/ollama-local-models.md:64-79]

## Core safety invariant

Escalation must not depend on an optional model-authored `escalate: true` flag.
The router uses the inverse rule:

```text
escalate = NOT(proven_local_eligibility)
```

A result is eligible to stay local only when all required conditions are
positively demonstrated:

- the input is complete and was not truncated;
- the structured output is valid;
- every mandatory coverage check has a status and evidence;
- no unknown, contradiction, or unresolved dependency remains;
- no code/API/document/runtime inspection is still required;
- no hard-risk category applies (security, authentication, consent/privacy,
  migration/data integrity, concurrency, or complex cross-file behaviour);
- the configured local verification passes agree.

Missing fields, uncertain evidence, model disagreement, schema repair failure,
or an unclassified risk all escalate. Silence never means "safe". Deterministic
input heuristics and periodic audited samples are still required because a
model can incorrectly label a check `not_required`.

## Reusable decomposition

```text
deterministic collection and normalization
  -> qwen3:8b extraction / classification
  -> qwen3:30b-a3b or qwen3:32b bounded judgement
  -> deterministic eligibility validator and router
  -> paid agent or human gate only for unresolved/high-risk cases
  -> deterministic rendering and side effects
```

Model choice remains task-specific. Local benchmarks found `qwen3:8b` strong
for bounded extraction/classification and larger Qwen models more suitable for
nuanced review; they also showed that local review can miss a real subtle
cross-file bug even when its formatting and grounding are correct. Therefore a
local reviewer is a pre-pass, not a universal final gate. [src: user:
2026-07-22: discussion 5243cdbc-7189-4e9f-8c54-cd81d3891aea]

## Candidate workflow map

| Current need | Future treatment |
|---|---|
| Jira ticket listing/filtering | Replace agent calls with `ApiCall` + `Exec`; do not use Ollama. |
| Daily briefing synthesis | Ollama 8B for ranking/extraction, then deterministic HTML rendering. |
| PR description drafting | Inject all files/results deterministically, use Ollama for structured prose, then render Markdown mechanically. |
| ToFrame ticket triage | Local pre-triage; escalate whenever code, API, linked-ticket, external-doc, or runtime verification is required. |
| PR review | Local finding pre-pass; escalate subtle, cross-file, high-risk, uncertain, or conflicting findings. Publication remains deterministic and fail-closed. |
| AutoPilot feasibility triage | Local extraction/draft manifest, paid-agent validation for adversarial planning and repository inspection. |
| AutoPilot implementation | Keep the tool-capable agent. Continue expanding the existing zero-agent path for fully `mechanical` manifest items instead. |

This map is a snapshot of the workflow inventory reviewed with Romuald, not a
commitment to migrate every candidate. [src: user: 2026-07-22: workflow
offload inventory review]

## First pilot when work resumes

Create a manual, disabled **ToFrame Triage SHADOW — Ollama** clone:

1. Reuse the existing deterministic fetch/reshape stages.
2. Run a local Quick Prompt with no tools and no write/chain action.
3. Emit a typed provisional verdict, missing facts, product decisions, risk
   tags, and explicit verification requests.
4. Apply the deterministic eligibility rule above.
5. Run the existing tool-capable triage only for escalated tickets.
6. Compare both paths before allowing any local-only result.

The critical metric is zero false "ready to frame" decisions. Also measure
recall of blockers/decisions, hallucinations, verification-checklist coverage,
escalation rate, latency, and disagreement rate.

After this router is proven, reuse the same contract for the higher-impact PR
review shadow pipeline.

## Known implementation caveat

Kronn currently retries a failed local `TypedSchema` step once on the paid
reasoning tier before applying `on_invalid`. A future strict zero-Claude mode
therefore needs an explicit configurable escalation target (`Claude`, `Codex`,
human gate, or none/fail-closed) rather than relying only on the existing
automatic schema-repair policy. [src: file:
docs/operations/ollama-local-models.md:64-79]

