# Testing & quality

**Rules:** Tests must pass after any change. Add/update tests when changing behavior. If no command output, ask user to paste it.

## Build checks

| Check | Command | Blocking? | Notes |
|-------|---------|-----------|-------|
| {{CHECK_1}} | {{COMMAND}} | Yes/No | {{NOTES}} |
| {{CHECK_2}} | {{COMMAND}} | Yes/No | {{NOTES}} |
| {{CHECK_3}} | {{COMMAND}} | Yes/No | {{NOTES}} |

## Test infrastructure

| Language | Runner | Config | Setup |
|----------|--------|--------|-------|
| {{LANG_1}} | {{RUNNER}} | {{CONFIG}} | {{SETUP}} |
| {{LANG_2}} | {{RUNNER}} | {{CONFIG}} | {{SETUP}} |

## Test suites

| Suite | Scope | Tests | Notes |
|-------|-------|-------|-------|
| {{SUITE_1}} | {{SCOPE}} | {{COUNT}} | {{NOTES}} |
| {{SUITE_2}} | {{SCOPE}} | {{COUNT}} | {{NOTES}} |

## Coverage

Current: {{COVERAGE_STATUS}} · Target: {{COVERAGE_TARGET}}

## NOT tested

- {{UNTESTED_1}}
- {{UNTESTED_2}}

## Smoke checks (pre-commit)

| # | Command | Verifies |
|---|---------|----------|
| 1 | {{COMMAND_1}} | {{WHAT_1}} |
| 2 | {{COMMAND_2}} | {{WHAT_2}} |
| 3 | {{COMMAND_3}} | {{WHAT_3}} |
