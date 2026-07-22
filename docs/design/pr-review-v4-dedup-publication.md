# Design spec — PR Review v4: dedup & publication policy

> Status: **DRAFT / DESIGN (pre-code)**. Product of a ClaudeCode × Codex debate, 2026-07-22 (disc `72806022-1669-4638-9c6d-ca7971937e8e`), grounded in an empirical analysis of the last 30 PRs of `Euronews-tech/front_euronews`. To be challenged line by line before any change to the WF.
>
> WFs involved: cron parent `f120ca51` → child "PR Review — single PR (PROD v3.2)" `0a792084` → skill `custom-pr-review-rubric-v3-2`.

## 1. Goal & diagnosis

The automated review is **accurate but too verbose/repetitive**. The problem is not detection (serious verification at HEAD, anti-injection, legitimate blockers well raised, no over-blocking) but the **publication policy**. Measured findings (30-PR sample):

- **Full re-post** of the general comment on every commit, even an unchanged APPROVE, in an ever-**growing** version (PR 1817/1824/1762).
- General comment that **duplicates** the inline threads (`## Summary` / `## Ticket compliance itemized`) — PR 1720/1854.
- **Cosmetic dedup**: "I won't re-raise" followed by a full re-statement of others' points (PR 1817/1865).
- Inline "carry / re-checked at HEAD" with no new info (PR 1806).
- Inline verbosity floor (~656 chars avg, never < ~350) where a 👍 reaction would suffice.
- **Acquiescence replies** instead of disagreement (PR 1806, 18/21 replies).
- **Verdict oscillation** at constant facts (PR 1824/1865).

**Structural** WF defects (not a mere rubric tweak):

- `skip_check` (step 9) only guards "already reviewed at THIS head"; otherwise `post` (step 16) is **unconditional** → a general comment is mechanically produced on every commit.
- `write_ctx` (step 13): `cs=[c for c in allc if login!=me]` → the bot **does not see its own inline comments** ⇒ no anti-repost possible; reviews are trimmed (`id`/`commit_id` dropped, body capped at 1500).
- `reason` (step 14): the prompt forces an action on **every** third-party comment (cause of the over-activity), and the TypedSchema carries **the effects to publish**, not facts → post cannot arbitrate anything.
- `event` and `verdict` are two free fields with no coherence invariant; no budget; `on_invalid: Continue` **before a write**.

## 2. Target architecture — 3 layers (detection / publication separation)

1. **Skill (rubric)** — decides *what a legitimate finding is*. Keeps all current rigor. Produces complete **candidate findings**, unconcerned with publication.
2. **`reason` step (typed schema)** — emits **facts**, one object per finding. **Never** the hash, never the GitHub effects.
3. **Deterministic planner/validator** (post-`reason`, before any write) — canonicalizes + hashes the fingerprint, reconciles against history, decides `publicationAction`, applies the invariants, emits markers + ledger. It is the only place that writes.

## 3. Fingerprint (fp) contract

The model emits **semantic components**; the pipeline canonicalizes (ordered JSON) then hashes. **Only the hashing is deterministic** — the stability of the components remains a model judgment (→ under test, see §9).

```
fp = hash(canonical_json({
  rule_id,          // CLOSED and versioned taxonomy (secret, injection, n+1,
                    //   dead-code, missing-test, a11y, ...). No free text.
  normalized_path,  // diff path, application/... prefix preserved
  semantic_scope,   // symbol / function / route / config key — NOT the line number
  subject,          // precise resource or behavior affected
  failure_mode      // concrete consequence
}))
```

- The **line** and the **evidence snippet** stay **out of the fp**: they serve anchoring and publication, not identity (resilience to rebase / HEAD drift).
- Anti-merge guard: an fp of `(path, symbol, missing-test)` that is too coarse would merge two distinct defects → `subject` + `failure_mode` are **mandatory**.
- **Minor change to the same problem**: exact-fp first; otherwise semantic match to an older fp via an **explicit** `same_issue_as` field + evidence. This path is **rare, auditable, tested** — never a silent fuzzy match by the pipeline.

## 4. Target `reason` step schema (facts, not effects)

One object per finding, plus the input context `OWN_COMMENTS` (see §6). Fields per finding:

- fp components: `rule_id, normalized_path, semantic_scope, subject, failure_mode`.
- `severity` (with the rubric's hard triggers).
- `evidence`: `{ path, line, side, snippet }` — out of the fp, used for anchoring.
- `newEvidence: bool` — has a new fact appeared since the last pass on this fp?
- `same_issue_as?: fp` + `same_issue_reason?` — see §3.
- For a point overlapping an existing comment: `covered_by_comment_id`, `coverage_reason`, `confidence` (0-1), `disagree: bool`.
- `scope: inline | transverse` — `transverse` = no single inline anchor.

Verdict: a **single** `verdict ∈ {approve, comment, request_changes}` field; the GitHub `event` is **derived** deterministically (coherence invariant, see §5). `on_invalid`: **Fail/retry**, never `Continue` before a write.

## 5. Deterministic planner — mechanical invariants

The planner maps each finding to `publicationAction ∈ { NEW_INLINE, REACT, REPLY_DISAGREE, NOOP }` and applies, **without depending on the model's goodwill**:

- `covered_by_comment_id` present ⇒ **REACT** (👍/🚀) — **prose forbidden** (neither inline nor reply).
- `REPLY_DISAGREE` allowed **only if** `disagree=true`. A reply without disagreement is rejected.
- same `fp` already published by the bot (ledger + marker) **and** `newEvidence=false` ⇒ **NOOP** (anti-repost).
- **General comment**: never a summary of inline. Absent by default; 1 line on `approve`; on `request_changes` = "N new blockers — see the targeted comments"; `scope=transverse` exception with no inline anchor = 1-2 lines, **carrying its own fp**, forbidden if it copies inline.
- Coherence `verdict → event` (e.g. `request_changes → REQUEST_CHANGES`), checked before write.
- **Verdict stable at constant facts**: if there is no `newEvidence` overall, the previous verdict is re-emitted as-is (anti-oscillation).
- Budget: **format validation**, never byte-truncation (see §7). Overflow ⇒ reject/repair.

## 6. Idempotence — markers + append-only ledger + completeness gate

Two complementary traces (GitHub carries the **effect**, the ledger proves the **intent**):

1. **Invisible marker** in each inline comment posted by the bot: `<!-- kronn-review:v1 fp=… head=… ev=… -->`. Survives GitHub rendering, restart-proof, idempotency key at the point of publication.
2. **Append-only ledger** (from v1) mirroring it: `{ repo, pr, fp, evidence_hash, head, github_review_id|comment_id, action, verdict, created_at }`.

> **Why the ledger from v1 (Codex correction, accepted)**: markers alone **do not prove non-loss**. A deleted/edited comment makes a GitHub-based reconstruction blind — it sees a "complete" paginated list with the element missing, without knowing it ever existed. `edited/deleted ⇒ INCOMPLETE` is **undetectable without a second trace**.

**3-state completeness gate**, evaluated on every pass:

- `COMPLETE` — full GitHub fetch (all Link-header pages) **and** consistent with the ledger ⇒ dedup + write allowed.
- `INCOMPLETE` — truncated fetch, or GitHub↔ledger divergence (expected marker/comment missing) ⇒ **shadow-only, zero write**, explicit reconciliation + operable error. "Unknown history" **never** becomes "nothing new".
- `ERROR` — hard fetch/parse failure ⇒ clean abort.

**Legacy bootstrap**: `ledger_started_at` + cutoff `head`. Prior comments (without markers) are `LEGACY` → handled by semantic matching, **without blocking all PRs forever**. The strong guarantee begins after the cutoff.

**WF impact**: `write_ctx` must **stop filtering out the bot's comments** and expose `OWN_COMMENTS` separately, keeping `id / commit_id / original_commit_id` (currently dropped). Parsing of the bot's own comments happens **before** they are split from the context.

## 7. Publication budget

- No arbitrary truncation: we **validate a format** then reject/repair if it overflows.
- General comment absent by default; `approve` = 1 line; `request_changes` = 1 line + pointer to the inline (the detailed justification **lives in the inline**).
- A non-anchorable blocker ⇒ general-only allowed with an **expanded, structured** budget, never byte-cut.

## 8. Escalation on insufficient confidence (never a silent NOOP)

Codex correction, accepted: **insufficient confidence** on a point (especially a potential blocker) **never** becomes a silent NOOP. It triggers a **mandatory escalation path**:

1. 2nd independent reasoner / `reasoning` tier.
2. If disagreement or uncertainty persists → **human gate**.

The planner **writes nothing until resolved**, but **the alert survives** (operable escalation). The worst failure to avoid = a silently dropped blocker.

## 9. Shadow / replay harness & parity gate (BEFORE any PROD change)

- **Finding-level annotated corpus** on the 30 PRs + synthetic cases: for each finding, label `new | already-covered | resolved | disagreement` and mark the **real blockers**.
- The new pipeline runs in **shadow** (zero publication), its output compared to what the current WF posted.
- **Separate metrics** for detection vs publication. Switch to PROD **only if**:
  1. **blocker recall = 100%** (non-negotiable — no legitimate blocker lost),
  2. **0 duplicates** (inter-pass + inter-reviewer),
  3. **0 acquiescence replies**,
  4. **0 verdict oscillation** at constant facts,
  5. budget respected.

## 10. Sequencing

1. Freeze §9 (corpus + metrics) — **before** touching the WF.
2. Implement `reason` schema (§4) + planner (§5) + traces (§6) + escalation (§8) in shadow.
3. Reach parity (§9).
4. Switch to PROD.
5. **Only then**: Qwen3/Ollama takes over only the sub-steps that reached parity, with paid escalation on ambiguity / schema failure.

## 11. Guarantee boundary (explicit structure — required by Codex)

**(a) Mechanical guarantees** (deterministic, carried by the planner): fp hashing; REACT-if-covered; reply-only-if-disagree; NOOP anti-repost on fp+`!newEvidence`; general never a summary of inline; `verdict↔event` coherence; 3-state completeness gate; budget validation.

**(b) Evaluated model judgments** (non-deterministic, under corpus/replay): choice of fp components; "my finding = human comment" mapping (`covered_by_comment_id`); `newEvidence`; `disagree`; `same_issue_as`.

**(c) Escalation procedures**: insufficient confidence → 2nd reasoner/tier → human gate; GitHub↔ledger divergence → INCOMPLETE + reconciliation; `on_invalid` → repair/retry/abort.

**(d) Limits not provable by the corpus**: marker loss via GitHub deletion/edit (mitigated, not eliminated, by the ledger); quality of semantic similarity on human comments; completeness of the annotated corpus itself.

---

*Open items out of scope: where to persist the ledger (dedicated table — the foreach done-set is unsuitable: attached to a `workflow_run`, appendable only while `Running`) — to be settled at implementation; relation to TD-20260722 (project-scoped skills/automation) and TD-20260630 (Link-header pagination, already mitigated by `per_page=100`).*
