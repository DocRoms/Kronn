- **ID**: TD-20260427-host-sync-backup-rotation
- **Area**: Backend
- **Severity**: Low

## Problem (fact)
On parse failure of a host config file (`~/.claude.json`, `~/.codex/config.toml`, etc.) Kronn copies the corrupt file to `<path>.kronn-backup` and aborts the sync (`load_json_config_for_merge` / `load_codex_config_for_merge`). This is a single-slot backup: subsequent corruption events overwrite the previous backup.

## Why we can't fix now (constraint)
Single-slot was a deliberate simplification for Phase-3 shipping. Rotation (e.g. `.kronn-backup.1` ... `.kronn-backup.5`) requires:
- Decide retention policy (count-based, time-based, both).
- Decide cleanup trigger (every sync? scheduled?).
- Cross-platform path conventions.
- UI surface to inspect/restore from a backup.

None of these are blockers, but they are scope-creep for what was meant to be a defensive guard.

## Impact
- Correctness: if the user has TWO consecutive corrupt files (rare, but possible after a Kronn bug + manual edit), the first backup is lost.
- Severity: low — most users will see at most 0 or 1 corruption event in their lifetime.

## Where (pointers)
- `backend/src/core/mcp_scanner.rs:769` — `load_codex_config_for_merge` writes `.toml.kronn-backup`
- `backend/src/core/mcp_scanner.rs:~1112` — `load_json_config_for_merge` writes `.json.kronn-backup`

## Suggested direction (non-binding)
- Rotate to `.kronn-backup.1` (most recent) → `.5` (oldest), `mv` chain on each new backup.
- Add a CLI command `kronn restore --backup <path>` that lists available backups and lets the user restore.
- Document in `docs/operations/mcp-servers/<cli>.md` that backups exist and where.

## Next step
Create ticket. Schedule when a user reports losing data through the single-slot mechanism (or annual review).
