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

## Reviewing a worker delivery

Record the exact delivered HEAD and run checks after the final test/fixture
edits. A passing Vitest run does not type-check its fixtures: also run the
frontend TypeScript build check (`tsc -b`) on that same HEAD. Locale parity
alone cannot detect a key absent from every dictionary; the i18n lint also
checks literal `t(...)` usages against the shipped keys. Reuse an existing
label where its meaning matches instead of inventing a second spelling.
`[src: file: frontend/scripts/lint-i18n.mjs:104-164]`

With multiple worktrees sharing a Cargo target directory, verify the crate
path and the selected regression-test count. A filtered command that succeeds
with zero tests is not evidence for that regression; a prior worktree's result
must not be attached to a different delivered HEAD. Resolve any stale-artifact
ambiguity before approving, without deleting another worker's shared cache.

For filter controls used by a memoized request callback, test changing only
that filter while keeping the query and other inputs unchanged. Changing the
query as well can rebuild the callback and hide a missing filter dependency.
`[src: file: frontend/src/components/__tests__/GlobalSearchPanel.test.tsx:169-190]`

## Release gate

Run from the repository root unless a working directory is shown.

| Layer | Command | Required result |
|---|---|---|
| Version surfaces | `make check-version` | Every manifest, README, site and the first changelog release agree |
| Diff hygiene | `make check-diff` (`scripts/check-diff.sh`: `git diff --check` from the merge base with `origin/main`, plus staged and unstaged changes; CI job `diff-hygiene`) | No whitespace errors or conflict markers |
| Rust formatting | `cd backend && cargo fmt --all -- --check` | Clean |
| Rust lint | `cd backend && cargo clippy --all-targets -- -D warnings` | Zero warnings (third-party code-generation parser notices are not clippy diagnostics) |
| Backend tests | `make test-backend` (`cd backend && cargo nextest run --workspace`: library, binary and integration tests, one process per test, with CI's runner and `backend/.config/nextest.toml`, then `cargo test --doc`) | Entire Rust suite passes |
| Backend coverage | `make test-backend-cov` (`cargo llvm-cov nextest` to a JSON summary, then `backend/scripts/ci/coverage_floors.py check`: 83 % total and the key-management file floors) | Both floors hold |
| Python helpers | `make test-python` | Entire helper suite passes |
| Shell | `make test-shell` | Entire bats suite passes |
| Frontend native TS | `cd frontend && pnpm typecheck:native` | Clean |
| Frontend legacy TS | `cd frontend && pnpm typecheck:legacy` | Clean |
| Frontend ESLint | `cd frontend && pnpm lint` | Zero errors; CI's pinned warning budget must not increase |
| Frontend fast lint | `cd frontend && pnpm lint:fast` | Zero warnings |
| i18n | `cd frontend && pnpm lint:i18n` | `fr`, `en`, `es` and `zh` have matching, valid keys |
| Theme tokens | `cd frontend && pnpm lint:theme` | Every text role meets its WCAG floor in every theme; no undefined `--kr-*`, literal white/black text on a token fill, or removed `:focus-visible` ring |
| Frontend unit/integration | `make test-frontend` | Entire Vitest suite passes |
| Frontend production build | `cd frontend && pnpm build` | TypeScript and Vite build succeed |
| Browser E2E | `make test-e2e` | Entire Playwright suite passes against the expected backend fixture |

`make test-backend-lib` runs the library unit tests only: a quick loop, not
evidence for the gate. `export_bindings` is skipped locally (the default
nextest profile filters it) so the ts-rs bindings stay untouched; CI's `ci`
profile runs it and checks generated-type drift. `make install-dev-tools`
installs cargo-nextest and cargo-llvm-cov; without nextest, `make test-backend`
stops and says so rather than fall back to `cargo test`: the integration tests
share one binary and assume a process per test. Under nextest every test is its own
process: an in-process lock (`#[serial]`, `ENV_LOCK`) no longer protects a
fixed path shared with another test or another run, so a test creates its own
directory (a temporary one, or a fixed name suffixed with the process id), and
`config_dir()` in a unit test without `KRONN_DATA_DIR` is a per-process
temporary directory, never the real install.
`[src: file: Makefile:352-390]` `[src: file: backend/.config/nextest.toml:1]`
A local green does not prove a CI green for tests that read the machine
(installed CLIs such as LiteLLM, RTK or `npx`, the agent preflight): reproduce
a CI-only failure in `rust:1-bookworm` before concluding.

CI also checks dependency audit, generated-type drift, desktop compilation and
repository-specific Rust safety lints. `.github/workflows/ci-test.yml` is the
authoritative job graph.

Run both frontend linters: passing ESLint or TypeScript does not imply the
zero-warning Oxlint gate passes. The `with_conn` safety scanner also reads
standalone Rust test files; a `#[cfg(test)]` on their parent module does not
exclude their contents. Propagate fallible fixture setup inside the closure
and assert its result after awaiting it.
`[src: file: backend/scripts/ci/lint_with_conn_unwrap.py:79-115]`
`[src: file: .github/workflows/ci-test.yml:449-454]`

Supervisor cleanup must also reap live shell jobs whose PID was not yet recorded:
a termination signal can arrive between the fork and that assignment, leaving
a child alive and keeping inherited test pipes open. The shell regression injects
TERM at this boundary for both the backend and the watcher.
`[src: file: scripts/dev-backend-supervisor.sh:54-62]`
`[src: file: tests/bats/ui.bats:560-644]`

## Backend CI timing SLO

The backend suite runs once, in three stages. `build-backend-tests` checks
formatting, compiles every library, binary, example and integration test
target with coverage instrumentation, runs the raw-command lint on that build
and uploads one nextest archive (`cargo llvm-cov nextest-archive`).
`test-backend-partition` runs `hash:K/N` of the archive under `NEXTEST_PROFILE=ci`
and uploads its JUnit report, a manifest (commit, archive digest) and its
profraw pool. `test-backend`, the aggregate that keeps the check name, fails
unless the build and every partition succeeded and
`backend/scripts/ci/backend_partitions.py verify` finds every partition from
the same archive and commit, with each test of the archive's inventory run
once and green; it then merges the profiles and checks the floors once with
`coverage_floors.py`. `test-backend-types` runs the ts-rs exports from the
archive and checks generated-type drift. N comes from `BACKEND_TEST_PARTITIONS`
(manual input `backend_partitions`, repository variable `CI_BACKEND_PARTITIONS`,
default 2). A one-partition manual run of the same commit gives the
non-partitioned reference: `coverage_floors.py compare` on the two
`backend-coverage-summary` artifacts must find no difference. The timing
observer measures `build-backend-tests` start to `test-backend` end. Before
this layout the suite ran in one `test-backend` job, and before that twice
(`cargo test`, then instrumented in `test-backend-coverage`); the history
below measures those older layouts.
Clippy and the project-specific budget checks run in `test-backend-quality`,
in parallel, and stay blocking through `ci-quality-gates`.
Its hot cache targets the three reusable directories of the instrumented
tree (`target/llvm-cov-target/debug/{.fingerprint,build,deps}`); a warmup only
validates them and writes a versioned marker (`.kronn-backend-cache-v3`)
before the cache action's post-job save. Profraw files, the nextest store and
other transient trees are never archived.
[src: file: .github/workflows/ci-test.yml:71-228]

The backend performance observer publishes the duration for every eligible run
and reports a warning rather than failing a green functional run when a hot
run exceeds the 10-minute SLO. Cargo's repository-level `target-dir` means
these artifacts live under the root `target/`, not `backend/target/`. A
versioned sentinel and the three required artifact directories must be present
before an Actions cache hit is accepted. A hot request that misses that cache
is published as `warmup/miss` and excluded from historical hot statistics.
Cold measurements use a unique cache key per run attempt, restore no compiled
artifacts, and are reported only for their current run. Historical hot
statistics use only successful same-branch pull-request runs whose job records
a verified restored compiled cache. The v3 key cannot restore the former
debug layout, so the first run is an explicit warmup.
[src: file: .cargo/config.toml:1-6] [src: file: scripts/ci/backend_ci_slo.mjs:1-153]
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

Ordinary jobs in the CI workflow have a 30-minute technical timeout.
`test-backend` has 35 minutes to let a compiled-cache miss finish its post-job
upload; verified hot runs stay subject to the independent 15-minute SLO.
Coverage and E2E have 45 minutes for their instrumented and release builds.
The macOS portability job also has 45 minutes: its lib-test rebuild took
29m13s in [run 36912793272](https://github.com/DocRoms/Kronn/actions/runs/36912793272/job/110539336655),
then the first suite passed as the former 30-minute limit cancelled the job,
leaving the maintenance and durable-cleanup suites unexecuted. The additional
budget covers all three suites and cache saving; Windows retains 30 minutes.
The required aggregate always runs,
includes `require-ci-label`, and fails when the label is removed or any other
gate is skipped or fails. A timeout is a functional failure; the SLO observer
does not retry, sleep, or mask it. An SLO breach is a warning, while missing,
duplicate, incomplete, or contradictory measurement evidence fails the
observer instead of publishing a misleading timing.
[src: file: .github/workflows/ci-test.yml:37-59] [src: file: .github/workflows/ci-test.yml:750-778]

[src: file: .github/workflows/ci-test.yml:719]

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

Agent access and tier controls are mounted only when their settings card is
expanded. Browser specs should call `SettingsPage.openAgentConfiguration()`
before inspecting those controls; it waits for the mounted body and leaves an
already-open card open without changing any setting.
[src: file: frontend/e2e/pages/SettingsPage.ts:16]

Media browser specs use `openMediaLauncher()` to open the current panel rail
and explicitly select the test's local provider slot. Restoring an already-open
panel must not toggle it closed; relying on the default provider could launch a
real generation instead of the test stub.

Discussion run-card coverage drives the four inline source-message actions,
not the removed attached-runs strip. Read each expanded card inside the viewport:
off-screen cards intentionally defer hydration. A real wheel gesture cancels
the initial bottom-settling window before walking earlier messages.

The Settings axe scan supplies a populated, typed usage report at the external
collector boundary and waits for its cost and filter controls before scanning.
It tests the rendered usage UI, not the `ccusage` process or private operator
history; collector timeout behavior needs separate backend coverage.

`pnpm lint:theme` measures tokens statically: in the dark themes every text
role and status colour needs 4.5:1 on the surface ramp, text roles also on a
hover chip over elevated; elsewhere faint/dim/ghost need 3:1. A fallback does
not excuse an undefined `--kr-*`; component-scoped properties drop the prefix.
White or black text pinned on a `--kr-*` fill is refused in stylesheets and in
inline `style={{ }}` objects alike.
`a11y-dark-themes.spec.ts` renders the main screens and a native action card in
`dark`, `gotham` and `matrix`, re-measures the text axe leaves undecided behind
gradients or pseudo-elements, and walks keyboard focus. It answers every non-GET
with 503 and stubs the WebSocket, so it may run read-only against a live backend.
`[src: file: frontend/src/styles/themeAudit.ts:1-60]`
`[src: file: frontend/e2e/specs/a11y-dark-themes.spec.ts:1-60]`

When a disposable backend runs in a container, run browser specs with local
provider stubs in the same network namespace. Their loopback callbacks then
reach the stubs, and destructive fixture cleanup uses the backend's real local
trust boundary. Do not spoof forwarding headers or weaken authentication to
make a remote runner look local. Keep this stack free of host credentials and
production data, serialize suites sharing its database, and collect artifacts
from a dedicated output directory.

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
- The same desktop sidecar build freezes `kronn-mcp` and runs
  `backend/sidecars/mcp/smoke_bundle.py` on the target platform. Its relocated
  executable starts with an empty PATH, reports a fresh bridge fingerprint and
  makes a real MCP-to-HTTP call to an ephemeral backend. This complements the
  Rust launch/configuration tests and Python stale-bridge tests; installed-app
  Claude authentication and OS trust checks remain separate qualification.
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
