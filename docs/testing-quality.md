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

`test-backend` is the measured backend critical-path job: formatting and the
Rust test suite. Its hot cache targets the three reusable Cargo debug
directories directly; a warmup only validates them and writes a versioned
marker before the cache action's post-job save. Clippy, coverage,
generated-type drift and project-specific
lint/budget checks run in `test-backend-quality` and `test-backend-coverage`;
the frontend build and desktop compilation run in `test-desktop-compile`.
Those jobs execute in parallel and all remain blocking through
`ci-quality-gates`. Clippy previously ran inside `test-backend` itself, but a
real warm run still measured 17m41 against the 15-minute SLO with it inline
([run 33368662050](https://github.com/DocRoms/Kronn/actions/runs/33368662050));
moving it to the parallel, still-required `test-backend-quality` gate keeps
the lint blocking without holding it on the measured path. After that move, a
real cache-hit run still measured 17m32 (`cargo test` 11m58, ~4m13 of overhead
before the test step, 1m17 staging the cache back after it) — a further 2m32
over the 15-minute SLO
([cold 33378197164](https://github.com/DocRoms/Kronn/actions/runs/33378197164),
[hot cache-hit 33378199511](https://github.com/DocRoms/Kronn/actions/runs/33378199511)).
The first review warmup proved that conditioning cleanup and staging only on
a cache hit was insufficient: there is no hit until a warmup survives its
post-job save. That run spent 6m48 deleting unrelated runner toolchains, then
copied the debug tree after a successful 21m54 test step and hit the 30-minute
timeout before the cache could be saved. The measured job therefore performs
neither operation in any mode. It caches `.fingerprint`, `build` and `deps`
in place, which removes the second full copy and its temporary disk doubling;
coverage and desktop jobs retain their own cleanup where they need it.
The first direct-layout warmup then completed its 27m55 test step and wrote the
v2 marker, but GitHub cancelled the cache upload 1m41 later at the job's former
30-minute ceiling. `test-backend` therefore has a 35-minute one-time seeding
budget. This does not relax the hot-path target: only a verified restored cache
is an SLO sample, and that sample must still fit within 15 minutes.
[src: file: .github/workflows/ci-test.yml:52-265] [src: file: .github/workflows/ci-test.yml:750-778]

The backend performance observer publishes the duration for every eligible run
and reports a warning rather than failing a green functional run when the hot
cache SLO exceeds 15 minutes. Hot measurements restore only bounded ordinary
Cargo debug artifacts alongside Cargo downloads; coverage,
incremental, temporary, and other target trees are not archived. Cargo's
repository-level `target-dir` means these artifacts are copied to and from the
root `target/debug` tree, not `backend/target/debug`. A versioned sentinel and
the three required artifact directories must be present before an Actions
cache hit is accepted. A hot request that misses that cache is published as
`warmup/miss` and excluded from historical hot statistics. Cold measurements
use a unique cache key per run attempt, restore no compiled artifacts, and are
reported only for their current run. Historical hot statistics use only
successful same-branch pull-request runs whose job records a verified restored
compiled cache. The v2 cache key cannot restore the former staging layout, so
the first run is an explicit warmup rather than an ambiguous hit.
[src: file: .cargo/config.toml:1-2] [src: file: .github/workflows/ci-test.yml:84-188] [src: file: scripts/ci/backend_ci_slo.mjs:47-150]
The observer's Node unit test runs in the blocking `test-python` gate.
[src: file: .github/workflows/ci-test.yml:299-324]

Run `CI Tests` manually with `cache_mode=hot` for a warmed compiled-artifact
measurement or `cache_mode=cold` for a current-run-only cold measurement.
Record the published job and step timing table from each run here before
comparing a change; do not combine cold measurements with historical hot
statistics or infer timings from a different runner class.

| Measurement | Run | Result | Job/step durations | Median | P95 | Consecutive SLO breaches |
| --- | --- | --- | --- | --- | --- | --- |
| Before — cold (monolithic job) | [GitHub Actions run 33354549462](https://github.com/DocRoms/Kronn/actions/runs/33354549462) | Failed at a separate flaky test; not an SLO sample | `test-backend`: 19m 46s before coverage/desktop completed; disk cleanup 1m 54s, clippy 3m 03s, library tests 14m 33s | Unavailable | Unavailable | Unavailable |
| Before — warmup (monolithic job) | [GitHub Actions run 33354667035](https://github.com/DocRoms/Kronn/actions/runs/33354667035) | Warmup evidence only; no cache-hit duration supplied | Library tests passed; coverage and desktop remained sequential, so the cache staging step could be pre-empted by the 30-minute timeout | Unavailable | Unavailable | Unavailable |
| After split, before clippy move — cold | [GitHub Actions run 33358154793](https://github.com/DocRoms/Kronn/actions/runs/33358154793) | Green | `test-backend` (format, clippy, tests, parallel coverage/quality/desktop gates); cold, no compiled-cache restore | Unavailable (single sample) | Unavailable (single sample) | 0 (not a hot sample) |
| After split, before clippy move — warmup | [GitHub Actions run 33358156450](https://github.com/DocRoms/Kronn/actions/runs/33358156450) | Green; bounded cache saved | `test-backend` warmup/miss; staged the compiled-artifact cache for a subsequent hot run | Unavailable (single sample) | Unavailable (single sample) | 0 (warmup, excluded from hot history) |
| After split, before clippy move — hot | [GitHub Actions run 33368662050](https://github.com/DocRoms/Kronn/actions/runs/33368662050) | Green; explicit compiled-cache hit | `test-backend`: **17m 41s total, still a 2m 41s SLO breach** with format + clippy + tests sharing one job | Unavailable (single sample) | Unavailable (single sample) | 1 |
| After clippy moved to `test-backend-quality` — cold | [GitHub Actions run 33378197164](https://github.com/DocRoms/Kronn/actions/runs/33378197164) | Green | `test-backend`: 17m 33s total; cold, no compiled-cache restore | Unavailable (single sample) | Unavailable (single sample) | 0 (not a hot sample) |
| After clippy moved to `test-backend-quality` — hot (cache-hit) | [GitHub Actions run 33378199511](https://github.com/DocRoms/Kronn/actions/runs/33378199511) | Green; explicit compiled-cache hit | `test-backend`: **17m 32s total, still a 2m 32s SLO breach** — `cargo test` 11m 58s, ~4m 13s overhead before the test step (checkout, cache restore, disk cleanup, toolchain install), 1m 17s staging the cache back after the test | Unavailable (single sample) | Unavailable (single sample) | 1 |
| Review warmup with conditional cleanup/staging | [GitHub Actions run 33416135756](https://github.com/DocRoms/Kronn/actions/runs/33416135756) | Timed out; cache post-step skipped | `test-backend`: 30m21s; cleanup 6m48s, `cargo test` 21m54s, staging cancelled after 1m13s; no hot cache seeded | Unavailable | Unavailable | 0 (warmup, excluded) |
| Review cold measurement | [GitHub Actions run 33416622439](https://github.com/DocRoms/Kronn/actions/runs/33416622439) | Green | `test-backend`: 16m54s; cleanup 1m16s, `cargo test` 15m22s; compiled artifacts intentionally neither restored nor saved | Unavailable (single sample) | Unavailable (single sample) | 0 (cold, excluded) |
| Direct bounded v2 cache — first warmup | [GitHub Actions run 33434235499](https://github.com/DocRoms/Kronn/actions/runs/33434235499) | Timed out while uploading the bounded cache | `cargo test` passed in 27m55s and the marker was written; the post-cache upload was cancelled after 1m41s by the former 30-minute job ceiling, so no hot cache was seeded | Unavailable | Unavailable | 0 (warmup, excluded) |
| Direct bounded v2 cache — successful warmup after seed-budget fix | [GitHub Actions run 33438929531](https://github.com/DocRoms/Kronn/actions/runs/33438929531) | Backend green; explicit warmup miss; bounded cache marker and post-job save completed (the aggregate failed only on the subsequently fixed frontend warning budget) | `test-backend`: **24m 25s total**; `cargo test` 21m 22s; bounded cache upload 2m 38s; no runner-toolchain cleanup and no debug-tree copy | Unavailable | Unavailable | 0 (warmup, excluded) |
| Direct bounded v2 cache — verified hot hit | [GitHub Actions run 33441299481](https://github.com/DocRoms/Kronn/actions/runs/33441299481) | **Green**; explicit compiled-cache hit; every functional, quality, coverage, E2E and portability gate passed | `test-backend`: **13m 16s total**; cache restore 1m 16s; `cargo fmt` 6s; `cargo test` 11m 46s; post-cache step 1s — **1m 44s below the 15-minute SLO** | 13m 16s (1 sample) | 13m 16s (1 sample) | 0 |
| Direct bounded v2 cache — second verified hot hit | [GitHub Actions run 33472489642](https://github.com/DocRoms/Kronn/actions/runs/33472489642) | **Green**; explicit compiled-cache hit; every functional, quality, coverage, E2E and portability gate passed | `test-backend`: **13m 33s total**; cache restore 1m 58s; `cargo fmt` 4s; `cargo test` 11m 21s; post-cache step under 1s — **1m 27s below the 15-minute SLO** | 13m 16s (2 samples) | 13m 33s (2 samples) | 0 |
| Direct bounded v2 cache — third verified hot hit | [GitHub Actions run 33474123275](https://github.com/DocRoms/Kronn/actions/runs/33474123275) | **Green**; explicit compiled-cache hit; every functional, quality, coverage, E2E and portability gate passed | `test-backend`: **13m 12s total**; cache restore 1m 13s; `cargo fmt` 4s; `cargo test` 11m 45s; post-cache step under 1s — **1m 48s below the 15-minute SLO** | **13m 16s (3 samples)** | **13m 33s (3 samples)** | **0** |

The populated rows above are real Actions evidence from this task. The
17m 32s hot cache-hit run with clippy already moved out still breached the
15-minute SLO by 2m 32s, split between pre-test overhead (disk cleanup running
unconditionally even though a cache hit needs less headroom) and post-test
cache staging that re-copies artifacts `actions/cache` will not re-save on an
exact-key hit. The subsequent review warmup then proved that both costs also
prevent the first cache from ever being seeded. The v2 layout removes them
from the measured job in every mode. Runs 33438929531, 33441299481,
33472489642 and 33474123275 now prove the complete sequence: the bounded
warmup survives its post-job save, three independent subsequent runs record
verified compiled-cache hits, and the measured backend job remains between
13m 12s and 13m 33s, with a 13m 16s median and 13m 33s P95, without removing
any blocking functional or quality gate.
[src: user: 2026-08-31: review reports GitHub Actions runs 33354549462 and 33354667035]
[src: user: 2026-08-31: reassignment reports GitHub Actions runs 33358154793, 33358156450 and 33368662050]
[src: user: 2026-08-31: escalation reports GitHub Actions runs 33378197164 and 33378199511 with cargo test 11m58, pre-test overhead 4m13, post-test staging 1m17]
[src: commit: 87d41331]

Every ordinary job in the CI workflow has a 30-minute technical timeout. The
only bounded exception is `test-backend`, whose 35-minute ceiling lets a
one-time compiled-cache miss finish its post-job upload; verified hot runs stay
subject to the independent 15-minute SLO. The required aggregate always runs,
includes `require-ci-label`, and fails when the label is removed or any other
gate is skipped or fails. A timeout is a functional failure; the SLO observer
does not retry, sleep, or mask it. An SLO breach is a warning, while missing,
duplicate, incomplete, or contradictory measurement evidence fails the
observer instead of publishing a misleading timing.
[src: file: .github/workflows/ci-test.yml:37-59] [src: file: .github/workflows/ci-test.yml:750-778]

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
