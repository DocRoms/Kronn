# Catalogue and reviewer selection

The workflow review picker keeps `reviewer_agent` and its nullable
`reviewer_tier` as the existing review configuration. Enabling review leaves
the tier `reasoning` for a newly enabled review. A saved `null` tier remains
`null` until the operator makes an explicit picker choice, including the
explicit return to the agent default. [src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4175-4225]

`AgentSwitchPicker` accepts a nullable current tier so an existing saved review
can be displayed without forcing a tier selection. Its concrete-tier and
default-tier callbacks remain the only paths that report a selection to its
caller. [src: file: frontend/src/components/AgentSwitchPicker.tsx:28-41]

The manual catalogue form keys a target by `runtime_target_id` and updates its
matching `agent_type` only in the searchable selector callback. During edit,
the picker is disabled and a saved target absent from the current snapshot is
kept as an option, preserving its runtime namespace. A target is described as
verified only after a successful, non-stale live refresh. [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:165-180] [src: file: frontend/src/components/settings/ModelCatalogSection.tsx:242-258]

## Principal qualification, September 10

Delivery `f5f50a9a` was reviewed after 104 focused tests passed. The integrated
candidate `ec0cc4e9` keeps backend tree `189eb0f5` and frontend tree `9aa5e80d`.
All eight persisted integration gates passed: full Vitest, native and legacy
TypeScript, Oxlint, ESLint, i18n, Vite and diff hygiene. ESLint retained its
63 existing warnings; i18n reported 709 static unused-key warnings.
[src: commit: ec0cc4e9]

Independent full replays also exposed harness failures: two worker-start errors
with four workers, then two test timeouts and a following assertion failure
with two workers. The unchanged four-worker command subsequently passed all
4,091 tests in 313 files in 346.29 seconds when stdout and stderr were captured
to regular temporary files. No test, timeout, assertion or retry policy was
changed. This is a successful replay, not proof of the cause of the earlier
failures; the unusually long setup duration remains an observation. Raw logs
were preserved in the principal's temporary qualification artifacts.

This checkpoint does not qualify browser behavior, providers, coverage, the
remaining Ollama catalogue surface, or the whole release.
