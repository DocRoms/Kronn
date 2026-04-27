- **ID**: TD-20260427-host-sync-flock
- **Area**: Backend
- **Severity**: Medium (data-loss risk, low frequency)

## Problem (fact)
Kronn's `sync_claude_global_config` / `sync_gemini_global_config` / `sync_codex_global_config` / `sync_copilot_global_config` all use `atomic_write` (write to `.tmp` + rename) which is atomic at the **filesystem** level. This protects against a reader seeing a partial write, but does NOT protect against a **concurrent writer** clobbering the user's changes:

- The user's running Claude Code CLI session is itself a writer of `~/.claude.json` (cache, recents, mcpContextUris, onboarding state).
- If Claude Code rewrites the file between Kronn's `read` and `rename` steps, Kronn's `rename` blindly overwrites Claude's change → the user loses Claude state edits.
- Same situation for Gemini CLI and `~/.gemini/settings.json`.

The window is small (~ms) but real. We hit it once during dogfooding when restarting Kronn with `kronn restart` while a Claude Code session was running.

## Why we can't fix now (constraint)
Adding `fs2::FileExt::try_lock_exclusive` (or `flock(2)` advisory lock) cross-platform requires a new dependency and careful retry semantics:
- Windows: `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY`.
- macOS/Linux: `flock(LOCK_EX | LOCK_NB)`.
- Timeout/retry policy: how long do we block sync before giving up? What error message do we surface to the UI?
- Claude Code itself doesn't take an advisory lock (we'd need to verify), so we'd have an asymmetric "we wait, they don't" situation. Not strictly safer.

## Impact
- Correctness: rare data loss of Claude Code app state (cache, mcpContextUris, onboarding flags).
- Trust: even one occurrence erodes user confidence in Kronn-managed configs.
- Severity tied to user behavior: heavy concurrent users (write Kronn UI + use Claude Code CLI) hit it more.

## Where (pointers)
- `backend/src/core/mcp_scanner.rs:120` — `atomic_write` (no flock)
- `backend/src/core/mcp_scanner.rs:1140+` — `sync_claude_global_config` calls `atomic_write` at line ~1233
- Plan v2 review: MCP Expert flagged as P0 ("race with Claude Code lui-même"), Senior Dev as P1.

## Suggested direction (non-binding)
1. Add `fs2 = "0.4"` to `backend/Cargo.toml`.
2. Wrap `atomic_write` for the 4 host-sync paths with a CAS-style approach: read mtime → flock_ex → re-read + diff → if mtime changed, abort with `ConcurrentWrite` error → UI surfaces "Claude Code seems to be writing — retrying in 5s" toast.
3. Don't wrap `atomic_write` globally — only for the host-sync paths where the concurrent-writer scenario is real.
4. Document in `ai/operations/mcp-servers/<cli>.md` that "closing your Claude Code session before clicking Save in Kronn" is the recommended workflow.

## Next step
Create ticket. Schedule for the next milestone where users start using the host sync feature heavily.
