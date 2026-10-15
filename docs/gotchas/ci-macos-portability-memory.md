# The macOS portability job cannot hold the full lib-test binary in memory

## Symptom

`test-backend-portability (macos-latest)` spent 45 to 55 minutes in its first
step, `cargo test --lib core::worktree`, with a valid cache restored and a
single `Compiling kronn` line: 52m 48s of pure rustc on one crate for 98 tests
in [job 116332790208](https://github.com/DocRoms/Kronn/actions/runs/38707823085/job/116332790208).
The log is silent between the ts-rs warnings of the first seconds and the
linker warning `__eh_frame section too large (max 16MB)` at the end.
[src: file: .github/workflows/ci-build.yml:211-225]

## Cause: memory, not CPU

`cargo test --lib` compiles every unit-test module of the single `kronn` crate
(about 563 000 lines, 8 300 `#[test]` functions, a 737 MB test binary) even
when the filter runs 218 tests. Local measurements with `-j 3`, the runner's
parallelism (`/usr/bin/time -l`, rustc nightly 1.96, orders of magnitude only):

| Build | Peak RSS |
|---|---|
| full lib-test binary | 13.2 GB |
| library alone, no test module (`cargo build --lib`) | 8.9 GB |

`-Z time-passes` on the full test build puts 7.5 GB in the single-threaded
front end (after `MIR_borrow_checking`) and the peak in `codegen_crate`. The
standard macOS runner has 3 vCPU and 7 GB of RAM against 16 GB on Linux and
Windows: rustc swaps for the whole build.
[src: url: https://docs.github.com/en/actions/reference/runners/github-hosted-runners]

Two runs on the same image, cache and source compiled the same crate in 6 and
7 minutes (jobs 116344143147 and 116329327939). Nothing in the logs explains
it; the `Toolchain and machine` step now prints `hw.memsize` and `vm_stat` so
the next outlier can be classified.

## Second, independent cause: the preinstalled rustc drifts between images

The job used the runner's preinstalled toolchain, and the cache key only
hashed `Cargo.lock`. Two `macos-26-arm64` images were in rotation the same
morning: 20260907 ships Rust 1.98.1, 20261013 ships Rust 1.99.0. A cache built
on main with 1.98.1 was restored on a 1.99.0 runner, then every one of the 353
crates recompiled (jobs 116332745405, 116333107963, 116330148144,
116314818839, 116288227826). See [actions-cache-scope](actions-cache-scope.md)
for the restore-only/save-on-main contract this stacks on.
[src: url: https://github.com/actions/runner-images/blob/macos-26-arm64/20260907.0351/images/macos/macos-26-arm64-Readme.md]
[src: url: https://github.com/actions/runner-images/blob/macos-26-arm64/20261013.0424/images/macos/macos-26-arm64-Readme.md]

## Mitigation in place: the `full-tests` feature

`backend/Cargo.toml` declares a default feature `full-tests`. Every
`#[cfg(test)] mod …` outside the four portability suites (`core::worktree`,
`api::maintenance`, `db::orchestration`, `agents::wsl`) is gated with
`#[cfg(all(test, feature = "full-tests"))]`; the portability job builds with
`--no-default-features`. Test-support modules that production code reaches
under `cfg(test)` keep the plain attribute (`acp::test_support`,
`agents::runner::test_acp_routes`, `api::workflows::before_write_hook`,
`core::checksums` test file, `core::config::test_saved_access`,
`core::rooted_io::tests`).
Every other job keeps the default feature set, so Linux coverage, clippy and
the Windows matrix leg are unchanged.
[src: file: backend/Cargo.toml:23-31]

Adding a test module: use the gated attribute unless production code under
`cfg(test)` calls into it; `cargo check --lib --profile test --no-default-features`
tells you in a minute.

Tooling that finds test code by the literal `#[cfg(test)]` must accept the
gated form too: `backend/scripts/ci/lint_with_conn_unwrap.py` flagged seven
test `unwrap()`s as production code on the first CI run of this change. It
now exempts exactly the item carrying a `#[cfg(test)]` or
`#[cfg(all(test, …))]` attribute (a `mod x;` line, a `mod x { … }` block, a
helper fn) and keeps scanning the production code around it, instead of
cutting the file at the first attribute.
[src: file: backend/scripts/ci/lint_with_conn_unwrap.py:19-22]

## Measured on the runner

Same image (20260907, Rust 1.98.1), same restored cache, one crate compiled:

| Run | First step | Whole job |
|---|---|---|
| before, [job 116332790208](https://github.com/DocRoms/Kronn/actions/runs/38707823085/job/116332790208) | 52 min 48 s | 55 min |
| after, [run 38740423116](https://github.com/DocRoms/Kronn/actions/runs/38740423116) | 12 min 23 s | 14 min |
| after, [run 38744187280](https://github.com/DocRoms/Kronn/actions/runs/38744187280) | 9 min 50 s | 11 min |

The `Toolchain and machine` step confirmed 3 CPUs and 7.5 GB (`hw.memsize`
7516192768). The Windows leg went from 13 to 7 minutes.

## What this does not fix

The library alone still peaks near 9 GB locally; the gated build is lighter
than the full one but not guaranteed under 7 GB. Only splitting the crate into
a workspace or running the job on a machine with more memory (self-hosted
Apple Silicon, larger runner, or main-only scheduling) removes the swap for
good.
