# Documentation relevance (« Revalider l'audit », KT-1168)

Citation fingerprints say which cited lines moved, changed or vanished. They
do not say whether a section of the AI docs still describes anything that
exists, and a doc that drifts from the code pollutes the context of every
agent that reads it. The relevance check gives each Markdown section a
verdict, deterministically and without a model.

## What the revalidation already did (reused, not rewritten)

- Per-citation fingerprints, `changed` / `removed` / `file missing` / `moved`,
  recorded at validation. [src: file: backend/src/core/citation_drift.rs:1013#check]
- The plan that re-anchors moved lines for free and sends the owning audit
  steps only the sections that need a model.
  [src: file: backend/src/api/audit/revalidate.rs:92#plan]
- The `audit-revalidation` skill the bounded agent pass receives.

## What relevance adds

Every section (heading to next heading, fenced code excluded, a heading with
nothing under it skipped) is judged from its anchors: its `[src: file: …]`
citations, read through the fingerprint report, and the repository paths its
prose names in backticks.
[src: file: backend/src/core/doc_relevance.rs:1186#judge]

| Verdict | Meaning |
|---|---|
| `anchors_intact` | every anchor resolves; a moved line counts as resolving |
| `outdated` | a cited line changed or fell past the end of its file, or some of the files it names are gone |
| `no_longer_relevant` | every anchor points at a file that is gone |
| `unanchored` | no citation and no path: Kronn cannot measure it |

[src: file: backend/src/core/doc_relevance.rs:54#SectionVerdict]

`anchors_intact` means Kronn found no contradiction, **not** that the prose
was proven true: the agent pass settles substance. For that reason every
`unanchored` section is listed in the panel (`RevalidationPlan.unanchored`)
and handed to the agent pass as « unverifiable: check or anchor », so each
section of a doc is confronted with the code by someone. They never raise
the badge by themselves. A doc made only of unanchored sections has no verdict
that could ever send it, so it is re-read once per change (KT-1292): `plan()`
queues it for its owning step when `citations.json` holds no review stamp for
it, or when the stamp no longer matches the doc's hash or the digest of that
step's sources. After the pass, the doc is stamped whether or not the agent
added an anchor, so an unchanged repo plans clean and there is no re-queue
loop. A validation stamps those docs too (`stamp_validated_rereads`, KT-1319):
the human has just checked the whole set, so a project validated a minute ago
queues no step. The panel lists each such doc with « re-read on <date> » (`RevalidationPlan.unanchored_docs`).
A doc no audit step owns is listed but never queued; the panel says « no audit
step re-reads this doc » for it (`UnanchoredDoc.owned`), not « not re-read ».

The sources digest is the one of the **owning step**, so how often a doc comes
back depends on that step (`AnalysisStep.sources`):

- steps 3 (`repo-map`), 8 (tech debt) and 9 (`decisions`) read the whole source
  tree (`__GIT_SOURCE_TREE__`): any code change queues their all-unanchored
  doc again. The re-read is bounded (one step per doc) and leaves the badge
  alone;
- steps 4 to 7 read the files they name (manifests, `Makefile`, compose, …):
  only a change there queues the doc;
- step 2 (`glossary`) has no sources, so its digest never changes: only an edit
  of the doc queues it again.

[src: file: backend/src/core/citation_drift.rs:86#DocReview]
[src: file: backend/src/core/citation_drift.rs:895#current_review]
[src: file: backend/src/api/audit/revalidate.rs:60#stamp_rereads]

### When a prose path counts as gone

A backticked path is a claim only when its first folder exists at the project
root (`path/to/file.rs` examples and other repositories' paths are ignored),
and it is called gone only when it resolves **nowhere**:

1. from the project root;
2. from the doc's own folder;
3. as the tail of any project file (`git ls-files`, untracked files that are
   not ignored; a bounded walk outside git): docs write `lib/routes.ts` for
   `frontend/src/lib/routes.ts`;
4. and it is neither a file Kronn generates or writes on demand
   (`briefing.md`, `checksums.json`, `citations.json`, `provenance.json`,
   `linked-repos.md`, …) nor git-ignored.

It is not a claim at all when it sits in:

- a fenced code block;
- a sentence that tells history or absence (« the former `x.rs` », « no
  longer », « does not exist », « if we add », « to be
  created »…; and a path said missing right where it is cited, « the missing
  `x.yml` » or « `x.yml` is missing from the repo »: a finding about a gone
  file names it to say so, while a « missing » elsewhere in the sentence
  exempts nothing), or an
  example sentence (« e.g. », « Detail file example: ») and, when it closes a
  line, the list right under it. The rule is scoped to the sentence holding
  the path, split on sentence punctuation outside inline code: Kronn's docs
  write whole paragraphs on one line, and « Phase `x` removed; … lives in
  `y.css` » still claims `y.css`;
- a section whose heading is a proposal as a whole or by a proposal phrase
  (« Ideas », « Roadmap », « Future work », « Suggested direction », « TODO
  list »). A single word inside a real title (« Ma Todo », « Workflow
  suggestions ») does not make one. A TD sheet's « Remediation », « Fix » or
  « Next step(s) » heading (Kronn's own TD shape) is one, and so is a line that opens with such a
  label (`- **Remediation**: …`, `- **Suggested fix**: …`; when the label
  closes the line, the text under it up to the next blank line, heading or
  sibling item): they name files still to be created (KT-1313). A section left
  with nothing else to verify is listed as unanchored and says so in its
  correction;
- a naming pattern (`TD-YYYYMMDD-slug.md`);
- a doc still holding the template's raw `{{PLACEHOLDER}}` slots;
- a changelog, release-notes, history or archive doc, which relevance does not
  analyze at all (their citations still go through the fingerprint check).

A path both cited and named in the same section counts once, as the citation.
Run `cargo nextest run --run-ignored only kronn_own_docs_report` to list the
remaining hits over this repository's own docs.

### Accepting a vanished mention

A mention a person knows is historical is accepted with « Vérifié, inchangé »,
and only then. `citations.json` stores, per section key, the gone paths the
human was shown (`known_missing`). A validation or a revalidation **carries
over** what was accepted and never adds to it; a path accepted under one
section is still flagged when another section names it.
[src: file: backend/src/core/citation_drift.rs:358#accepted_missing]

From the project card, the stale block lists each stale doc's gone prose paths
with their section (`DriftSection.missing_paths`), and the acknowledgement
sends exactly those (`shown_paths`): a path that is not in the request is not
accepted, whatever is gone at the click, and an older client that sends none
accepts none. The per-doc panel, which lists every gone path of its doc, still
accepts them all.
[src: file: backend/src/core/citation_drift.rs:595#acknowledge_accepting]

## The measure

- Counts per verdict, the share of anchored sections whose anchors are intact
  (`anchored_intact_pct`), and `untouched_stale`: outdated or no-longer-relevant
  sections whose own digest equals the one recorded at validation, i.e.
  nobody revised them although the code they describe changed.
  [src: file: backend/src/core/doc_relevance.rs:123#RelevanceCounts]
- The same input gives the same output; the check never calls a model.

## Where it surfaces

- **Plan**: `RevalidationPlan.relevance` (counts), `.unanchored` (the sections
  nothing anchors) and `RevalidationSection.relevance` (the stale sections
  with evidence and a proposed correction). A doc with a stale section is sent
  to its owning audit step; one with no owning step is left to a person.
- **Agent block**: each finding and its proposed correction is written into
  the step's prompt next to the skill, with the unanchored sections as
  unverifiable.
- **Badge**: `sources_state` is raised from `clean` to `needs_reverification`
  when a section is outdated or gone. The paths no fingerprint covers are
  counted apart in `missing_paths`, so `broken_citations` stays a count of
  citations.
  [src: file: backend/src/core/checksums.rs:844#check_drift]
  [src: file: backend/src/core/doc_relevance.rs:173#escalate]
- **Validation state**: a human-validated doc whose prose names a vanished path
  stops counting as human-validated for learning sources, like one with a
  broken citation, whether or not the caller passes a citation report.
- **Human gate**: nothing is rewritten by the check. The corrections are
  proposals the human launches through « Revalider l'audit », then reviews
  and validates; « Vérifié, inchangé » acknowledges a doc, recorded or not,
  that was judged stale but is fine.
  [src: file: backend/src/core/checksums.rs:813#acknowledge_stale]

A citation the git diff maps to a new line past the fingerprint window is
treated as moved, exactly as the plan re-anchors it.
[src: file: backend/src/core/doc_relevance.rs:1317#analyze_with_moves]
