# Code hygiene and extraction roadmap

This note turns KT-147 into bounded refactors. It is a sequencing contract,
not permission for one large rewrite: every slice keeps the public API stable,
adds or moves tests before deleting the old path, and must be independently
revertible.

## Completed first slice: locale chunks

`frontend/src/lib/i18n.ts` now owns only locale detection, persistence and the
small lazy-loader runtime. The dictionaries live in
`frontend/src/lib/i18n/locales/{fr,en,es}.ts`; application bootstrap loads only
the selected UI locale, while a language switch loads its chunk before changing
the rendered locale. Agent-facing API helpers also preload the configured
output locale before building translated prompts. The test-only static aggregate
is isolated in `frontend/src/lib/i18n/testing.ts` and is absent from production.

Production build measurements (Vite 8, 2026-08-02):

| Build | Runtime i18n chunk | Active locale | Initial i18n total |
|---|---:|---:|---:|
| Before | 737.22 kB / 239.17 kB gzip | bundled together | 737.22 kB / 239.17 kB gzip |
| After (French) | 32.64 kB / 9.18 kB gzip | 243.17 kB / 79.98 kB gzip | 275.81 kB / 89.16 kB gzip |

The French first-load i18n payload is therefore about 63% smaller gzip. English
and Spanish are separate 223.33 kB and 238.77 kB chunks; neither is fetched for
a French first paint.

## WorkflowWizard.tsx

Keep `WorkflowWizard` and `WorkflowWizardProps` as the stable entry point.
Extract in this order:

1. Move pure cron parsing/formatting, blank-step construction and step mutation
   helpers to `workflowWizard/model.ts`, with unit tests independent of React.
2. Move state, data loading and save-payload construction to
   `useWorkflowWizardController.ts`. The hook owns async effects and exposes
   explicit commands; view components never call the API directly.
3. Split the five screens into `InfoStep`, `TriggerStep`, `StepsStep`,
   `ConfigStep` and `SummaryStep`, then extract repeated step editors by
   `step_type` only when two screens genuinely share behavior.

Target: an orchestrator below 600 lines and screen/editor modules below 500
lines. Preserve existing wizard-mode, preset and accessibility tests at every
slice; do not combine this work with a visual redesign.

## backend/src/api/workflows.rs

First turn the file into `api/workflows/mod.rs` that re-exports the existing
handler names, so router call sites and generated contracts do not move. Then
extract cohesive modules in this order:

1. `validation.rs`: guards, required fields, launch variables, sub-workflow
   graph and allowlist validation (pure code first).
2. `crud.rs` and `portability.rs`: list/get/create/update/delete versus
   import/export and re-binding.
3. `runs.rs` and `recovery.rs`: run history/cancel/decide versus interrupted-run
   resume and replay decisions.
4. `testing.rs` and `suggestions.rs`: step previews/worktrees/API extraction
   versus catalogue suggestions.

Validation and transition rules then move behind `core::workflows` services;
SQL stays in `db`, and Axum handlers are limited to authentication, request
parsing, service invocation and response/SSE mapping. Characterization tests
must pin HTTP status, JSON/SSE shape and resume semantics before each move.

## backend/src/core/anti_halluc.rs

Keep `core::anti_halluc` as the public facade and re-export today's functions.
Split implementation by responsibility:

1. `mode.rs` and `prompt.rs` for runtime mode, policy and preamble text.
2. `lexical.rs` for fenced/inline-code stripping, sentence splitting and
   normalization.
3. `markers.rs` for source-marker extraction/parsing and line specifications.
4. `verify.rs` for path resolution and source checks.
5. `report.rs` for assertion linting, report merging and enforce decisions.

Move the current monolithic test module beside the owning module as each slice
lands. Preserve the facade until all downstream imports have migrated; no
behavioral tuning is mixed into the file move.

## Progressive api/ to core/ extraction

File size alone does not decide ownership. Extract when a rule can be expressed
without Axum types and is reused, stateful, or independently testable. The
sequence is: pure validation and transitions, domain services with explicit
ports for DB/filesystem/agent execution, then thin transport adapters. Database
queries remain in `db`; HTTP/SSE serialization remains in `api`.

Each PR should remove more domain branching from handlers than it adds, include
a focused service test plus an unchanged route-contract test, and avoid a
repository-wide module rename. This keeps a future headless CLI/library surface
possible without making that future surface a prerequisite for current work.

## Changelog rotation contract

`CHANGELOG.md` remains the release entry point and always starts with the current
release heading in exactly one of these forms:

```text
## [X.Y.Z]
## [X.Y.Z] - YYYY-MM-DD
```

That contract is consumed by `scripts/check-version-sync.sh` and covered by
`tests/bats/version_sync.bats`; rotation must not teach the version check to
read an archive or an `Unreleased` section first.

When the root changelog next becomes cumbersome, keep the current minor series
plus the two previous minor series in `CHANGELOG.md`, and move older complete
release blocks unchanged to immutable files under `docs/changelog/` grouped by
minor range (for example `CHANGELOG-0.1-to-0.8.md`). Add a short archive index
at the bottom of the root file. Never split a release block, rewrite historical
dates, or move link definitions away from the file that references them.

The future rotation script must write to temporary files and replace outputs
only after validating that every release heading appears exactly once across
root plus archives. Its Bats fixture must prove that an archived newer-looking
heading cannot override the first root heading and that a dated current heading
continues to pass. Until that script and test exist, rotation stays manual and
requires `bats tests/bats/version_sync.bats` plus
`scripts/check-version-sync.sh` before commit.
