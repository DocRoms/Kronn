This is the FINAL step. Execute the two phases in order:

# PHASE 1 — Final quality review (reports excluded)

Read current guidance under `docs/`, but EXCLUDE `docs/reports/`: reports are dated snapshots outside routing and must not be rewritten during final review. EXCLUDE every `TEMPLATE.md` too: those gabarits are copied per instance, never filled in place. Fix issues directly in the remaining files (Write/Edit each file as needed).

Check:
- **No remaining `{{...}}` placeholders** — replace with content or `N/A — not used` for missing features. Any surviving uppercase double-brace placeholder is a hard failure: the file looked rendered but isn't.
- **Marker discipline** — there are 3 marker types, each with a strict semantic:
· `<!-- TODO: ask user -->` — info requires human decision (intent, archi choice). KEEP — Phase 2 validation asks the user.
· `<!-- TODO: verify -->` — you couldn't verify (sandbox blocked, file out-of-tree). FOR EACH ONE: try a final Glob/Read pass; if still impossible, CONVERT to `<!-- TODO: ask user -->` so Phase 2 escalates. If verification succeeded, write the conclusion WITHOUT any marker.
· `<!-- TODO: unknown -->` — placeholder from a previous validation skip. KEEP — same path as `ask user`.
- No duplicated facts across files (one canonical home per concept).
- Consistent terminology with `glossary.md`.
- Valid cross-references (clickable links resolve to existing files).
- **No contradictions between files.** Do this pass explicitly, not by impression. List every cross-document contradiction you find as the claim in doc A against the claim in doc B (`file:line` each), then resolve it against the SOURCE (manifest, config, code), never by picking the document you read last, and fix the wrong place. Check at least these families:
  · **Stack versions**: the framework / language / runtime versions in `docs/AGENTS.md` (Stack table), every language section heading of `coding-rules.md`, the tech-debt index and `testing-quality.md` must all agree with each other and with `composer.json`/`composer.lock`, `package.json`, the Dockerfile `FROM`, `.node-version` and the CI `setup-*` versions. Kronn runs the same comparison after you and sends each mismatch back with both places named.
  · **Absence claims** ("no test coverage gaps", "HTML has no Cache-Control", "no linter") against what other docs list (the TD index, the repo map) or what the code does (a `#[Cache]` attribute, a header subscriber).
  · **Universal claims** ("every locale serves the same path") against the routes, configs or fixtures that show an exception.
  · **Closed findings** (resolved, mitigated, false alarm): each cites the code that proves it, or is reopened.
  Example: coding-rules says enforced X but testing-quality says no linter exists.
- No empty critical sections.
- Clean markdown (no broken tables, no stray HTML).
- Each tech-debt entry in `inconsistencies-tech-debt.md` has a matching detail file under `tech-debt/`.
- Empty sections for missing features → write `N/A — not used` (don't leave the heading bare).

# PHASE 2 — Fill docs/decisions.md

This file records **intentional architectural choices** as ADRs (architecture decision records): choices that might look unusual to a newcomer, made on purpose. It is the *positive* counterpart to `inconsistencies-tech-debt.md`. An ADR accepted before this run is never reported as debt: Kronn settles every TD sheet it covers. One this run writes or flips to `accepted` goes back to `proposed` for a human.

Read the source code (entry points, key modules, configs) AND the docs you just reviewed in Phase 1. Replace the placeholder entry with **real decisions** you can trace to evidence, one ADR per decision, in the format of the template's example:
- Heading `## ADR-NNNN: <title>`: ids are sequential and never reused; continue after the highest existing id.
- `- **Status**:` `proposed`, `accepted` or `superseded by ADR-NNNN`. Write `accepted` only when the source is `user`, `briefing.md`, or a rule the team already wrote down (legacy docs, AGENTS.md, a code comment that forbids changing it). A decision you only infer from code is `proposed`: only a human makes it `accepted`.
- `- **Date**:` YYYY-MM-DD, when the decision was taken if the source says so, else today.
- `- **Supersedes**:` / `- **Covers**:` / `- **Links**:` optional. `Covers` lists the TD ids or slugs this decision explains.
- `- **Source**:` a verifiable `file:line` (cite with `[src: file: path:line]`), `briefing.md`, or `user`. **FORBIDDEN**: `inferred from <evidence>`; a decision you cannot anchor to one concrete source does not belong here.
- `### Context`, `### Decision`, `### Consequences`: why it was needed, what was chosen, and what follows, including what NOT to do.

Rules:
- **No count target**: record every real decision you can source, and none you cannot. Zero is a valid result: then replace the placeholder entry with `_No recorded decision yet._`.
- **Keep existing entries as written**: never renumber, rewrite or delete one. A decision that changed gets a new ADR whose `Supersedes` names the old one; the old one's status becoming `superseded by ADR-NNNN` is the only edit allowed.
- A table of an earlier format stays as it is; add new decisions as ADRs below it.
- **Do NOT invent**: a 'why' you cannot anchor in `file:line`, `briefing.md` or `user` is a post-hoc rationalisation, not a recorded decision.

**End state**: `docs/decisions.md` has zero `{{...}}` placeholders, every ADR carries its id, title, status, date and the three sections, and all OTHER docs/ files passed the Phase 1 review with markers cleaned up per the discipline rules above.