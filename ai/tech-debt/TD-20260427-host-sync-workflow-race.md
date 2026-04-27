- **ID**: TD-20260427-host-sync-workflow-race
- **Area**: Backend
- **Severity**: Low-Medium (rare, hard to detect)

## Problem (fact)
`sync_affected_projects` (called from `api/mcps.rs` after every config edit) writes `~/.claude.json`, `~/.gemini/settings.json`, etc. **without checking** whether a Kronn workflow run is currently executing an agent that would be reading those files at startup.

Concretely:
1. User clicks "Save" on an MCP config in Plugins → backend triggers `sync_affected_projects` → rewrites `~/.claude.json`.
2. Meanwhile, a workflow step is mid-execution: `cargo run claude-code` just spawned, agent is reading `~/.claude.json` to resolve MCPs.
3. Race: the agent might see an inconsistent intermediate state if the read interleaves with our atomic-write rename, OR see the new state mid-run when it expected the old state from the original `--mcp-config` flag.

The Tech Lead reviewer flagged this as P1 in plan v2. Currently mitigated only by atomic_write (rename is atomic, but doesn't help the agent that already opened the file).

## Why we can't fix now (constraint)
The fix requires a `workflow_runs` query gate in the sync flow:
- Read `SELECT id FROM workflow_runs WHERE status IN ('Running', 'Pending')`
- If non-empty: skip outbound sync, queue retry with debounce, surface UI feedback "Sync delayed — workflow X running"
- Plus: a hook to retry once the workflow ends (via `WorkflowRunCompleted` event)

This couples `mcp_scanner` to `workflow_runs` ownership semantics, which currently live in `workflows/` as separate concerns. Cross-module dependency needs a clean event/observer pattern, not direct DB query (would create circular deps if not careful).

## Impact
- Correctness: agent receives stale or inconsistent MCP config in the rare interleave case.
- Severity: low — only fires when user edits MCPs WHILE a workflow is running. Most users don't.
- Detection: silent. The agent might fail with a confusing "MCP X not found" error that doesn't blame the race.

## Where (pointers)
- `backend/src/core/mcp_scanner.rs:1031` — `sync_affected_projects` entry point
- `backend/src/db/workflows.rs` — `workflow_runs` table
- Plan v2 review: Tech Lead P1 ("Race write-vs-agent-read")

## Suggested direction (non-binding)
1. Add a method `db::workflow_runs::has_running()` returning `bool`.
2. `sync_affected_projects` checks at entry; if true, log + queue retry via tokio task with 30s debounce.
3. Add a `WorkflowRunFinished` event already broadcast on the WS — `sync_affected_projects` listener triggers immediate sync when the queue drains.
4. UI: add a small status indicator "Sync pending" when an edit is queued.

## Next step
Create ticket. Lower priority than `TD-20260427-host-sync-flock`.
