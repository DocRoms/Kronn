# Template v2 review — 2026-10-01

Scope: the instruction-template changes included in 0.14.2 at `1b67e8d3`,
including the installation path that adds managed instructions to existing
agent files. [src: commit: 1b67e8d3]

## Findings corrected

- **Conflicting language instructions on re-audit.** The generated managed
  block imposed English even when the project chose another documentation
  language. It now refers to `docs/AGENTS.md` for project parameters and rules.
  The existing replacement regression covers an old English-only block above
  French user instructions, preserving those user bytes.
  [src: file: backend/src/core/root_agent_files.rs:299]
  [src: file: backend/src/core/root_agent_files.rs:519]
- **A second working-language value in adapter files.** New adapters copied
  `PROJECT_LANGUAGE`, whose bootstrap replacement is English. A later audit
  can choose another language in `docs/AGENTS.md`, leaving the adapter stale.
  The ten adapter templates now point to the canonical parameter instead.
  [src: file: backend/src/core/docs_migration.rs:660]
  [src: file: templates/AGENTS.md:3]
  [src: file: backend/src/core/template_homogeneity_test.rs:86]
- **A checklist overriding the configured test policy.** The test selection
  examples and completion checklist now apply only where required by the
  project policy and CI; they no longer introduce a universal full-suite gate.
  [src: file: templates/docs/testing-quality.md:23]
- **Broken parameter anchors.** `Project parameters` was bold text, despite
  three workflow links targeting `#project-parameters`. It is now a heading.
  [src: file: templates/docs/AGENTS.md:14]
  [src: file: templates/docs/workflow/commits.md:10]
  [src: file: templates/docs/workflow/tickets.md:12]

## Qualification boundary

Validation: 22 instruction-file tests and 273 template-related tests pass.
The three `#project-parameters` links and the nine source citations above
resolve. `cargo fmt --check`, Clippy with `--all-targets -- -D warnings`,
`make check-version` and `git diff --check` pass.

These findings come from source review and deterministic regression checks.
The A/B artifacts were subsequently recovered and assessed by two independent
Codex sessions; see the [Sonnet comparison](sonnet-template-comparison-2026-10-01.md).
Those runs predate these corrections. They show pipeline completion and cleaner
concrete citation ranges, but do not establish better overall content. No
quality, token or latency gain is attributed to the fixes above.

Any subsequent A/B comparison should identify the exact template revisions,
models, tasks and delivered context for both arms. Assess rule compliance and
task correctness alongside cost; a shorter context alone does not establish
that the revised template works better.

## Follow-up implementation — 2026-10-02

The design target is reliable audits with inexpensive models and limited
context. Routing remains explicit, but a link no longer instructs the agent to
load every target. Known paths are accessible directly after the entry rules;
three files/sections is an initial search budget, not a reason to stop checking
evidence. Stack/source inventories live in `repo-map.md`; prerequisites and
operational commands live in `operations/debug-operations.md`.
[src: file: templates/docs/AGENTS.md:40]
[src: file: templates/docs/operations/debug-operations.md:5]

Full audits now execute eight foundation steps, seven specialized analyses,
and then the consolidation. Stored resume rows are matched by target document,
so old numeric positions cannot skip the wrong section. A repaired section
invalidates the prior consolidation. Failed prerequisites defer consolidation
without another model call; partial selections are sorted after their stored
positions are resolved.
[src: file: backend/src/api/audit/mod.rs:1121]
[src: file: backend/src/api/audit/full.rs:1008]
[src: file: backend/src/api/audit/full.rs:2821]
[src: file: backend/src/api/audit/drift.rs:240]

Every findings step receives the same applicability, counter-check and
verification contract. A citation's existence does not establish the defect.
The final review reconciles domain summaries and links specialized indexes
without copying every TD into one global list. Architectural decisions have
no minimum count; a documented rationale or human confirmation is required.
An explicit short note with no evidenced decisions is accepted, while empty
sections and unfilled placeholders remain failures.
[src: file: backend/src/api/audit/mod.rs:1143]
[src: file: templates/docs/decisions.md:9]
[src: file: backend/src/api/audit/validation.rs:218]

These are implementation contracts, not a measured model-quality gain.
The follow-up Sonnet benchmark uses the same reference source and the exact
model observed in CS1. It must report audit/retry usage separately from the
validation discussion and distinguish byte-volume proxies from actual model
tokens. The historical blind judgments remain frozen; the follow-up is not
another blind judgment or a qualification of every small-context model.

The follow-up runtime and templates are frozen at `8847d7f7`. A subsequent
validator correction accepts Windows line endings in a short decision note
and refuses an empty Decisions section followed by content in another section.
Its regression failed before the correction; all 30 validator tests pass after
it. This parser correction is tested separately from the frozen Sonnet run.
Follow-up measurements and their qualification limits are tracked in
[PR #219](https://github.com/DocRoms/Kronn/pull/219).
[src: commit: 8847d7f7]
[src: file: backend/src/api/audit/validation.rs:218]
[src: file: backend/src/api/audit/validation.rs:983]


## Coverage follow-up after CS2

CS2 completed all 16 steps but the implementing agent's non-blind review found
10/15 known defects (8/11 certain), versus 13/15 (10/11 certain) in both frozen
CS1 reviews. Its observed cumulative audit usage, including cache, was 5,242,693
tokens versus 5,153,836; maximum reported input was 66,669 versus 60,005.
This is an adverse single-run signal, not a quality acceptance or a causal
estimate. Frozen measurements and methodology are summarized in
[PR #219](https://github.com/DocRoms/Kronn/pull/219).

The next prompt revision separates breadth from the initial read batch:
inspect all discovered build entry points, outbound transport, client timeouts
and retries, cache capacity and pagination without assuming an ORM. Dependency
absence checks include workspace tooling and dynamic loading. These are generic
coverage rules; the benchmark's source paths and known answers are not inputs.
[src: file: backend/src/api/audit/mod.rs:460]
[src: file: backend/src/api/audit/mod.rs:630]
[src: file: backend/src/api/audit/mod.rs:759]

A canonical TD template is now shipped and embedded in each findings step and
final consolidation, including when an older project lacks the template file.
Named fields, body/history agreement and an actual index link are explicit
output requirements. Final review requests a structural sweep and targeted
repairs of generated documents; it is still an agent instruction, not a new
mechanical quality gate. Prompt contract tests cannot prove model compliance;
the same-source/model CS3 follow-up must measure it before acceptance.
[src: file: templates/docs/tech-debt/TEMPLATE.md:1]
[src: file: backend/src/api/audit/mod.rs:1143]
[src: file: backend/src/api/audit/mod.rs:557]


CS3 restores all 13 known defects found in CS1 (10/11 certain) in the same
non-blind review, with 34/34 linked TDs and no schema errors in the structural
check. However, cumulative reported audit usage reaches 9,306,996 tokens,
including cache, and peak input reaches 126,798 tokens. The code-quality step
retrieved 169,816 characters in 38 tool results, including whole source files.
These measurements do not establish suitability for a small context window;
the extra evidence checks must be assessed for usefulness, not cost alone.
The next prompt revision requests relevant function/section reads and batched
queries, explicit TD citation grammar, and error-only artifact verification.
Following the user clarification, evidence quality takes precedence over token
savings: no fixed reading or TD word cap may truncate a necessary check. These
are prompt requirements, not a runtime context limit; CS4 must measure actual
compliance and retain the same defect-coverage criteria. CS3 remains frozen at `33e5a850`.
[src: commit: 33e5a850]
Results and evidence limits: [PR #219](https://github.com/DocRoms/Kronn/pull/219).
