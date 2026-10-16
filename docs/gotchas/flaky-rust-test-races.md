# Flaky Rust tests: races that only show up on CI

`test-backend` runs ~10 000 tests in parallel processes under coverage
instrumentation, so timing windows invisible on a laptop open on CI. Three
seen in October 2026, with their fixes:

- **Detached `git maintenance` in fixtures.** `git commit` spawns a detached
  `git maintenance run --auto` that keeps writing inside `.git` after the
  command returns. A test that snapshots or walks `.git` races it. Fixture
  git helpers pass `-c maintenance.auto=false -c gc.auto=0`.
  [src: file: backend/src/core/project_profile_test.rs:38-53#git]
- **Idempotent retries in concurrency tests.** A second approve that reads
  the execution while it is `Approved` (before the integration task claims
  `Integrating`) is a valid retry and succeeds. Assert the durable invariant
  (one review, one claim, one validation), not which transport won.
  [src: file: backend/src/api/orchestration.rs:30145-30244#concurrent_review_retries_share_one_durable_integration_claim]
- **A copied git index loses its racy-entry protection.** `snapshot_tree`
  copied the real index with `std::fs::copy`. On Linux the copy gets a fresh
  mtime, so git no longer treats an entry as recent as the index as "racy"
  and trusts its stale stat data: a same-size rewrite in the same instant
  goes unseen. macOS clones the file and keeps its mtime, which is why the
  case never failed locally. Fixed in d528cada (#249): the copy keeps the
  real index's mtime, and a test pins the case.
  [src: commit: d528cada]

Related: [testing-quality](../testing-quality.md).
