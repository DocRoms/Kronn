- **ID**: TD-20260429-uv-cache-uid-mismatch
- **Area**: Infra (Docker)
- **Severity**: ~~High~~ → Medium after partial fix below; remaining risk is regression on `APP_UID` build arg + the broader bind-mount-uid pattern.
- **Status**: PARTIALLY FIXED 2026-04-29 — `~/.cache/uv` swapped from host bind to named volume (`uv-cache`). Items 3-5 below remain open.

## Problem (fact)
`docker-compose.yml:31` (pre-2026-04-29) bind-mounted the host's `~/.cache/uv` rw into the container at `/home/kronn/.cache/uv`. Today the container runs as uid 1000 (`kronn` user, set via `APP_UID` build arg defaulting to host uid) so new writes are correctly user-owned. Pre-`APP_UID` builds (released ≤ 0.5.x) ran as **root** (uid 0) and left root-owned dirs/files inside the user's `.cache/uv` tree. Those legacy files persist across upgrades and silently break `uvx` on host:

Once Kronn has run any `uvx` command in-container (audit, MCP scan, agent spawn), the host's `uvx` — invoked by Claude Code CLI / Codex / Gemini CLI when launching MCP servers from `.mcp.json` — fails with:

```
error: Failed to initialize cache at `/home/priol/.cache/uv`
  Caused by: failed to open file `/home/priol/.cache/uv/CACHEDIR.TAG`: Permission denied (os error 13)
```

This silently breaks **every uvx-based MCP** when launched from the host CLI, while those same MCPs still work from inside Kronn. Concretely, on `front_euronews` we observed: `atlassian` (`uvx mcp-atlassian`), `aws cloudwatch` (`uvx awslabs.cloudwatch-mcp-server`), `Docker` (`uvx mcp-server-docker`), `Git` (`uvx mcp-server-git`) all `Failed to connect` in `claude mcp list` until the host cache was reset.

This is exactly the kind of asymmetry the 0.6.0 host-sync hardening was supposed to prevent. The `_kronn` marker, the orphan cleanup, the scope-aware routing all assume that **what works inside the container also works on the host**. They don't validate the runtime tooling chain (binary present, binary version, cache writable as host user).

## Why we can't fix now (constraint)
Three options, all with trade-offs:

1. **Drop the bind, use a named volume** (mirror `npm-cache` and `uv-tools`):
   - Pro: zero uid surface, fully isolated, simplest fix.
   - Con: container's uv re-downloads packages already cached on host (one-time cost ~50 MB).
   - Con: the documented benefit "share host uv cache for Python MCP servers" is lost.

2. **Match container uid to host uid via `USER_UID` build arg / runtime user remapping**:
   - Pro: real fix, host and container coexist on the same files.
   - Con: requires re-baking the image per host (or `--user` at runtime + tweaking `chown`s in Dockerfile).
   - Con: doesn't solve the broader pattern (RTK data dir at line 53 has the same shape).

3. **Run the container as the host's uid via `user: "${UID}:${GID}"` in compose**:
   - Pro: surgical, no Dockerfile change.
   - Con: breaks `apt`/system writes inside the container if anything still expects root.
   - Con: WSL2 vs native Docker have different `${UID}`/`${GID}` propagation; compose env interpolation needs explicit export.

Option 1 is the safe default. Option 2/3 are correct-but-fiddly and need cross-platform validation (Linux native, WSL2, macOS Docker Desktop).

## Impact
- Correctness: silently kills `uvx`-based MCP servers when invoked from host CLIs (CC, Codex, Gemini). User sees `Failed to connect`, no actionable error.
- Trust: the whole point of `Host MCP sync — bidirectional CLI integration` (section 9 of `docs/AGENTS.md`) is that Kronn-configured MCPs *just work* in the host CLI. This breaks that promise.
- Severity climbs with usage: every Kronn run as agent (audit, workflow, MCP test) re-creates root-owned dirs even if the user fixes the cache manually.

## Where (pointers)
- `docker-compose.yml:31` — the offending bind mount.
- `docker-compose.yml:53` — `~/.local/share/rtk` has the same shape (lower visibility because RTK is rarely invoked manually from host).
- `backend/Dockerfile` — installs `uv` as system package; container `uv` runs whichever uid PID 1 was started with.
- `backend/src/core/mcp_scanner.rs` — host-sync writers; they don't inspect or repair host cache state when writing the MCP entries.

## Suggested direction (non-binding)
1. ~~**Short-term**: switch `~/.cache/uv` to a named volume (`uv-cache:/home/kronn/.cache/uv`)~~ — **DONE 2026-04-29**. Mirrors `npm-cache` and `uv-tools`. Trade-off accepted: container's uv re-downloads packages (~50 MB one-time, no impact on host's own cache).
2. ~~**Same patch**: do the same for `~/.local/share/rtk` (line 53)~~ — **REJECTED 2026-04-29**. RTK's design *requires* the bidirectional bind so `rtk gain` inside the container can read the host SQLite for the savings counter (see CLAUDE.md L419). Switching to a named volume would silently break the RTK integration shipped in 0.5.1 (counter always reports zero). Container + host share uid 1000 today, so the bind is safe as long as `APP_UID` stays correctly wired. Risk = regression on that build arg.
3. **Defensive** (broader sweep, in a follow-up): add a `kronn doctor` command (or auto-run on startup) that detects host paths owned by uid 0 under `$HOME/.cache` / `$HOME/.local/share` and surfaces a clear warning + one-click fix in the UI. Catches future drift as we add new bind-mounted dirs **and** flags legacy root-owned files left over from pre-`APP_UID` upgrades. The actual mitigation for the surviving RTK bind mount.
4. **Documentation**: add a section to `docs/operations/mcp-servers/` (or a new `docs/operations/host-mcp-runtime.md`) listing the **runtime prerequisites on the host** for each MCP transport (`uvx` ≥ 0.x, `glab` ≥ 1.59, npx clean cache, etc.) — the bidirectional sync is only as good as the host toolchain.
5. **Enhancement**: have the host-sync writer (`sync_*_global_config`) emit a one-time warning when it writes an MCP entry whose `command` (`uvx`, `glab`, `fastly-mcp`, …) isn't found in the host's PATH. Surfaces the missing-binary class of bug at config-write time, not at MCP-spawn time.

## Next step
Items 1+2 resolved 2026-04-29. Open items 3-5 (`kronn doctor`, host runtime prereqs doc, host-sync writer warning on missing host binary) belong in a follow-up "host MCP runtime hardening" milestone. The doctor command should ship before we add any new host-bind-mount; that's the systemic mitigation.
