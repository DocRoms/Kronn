# TD-20260318-token-tracking-incomplete

- **ID**: TD-20260318-token-tracking-incomplete
- **Area**: Backend (agent output parsing)
- **Status (KT-1601)**: partly resolved. `parse_token_usage` now returns
  `Option<u64>`: `None` is "unmeasured", never 0.
  - Gemini CLI runs with `--output-format json`; `GeminiJsonAssembler` unwraps
    the envelope while streaming and keeps `stats.models.*.tokens.total`
    (summed) as the run's usage. Flag and shape checked against Gemini CLI
    0.66.0.
    `[src: file: backend/src/agents/runner.rs#unwrap_gemini_json]`
  - Vibe: `vibe-runner.py` prints `vibe_tokens:<prompt>:<completion>` on stderr
    from the Mistral stream's `usage` chunk. The Vibe SDK path
    (`run_programmatic`) exposes no usage the script can read, so that path
    stays unmeasured.
    `[src: file: backend/scripts/vibe-runner.py#report_usage]`
- **Remaining (constraint)**: `messages.tokens_used` (and the summary-range
  cache) is `INTEGER NOT NULL DEFAULT 0` and `DiscussionMessage.tokens_used` is
  a `u64`, so a discussion reply with no count still stores 0. Workflow steps
  already store NULL (`AgentOutput.tokens_used: Option<u64>`). Making the
  message column nullable needs a rebuild of `messages` (17 migrations
  reference it) and a model/UI change.
- **Impact**: usage and cost figures for discussion replies of agents that
  report nothing read as 0 instead of "unmeasured".
- **Where (pointers)**: `backend/src/agents/runner.rs` (`parse_token_usage`),
  `backend/src/api/discussions/streaming.rs` (the two `unwrap_or(0)` call sites).
