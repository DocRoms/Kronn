---
name: testing
description: Use when adding or modifying logic, fixing bugs, or refactoring. Covers test strategy, TDD workflow, mocking boundaries, and test architecture.
license: AGPL-3.0
category: domain
icon: 🎯
builtin: true
---

## Procedures

1. **Write tests WITH the code, never after** — red-green-refactor. Failing test first, make it pass, then clean up.
2. **Respect the pyramid** — many unit tests (fast), fewer integration tests (real DB/HTTP), minimal e2e (critical flows only).
3. **Mock at boundaries only** — I/O, network, clock, filesystem. Never mock internal collaborators; it couples tests to implementation.
4. **Name tests as behavior** — `should_reject_expired_token` not `test_validate`. Tests are documentation.
5. **Each test owns its data** — no shared mutable state. Use factories/builders. Teardown what you set up.
6. **Test error paths explicitly** — happy path coverage is table stakes. Untested error paths are bugs.

## Gotchas

- **Flaky e2e tests** destroy CI trust — if you can't fix it in 1h, delete it and replace with integration test.
- **100% coverage is a vanity metric** — branch coverage > line coverage. Focus on decision points and error paths.
- **Property-based testing** catches edge cases humans miss — use for parsers, serializers, algorithms. QuickCheck/Hypothesis/proptest.
- **Test doubles hierarchy**: stub (returns canned data) < mock (verifies calls) < fake (simplified implementation). Use the lightest one that works.
- **Integration tests need containers** — testcontainers or docker-compose in CI. Never hit shared dev databases.
- **No real names in test data** — use generic identifiers: PeerAlpha, TestUser, Org42. Never "John Smith".
- **Timing-based assertions are flaky** — assert on state/events, not on "sleep then check". Use polling with timeout.

## Validation

- Every new function/method has at least one test covering the happy path and one error case.
- No tests depend on execution order or shared mutable state.
- CI test suite runs green and under 5 min for unit tests.

✓ New `calculate_discount()` ships with tests covering edge cases.
✗ New `calculate_discount()` ships, tests planned "for later."
