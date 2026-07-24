# Roadmap: 0.8.13 to Planning

Status: **sequencing proposal** (2026-07-24), not a calendar commitment.

This roadmap preserves the maintainer-approved order: quick wins, delivery
branch, production verification, merge, audit follow-up, release, then
Planning. `[src: user: 2026-07-24: validated delivery order]`

It also places every currently open TD into a delivery train. The canonical
open list remains `docs/inconsistencies-tech-debt.md`; this document explains
order, not status. `[src: file: docs/inconsistencies-tech-debt.md:25-50]`

## Timeline

### 1. 0.8.13 — stabilization and audit release

- [x] Finish and validate the quick-win working tree.
- [x] Create the delivery branch.
- [x] Deploy and verify document exports, stable IDs and MCP reads.
- [x] Merge the quick wins.
- [x] Finish the Audit follow-up on the local pre-release branch.
- [x] Run the complete local quality gates.
- [ ] Push the pre-release branch, run CI and merge it.
- [ ] Publish 0.8.13.

The headless-backend design remains a later 0.8.14 candidate, but it must not
enter or delay the 0.8.13 stabilization path. Its own estimate and candidate
status are documented separately.
`[src: file: docs/design/headless-backend.md:90-93]`

`TD-20260314-openapi-coverage` is continuous rather than a release blocker:
every touched endpoint should gain its annotation incrementally.

### 2. Immediately after 0.8.13 — Planning foundation starts

Start the product work in the already validated slices:

1. task schema/domain and event log;
2. compact read API and MCP contract;
3. discussion-plan side panel;
4. global prioritized backlog;
5. agent proposals, direct unambiguous writes and delta notifications;
6. task-to-discussion delegation only after the manual workflow is proven.

This is `TD-20260724-planning-and-discussion-plans`. The accepted product model,
MCP surface and UX are already captured in the dedicated design.
`[src: file: docs/design/planning-and-discussion-plans.md:37-170]`

### 3. Reliability guardrail — before unattended Planning delegation

Close the high-risk lifecycle gaps before tasks can launch autonomous work:

| Order | TD | Outcome |
|---|---|---|
| 1 | `TD-20260721-edit-resend-presence-dispatch` | Monotonic discussion events and atomic presence-aware resend; no duplicate local agent. |
| 2 | `TD-20260715-ws-dead-after-restart-silent-send` | Reconnect state plus persisted-send acknowledgement; no silent message loss. |
| 3 | `TD-20260715-agent-queue-restart-loss` | Persisted/idempotent agent queue and graceful drain; restart becomes lossless. |
| 4 | `TD-20260717-run-power-assertion-sleep` | Shared power guard for workflow and detached agent runs, then portable OS implementations. |
| 5 | `TD-20260701-print-agent-permission-bridge` | Explicit unattended tool policy and truthful denial reporting. |

These five TDs are the direct safety boundary for future “Create and run”
delegation. Their current residuals are listed in the canonical index.
`[src: file: docs/inconsistencies-tech-debt.md:40-46]`

### 4. Planning completion — the major product evolution

The first complete Planning release is reached when:

- the global backlog and discussion plan are two views of one task model;
- cross-project tasks, subtasks, blockers and discussion links work;
- agents can read compact context and make attributable updates;
- no unchanged plan is injected into prompts;
- delegation can create a structured discussion with “Create only” and,
  after the reliability guardrail, “Create and run”.

The acceptance anchors are defined in the Planning design.
`[src: file: docs/design/planning-and-discussion-plans.md:188-201]`

## Parallel and follow-up trains

These tracks can proceed alongside Planning when they do not destabilize the
active release. They are ordered inside each track, but do not all gate the
first Planning UI.

### Self-hosting and network

1. `TD-20260629-p2p-native-binding` — complete peer authentication and platform
   guidance before widening exposure.
2. `TD-20260314-no-tls` — production HTTPS termination and documented
   migration.
3. `TD-20260713-graph-delegated-oauth` — delegated refresh-token broker and
   a real Microsoft Graph API plugin.

`[src: file: docs/inconsistencies-tech-debt.md:32-41]`

### Data, configuration and deterministic tests

1. `TD-20260626-export-residuals` — bundle context-file rows and blobs.
2. `TD-20260627-configurable-docs-dir` — one docs-root source of truth.
3. `TD-20260629-e2e-seed-vs-real-db` — state-relative E2E assumptions.
4. `TD-20260721-hermetic-notify-e2e` — local runner-to-Notify harness with the
   production SSRF boundary unchanged.

`[src: file: docs/inconsistencies-tech-debt.md:36-38]`
`[src: file: docs/inconsistencies-tech-debt.md:47-47]`

### Frontend and toolchain

1. `TD-20260509-react19-effect-rules` — reduce the existing warnings in
   directory-sized passes.
2. `TD-20260713-typescript-7-native` — side-by-side compiler benchmark and
   diagnostic comparison.
3. `TD-20260721-oxlint-migration` — dual-run diagnostic parity before any
   ESLint replacement.

`[src: file: docs/inconsistencies-tech-debt.md:35-35]`
`[src: file: docs/inconsistencies-tech-debt.md:42-48]`

### Automation as code

`TD-20260722-project-scoped-automation-fs` is the next large platform track
after its ADR: project-scoped skills, workflows, prompts and Quick APIs become
versionable, diffable automation with secret-free cross-references. The full
MCP getters are already shipped; source-of-truth and sync semantics remain.
`[src: file: docs/tech-debt/TD-20260722-project-scoped-automation-fs.md:5-32]`

### Upstream watch

`TD-20260318-token-tracking-incomplete` stays blocked on Gemini CLI and Vibe
exposing token counts. Recheck it on provider/CLI upgrades; do not invent usage
locally. `[src: file: docs/inconsistencies-tech-debt.md:34-34]`

## Completeness check

Every open TD is assigned once as a primary home:

- continuous: OpenAPI coverage;
- 5 Planning/reliability items;
- 3 self-hosting/network items;
- 4 data/test items;
- 3 frontend/toolchain items;
- 1 automation-as-code item;
- 1 upstream-watch item;
- Planning itself.

That accounts for all 19 rows in the canonical open index as of 2026-07-24.
