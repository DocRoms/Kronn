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

---

## Guiding principle for promoting anything here into the core

Before moving an RFC into `agents-md-format-v1.md`, it must pass three tests:
1. **Implemented + tested** (no vocabulary the runtime can't honour).
2. **Explainable in one sentence** to a new adopter.
3. **Pays for its own cognitive cost** — measured against real large-doc pain, not hypothetical.

If it doesn't pass all three, it stays here.
