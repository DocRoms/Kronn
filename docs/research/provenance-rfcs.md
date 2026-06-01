# Kronn provenance : research & proposed extensions (RFCs)

> **Status: research / non-binding.** This document holds the *advanced* and *future*
> semantics deliberately kept OUT of the core convention
> ([`docs/conventions/agents-md-format-v1.md`](../conventions/agents-md-format-v1.md)).
>
> Nothing here is enforced. The core spec stays small, stable, and adoptable in 30 seconds;
> this is where the stronger ideas live until they earn their way into a future major version
> (with implementation + tests). Splitting the two is intentional — a brilliant system that
> exhausts its users dies fast. Keep the core simple; experiment here.

These notes came out of a multi-agent design review (Claude Code + Codex + the maintainer,
2026-05-27). They are written as proposals, not contracts.

---

## RFC-1 — Citation status lifecycle

The core spec records existence + bounds. This RFC proposes a richer *freshness* model: a
citation's declared status degrades monotonically over its lifetime.

```

verified → stale → degraded → invalidated
```

| Status | Meaning | Typical trigger |
|---|---|---|
| `verified` | Checked and correct at audit time | Fresh audit / explicit re-verification |
| `stale` | Section `audit` date older than the freshness window — may have drifted | Time elapsed (≥ 6 months) |
| `degraded` | The cited file still exists but the line moved / context changed | Code refactor touching the cited path |
| `invalidated` | The cited file/line no longer exists, or the claim was superseded | File deleted, claim contradicted, learning retired |

A status only moves DOWN automatically; promotion back to `verified` requires an explicit
re-verification. This mirrors the "human authority outranks AI" rule — degradation is
automatic, restoration is deliberate.

**Why it's out of core (for now):** transitions need tooling (a revalidation pass), edge cases,
and UX. Until that ships it would be vocabulary the system can't honour — exactly the
over-claiming the core spec must avoid. Candidate runtime: the 0.9.0 staleness revalidation
cron.

### Semantic status vs runtime-validated (the distinction this RFC rests on)

- **`semantic_status`** = a *claim by the author*. Text the writer typed; no guarantee.
  "I believe this is still fresh."
- **`runtime_validated`** = a *fact established by Kronn*. The backend actually resolved the
  path/line and recorded the result. "Kronn checked it and it exists."

A `semantic_status: verified` written by an agent is NOT a `runtime_validated: Verified`.
Only the backend can set the second. Conflating them is the false-confidence trap. The core
spec keeps only the minimal honest version of this ("verified means exists, not true"); the
full two-layer model belongs here until the lifecycle is enforced.

---

## RFC-2 — `claim_id` references

An optional stable identifier for claims that need to be referenced, updated, or invalidated
over time — the hook continual-learning needs to retire or supersede a specific learning.

```markdown
- API base path is `/api` [src: file: backend/src/lib.rs:440][claim:api-base-path-v1]
```

**Grammar.** `[claim:<slug>]` placed immediately after the `[src: …]` marker on the same line
(no space between the two brackets):

- `<slug>` matches `[a-z0-9][a-z0-9-]*` (lowercase alphanumerics + hyphens, starts with an
  alphanumeric). No spaces, no uppercase, no underscores — URL-safe and greppable.
  `[claim: id]` (space) or `[claim:My_ID]` are invalid.
- Unique within the file; SHOULD carry a version suffix (`-v1`, `-v2`) when a claim is
  intentionally superseded rather than edited in place.

| Condition | Proposed obligation |
|---|---|
| Learning, architecture decision, policy statement | **REQUIRED** — enables targeted invalidation/update in continual-learning flows |
| Ordinary factual assertion | **OPTIONAL** — the tuple `(file, section, [src:] marker)` already locates it |

**Why it's out of core:** mandatory claim ids on every assertion would re-introduce exactly
the verbosity the core spec fights. Keep it optional + reserved for referenceable claims;
promote to the core only if continual-learning proves the need at scale.

---

## RFC-3 — Source-map aliases

`[src: file: backend/src/lib.rs:440]` repeated 100× is heavy in large docs. Aliases compress it:

```
[source-map] api = backend/src/lib.rs
...
[src: api:440]
```

**Required invariants (without these it becomes a staleness nest, so it does NOT ship without
them):**
- alias resolved to a canonical path at lint time;
- undefined alias = error;
- redefined alias = error;
- unused alias = warning;
- an `expanded` audit mode resolves all aliases before export, so auditors never chase
  indirection.

**Why it's out of core:** an indirection layer that can itself drift. Only worth it once docs
are large enough that verbosity is a real pain — measure first.

---

## RFC-4 — Runtime freshness enforcement

Promote declared `stale` / `deprecated` semantic statuses (RFC-1) to actual lint signals once
the system can verify them: a revalidation pass re-runs `verify_source_marker` on stored
citations, flags `OutOfBounds`/`NotFound` that appeared since the last audit (= code drift),
and surfaces a re-validation prompt. Candidate: 0.9.0 cron.

---

## RFC-5 — Claim propagation / trust graph (open question)

The strongest and least-defined idea: if claim B is derived from claim A, invalidating A should
propagate a `needs-review` to B. This edges toward a distributed truth graph — powerful, but a
large surface (maintenance, UX, scale). Explicitly parked as research; not a near-term target.

## RFC-6 — Local NLI faithfulness layer ("Niveau 2", zero-token)

> ⚠️ **Needs expert validation before any decision.** The model/distribution claims below are an
> engineer's sketch, not an ML specialist's verdict — validate with a dedicated NLI/ML expert panel
> (min 5 distinct experts + consensus + an anti-hallu fact-checker) before committing.

The core spec + RFCs 1–5 verify that a citation *exists* (file/line resolves, in bounds). None
verify that the cited content **actually supports** the claim. This RFC closes that gap with a
**local Natural-Language-Inference cross-encoder** (zero token, full backend) scoring
`claim ⊨ source` → {entailment, neutral, contradiction}; contradiction / weak-entailment → flag.
It is the zero-token form of the "Niveau 2" the anti-hallu program reserved for an LLM judge.

**Empirical motivation (2026-05-31 deep re-pass).** A 4-conversation forensic re-pass (one linguistic
expert per persona, reconciled against the machine verdict) on a real Symfony project produced a
clean motivating case: the *only* genuine hallucination in the corpus was a **content-semantic**
claim — an agent asserted "the CSS `nth-of-type(even)` already in place alternates the background",
when `grep` confirms **no such rule exists** in the stylesheet. Niveau-1 cannot see it: the sentence
carries no `[src:]` / inline anchor (nothing to resolve), and even if it did, existence+bounds never
compares the prose against the file's *content*. This is precisely the `claim ⊨ source` gap RFC-6
closes, and a ready seed for the de-risking pair-set below (a real `(stylesheet, "nth-of-type rule
exists")` → expected **contradiction**). The same re-pass also confirmed `.xlf` **key** existence and
verbatim-quote fidelity stay out of Niveau-1's reach for the same structural reason.

**What it adds.** "The file exists" → "the file says that." Mechanical verification (Niveau 1)
stays the authority for **code-anchored** claims (does `foo.rs:42` exist?); NLI is the authority for
**NL↔NL faithfulness** (does the agent's prose answer follow from an NL source). Clean division of
labour — complementary, not competing.

**Two caveats.**
1. *Distribution.* HHEM / DeBERTa-NLI are trained on natural language, not code. Mitigated by
   scoping NLI to the agent's **NL response** (hypothesis = NL ✓) against an **NL premise** (docs,
   conversation context, another agent's output, or `evidence[]`). A raw-code premise stays
   Niveau-1's job.
2. *Decomposition.* True atomic-claim splitting needs an LLM (which we avoid). Syntactic
   sentence-split (already done by the Niveau-0 linter) is the free ceiling → sentence-level NLI,
   fine for a non-blocking pill, not for a hard gate.

**Highest-value placements** (most → least tractable):
1. **Faithfulness gate on continual-learning** (`learning_propose(claim, evidence[])` → verify
   `claim ⊨ evidence[]` before persisting): `evidence[]` is a clean, pre-decomposed NL premise and
   the stakes are maximal (a bad claim in memory contaminates everything). This is the *teeth* of
   the continual-learning safeguard.
2. **Faithfulness of the agent's NL reply** against the doc/context it used (prose↔prose).
3. **NLI on cited content** (Niveau-1 resolves `[src:]` → fetch chunk → NLI), best when the chunk
   is a doc rather than code.

**Stack.** ONNX export + `ort` (fast to integrate, native per-OS dep → real packaging friction for
the Tauri desktop / Docker targets) vs `candle` (pure-Rust, lighter packaging, heavier model
porting). HHEM-2.1-Open (Apache-2.0, Flan-T5-based, faithfulness-tuned) vs a vanilla
`cross-encoder/nli-deberta-v3` (cleaner 3-class ONNX export). Zero *token*, not zero *cost* (CPU,
latency, a few-hundred-MB model to distribute).

**De-risking before any Rust.** A throwaway Python proto over ~30 real `(source, claim)` pairs from
Kronn — including code claims and NL claims — to measure whether the model actually discriminates.
If it fails on code, NLI is confined to NL↔NL (per the division of labour above).

**Sequencing.** Not near-term. Precondition: the Niveau-0/1 telemetry (added 0.8.7) must first show
that claims are *cited often enough* for NLI to have a premise to chew on. Then it lands as the
continual-learning safeguard's verifier, not as a standalone "score every message" pass.

**Proto result (2026-05-31 — see `nli-proto-findings.md`).** A throwaway proto over **255 real
pairs** (mined from 32 Kronn conversations + 82 verified subtle-hallucination adversarials + gold
cases, 3-judge labels at 84% unanimity) tested two local multilingual NLI models. Conclusion: a
local NLI is **NOT reliable as a standalone gate** here — both models under-recognize the loose
descriptive entailments agents actually write (acc 0.34–0.42; would false-flag ~85% of legitimate
claims). BUT the stronger model (mDeBERTa) caught **all gold hallucinations**, incl. the
`nth-of-type` case at ent_p 0.004 vs 0.972 for a true match — real signal **at the extremes**.
Net: ship the `FaithfulnessChecker` trait with **LLM-judge as the quality backend**, local NLI as
an optional cheap tail-signal (`ent_p` < ~0.1), `off` by default — and **posture B (informative,
human-gated), never auto-blocking**, is the data-backed call. CPU latency (~4 s/pair) confirms NLI
must be async/opt-in, not per-message.

---

## Guiding principle for promoting anything here into the core

Before moving an RFC into `agents-md-format-v1.md`, it must pass three tests:
1. **Implemented + tested** (no vocabulary the runtime can't honour).
2. **Explainable in one sentence** to a new adopter.
3. **Pays for its own cognitive cost** — measured against real large-doc pain, not hypothetical.

If it doesn't pass all three, it stays here.
