# GitHub Actions cache: pull-request caches are private and evict the shared one

**Symptom.** Every `test-backend` run recorded `Record compiled cache warmup
miss` (runs 38073563724, 38371329844, 38372290379, 38519844252 on 2026-10-13
and 2026-10-14) and recompiled the whole instrumented workspace, 21 to 26
minutes, even when `backend/Cargo.lock` had not changed.

**Cause.** Three rules of Actions caching combined:

1. A cache created by a `pull_request` run is scoped to that pull request's
   merge ref. Other pull requests cannot restore it; only the default branch
   cache is shared with every branch.
   [src: url: https://docs.github.com/en/actions/writing-workflows/choosing-what-your-workflow-does/caching-dependencies-to-speed-up-workflows]
2. The repository budget is 10 GB, evicted by last access. Each pull request
   was saving its own `cargo-hot`, `clippy`, `e2e-dev`, `release` and two
   `portability` caches, 2 to 4 GB per PR, so the `main` copies were the
   oldest and left first (`gh cache list` showed 10.29 GB used and no Rust
   cache left under `refs/heads/main` on 2026-10-14).
3. Keys hashed the exact `backend/Cargo.lock` without `restore-keys`, so any
   lockfile change was a full cold build, and `actions/cache` only saves when
   the job succeeds, so a red run never seeded anything.

**Rule.** Rust caches use `actions/cache/restore` on every run with a
same-OS `restore-keys` prefix, and `actions/cache/save` only on
`push` to `main` (`if: success()` for the clippy save, skipped on an exact hit;
the backend save runs under `always()` but only with its verified `ready`
marker, and is likewise skipped on an exact hit). A push to `main` builds and
saves only: its tests already ran on the pull request. The backend
observer reports a fallback restore as `partial`, excluded from exact hot
history, so the 10-minute SLO statistics stay honest.
[src: file: .github/workflows/ci-test.yml:102-116#cache-backend-hot]
[src: file: .github/workflows/ci-test.yml:243-262#mark-backend-cache]
[src: file: .github/workflows/ci-test.yml:279-291#cache-backend-hot]
[src: file: .github/workflows/ci-test.yml:458-491#cache-clippy]
[src: file: scripts/ci/backend_ci_slo.mjs:96-104#effectiveMeasurementMode]
[src: file: scripts/ci/backend_ci_slo.mjs:179-185#comparableRuns]

**Check.** `gh cache list --json key,ref,sizeInBytes` must show the Rust
caches under `refs/heads/main` and the total under the 10 GB budget. A pull
request whose lockfile matches `main` must log `Record compiled cache hit`.

Related: [testing-quality](../testing-quality.md).
