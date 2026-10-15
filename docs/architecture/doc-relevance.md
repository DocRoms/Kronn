# Documentation relevance (« Revalider l'audit », KT-1168)

Citation fingerprints say which cited lines moved, changed or vanished. They
do not say whether a section of the AI docs still describes anything that
exists, and a doc that drifts from the code pollutes the context of every
agent that reads it. The relevance check gives each Markdown section a
verdict, deterministically and without a model.

## What the revalidation already did (reused, not rewritten)

- Per-citation fingerprints, `changed` / `removed` / `file missing` / `moved`,
  recorded at validation. [src: file: backend/src/core/citation_drift.rs:770#check]
- The plan that re-anchors moved lines for free and sends the owning audit
  steps only the sections that need a model.
  [src: file: backend/src/api/audit/revalidate.rs:50#plan]
- The `audit-revalidation` skill the bounded agent pass receives.

## What relevance adds

Every section (heading to next heading, fenced code excluded) is judged from
its anchors: its `[src: file: …]` citations, read through the fingerprint
report, and the repository paths its prose names in backticks.
[src: file: backend/src/core/doc_relevance.rs:444-470#judge]

| Verdict | Meaning |
|---|---|
| `still_true` | every anchor resolves; a moved line counts as resolving |
| `outdated` | a cited line changed or fell past the end of its file, or some of the files it names are gone |
| `no_longer_relevant` | every anchor points at a file that is gone |
| `unanchored` | no citation and no path: Kronn cannot measure it |

[src: file: backend/src/core/doc_relevance.rs:42-47#SectionVerdict]

`still_true` means Kronn found no contradiction, not that every claim was
proven: the agent pass settles substance. A prose path is a claim only when
its first folder exists at the project root, so `path/to/file.rs` examples and
other repositories' paths are ignored. A path that was already gone when a
human validated or acknowledged the doc is recorded in `citations.json`
(`known_missing`) and stops counting: it is a deliberate historical mention.

## The measure

- Counts per verdict, the share of anchored sections that are still true
  (`relevance_pct`), and `untouched_stale`: outdated or no-longer-relevant
  sections whose own digest equals the one recorded at validation, i.e.
  nobody revised them although the code they describe changed.
  [src: file: backend/src/core/doc_relevance.rs:109-134#RelevanceCounts]
- The same input gives the same output; the check never calls a model.

## Where it surfaces

- **Plan**: `RevalidationPlan.relevance` (counts) and
  `RevalidationSection.relevance` (the stale sections with evidence and a
  proposed correction). A doc with a stale section is sent to its owning audit
  step; one with no owning step is left to a person.
- **Agent block**: each finding and its proposed correction is written into
  the step's prompt next to the skill.
- **Badge**: `sources_state` is raised from `clean` to `needs_reverification`
  when a section is outdated or gone, and `broken_citations` counts the prose
  paths no fingerprint covers.
  [src: file: backend/src/core/checksums.rs:777-784#check_drift]
  [src: file: backend/src/core/doc_relevance.rs:152#escalate]
- **Validation state**: a human-validated doc whose prose names a vanished path
  stops counting as human-validated for learning sources, like one with a
  broken citation.
- **Human gate**: nothing is rewritten by the check. The corrections are
  proposals the human launches through « Revalider l'audit », then reviews
  and validates; « Vérifié, inchangé » acknowledges a section that was
  judged stale but is fine.

A citation the git diff maps to a new line past the fingerprint window is
treated as moved, exactly as the plan re-anchors it.
[src: file: backend/src/core/doc_relevance.rs:534#analyze_with_moves]
