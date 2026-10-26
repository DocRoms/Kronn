# Kronn as a background service

`kronn service` runs the native Kronn backend as a per-user background service:
API and web UI on `http://localhost:3140`, no terminal, no prompt at login. It
is a macOS LaunchAgent or a Linux `systemd --user` unit.

The service never runs from your development checkout. It runs a staged,
immutable release under `~/.kronn/service`, so a `git pull`, a branch switch or
a `cargo clean` in the checkout cannot affect it.

> Scope of this page: install, layout, lifecycle commands, logs, coexistence
> with `start-dev`, update and rollback.

## Install

```bash
kronn service install                 # latest release tag
kronn service install --from-checkout # current checkout HEAD (testing)
kronn service install --yes           # no questions (reinstall, symlink)
```

`install` is the only interactive step. It:

1. checks the prerequisites: `git`, `cargo` (rustup), `node` (>= 24) and `pnpm`;
2. clones the repository into `~/.kronn/service/src` (the URL is the `origin` of
   the checkout the CLI runs from, `https://github.com/DocRoms/Kronn.git` if it
   has none) and checks out the highest `X.Y.Z` tag;
3. builds it: `cargo build --release` for the backend, then
   `pnpm install --frozen-lockfile && pnpm build` for the frontend;
4. stages the result as `releases/<version>` and points `current` at it;
5. writes the unit, loads it and waits until `/api/health` answers with the
   expected version;
6. offers to point `~/.local/bin/kronn` at `current/cli/kronn`, so `kronn`
   follows the installed release;
7. prints the URL.

Releases are built from source for now; prebuilt archives are future work.

`--from-checkout` builds the checkout the CLI runs from instead of a tag. The
release is named `<VERSION file>-dev-<short sha>` (plus `-dirty<timestamp>` when
the working tree has uncommitted changes, so every install gets its own
release). The health check still compares against the backend's own version,
which is the plain `VERSION`. Such an install has no `src/` clone; the first
`kronn service update` (even with `--check`) creates it, so a dev install can
move to a tagged release.

## Layout

```
~/.kronn/service            ($KRONN_SERVICE_HOME)
  src/                      git clone used only by the service
  releases/<version>/       immutable once staged
    bin/kronn               release backend binary
    ui/                     frontend build
    scripts/                backend helper scripts (disc-introspection-mcp.py, ...)
    cli/kronn, lib/, completions/   the CLI itself
    VERSION                 e.g. 0.15.0 (written last: a release without it is incomplete)
    SOURCE_SHA              git sha it was built from
  current -> releases/<version>     symlink, swapped atomically
  update.lock
```

The unit always references `current/`, never a versioned directory, so switching
releases is a single atomic symlink rename. The Kronn data directory
(database, config) is unchanged and shared with dev mode, guarded by the data
directory lock; `kronn service` never touches it.

## What the service runs

The unit starts `~/.kronn/service/current/bin/kronn` with:

| Variable | Value |
|----------|-------|
| `KRONN_SERVICE` | `1`: the backend merges your login-shell `PATH` into its own, because launchd and systemd give a minimal one (agent CLIs such as `claude` or `codex` would not be found otherwise) |
| `KRONN_UI_DIST` | `~/.kronn/service/current/ui`: the backend serves the web UI on the API port |
| `KRONN_DISC_INTROSPECTION_MCP` | `~/.kronn/service/current/scripts/disc-introspection-mcp.py` |
| `KRONN_DOCS_SIDECAR` | only when the release ships `bin/kronn-docs-sidecar` (a source build does not) |
| `KRONN_DATA_DIR` | only when set: the service opens the database that `update` and `rollback` watch |
| `XDG_CONFIG_HOME` | only when set: without `KRONN_DATA_DIR` it places the Linux data dir, and a `systemd --user` unit does not inherit it from your shell |
| `KRONN_PORT` | always (default `3140`); it overrides `server.port` of `config.toml` so the health check and the drain reach the service |
| `HOME` | your home directory |

`install` takes these three from your shell, else keeps the value of the unit
already installed, so a reinstall from another shell does not drop them. The
CLI then reads them back from the installed unit (`service_unit_env`), not from
the shell running it: an `update` from a shell without the install-time
`KRONN_PORT` or `KRONN_DATA_DIR` still probes the service's port and watches its
database. Once the service is installed, a key the unit does not set means the
default, even when the shell exports one (a dev database, say). `kronn status`
also asks the service's port, unless `KRONN_API_BASE` is set to something else.

| | macOS (launchd) | Linux (systemd --user) |
|---|---|---|
| Definition | `~/Library/LaunchAgents/com.kronn.service.plist` | `~/.config/systemd/user/kronn.service` |
| Identity | label `com.kronn.service`, domain `gui/<uid>` | unit `kronn` |
| Restart | `KeepAlive` on failure, `ThrottleInterval` 10 s | `Restart=on-failure`, `RestartSec=10` |
| Logs | `~/Library/Logs/Kronn/kronn.out.log`, `kronn.err.log` | journald |
| Start at login | `RunAtLoad`, GUI session only (`LimitLoadToSessionType=Aqua`) | `WantedBy=default.target` |

## Commands

| Command | Effect |
|---------|--------|
| `kronn service install [--from-checkout] [--yes]` | see above |
| `kronn service start` | load the service |
| `kronn service stop` | unload it (launchd `bootout` / `systemctl --user stop`); it starts again at your next login |
| `kronn service restart` | `launchctl kickstart -k` / `systemctl --user restart` |
| `kronn service status` | installed, running, current version, URL, health |
| `kronn service logs [--no-follow]` | tail the two log files / `journalctl --user -u kronn -f` |
| `kronn service update [--to <X.Y.Z>] [--check] [--force] [--yes]` | see [Update](#update) |
| `kronn service rollback [--yes]` | see [Rollback](#rollback) |
| `kronn service uninstall [--yes] [--purge]` | unload and remove the unit; asks before deleting `~/.kronn/service` (`--purge` deletes without asking, `--yes` keeps it) |

The usual commands follow the service once it is installed: `kronn stop`
stops it, `kronn restart` restarts it, `kronn logs` shows its logs and
`kronn status` prints its state before the usual overview. `kronn web` and
`kronn start-dev` need a checkout (they use its Makefile) and refuse to run from
the CLI staged in a release.

## Coexistence with `start-dev` and `web`

A running service owns port 3140 and the data directory lock. `kronn start-dev`
and `kronn web` notice it and offer to stop it. The service is unloaded rather
than killed: `KeepAlive` would restart a killed process. When `start-dev`
stopped the service it loads it again when it exits; for `web` run
`kronn service start` yourself afterwards. Declining leaves the service running
and the command exits. `kronn start` (the interactive menu) leaves the service
alone: its web option goes through `kronn web`, which makes the offer, and its
CLI option does not need the port.

## Notes

- **macOS Login Items.** A LaunchAgent shows up under System Settings > General >
  Login Items & Extensions > "Allow in the Background" as `kronn` (or the
  developer name of the binary). Turning it off there prevents the service from
  starting at login; `kronn service uninstall` removes it properly.
- **Linux, running without a login session.** A user unit stops when your last
  session ends. To keep it running, enable lingering once:
  `loginctl enable-linger "$USER"`.
- **Atomic switch.** `current` is replaced by creating a temporary symlink next
  to it and renaming it over `current` (`mv -fh` on BSD/macOS, `mv -fT` on GNU,
  Python's `os.replace` as a fallback). A plain `mv -f` onto a symlink to a
  directory would move the link inside the directory instead.
- **Overrides (tests).** `KRONN_SERVICE_HOME`, `KRONN_LAUNCHCTL`,
  `KRONN_SYSTEMCTL`, `KRONN_JOURNALCTL`, `KRONN_SERVICE_KIND`
  (`launchd|systemd`) and `KRONN_PORT` (health probes, when no unit sets it) are read by
  `lib/service.sh`; `tests/bats/service.bats` uses them to avoid the real
  service manager and the real `$HOME`.

## Update

```bash
kronn service update               # latest X.Y.Z tag
kronn service update --check       # report only
kronn service update --to 0.15.1   # a specific tag (downgrades are allowed)
kronn service update --force       # do not wait for running work
kronn service update --yes         # no questions
```

Steps, in order (nothing live changes before step 6):

1. refuse unless the service is installed; take `update.lock` (a directory with
   a `pid` file; a lock whose pid is dead is cleared, a live one refuses);
2. `git -C src fetch --tags --force origin`; the target is `--to` (it must be an
   existing `X.Y.Z` tag) or the highest tag;
3. target equal to the current version: "already up to date", exit 0.
   `--check` stops here and prints current and target;
4. `git -C src checkout --detach <tag>` (refused when `src/` has local
   modifications), build and stage `releases/<tag>`. A release that is already
   staged is reused without a rebuild;
5. drain: wait until nothing is in flight. The check reads two backend routes
   with the CLI's usual Bearer token (`~/.config/kronn/.auth_token`, a file
   you write yourself with the backend's API token; nothing in Kronn creates it):
   `GET /api/workflows` (a latest run in `Running` or `Pending`, same reading as
   the `workflow_active_runs` tool; `WaitingApproval` waits for a human and does
   not block) and `GET /api/discussions/running` (live agent streams). When the
   timeout elapses it asks whether to restart anyway (`--yes` restarts, declining
   aborts with nothing changed). If the API is unreachable it warns and proceeds.
   If the backend refuses the token (401/403, e.g. `server.auth_strict_localhost`
   with no matching `~/.config/kronn/.auth_token`), or accepts the connection
   but does not answer within `KRONN_API_MAX_TIME` seconds, it cannot tell what
   is running and asks before restarting (`--yes` restarts, declining aborts).
   `--force` skips the wait;
6. record the previous version (`previous`) and the modification time of the
   database backup (`update.state`), switch `current`, restart, wait for
   `/api/health` to report the target version;
7. on failure: automatic rollback (below), exit 1. On success: prune
   `releases/` (3 most recent by semver, never the current or the previous
   one) and print the `## [<version>]` section of the changelog.

The CLI that runs the update lives in the old release directory, which is never
modified, so an update can replace the script that is executing it.

### Exit codes

| Code | Meaning |
|------|---------|
| 0 | updated, or already up to date |
| 1 | failure (including an update that was rolled back), refused, aborted |
| 10 | `--check` only: an update is available |

### Environment

| Variable | Default | Effect |
|----------|---------|--------|
| `KRONN_SERVICE_DRAIN_TIMEOUT` | `1800` | seconds to wait for running work |
| `KRONN_SERVICE_DRAIN_INTERVAL` | `10` | seconds between two drain polls |
| `KRONN_SERVICE_HEALTH_TIMEOUT` | `90` | seconds to wait for a healthy backend after a switch |
| `KRONN_DATA_DIR` | platform data dir | where the database lives (same override as the backend) |
| `KRONN_API_MAX_TIME` | `30` | seconds one in-flight check may take before its answer counts as unknown |

## Rollback

`kronn service rollback [--yes]` returns to the release stored in `previous`
(running it again goes forward again). An automatic rollback after a failed
update runs the same code:

1. stop the service;
2. if the database backup `<data dir>/kronn.db.backup` (written by the backend
   before a migration) is newer than it was before the update, the new version
   migrated the database. The current database is moved to
   `kronn.db.failed-<version>` together with its `-wal` and `-shm` files, and
   the backup is copied back. The manual command asks first, because everything
   written since the update is dropped (the failed database is kept);
3. switch `current` back, load the service, wait for the previous version to be
   healthy.

Without an update record (`update.state`) a manual rollback only switches the
release and leaves the database alone.

## Where the code lives

- `lib/service.sh`: layout, unit rendering, staging, symlink switch, service
  manager calls, commands. Sourced by `kronn`.
- `lib/service-update.sh`: update, drain, rollback, pruning. Sourced by `kronn`.
- `tests/bats/service_update.bats`: update and rollback against a local bare
  repository, a fake build and fake `launchctl`.
- `tests/bats/service.bats`: rendering, staging, switching, tag sorting and
  command routing against fake `launchctl` / `systemctl`.
