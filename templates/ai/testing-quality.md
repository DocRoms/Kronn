# Testing & quality

> **TEMPLATE FILE.** Sections marked `{{...}}` must be filled by the AI audit.
> If the test runner or commands are not filled, say `NOT_FOUND` — **never assume Jest, Mocha, or any specific runner**.

> **Rule: every code change MUST include tests.** See `ai/index.md` § 4.

## Build checks

<!-- Fill after audit: list all quality gates (lint, typecheck, test, build) -->
{{BUILD_CHECKS}}

## Test infrastructure

<!-- Fill after audit: test runner, config file, setup file, Node/runtime version -->
{{TEST_INFRASTRUCTURE}}

## Test suites

<!-- Fill after audit: list each test file/suite with scope and count -->
{{TEST_SUITES}}

## What to test (by change type)

| Change type | Required tests | Where |
|-------------|---------------|-------|
| New API endpoint | Integration test (HTTP request → response) | API test file |
| New function | Unit test in same file (`#[cfg(test)]` or `__tests__/`) | Same module |
| Bug fix | Regression test (fails without fix, passes with) | Relevant test file |
| Frontend component | Render + key user interactions | `__tests__/` |
| Database migration | Verify migration applies + data integrity | DB test file |

## Test quality checklist

Before declaring a task done:
- [ ] Tests cover the **happy path**
- [ ] Tests cover at least one **error path** (invalid input, missing data)
- [ ] Tests cover **edge cases** (empty, unicode, large input)
- [ ] Assertions are **meaningful** (not just "renders" or "is defined")
- [ ] Mocks match **real API shapes** (check generated types)
- [ ] No **flaky** tests (no sleeps, no timing assumptions)
- [ ] Full test suite passes
- [ ] If a test is flaky, **fix the root cause** — do not add retries

## Coverage

<!-- Fill after audit -->
{{COVERAGE}}

## NOT tested (known gaps)

<!-- Fill after audit: list areas with no test coverage -->
{{UNTESTED}}

## Smoke checks (pre-commit)

<!-- Fill after audit: quick commands to verify nothing is broken -->
{{SMOKE_CHECKS}}
