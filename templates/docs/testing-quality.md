# Testing & quality

> **TEMPLATE FILE.** Sections marked `{{...}}` must be filled by the AI audit.
> If the test runner or commands are not filled, say `NOT_FOUND` — **never assume Jest, Mocha, or any specific runner**.

> **Test policy:** see [AGENTS](AGENTS.md) § Project parameters — this file is the checklist, not the source of the policy.

## Build checks

<!-- Fill after audit: list all quality gates (lint, typecheck, test, build) -->
{{BUILD_CHECKS}}

## Test infrastructure

<!-- Fill after audit: test runner, config file, setup file, Node/runtime version -->
{{TEST_INFRASTRUCTURE}}

## Test suites

<!-- Fill after audit: list each test file/suite with scope and count -->
{{TEST_SUITES}}

## Test selection (when required by the project policy)

Use the project test policy to decide which checks the change requires. Adapt
the examples below to the project's stack and existing test locations; they
do not impose additional gates or override an explicit exemption.

| Change type | Suitable checks | Where |
|-------------|---------------|-------|
| New API endpoint | Integration test (HTTP request → response) | API test file |
| New function | Unit test in same file (`#[cfg(test)]` or `__tests__/`) | Same module |
| Bug fix | Regression test (fails without fix, passes with) | Relevant test file |
| Frontend component | Render + key user interactions | `__tests__/` |
| Database migration | Verify migration applies + data integrity | DB test file |

## Test quality checklist

For the tests required by the project policy, check the applicable items before
declaring a task done:

- [ ] Tests cover the **happy path**
- [ ] Tests cover at least one **error path** (invalid input, missing data)
- [ ] Tests cover **edge cases** (empty, unicode, large input)
- [ ] Assertions are **meaningful** (not just "renders" or "is defined")
- [ ] Mocks match **real API shapes** (check generated types)
- [ ] No **flaky** tests (no sleeps, no timing assumptions)
- [ ] The checks required by the project policy and CI pass
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
