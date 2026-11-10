# TD-20260318-token-tracking-incomplete

- **ID**: TD-20260318-token-tracking-incomplete
- **Area**: Backend (agent output parsing)
- **Status (KT-1601)**: partly resolved. `parse_token_usage` now returns
  `Option<u64>`: `None` is "unmeasured", never 0.
  - Gemini CLI runs with `--output-format stream-json` (listed by `gemini
    --help` on 0.66.0, with `text` and `json`). It emits JSONL events: `init`,
    `message` (`role: "assistant"`, `delta: true`), `tool_use`, `tool_result`,
    `error`, and a final `result` carrying `stats.{total_tokens, input_tokens,
    cached}`. `GeminiStreamAdapter` turns the assistant deltas back into reply
    lines as each one completes (the run stays `OutputMode::Text` for its
    consumers, so the reply still streams), drops the prompt echo, and records
    the `result` usage on the process. The single-envelope `--output-format
    json` was tried first and rejected: the whole reply arrived at the end.
    A run with no `result` event, or with zero tokens, stays unmeasured.
    `[src: file: backend/src/agents/runner.rs:15724#GeminiStreamAdapter]`
  - Gemini stays unpriced: `TokenCounters::from_agent_report` has no
    `GeminiCli` arm on purpose. The reported usage is summed over every model
    the run used (the CLI routes between a pro and a flash model), while a cost
    needs the counters of ONE served model; pricing the sum at one model's rate
    would be wrong. The per-model split is in `stats.models` and is not kept.
  - Vibe: `vibe-runner.py` prints `vibe_tokens:<prompt>:<completion>` on stderr
    from the Mistral stream's `usage` chunk. The Vibe SDK path
    (`run_programmatic`) exposes no usage the script can read, so that path
    stays unmeasured.
    `[src: file: backend/scripts/vibe-runner.py:108#report_usage]`
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
