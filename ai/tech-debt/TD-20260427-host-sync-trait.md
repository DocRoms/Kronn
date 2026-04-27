- **ID**: TD-20260427-host-sync-trait
- **Area**: Backend
- **Severity**: Medium

## Problem (fact)
4 nearly-identical functions in `backend/src/core/mcp_scanner.rs` write Kronn-managed MCPs to host CLI config files:
- `sync_codex_global_config` (~150 LoC) — TOML
- `sync_copilot_global_config` (~80 LoC) — JSON, top-level
- `sync_claude_global_config` (~130 LoC) — JSON, scope-aware (top-level + per-project)
- `sync_gemini_global_config` (delegating to `sync_json_global_config`) — JSON, top-level

Each function repeats: list configs → filter `should_host_sync` → resolve home path → load existing → merge + atomic write + chmod. The shared logic between Claude and Gemini was partially extracted (`sync_json_global_config`, `merge_kronn_entries`, `load_json_config_for_merge`, `count_kronn_entries*`) but the trait abstraction the Tech Lead reviewer recommended in plan v2 was deferred.

## Why we can't fix now (constraint)
The current set of 4 CLIs is small and the duplication has been kept manageable by extracting the JSON-format helpers into shared functions. Refactoring into a `HostAgentSync` trait was intentionally deferred to keep the Phase-3 PR focused on getting the Claude scope-aware routing right (the user-visible feature). Refactor risks regressing existing Codex/Copilot sync behavior — needs a dedicated session.

## Impact
- Code quality: scaling friction. Adding Ollama/OpenCode/DeepSeek (planned 0.4.x+) means 3+ more sync functions if no abstraction lands.
- Maintenance: any cross-cutting change (e.g. `flock`, observability spans, scope-aware routing for Codex/Gemini if they ever gain it) has to be done 4× with risk of drift.
- Test surface: each function has its own merge/write tests; a trait would give us one test suite per impl.

## Where (pointers)
- `backend/src/core/mcp_scanner.rs:792-934` — `sync_codex_global_config`
- `backend/src/core/mcp_scanner.rs:937-1027` — `sync_copilot_global_config`
- `backend/src/core/mcp_scanner.rs:1140-1240` — `sync_claude_global_config` (scope-aware)
- `backend/src/core/mcp_scanner.rs:1265-1276` — `sync_gemini_global_config` (delegate)
- `backend/src/core/mcp_scanner.rs:1281-1370` — `sync_json_global_config` (current shared impl)

## Suggested direction (non-binding)
Define `trait HostAgentSync` with associated `Config` type and methods `path()`, `read_existing()`, `merge_strategy()`, `write_atomic()`. Each CLI gets an impl in `core/host_sync/<cli>.rs`. The `sync_affected_projects` flow iterates `Vec<Box<dyn HostAgentSync>>`. Claude's scope-aware routing becomes a method override (or an associated type for the merge strategy).

The pattern `load_codex_config_for_merge` + `CodexLoadOutcome` (`mcp_scanner.rs:730-787`) and `load_json_config_for_merge` + `JsonLoadOutcome` are already isomorphic — they should share an interface.

## Next step
Create ticket. Aim for the Ollama or OpenCode integration as the trigger — at that point the duplication cost crosses the refactor cost.
