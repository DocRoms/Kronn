# Architecture decisions (why, not what)

> **TEMPLATE FILE.** Filled during audit Step 9 (final review + decisions). Do not invent decisions — they must come from code analysis or user confirmation.

This file records **intentional choices** as ADRs (architecture decision records): patterns that look unusual but are deliberate. It prevents agents from "fixing" things that aren't broken.

It is the *positive* counterpart to `inconsistencies-tech-debt.md`: decisions the team made on purpose, NOT problems to fix. An `accepted` ADR is never reported as debt: after each audit, Kronn settles every TD sheet it covers (status `Accepted decision`, with a `Decision record` line naming the ADR). Only an ADR accepted before the audit started, or by a human in a validation card, counts: one an audit writes as `accepted` goes back to `proposed`.

## ADR format

One `## ADR-NNNN: <title>` entry per decision. There is no count target: record every decision you can source, and none you cannot.

| Field | Required | Value |
|-------|----------|-------|
| `ADR-NNNN` (heading) | yes | Sequential id, never reused or renumbered |
| Title (heading) | yes | The choice, in one line |
| `Status` | yes | `proposed`, `accepted`, or `superseded by ADR-NNNN` |
| `Date` | yes | `YYYY-MM-DD`, when the decision was taken |
| `Supersedes` | no | ADR ids this decision replaces |
| `Covers` | no | TD ids or slugs this decision explains (`TD-20260828-ci-label-gate` or `ci-label-gate`); a slug matches the finding on any date |
| `Source` | yes | `file:line`, `briefing.md`, or `user` — never "inferred" |
| `Links` | no | Tickets, PRs, docs |
| `### Context` | yes | Why a decision was needed |
| `### Decision` | yes | What was chosen |
| `### Consequences` | yes | What follows, including what NOT to do |

- **Status.** `accepted` only when the source is `user`, `briefing.md`, or a rule the team already wrote down. A decision inferred from code alone is `proposed` until a human accepts it. `proposed` and `superseded` ADRs cover nothing.
- **Changes.** Never edit or delete an entry to change a decision: add a new ADR whose `Supersedes` names the old one, and set the old one's status to `superseded by ADR-NNNN`.
- **Legacy tables.** A table of an earlier format stays as written; add new decisions below it.

## Example (good shape — adapt to the actual repo)

```markdown
## ADR-0001: CI runs only when a pull request carries the `ci` label

- **Status**: accepted
- **Date**: 2026-08-28
- **Covers**: quality-gate-not-enforced
- **Source**: `.github/workflows/ci.yml:3` + user
- **Links**: docs/workflow/README.md

### Context

A full CI run costs about 40 minutes of paid runners. Most pushes are work in progress.

### Decision

CI is triggered by the `ci` label, added when the pull request is ready for review.

### Consequences

Unlabelled pushes get no CI feedback. Do NOT switch the workflow to `on: push` or "fix" the label trigger: the budget guard is deliberate.
```

## Decisions

<!-- Fill during audit. Each ADR must be traceable to code evidence or user confirmation. Delete the placeholder entry below; if the project has no sourced decision yet, write `_No recorded decision yet._` instead. -->

## ADR-0001: {{DECISION_TITLE}}

- **Status**: {{STATUS}}
- **Date**: {{DATE}}
- **Source**: {{FILE_OR_USER}}

### Context

{{CONTEXT}}

### Decision

{{DECISION}}

### Consequences

{{CONSEQUENCES_AND_WHAT_NOT_TO_DO}}
