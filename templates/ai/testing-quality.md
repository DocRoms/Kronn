# Testing & quality (AI rules)

## Rules

- **Quality gate is non-negotiable**: tests must pass after any code change.
- **Tests are mandatory when changing behavior**: add/update tests.

## Build & quality checks

| Check | Command | Blocking? | Notes |
|-------|---------|-----------|-------|
| {{CHECK_1}} | {{COMMAND}} | Yes/No | {{NOTES}} |
| {{CHECK_2}} | {{COMMAND}} | Yes/No | {{NOTES}} |
| {{CHECK_3}} | {{COMMAND}} | Yes/No | {{NOTES}} |

## Test infrastructure

| Language | Runner | Config file | Setup |
|----------|--------|------------|-------|
| {{LANG_1}} | {{RUNNER}} | {{CONFIG}} | {{SETUP}} |
| {{LANG_2}} | {{RUNNER}} | {{CONFIG}} | {{SETUP}} |

## Test suites

| Suite / File | Scope | Approx. tests | Notes |
|-------------|-------|---------------|-------|
| {{SUITE_1}} | {{SCOPE}} | {{COUNT}} | {{NOTES}} |
| {{SUITE_2}} | {{SCOPE}} | {{COUNT}} | {{NOTES}} |

## Coverage

- Current: {{COVERAGE_STATUS}}
- Target: {{COVERAGE_TARGET}}

## What is NOT tested

- {{UNTESTED_1}}
- {{UNTESTED_2}}

## Fast smoke checks (run before committing)

| # | Command | What it verifies |
|---|---------|-----------------|
| 1 | {{COMMAND_1}} | {{WHAT_1}} |
| 2 | {{COMMAND_2}} | {{WHAT_2}} |
| 3 | {{COMMAND_3}} | {{WHAT_3}} |

## Troubleshooting (when command output is missing)

### Rule (critical)
- If output is missing, state it explicitly: **"I did not receive the command output"**.
- Retry once.
- If output is still missing, ask the user to **copy/paste the full command output** into the chat.

### Why
- Without the actual output, we cannot confirm PASS/FAIL or diagnose failures safely.
