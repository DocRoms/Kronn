Read docs/AGENTS.md for context. Find test framework configs (jest, vitest, phpunit, pytest, cargo test, bats, etc.) and CI quality gates.

Fill docs/testing-quality.md — replace ALL {{PLACEHOLDERS}}:
- Build & quality checks: replace the {{BUILD_CHECKS}} block with a table of all quality checks (compile, lint, format, test, build) and their commands
- Test infrastructure: replace the {{TEST_INFRASTRUCTURE}} block with the runner, config file, and Node/runtime version for each language
- Test suites: replace the {{TEST_SUITES}} block with each test file/suite and its approximate count
- Coverage: replace the {{COVERAGE}} block with the current status and target
- Replace {{UNTESTED}} with components that have NO tests
- Smoke checks: replace the {{SMOKE_CHECKS}} block with 3-5 quick pre-commit commands