# AGENTS.md tiering — per-section decisions (KT-191)

Every section that was in `docs/AGENTS.md` at the start of KT-191 has a decision
and, when moved, a destination that exists. This file is the record the DoD asks
for, so a future reader can tell a deliberate placement from an accident.

Starting point: 84 224 B, ~22 700 estimated tokens, read in full by every
session before any useful work. One section was 63.6% of it.

## Decisions

| Section | Bytes | Decision | Destination / reason |
|---|---|---|---|
| 0. Anti-Hallucination Protocol | 1 570 | **keep** | The behavioural contract itself. Tier 0. |
| Documentation index (preamble) | 2 317 | **keep** | This is the router. Condensing it is a later pass, moving it is meaningless. |
| 1. Entry procedure (mandatory) | 1 831 | **keep** | Tells an agent what to read before acting; removing it breaks the tiering. |
| 2. Prerequisites before running commands | 6 441 | **move** | → `operations/running-the-stack.md`. Needed to run the stack, not to start a task. |
| 3. DO NOT (common mistakes) | 1 768 | **keep** | Prohibitions. Cheapest rules to state, most expensive to omit. |
| 4. Development constraints | 2 645 | **keep** | Carries 6 blocking rules (`clippy -D warnings` must pass, test gates). Verified line by line before deciding. |
| 5. Source of truth | 641 | **keep** | Small, and it settles disputes about which file wins. |
| 6. Code placement | 433 | **keep** | Small, prevents files landing in the wrong tree. |
| 7. Code generation (critical behavior) | 648 | **keep** | Titled critical, and it is: typegen discipline. |
| 8. Stack (facts) | 2 302 | **move** | → `stack.md`. A version table with **zero** imperative lines. Not a duplicate of `architecture/overview.md`, which has services/ports but no version table. |
| 9. UI structure | 53 167 | **move** | → `architecture/ui-structure.md`. 63.6% of the file, needed only when touching the UI. |
| 10. RTK integration | 6 327 | **move** | → `architecture/rtk-integration.md`. Product internals. Its 5 imperative-looking lines are descriptive; the rule that binds an agent (prefix with `rtk`) is in `CLAUDE.md`. |
| 11. Multi-agent configuration | 438 | **keep** | Lists the redirector files and the rule that content lives in `docs/`. Cheap insurance against editing the wrong file. |
| 12. Last updated | 3 031 | **move** | → `release-notes-archive.md`. A changelog every session paid for. **Not** a duplicate of `CHANGELOG.md`: `WorkflowGuards`, `HostSyncMode`, `0.7.0` and `0.5.1` are absent there, so deleting it would have destroyed the only record. |
| Learned conventions | 203 | **keep** | Live router pointer to `learnings.md`, not history. |

Nothing was deleted. Two candidates looked like duplicates and were checked
before deciding — one was (partly) not, and that check is the reason the record
above says *move* rather than *delete-duplicate*.

## Result

| | start | now |
|---|---|---|
| `docs/AGENTS.md` | 84 224 B (~22 700 tok) | 13 488 B (~3 645 tok) |
| mandatory bootstrap | 89 760 B (~24 300 tok) | 19 024 B (~5 140 tok) |

−84.0% on the file, −78.8% on the bootstrap.

**Ceiling met, stretch goal recorded.** The DoD ceiling is 16 KiB / 4 000 estimated
tokens; `docs/AGENTS.md` now clears both, so the figure no longer depends on which
estimator you pick. The stretch goal is **12 KiB / ~3 000 tokens** — 1 200 B below
where the file sits today. The gate prints that remaining distance on every run
rather than reporting a flat "at target", because a file that only ever hears
"fine" is a file that stops improving.

Reaching it means condensing kept sections, not moving more out: what remains is
rules, and rules belong in the bootstrap. That is a rewriting pass with its own
review, deliberately not mixed into the moves above.

Every move is verbatim: each extracted body was asserted byte-identical to the
block it replaced, in the migration script rather than by eye. Condensing the
kept sections is deliberately a separate pass, so "no critical rule was lost"
stays provable rather than argued.

## What verbatim cost, and what the ratchet bought

Two defects came out of moving content byte-identically, and both were introduced
by this work rather than found in it:

- **Cross-references went stale in both directions.** `glossary.md` pointed at
  "§10 of `docs/AGENTS.md`" for the full RTK contract, which after the move was a
  four-line stub — the reader would have concluded the contract was gone. And the
  extracted `ui-structure.md` still said "see §12", "section 9 below", "See §13",
  numbers that only meant something while the text lived inside AGENTS.md. All
  four now point at real anchors. Verbatim preserves content, not context; the
  context has to be repaired deliberately.
- **Section numbers cannot be renumbered.** Other docs cite them, so the gaps
  left by moved sections stay as numbered stubs. Ugly, but stable.

The ratchet also earned its keep. Adding the router entries pushed the file 279 B
over its ceiling and the gate refused — printing back the rule that a ceiling must
not be raised to make a build pass. The right answer was not a higher ceiling: the
per-section stubs were duplicating what the router table now says, so trimming
their prose paid for the new entries and left the file *smaller* than before.
Without the gate, the ceiling would have gone up and the duplication would have
stayed.

## What this record does not cover

- The Tier 0/1/2/3 structure itself has not been reworked; the moved sections are
  reachable through pointers, which respects the intent without redesigning it.
- Duplication detection and per-section deltas in CI are not implemented, so a
  future duplicate could reappear without the gate noticing.
- The ten-task regression matrix has not been run, so the claim is "the bootstrap
  is smaller", not yet "nothing regressed".
