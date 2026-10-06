# TD-20260318-token-tracking-incomplete

- **ID**: TD-20260318-token-tracking-incomplete
- **Area**: Backend (agent output parsing)
- **Problem (fact)**: on the CLI text path, `parse_token_usage` returns 0
  tokens for Gemini CLI and Vibe, pinned by tests.
  `[src: file: backend/src/agents/runner.rs:13450-13454]`
  `[src: file: backend/src/agents/runner_test.rs:9187-9197]`
- **Why we can't fix now (constraint)**: neither CLI prints a token count on
  that path; the measurement has to come from upstream.
- **Impact**: correctness of usage and cost figures for those two agents.
- **Where (pointers)**: `backend/src/agents/runner.rs` (`parse_token_usage`).
- **Suggested direction (non-binding)**: read usage from a structured output
  mode or transport when the CLI offers one, and report the tokens as
  unmeasured rather than zero meanwhile.
- **Next step**: create ticket once upstream exposes usage.
