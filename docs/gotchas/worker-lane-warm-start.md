# Worker lane warm start (KT-1250)

A Claude task worker builds in its own lane (`<repo>/target/wt-<lane>`, see
[claude-task-worker-e2big.md](claude-task-worker-e2big.md)). Kronn warms that
lane before the worker starts: it clones a warm `debug/` into it, runs the
first build under a host-wide cap, then dispatches the worker.

## What happens at launch

1. The execution, its child room and its worktree are persisted.
2. If a first build has to run (the worktree has a `Cargo.toml` and a
   `Cargo.lock`, and the lane is not already built for that lock), the launch
   **returns at once**, the execution still `Provisioning`, journalled as
   `lane_warming`. `task_exec_status` reports `progress.phase = "warming"` and
   `queued_since`. The silent-worker watchdog only covers `Working`, so this
   phase is the visibility for a lane that is stuck in `Provisioning`.
3. A background task then seeds the lane target, runs `cargo test --no-run
   --locked --offline` through the host gate, smoke-checks the toolchain,
   writes the brief and takes the `Provisioning -> Working` checkpoint that
   dispatches the worker.

Failure modes, all ending in a (cold) dispatch:

- a failed or timed-out first build is journalled in `lane_warm_start` and the
  worker still starts;
- a queue that does not drain is given up after
  `KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS` (default 30 min) and the worker
  starts with the target it has;
- a backend restart during the warm-up is the ordinary boot recovery
  (`ResumeProvisioning`): the saga skips what is persisted and warms again; the
  dispatch is made only by the checkpoint compare-and-swap, so it happens once;
- an unexpected error of the background task parks the execution `Blocked`
  (resumable), never silently `Provisioning`.

## Seeding rules

The source is the most recently warmed `target/wt-*` holding a `debug/` for the
same `Cargo.lock` hash **and** the same `CARGO_HOME`, else the main checkout's
`target/` (when the lane uses the host's `CARGO_HOME`). A target is never a
source while a build holds its Cargo lock. Where the file system cannot clone
(`clonefile(2)` on APFS, a reflink elsewhere) the lane starts empty.

## Measured on this host (15 Oct 2026, load average 9-30, `CARGO_BUILD_JOBS=4`)

`cargo test --no-run --locked --offline` of `backend/`, main `target/debug` is
103 GB:

| Run | Wall time | `Compiling` lines |
| --- | --- | --- |
| cold, host `CARGO_HOME` | 373 s / 290 s | 353 / 353 |
| clone of main target, host `CARGO_HOME` | 313 s / 179 s | 1 / 1 |
| clone of main target, **lane-private** `CARGO_HOME` | aborted after 10 min | 353+ (everything) |

The clone itself takes 14-38 s and no disk space.

## Gotchas

- **The registry path is part of the fingerprint.** With a private
  `CARGO_HOME`, Cargo logs `dirty: PathToSourceChanged` for every registry
  crate: the clone is worthless. This is why lanes use the host's home.
- **The workspace crate is always rebuilt.** Checked-out sources are newer than
  the cloned dependency info, and a worktree has another path. The gain is the
  dependency graph (353 crates), not the `kronn` crate (about 3 minutes on this
  host). Do not backdate source mtimes to hide this: Cargo would then trust
  stale artifacts.
- **`ps`/`pgrep` are not available to a sandboxed worker**; measure with the
  `Compiling` count and wall time.

## Knobs

| Variable | Default | Effect |
| --- | --- | --- |
| `KRONN_WORKER_CARGO_JOBS` | performance cores / 2 | `CARGO_BUILD_JOBS` of a worker |
| `KRONN_WORKER_MAX_CONCURRENT_BUILDS` | 2 | first builds running at once on this backend process |
| `KRONN_WORKER_FIRST_BUILD_TIMEOUT_SECS` | 1800 | bound of the first build and of its queue wait |
| `KRONN_WORKER_PRIVATE_CARGO_HOME` | unset | `1` keeps KT-1246's lane-private `CARGO_HOME` (no seeding) |
