# Windows boot gate and nightly lib suite (KT-1629)

#509 / KT-1625 shipped because nothing in CI ever started Kronn on Windows: the
pre-migration backup failed with os error 5 at boot, the local service stopped
and the desktop showed its startup-error screen. Three things now stand between
that class of bug and a release.

## 1. `test-boot-windows` (ci-build.yml, every backend/ or frontend/ change)

Builds the real `kronn.exe` on `windows-latest` and runs
`frontend/e2e-boot/first-run.spec.ts` against it, **with no agent configured**
(a fresh install, and CI; `KRONN_E2E_EXPECT_NO_AGENT=1` fails the run if the
runner has an agent CLI):

| Case | What it proves |
|---|---|
| Fresh data directory | boots, `/api/health` 200, onboarding renders, then the dashboard; Settings, Projects and Discussions open; no 5xx; no error screen |
| Previous release database | `backend/tests/fixtures/db-previous-release.sqlite` (0.15.2, migrations pending) is backed up to `kronn.db.backup`, upgraded, and boots with `/api/health` 200 |
| Same fresh instance | a project, a manual backup (`POST /api/db/backup`) and a sealed secret (`POST /api/config/api-keys`) are read back after a restart; the plaintext secret is never in `config.toml` |

The UI is the one the binary serves: `KRONN_FRONTEND_DIST=<frontend/dist>` makes
a headless `kronn` mount the built frontend next to its API, the same mount as
the desktop's embedded server (`backend/src/main.rs`, `frontend_dist_dir`).
Unset, nothing changes.

`boot-scope` skips the job on a pull request that touches neither `backend/` nor
`frontend/` (nor `ci-build.yml`); `ci-build-gates` accepts that skip. Like every
build job it runs once the pull request carries the `ci-build` label.

### Run it locally

```bash
cd backend && cargo build --bin kronn
cd ../frontend && pnpm build && pnpm test:boot --empty-home
```

`pnpm test:boot` defaults to `<target>/debug/kronn` and `frontend/dist`;
`--empty-home` keeps Kronn's first-run scan out of your repositories.

### The upgrade fixture

`db-previous-release.sqlite` is a WAL-mode database at the last migration of the
previous release (`287_quota_fallback_refusals`, 0.15.2) with one project and one
discussion. It does not change from release to release: every new migration is
one more pending migration. `db::upgrade_fixture_tests` keeps it honest (it must
stay older than the code, and upgrading a copy must back it up and keep its
data). Rebuild it only if it stops being a valid database:
`cargo test --lib upgrade_fixture_tests::regenerate -- --ignored`.

### Proving the gate

The gate is only worth something if it fails on the bug it exists for. Dispatch
`CI Build` (Actions, "Run workflow") with `ref` on a commit that has
`test-boot-windows` but not the KT-1625 fix (`fa7d7d7b8`): the upgrade case must
go red with the os error 5 in its attached `kronn.log`. Then run it on a ref that
has the fix: green. The first run of the pull request that added the job is that
proof, as long as the fix has not merged yet.

## 2. A test skipped on Windows is `#[cfg(unix)]` with a reason

`scripts/ci/lint_windows_skips.py` (job `lint-windows-skips`, unit tests in
`scripts/ci/test_lint_windows_skips.py`) refuses, in `backend/src` and
`backend/tests`:

- a unix-only test whose gate has no reason: write
  `#[cfg(unix)] // what Windows lacks` (or a `//` comment on the line above; a `///`
  doc comment is not a reason);
- any other way to skip on Windows: `cfg(not(windows))`, `cfg_attr(windows, ignore)`,
  or `if cfg!(windows) { return }` in the body.

`python3 scripts/ci/lint_windows_skips.py --list` prints the inventory with its
reasons. A reason that starts with "unverified on Windows" was written before the
first nightly run: when the nightly shows the test passes there, drop the gate.

## 3. Nightly: the full lib suite on Windows

`ci-nightly-windows.yml` runs `cargo nextest run --lib` (profile `ci`, one process
per test, `--build-jobs 2` because the full lib-test binary needs about 13 GB of
rustc, see [ci-macos-portability-memory](../gotchas/ci-macos-portability-memory.md))
at 02:17 UTC and on demand. Every failure is a Windows defect: fix it, or ticket it
and gate the test `#[cfg(unix)] // see KT-<n>`. The JUnit report is attached to the
run.
