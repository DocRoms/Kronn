# Testing & quality (AI rules)

## Rules

- **Quality gate is non-negotiable**: code must compile and build after any change.
- No test framework is set up yet (planned with SQLite migration).

## Build checks

| Layer | Command | Notes |
|-------|---------|-------|
| Rust compile | `cargo check` | Fast type check |
| Rust lint | `cargo clippy` | Must pass without warnings |
| Rust format | `cargo fmt --check` | Formatting check |
| TS compile | `cd frontend && npx tsc --noEmit` | Type check |
| Frontend build | `cd frontend && npm run build` | Production build |
| Full stack | `make start` | Docker Compose build + up |

## Fast smoke checks

| Action | Command |
|--------|---------|
| Backend compiles | `cd backend && cargo check` |
| Frontend compiles | `cd frontend && npx tsc --noEmit` |
| Type generation | `make typegen` |

## Troubleshooting (when command output is missing)

### Rule (critical)
- If output is missing, state it explicitly: **"I did not receive the command output"**.
- Retry once.
- If output is still missing, ask the user to **copy/paste the full command output** into the chat.

### Why
- Without the actual output, we cannot confirm PASS/FAIL or diagnose failures safely.
