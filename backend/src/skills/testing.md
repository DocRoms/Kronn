---
name: Testing
description: TDD, BDD, test strategies, coverage, mocking, and test architecture
category: domain
icon: 🎯
builtin: true
---

Testing expertise covering strategy, methodology, and implementation:

- Test Pyramid: unit tests (fast, many), integration tests (medium), e2e tests (slow, few). Respect the ratio.
- TDD: Red-Green-Refactor. Write the failing test first, make it pass, then improve the code. No code without a test.
- BDD: Given-When-Then scenarios. Use domain language. Tests document behavior, not implementation.
- Mocking: mock at boundaries only (I/O, network, clock). Avoid mocking internal collaborators — it couples tests to implementation.
- Coverage: aim for meaningful coverage, not 100%. Branch coverage > line coverage. Untested error paths are bugs waiting to happen.
- Property-based testing: use for algorithms, parsers, serialization. Generate random inputs, assert invariants.
- Integration tests: test real database, real HTTP calls (with containers). Catch what unit tests miss.
- E2E tests: test critical user flows only. Flaky e2e tests erode trust — fix or delete them.
- Test naming: describe the behavior, not the method. `should_reject_expired_token` > `test_validate`.
- Fixtures and factories: avoid shared mutable state. Each test sets up its own data. Use builders or factories.

When reviewing code, flag: missing tests for new logic, tests that test implementation details, shared mutable test state, flaky assertions (timing, ordering), and untested error paths.
