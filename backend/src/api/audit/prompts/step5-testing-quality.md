Read docs/AGENTS.md for context. Find test framework configs (jest, vitest, phpunit, pytest, cargo test, bats, etc.) and CI quality gates.

Fill docs/testing-quality.md — replace ALL {{PLACEHOLDERS}}:
- Build & quality checks: replace the {{BUILD_CHECKS}} block with a table of all quality checks (compile, lint, format, test, build) and their commands
- Test infrastructure: replace the {{TEST_INFRASTRUCTURE}} block with the runner, config file, and Node/runtime version for each language
- Test suites: replace the {{TEST_SUITES}} block with each test file/suite and its approximate count
- Coverage: replace the {{COVERAGE}} block with the current status and target
- Replace {{UNTESTED}} with components that have NO tests
- Smoke checks: replace the {{SMOKE_CHECKS}} block with 3-5 quick pre-commit commands

**TEST PREREQUISITES COME FROM THE TEST SETUP** — before writing what must be running or configured before the tests (a database, a search engine, another service, environment variables, a registry login), read the test setup files: `setupServer.*`, `vitest.setup.*`, `jest.config.*`, `conftest.py`, `phpunit.xml*` and its bootstrap, the Makefile test target, the compose services a test target starts. When the setup starts a mock, an in-memory fake or its own container, say that, and do not list the real service as a prerequisite. A prerequisite you cannot trace to one of those files is not written.
**ONLY COMMANDS THAT EXIST** — every command in the tables must be a Makefile target, a `package.json` or `composer.json` script, or a tool binary you saw configured; a runtime that exists only in the dev image does not make `yarn install` an application step. Kronn checks every command written as code against those files and sends the step back for one it cannot find.
