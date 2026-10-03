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
