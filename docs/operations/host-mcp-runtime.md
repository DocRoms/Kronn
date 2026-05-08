# Host MCP runtime prerequisites

When Kronn syncs MCP servers to the host CLI configs (`~/.claude.json`,
`~/.gemini/settings.json`, `~/.codex/config.toml`,
`~/.copilot/mcp-config.json`), the host CLI is the one that actually
spawns the MCP server. Each transport needs its own host-side
runtime; if the binary isn't there or is too old, the MCP fails with
a confusing "Failed to connect" that doesn't blame the missing tool.

`kronn doctor` (added in 0.7.1) automates these checks. This page is
the manual reference.

## Required binaries

| Tool | Used by | Minimum version | Install |
|---|---|---|---|
| `docker` | every Kronn command (containerised backend) | recent | https://docs.docker.com/get-docker/ |
| `npx` | npm-based MCPs (`@modelcontextprotocol/...`, GitHub MCP, Atlassian, etc.) | bundled with Node ≥ 18 | https://nodejs.org/ |
| `uvx` | Python MCPs (`mcp-atlassian`, `awslabs.cloudwatch-mcp-server`, `mcp-server-docker`, `mcp-server-git`, …) | `uv ≥ 0.4` | `curl -LsSf https://astral.sh/uv/install.sh \| sh` |
| `glab` | GitLab MCP integration | **≥ 1.59** (for `glab mcp serve`) | https://gitlab.com/gitlab-org/cli (or distro pkg) |
| `kiro-cli` | Kiro agent (AWS Builder ID auth) | latest | `curl -fsSL https://cli.kiro.dev/install \| bash` |

The Kronn-managed CLIs themselves (`claude`, `codex`, `gemini`,
`copilot`, `vibe`, `kiro-cli`) are detected separately from the
Settings → Agents page. This page only covers the **runtime
dependencies** of the MCP servers Kronn writes into those CLIs'
configs.

## Diagnosing problems

### `kronn doctor`

```bash
kronn doctor
```

Checks (in order):

1. **Host cache permissions** — finds files / dirs under
   `~/.cache` and `~/.local/share` owned by uid 0 (root).
   These are leftovers from pre-`APP_UID` Kronn (≤ 0.5.x) when
   the container ran as root and write-cached into a
   bind-mounted host dir. Symptom : host `uvx` fails with
   `Permission denied (os error 13)` on the cache. Fix :
   `sudo chown -R $(id -u):$(id -g) ~/.cache/uv` or
   `sudo rm -rf ~/.cache/uv` (uv re-downloads on next use).

2. **Runtime prerequisites** — checks that `uvx`, `glab` (≥
   1.59), `npx` are in `PATH`. Each missing tool degrades a
   class of MCPs without erroring at config-write time.

3. **Docker** — confirms `docker` is installed and the daemon
   is reachable. Without it, `kronn start` can't run.

The command exits non-zero when any check fails, so it's safe to
chain in CI or shell hooks.

### Manual checks

```bash
# uv / uvx
which uvx && uvx --version

# glab
which glab && glab --version | head -1
# Must be ≥ 1.59 for `glab mcp serve` (the agent-friendly MCP wrapper)
glab mcp serve --help

# npx
which npx && npx --version

# Test a specific MCP launches without error
npx -y @modelcontextprotocol/server-everything --help    # npm
uvx --from mcp-atlassian mcp-atlassian --help            # uvx
```

## Why this exists (history)

Pre-2026-04-29, `~/.cache/uv` was bind-mounted from host into the
container. The container ran as root → wrote root-owned cache files
back into the host home → host `uvx` (used by Claude Code / Codex /
Gemini for Python MCP servers) silently failed with
`Permission denied`. Switched to a named volume on 2026-04-29
(see `docker-compose.yml::volumes::uv-cache`), but the surviving
host-side shape (RTK at `~/.local/share/rtk` is bind-mounted by
design for the bidirectional savings counter) means we still need
`kronn doctor` to catch legacy uid-0 leftovers and any new bind
mounts that get added later. See
[TD-20260429-uv-cache-uid-mismatch](../tech-debt/TD-20260429-uv-cache-uid-mismatch.md)
for the full post-mortem.

## Adding a new bind mount?

Before merging a new bind-mount in `docker-compose.yml`:

1. Confirm the container's `APP_UID` matches the host's uid for
   the dir's owner (today: defaults to `1000`, configurable at
   build).
2. Add the path to `kronn doctor`'s scan list so we catch any
   future drift.
3. Update this doc.
