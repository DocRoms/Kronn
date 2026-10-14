# Claude task-worker sandbox and large worktree catalogues

Claude Code `2.1.247` reproduced the sandbox-internal `E2BIG` failure during a
real task-worker run, so a successful `claude --version` command is not evidence
that sandboxed Bash can spawn. The upstream defect remains open.
[src: user: 2026-08-28: KT-492 dogfood and review feedback]
[src: url: https://github.com/anthropics/claude-code/issues/73437]
[src: url: https://github.com/anthropics/claude-code/issues/73468]

On macOS, Kronn resolves the target project's local repository and linked local
repositories, deduplicates their Git common directories, and conservatively
measures the worktrees registered by Git before provisioning a punctual Claude
worker. A catalogue over either bound is refused with the stable
`claude_sandbox_catalogue_unsafe` reason, counts and byte totals only, plus a
`task_exec_reassign` recovery instruction. A linked local repository whose
path does not exist is skipped with a warning: it has no worktree catalogue for
the sandbox to enumerate. Any other unreadable Git root still fails closed with
`claude_sandbox_catalogue_unreadable`, naming the project or linked repository
(never its path).
[src: file: backend/src/agents/runner.rs:12346-12376#ClaudeSandboxCatalogueReceipt]
[src: file: backend/src/agents/runner.rs:12382-12431#claude_sandbox_catalogue_receipt]
[src: file: backend/src/agents/runner.rs:12433-12462#claude_sandbox_catalogue_roots]
[src: file: backend/src/agents/runner.rs:12464-12471#claude_task_worker_catalogue_preflight]
[src: file: backend/src/api/orchestration.rs:9857-9867#worker_project_refusal]

The worker remains fail-closed: sandbox availability is mandatory,
unsandboxed commands are disabled, and the explicit write roots are the
canonical managed task worktree and its lane's private Cargo target (see
"Build and test policy" below). [src: file: backend/src/agents/runner.rs:12257-12292#claude_task_worker_settings]

Regression coverage uses real synthetic Git common directories to exercise
accepted and refused catalogue sizes, common-directory deduplication and
path-free diagnostics, and separately preserves the write boundary: an
unmanaged worktree is the only write root, a managed one adds only its lane
target.
[src: file: backend/src/agents/runner_test.rs:9998-10197#synthetic_git_worktree_catalogue]
[src: file: backend/src/agents/runner_test.rs:9714-9764#claude_task_worker_uses_fail_closed_workspace_sandbox]
[src: file: backend/src/agents/runner_test.rs:9778-9819#claude_task_worker_sandbox_allows_loopback_and_its_lane_without_a_network_domain]

## Build and test policy (KT-1246)

A worker that cannot build or test forces the principal to re-run every
delivery centrally. The Claude task-worker launch therefore prepares a *lane*
(`backend/src/core/task_worker_env.rs`, runtime-agnostic) and widens the sandbox
by exactly what building needs. Claude Code only: Codex and OpenCode workers
keep their own sandbox and receive no lane variables.

| Concern | Policy |
| --- | --- |
| Write roots | the worktree and `<repo>/target/wt-<lane>` (outside `backend/`: the dev backend's watcher restarts on writes there). A worktree outside the managed `<repo>/.kronn/worktrees/` layout keeps its target in `<worktree>/.kronn/target`. |
| Reads | `filesystem.allowRead`: `~/.cargo/registry`, `~/.cargo/git`, existing pnpm stores, the main checkout's `frontend/node_modules`. Never a write root. |
| Network | `network.allowLocalBinding: true` (loopback). No `allowedDomains`: no external host. |
| Fail-closed | `enabled`, `failIfUnavailable`, `allowUnsandboxedCommands: false` unchanged; an unresolvable worktree still yields an empty write list and no network key. |
| Environment | `KRONN_DATA_DIR=<worktree>/.kronn/data`, `CARGO_TARGET_DIR=<lane target>`, `CARGO_NET_OFFLINE=true`, `CARGO_BUILD_JOBS` (KT-1250), and `CARGO_HOME` when the registry is in place. No `VAR=x cmd` prefix is needed. |
| Cargo home | the host's own `CARGO_HOME`, read-only to the sandbox (KT-1250: Cargo fingerprints a registry crate by the absolute path of its sources, so a lane under another home can never reuse a warm target — see [worker lane warm start](worker-lane-warm-start.md)). `KRONN_WORKER_PRIVATE_CARGO_HOME=1` restores the KT-1246 lane-private home (`<lane target>/.kronn-cargo-home`: `registry`, `git`, `config.toml` symlinked, credentials never linked) at the price of no seeding. |
| Frontend | `frontend/node_modules` is a symlink to the main checkout's, created without any network install. Missing there: frontend tests are reported unavailable. |

Two side effects are deliberately neutralised, because both made a clean
checkout look dirty during testing: `/target/wt-*/` is added to the main
checkout's local Git exclude (the integration preflight refuses a dirty target)
and `/frontend/node_modules` to the worktree's when a project only ignores
`node_modules/` (directories only, a link is not one).

### Smoke check and capability record

Before the brief is written, the launch saga provisions the lane and runs a
bounded check with the worker's own environment: `cargo nextest --version`,
`cargo fetch --offline --locked` (proves the offline dependency cache; a cold
`cargo check` would compile the whole graph), `node_modules/.bin/vitest
--version`, and a loopback bind. The result, `rust_tests` / `frontend_tests` /
`port_binding` each `{available, reason}`, is journalled on the execution as a
`toolchain_capabilities` event (`db::orchestration::latest_toolchain_capabilities`
reads the latest) and rendered in the brief under "Capacités de test". A check
that cannot run is recorded as unavailable with the reason; it never blocks the
launch.

The check runs as ordinary child processes with the lane environment; it is not
wrapped in Claude's own OS sandbox, so it proves the toolchain and the cache,
not that the sandbox lets each syscall through. That is the live proof the
principal performs with a real worker.

### DeliveryManifest `tests_executed`

`tests_executed: {executed: bool, which: [command]}` is required in a spawned
worker's `task_exec_deliver` projection and optional for legacy and joined-CLI
deliveries. `executed: true` needs named commands and at least one `pass`/`fail`
test; `executed: false` needs `which: []` and no `pass`/`fail` test.

### Known limits

- Lane targets under `<repo>/target/wt-*` are removed with a clean worktree and
  swept at boot when stamped with `.kronn-lane.json` (see [worker lane warm
  start](worker-lane-warm-start.md)); a dirty preserved worktree keeps its lane.
- A worker needing a crate absent from the host cache cannot fetch it.
- Vite's dev server may refuse files outside its root through the
  `node_modules` link; `vitest` runs are unaffected.
