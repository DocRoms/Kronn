# Screenshot sandbox

Reproducible Kronn instance for capturing README / website / blog screenshots without leaking the maintainer's real project names, Jira tickets or MCP secrets.

Lives at [`scripts/seed-demo-fixtures.sh`](../../scripts/seed-demo-fixtures.sh).

## What it does

Spins up an isolated Kronn backend on port `3145` (vs the default `3140`) backed by a `tmpdir` data directory, so it coexists with your real Kronn. Pre-seeds:

- 3 demo projects (`acme-blog`, `demo-monorepo`, `sample-rust-cli`)
- 4 Quick Prompts with marketing-friendly names
- `setup_complete = true` to skip the first-run wizard

## When to use it

- Refreshing the README screenshots after a UI redesign
- Recording a marketing GIF
- Reproducing a bug report on a known-clean state
- Onboarding tutorial demos

## How to run

```bash
./scripts/seed-demo-fixtures.sh
```

The script prints the next command to launch the matching Vite dev server (frontend on `:5174`, proxying to the sandbox backend on `:3145`).

When done, it also prints the teardown command (kills the backend, removes the tmpdirs).

## Requirements

- `curl` and `git` on `PATH` (no `jq`, no Python; pure Bash + curl)
- Kronn backend built (`cargo build --release --bin kronn` or debug)

## Env overrides

| Var | Default | Purpose |
|---|---|---|
| `KRONN_SANDBOX_PORT` | `3145` | Where the sandbox backend listens |
| `KRONN_SANDBOX_DATA` | `mktemp -d` | Data dir |
| `KRONN_SANDBOX_REPOS` | `mktemp -d` | Demo repo dir |
| `KRONN_BINARY` | `./backend/target/release/kronn` (falls back to debug) | Backend binary path |

## Anti-leak guarantees

The script never touches:

- Your real `~/.config/kronn/`
- Your real repos
- The default port `3140`

If you see your real projects in the screenshots, something is wrong. Stop, file an issue, and check the env overrides aren't pointing at your real data dir.
