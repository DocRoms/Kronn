# Testing & quality (AI rules)

## Rules

- **Quality gate is non-negotiable**: tests must pass after any code change.
- **Tests are mandatory when changing behavior**: add/update tests.

<!-- Add project-specific rules. Example:

## Config files

| Tool | Config |
|------|--------|
| Jest | `jest.config.js` |
| PHPUnit | `phpunit.xml` |

## Fast smoke checks (run before full suites)

| Language | Command | Notes |
|----------|---------|-------|
| TS | `npm test -- --filter Foo` | Single test file |
| Full | `npm run test:all` | Blocking gate |
-->

## Troubleshooting (when command output is missing)

### Rule (critical)
- If output is missing, state it explicitly: **"I did not receive the command output"**.
- Retry once.
- If output is still missing, ask the user to **copy/paste the full command output** into the chat.

### Why
- Without the actual output, we cannot confirm PASS/FAIL or diagnose failures safely.
