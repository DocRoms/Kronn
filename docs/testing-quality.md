# Testing and quality

Kronn treats tests as release evidence, not as an approximate health signal.
Never publish hard-coded test counts in this document: the suite changes often
and the runner's final summary is the source of truth.

## Mandatory contract

- Every behavior change includes a regression test that fails without the
  change.
- Tests assert user-visible behavior or a durable protocol invariant, not
  component implementation details.
- A flaky test is a defect. Fix its synchronization or isolation; do not add
  sleeps, retries or a looser assertion to make it green.
- Frontend API mocks must match the generated Rust DTOs. Rust models remain the
  type source of truth; regenerate TypeScript with `make typegen`.
- All commands in the release gate must pass before a tag is created.

## Release gate

Run from the repository root unless a working directory is shown.

| Layer | Command | Required result |
|---|---|---|
| Version surfaces | `make check-version` | Every manifest, README, site and the first changelog release agree |
| Diff hygiene | `git diff --check` | No whitespace errors |
| Rust formatting | `cd backend && cargo fmt --all -- --check` | Clean |
| Rust lint | `cd backend && cargo clippy --all-targets -- -D warnings` | Zero warnings (third-party code-generation parser notices are not clippy diagnostics) |
| Backend tests | `make test-backend` | Entire Rust suite passes |
| Python helpers | `make test-python` | Entire helper suite passes |
| Shell | `make test-shell` | Entire bats suite passes |
| Frontend native TS | `cd frontend && pnpm typecheck:native` | Clean |
| Frontend legacy TS | `cd frontend && pnpm typecheck:legacy` | Clean |
| Frontend ESLint | `cd frontend && pnpm lint` | Zero errors; CI's pinned warning budget must not increase |
| Frontend fast lint | `cd frontend && pnpm lint:fast` | Zero warnings |
| i18n | `cd frontend && pnpm lint:i18n` | `fr`, `en`, `es` and `zh` have matching, valid keys |
| Frontend unit/integration | `make test-frontend` | Entire Vitest suite passes |
| Frontend production build | `cd frontend && pnpm build` | TypeScript and Vite build succeed |
| Browser E2E | `make test-e2e` | Entire Playwright suite passes against the expected backend fixture |

CI also checks dependency audit, generated-type drift, desktop compilation and
repository-specific Rust safety lints. `.github/workflows/ci-test.yml` is the
authoritative job graph.

## Backend CI timing SLO

`test-backend` is the measured backend critical-path job. Its functional gates
remain blocking: formatting, clippy, Rust tests, coverage floors, generated
type drift, safety/budget checks, the frontend build needed by the desktop
compile check, and the desktop compile check itself. The `ci-quality-gates`
job fails unless every independent quality job succeeds; configure that job as
the required branch-protection check. [src: file: .github/workflows/ci-test.yml:52-299] [src: file: .github/workflows/ci-test.yml:756-780]

The backend performance observer publishes the duration for every eligible run
and reports a warning rather than failing a green functional run when the hot
cache SLO exceeds 15 minutes. Hot measurements restore only a bounded staging
cache of ordinary Cargo debug artifacts alongside Cargo downloads; coverage,
incremental, temporary, and other target trees are not archived. A hot request
that misses that cache is published as `warmup/miss` and excluded from
historical hot statistics. Cold measurements use a unique cache key per run
attempt, restore no compiled artifacts, and are reported only for their current
run. Historical hot statistics use only successful same-branch pull-request
runs whose job records a restored compiled cache. [src: file: .github/workflows/ci-test.yml:84-157] [src: file: scripts/ci/backend_ci_slo.mjs:47-120]
The observer's Node unit test runs in the blocking `test-python` gate.
[src: file: .github/workflows/ci-test.yml:383-401]

Run `CI Tests` manually with `cache_mode=hot` for a warmed compiled-artifact
measurement or `cache_mode=cold` for a current-run-only cold measurement.
Record the published job and step timing table from each run here before
comparing a change; do not combine cold measurements with historical hot
statistics or infer timings from a different runner class.

| Measurement | Sample window | Job/step durations | Median | P95 | Consecutive SLO breaches |
| --- | --- | --- | --- | --- | --- |
| Before | Pending first representative run | Published in Actions summary | Pending | Pending | Pending |
| After | Pending first representative run | Published in Actions summary | Pending | Pending | Pending |

Every job in the CI workflow has a 30-minute technical timeout. The required
aggregate always runs, includes `require-ci-label`, and fails when the label is
removed or any other gate is skipped or fails. A timeout is a functional
failure; the SLO observer does not retry, sleep, or mask it.
[src: file: .github/workflows/ci-test.yml:37-55] [src: file: .github/workflows/ci-test.yml:756-780]

## Test placement

| Change | Primary coverage |
|---|---|
| Pure Rust function | Unit test in the same module or its sibling `*_test.rs` |
| HTTP route / persistence contract | `backend/tests/` or the relevant DB test module |
| React hook / component | Adjacent `__tests__/` suite with Testing Library |
| Cross-page browser behavior | `frontend/e2e/specs/` using stable roles or `data-*` test hooks |
| Shell helper | `tests/bats/` |
| Python MCP/helper script | Its stdlib unittest suite under `backend/scripts/` |
| Desktop sidecar bootstrap | `backend/sidecars/docs/test_build_bundle.py` plus the platform build smoke test |
| Database migration | Migration registry test plus an upgrade/backfill assertion |

Use `frontend/src/test/apiMock.ts` for the shared frontend API mock. Its
completeness guard fails when a new API export is missing. Use the extended
Playwright fixture in `frontend/e2e/fixtures/kronn-fixture.ts` unless the test
explicitly owns boot/setup behavior.

## 0.9.4 interaction regression map

- `AgentSwitchPicker` tests cover the shared agent × reasoning-tier selection.
- `MarkdownComposerTools` tests cover edit/preview tabs, help disclosure,
  Markdown insertion and emoji examples.
- `NewDiscussionForm`, `ChatInput`, `QuickPromptForm` and `WorkflowWizard`
  suites cover their integration with those shared controls.
- Workflow wizard unit and browser suites cover step types, direct navigation,
  save/cancel availability and advanced-mode progressive disclosure.
- Multi-model discussion E2E covers one placeholder and one ordered reply slot
  per durable target, including late local-model replies.
- Backend runner and discussion tests cover exact provider/model attribution,
  target-tier persistence, LiteLLM failure diagnostics and explicit MCP
  discussion routing.

## 0.9.5 reliability regression map

- `backend/src/agents/runner_test.rs` pins the leading-thinking filter across
  split chunks, unclosed private reasoning and legitimate later literal tags.
- Discussion routing tests pin independent new-discussion fan-out, explicit
  handoff markers, duplicate suppression and collaboration policy.
- `frontend/src/hooks/__tests__/useWebSocket.test.ts` pins first connect,
  reconnect resync, pong deadlines, half-open close, backoff reset, stale socket
  callbacks and unmount cleanup.
- `frontend/src/components/__tests__/BackendStatus.test.tsx` pins fast outage
  recovery plus `online` and tab-visibility probes without healthy-state noise.
- `frontend/src/pages/__tests__/DiscussionsPage.test.tsx` pins the reconnecting
  explanation, active-room resync, interrupted-run cleanup and pre-receipt send
  rollback.
- `frontend/e2e/specs/ws-reconnect.spec.ts` proves the global outage indicator
  appears and clears in a real browser.
- `frontend/e2e/specs/disc-send-receipt-resilience.spec.ts` proves a failed
  pre-receipt send restores the exact draft and removes the optimistic message.

## 0.9.6 reliability regression map

- `backend/src/api/disc_prompts.rs` tests pin first-turn Planning discovery,
  explicit CLI discussion targeting, native HTTP-agent instructions and Vibe's
  honest human-gated fallback.
- `backend/src/api_tests.rs` proves an HTTP agent can read a plan and create an
  idempotent task in the current discussion while Kronn owns discussion scope,
  actor identity and source-message provenance.
- `backend/sidecars/docs/test_build_bundle.py` pins Windows UCRT64 precedence,
  the dynamic `setup-msys2` install location, loader diagnostics and the rule
  that Cargo caches never archive Python/PyInstaller output. Desktop CI builds
  the sidecar before Rust,
  verifies each DMG checksum, mounts it and strictly verifies the contained
  application signature.
- `WorkflowDetail.steps.test.tsx` pins the workflow step inspector's default
  Preview tab, shared focused editor, save/refresh path and draft cancellation.
- `backend/src/db/agent_dispatch.rs` pins distinct queued, claimed,
  agent-started and settled timestamps. `backend/src/workflows/batch_step.rs`
  expires a real eight-child BatchQuickPrompt under an accelerated active-time
  budget and proves that all eight dispatches settle as cancelled, none remains
  active and no discussion retains `awaiting_agent`.

## 0.9.7 reliability regression map

- Discussion-prompt, MCP-initialization and join-protocol tests pin the shared
  rich-output contract: Mermaid diagrams, sandboxed HTML previews and
  CSV/XLSX/PPTX export are discoverable by native and CLI agents without
  loading the full document-generation manual. Mermaid component tests also
  pin shared, bounded zoom controls across inline and fullscreen rendering.
- Discussion dispatch, component and browser tests pin a durable attributed
  error for an unavailable native agent, the shared model/provider diagnostic
  card, and an idempotent one-target retry anchored to the original turn. A
  failed LiteLLM target cannot replay successful Claude, Codex or Ollama
  siblings, and legacy structured 404 messages remain parseable.
- Discussion-session and peer-wait suites pin expected-room resume, credential
  rotation rollback, cursor-based peer receipts and content-free awareness.
- Workflow workspace, dispatch and restart suites pin shared ownership leases,
  inherited child references, safe terminal cleanup and stale-child cancellation.
- Template tests exercise every executing step family and reject unknown keys,
  unsupported filters and unclosed placeholders before side effects.
- LiteLLM workflow integration performs a real two-request tool loop against a
  scripted OpenAI-compatible server; catalogue tests pin project/global API and
  Quick API scope, read-only Planning and secret-free durable receipts.
- Plugin portability unit and browser tests require explicit post-import scope
  confirmation and prove the default Global choice reaches the persisted config.
- Project audit tests distinguish legacy evidence, bootstrap, completed audit,
  human attestation and validation; Context Audit tests pin persisted drift.
- `backend/sidecars/docs/test_build_bundle.py` pins Windows UCRT discovery and
  diagnostics. Desktop CI additionally rejects any incomplete or empty
  four-platform installer matrix.
- The axe browser suite scans Projects, Discussions, Plugins, Workflows and
  Settings against a zero serious/critical baseline and attaches exact targets.

The browser tests deliberately simulate network boundaries without launching a
paid agent. Unit and integration suites own transport edge cases; Playwright
owns the assembled UI contract. Restarting the CI runner's backend process from
inside a browser spec is intentionally avoided because it couples the test to
process ownership and creates a flaky global side effect.

## Useful focused commands

```bash
cd frontend
pnpm vitest run src/hooks/__tests__/useWebSocket.test.ts
pnpm vitest run src/components/__tests__/BackendStatus.test.tsx
pnpm vitest run src/pages/__tests__/DiscussionsPage.test.tsx
pnpm playwright test e2e/specs/ws-reconnect.spec.ts \
  e2e/specs/disc-send-receipt-resilience.spec.ts
```

```bash
cd backend
cargo test leading_thinking_filter
cargo test discussion
```

## Tooling

- Node: package constraint `>=23.6.0`; CI uses Node 24.
- Package manager: the `packageManager` field pins pnpm.
- Frontend unit runner: Vitest with happy-dom and Testing Library.
- Browser runner: Playwright Chromium by default; see
  `frontend/playwright.config.ts` and `frontend/e2e/README.md`.
- Coverage: `cd frontend && pnpm test:coverage`; backend coverage runs in CI.
