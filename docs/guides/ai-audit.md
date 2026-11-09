# Audit a repository and keep its AI docs true

Most "ask an AI about my repo" flows restart cold every conversation. The
first time you onboard a project, a Kronn audit reads your source and configs
and writes a structured `docs/` tree that lives in the repository. That tree is
injected as project context into every Discussion, Quick Prompt and Workflow on
the project, so an agent does not start from scratch in each chat.

This page is the user guide. The engine, its states and its recovery paths are
in the [AI audit pipeline](../architecture/overview.md#ai-audit-pipeline) and
[audit run states](../architecture/audit-run-states.md).

## What the audit writes

- `docs/AGENTS.md`: stack, common tasks, source-of-truth files, code-placement rules
- `docs/repo-map.md`: folder tree and key entry points
- `docs/coding-rules.md`: linters, formatters and the conventions actually observed
- `docs/testing-quality.md`: coverage, untested components, smoke commands
- `docs/architecture/overview.md`: patterns, data flow, a **legacy migrations table**
- `docs/operations/debug-operations.md`: commands and troubleshooting
- `docs/operations/mcp-servers.md`: per-MCP capabilities discovered by introspection
- `docs/glossary.md`: a domain glossary built from your code
- `docs/tech-debt/*.md` and `docs/inconsistencies-tech-debt.md`: one detail file
  per finding, severity-tagged, all linked from a single index table

## Briefing, validation and status

An optional **Briefing** asks a few questions before the scan, so the audit
starts from what you know. Unknowns are marked `<!-- TODO: verify -->` or
`<!-- TODO: ask user -->`; after the audit, a validation discussion walks you
through them. The project status moves
`NoTemplate → TemplateInstalled → Bootstrapped → Audited → Validated`.

Once the agent confirms the validation, the page offers two actions: **Mark audit
as validated** (it freezes the tech-debt baseline) and **Setup AutoPilot** (see
the [Ticket Autopilot guide](../ticket-autopilot.md#start-from-a-validated-audit)).

## Drift, revalidation and checked citations

- **Drift detection is granular.** Each section tracks its source files (for
  example `coding-rules.md` watches `package.json`, `tsconfig.json` and
  `rustfmt.toml`). When those change, the section is flagged as stale and you
  re-audit only that step.
- **Revalidation** re-anchors moved citations for free and sends a model only
  the sections that need one; a relevance check gives each section a verdict
  without a model. See [documentation relevance](../architecture/doc-relevance.md).
- **Citations are checked.** A `[src: file: <path>:<line>]` that points to a
  missing file or line is flagged as fabricated. See the
  [anti-hallucination guard](../conventions/anti-hallucination-guard.md).

## Baselines learned from real-world misses

- **Mandatory baseline checklist.** The tech-debt step always scans five
  categories that are too common to skip: Dockerfile (`USER`, `display_errors`,
  opcache, `HEALTHCHECK`), Compose resource limits, the CI quality gate and
  `StrictHostKeyChecking`, `.env*` tracking and deploy hygiene (never reading
  secret values), and web accessibility / CSP. Each item produces an explicit
  "verified present", "verified absent" or tech-debt line, so you can trust the
  audit did not look the other way.
- **Anti-repetition memory.** Each audit reads the existing tech-debt entries as
  priors and reuses their IDs instead of churning slugs. An `audit_history` YAML
  block on each detail file tracks every pass. A reconciliation report
  (`_reconciliation-<date>.md`) classifies dropped entries as
  `Fixed / Stale / Missed / Uncertain`, so nothing silently vanishes between
  audits.
- **Two-tier status.** `Verified in source` (the agent opened the file and
  confirmed) versus `Inferred` (pattern-matched only). The validation phase
  skips the verified ones to save your time.
- **Specialized audit kinds.** The Full audit is chained: 9 documentation steps,
  then every focused pass (`Security`, `Docker`, `Performance`, `Accessibility`,
  `Database`, `ApiDesign`, `CodeQuality`) in one launch, 16 steps in total. Each
  focused pass opens with a relevance gate ("not applicable → one line and move
  on"). The focused kinds, plus `Drift`, `Rgaa` and a `Custom` escape hatch, can
  also be launched alone; each writes its own index file, so it does not clobber
  the Full audit's entries.
- **Health badge with cluster recommendations.** Every run is stored in
  `audit_runs` with its duration, severity counts and a recommendations list.
  The score runs from 0 to 100 (Critical × 12 + High × 4 + Medium × 1.5 +
  Low × 0.3 points of debt). When three or more findings cluster in one
  dimension (for example five Docker entries), the dashboard suggests "Run a
  focused Docker audit".
- **Community-standards gate.** When the project shows open-source intent (a
  `LICENSE`, a remote on GitHub, GitLab or Codeberg, or a README that mentions
  contributing), the tech-debt step also flags a missing `LICENSE`,
  `SECURITY.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` and issue or PR
  templates. Private projects skip this block.

This is what makes Kronn a knowledge-persistence layer with strict baselines,
not only a prompt launcher.
