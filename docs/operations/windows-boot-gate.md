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

`boot-scope` skips the job on a pull request that touches none of `backend/`,
`frontend/`, `templates/`, `desktop/`, `scripts/ci/`, `.cargo/` or `ci-build.yml`;
`ci-build-gates` accepts that skip. Like every
build job it runs once the pull request carries the `ci-build` label.

### Run it locally

```bash
cd backend && cargo build --bin kronn
cd ../frontend && pnpm build && pnpm test:boot --empty-home
```

`pnpm test:boot` defaults to `<target>/debug/kronn` and `frontend/dist`;
`--empty-home` keeps Kronn's first-run scan out of your repositories.

### The upgrade fixture

`backend/tests/fixtures/db-previous-release.sqlite` is a WAL-mode database at the
last migration of the previous release (`287_quota_fallback_refusals`, 0.15.2),
holding what a real install holds so that a data-dependent migration has rows to
rewrite: three projects (one with linked repositories and a docs directory), four
discussions with ten messages (one archived, one pinned, one in no project, usage
rows on the agent replies), a workflow, two planning tasks with a definition of
done and a linked discussion, and one `provider_key` credential sealed with a
**test key**. Nothing in it is a secret: names and paths are invented, the
credential is a public sentence sealed with a public key (both constants in
`backend/src/db/upgrade_fixture_tests.rs`), and Kronn cannot open that row, which
is also the state of a database moved to another machine: the boot must still come
up.

It does not change from release to release: every new migration is one more
pending migration. `db::upgrade_fixture_tests` keeps it honest: it must stay older
than the code; upgrading a copy must back it up and keep every kind of row (the
workflow still parses with the current models, the sealed credential is byte for
byte the same); and the committed bytes must hold no secret shape.

**Regenerating it** (only to change what it holds, never for a new release):
edit the rows in `regenerate_the_previous_release_fixture`, then

```bash
cd backend
cargo test --lib upgrade_fixture_tests::regenerate -- --ignored
git add tests/fixtures/db-previous-release.sqlite
```

and check `cargo test --lib upgrade_fixture_tests` and
`cd ../frontend && pnpm test:boot --empty-home` still pass. The rows are written
with raw SQL against the previous release's schema, because the insert helpers
write columns a later migration adds.

`KRONN_FRONTEND_DIST` pointing at a directory with no `index.html` is reported at
startup (`kronn` logs a warning), so a typo or an unbuilt frontend shows up in the
`kronn.log` the job attaches, not as a blank page.

### Proving the gate

The gate is only worth something if it fails on the bug it exists for. Dispatch
`CI Build` (Actions, "Run workflow") with `ref` on the head of the branch that
added `test-boot-windows`: it predates the KT-1625 fix (PR #510, commit
`fa7d7d7b8`, which is the fix, not a commit without it), so the upgrade case must
go red with the os error 5 in its attached `kronn.log`. Then run it once #510 is
in the branch (or on `main` after both merged): green. The first run of the pull
request that added the job is the red half, as long as #510 has not merged. After
it has, redo the red half on a scratch branch off the current head with
`git revert fa7d7d7b8` applied, and dispatch on that.

## 2. A test skipped on Windows is `#[cfg(unix)]` with a concrete reason

A skip that is not visible and justified hides real Windows bugs, so the nightly
(section 3) must be the one to decide. `scripts/ci/lint_windows_skips.py` (job
`lint-windows-skips`, unit tests with a fixture per form in
`scripts/ci/test_lint_windows_skips.py`) refuses, in `backend/src` and
`backend/tests`:

- a gate with no reason: write `#[cfg(unix)] // creates symlinks through
  std::os::unix` on the gate line. When rustfmt moves a comment that runs past the
  line width, the plain `//` line right below the gate is the reason; a comment
  above it, or a `///` doc, is not. Keep the gate line within 100 columns so
  `cargo fmt` leaves it alone;
- a reason that names no concrete POSIX cause: symlinks, chmod mode bits, fork and
  signals, unix sockets, FIFOs, `/proc`, a `#!/bin/sh` or python fixture, raw-byte
  names, a program Windows lacks. "unverified on Windows", "not yet run", "flaky"
  and the like are refused outright: **do not gate a test you have not seen fail on
  Windows; let the nightly run it.** A gate kept for a known Windows defect says
  `see KT-<n>` and is accepted on that ground alone;
- any form that skips on Windows but is not `unix`: `cfg(not(windows))`,
  `cfg(not(target_os = "windows"))`, `cfg(target_family = "unix")`,
  `cfg_attr(windows, ignore)` (`all(unix, ...)` and `all(test, unix, ...)` are
  `unix`; `target_os = "linux"` or `"macos"` is accepted for what only that OS has);
- a skip decided at run time: in a test body, an `if` or `match` on a `cfg!(..)`
  that names a platform (`windows`, `unix`, `target_os`, `target_family`,
  `target_env`, any nesting of `not`/`all`/`any`), or on `std::env::consts::OS` /
  `FAMILY`, whose branch returns. Branches are read with balanced braces, so a
  nested block, an `else` chain or a `match` arm before the `return` does not hide
  it. A skip hidden in a helper, or in a flag computed on an earlier line, is not
  seen: gate that test instead;
- the same on a module that holds tests (an inline `mod tests { .. }`, a
  `mod x;` or `#[path = ".."]` file, a whole-file `#![cfg(unix)]`). A gate on a
  helper, a `use` or production code is not checked: a helper only gates the
  tests that call it, and those carry a reason.

A reason may also name code that rejects Windows by design
(`// VerifiedCheckout::verify refuses Windows in check_platform`): the lint accepts
"refuses Windows" for exactly that, and the named function is the thing to check.

Un-gating a module makes every test in it build on Windows, so it is also a
compile check: a test that calls `std::os::unix` without its own gate breaks the
whole nightly build, not one test. The lint does not see that; read the module
(or search it for `os::unix`, `PermissionsExt`, `MetadataExt`) before removing a
module gate.

### Which tests in `read_only_repos` and `room_reach` run on Windows

`ReadOnlyRepos` is Claude Code's and Codex's file sandbox, which Kronn only wires
on macOS and Linux. `check_platform` (`backend/src/agents/read_only_repos.rs`)
returns an error when `cfg!(windows)`, and `identity` has a non-unix arm that
errors too. So a test that reaches `ReadOnlyRepos::resolve*`,
`VerifiedCheckout::verify`, or `room_reach::plan` with Claude Code and no full
access cannot pass there: `plan` swallows the `verify` error and returns
`Files::Closed`, so a test asserting an open checkout fails, and one asserting a
closed checkout would pass for the wrong reason. Those are `#[cfg(unix)]`; only
the tests that never get past an earlier, platform-independent return stay:

| test | why it passes on Windows |
| --- | --- |
| `read_only_repos_empty_preserves_legacy_providers` | `resolve_with_dirs` returns `Ok(None)` for empty input before `check_platform` |
| `a_room_that_reaches_nothing_and_can_ask_for_nothing_has_no_block` | `RoomReachPlan::default().prompt()`, no I/O |
| `names_cannot_forge_lines_in_the_block` | `prompt()` on a plan with only requestable names, no I/O |
| `nothing_granted_opens_nothing_and_says_what_can_be_asked_without_naming_it` | an empty `granted` list never reaches `verify` |
| `a_full_access_agent_is_told_a_code_grant_is_not_enforced_for_it` | `full_access = true` skips `verify` (`room_reach.rs` `plan`); the prompt is text |
| `a_full_access_agent_keeps_its_launch_and_is_still_told_what_it_reaches` | same: `full_access = true` |
| `providers_without_the_policy_and_missing_work_dirs_open_nothing` | Ollama, OpenCode and Gemini are refused by the agent-type check that runs before the platform check, and Claude Code with no working directory never calls `verify` |

The two helpers those tests use, `checkout` and `reached`, stay portable; every
other helper in the module is `#[cfg(unix)]` so the Windows build has no dead code.
This split was checked by compiling the two modules with every `#[cfg(unix)]` in
them switched off and `check_platform` forced to refuse: those seven tests (and
only those) build and pass. It is a simulation on macOS, not a Windows run.

A module gate hides every test in it, so prefer gating the few tests that need
the POSIX feature over the module, and prefer a portable fixture (a tempdir path,
`std::env::current_exe()`, `cmd.exe` for `/bin/ls`) over a gate.

`python3 scripts/ci/lint_windows_skips.py --list` prints the inventory with its
reasons.

## 3. Nightly: the full lib suite on Windows

`ci-nightly-windows.yml` runs `cargo nextest run --lib` (profile `ci`, one process
per test, `--build-jobs 2` because the full lib-test binary needs about 13 GB of
rustc, see [ci-macos-portability-memory](../gotchas/ci-macos-portability-memory.md))
at 02:17 UTC and on demand. Every failure is a Windows defect: fix it, or ticket it
and gate the test `#[cfg(unix)] // see KT-<n>`. The JUnit report is attached to the
run.
