# Architecture decisions (why, not what)

> **TEMPLATE FILE.** Filled during audit Step 9 (final review + decisions). Do not invent decisions — they must come from code analysis or user confirmation.

This file captures **intentional choices** — patterns that look unusual but are deliberate. It prevents agents from "fixing" things that aren't broken.

It is the *positive* counterpart to `inconsistencies-tech-debt.md`: decisions the team made on purpose, NOT problems to fix.

## How to fill (audit Step 9)

- **Target 3-8 real decisions** observed in code + briefing. Quality > quantity — a list of 2 strong decisions is fine; 15 fluff items is worse than 3 strong ones.
- **Every row must cite a source**: `file:line`, `briefing.md`, or `user` (if confirmed during validation). No inferred rows — a decision you cannot anchor to a source is a `<!-- TODO: ask user -->` question, not a decision.
- **Remove unused rows entirely** — if the project has only one real decision, delete the `{{DECISION_2}}` placeholder row instead of padding.

## Examples (good shape — adapt to the actual repo)

| Decision | Why chosen | What NOT to do | Source |
|----------|-----------|---------------|--------|
| Single Mutex on SQLite | Single-writer model fits our access pattern; multi-writer would need WAL + busy_timeout tuning | Don't add a connection pool | `src/db/conn.rs:42` |
| No ORM | Pure SQL is faster for our 12-table schema; ORM dependency cost exceeds the win | Don't introduce diesel/sea-orm | `src/db/queries.rs` + user |

## Decisions

<!-- Fill during audit. Each row should be traceable to code evidence or user confirmation. -->
| Decision | Why chosen | What NOT to do | Source |
|----------|-----------|---------------|--------|
| {{DECISION_1}} | {{REASON}} | {{ANTI_PATTERN}} | {{FILE_OR_USER}} |
| {{DECISION_2}} | {{REASON}} | {{ANTI_PATTERN}} | {{FILE_OR_USER}} |
