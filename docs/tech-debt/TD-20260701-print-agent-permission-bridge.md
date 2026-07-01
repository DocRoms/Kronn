# TD-20260701-print-agent-permission-bridge

- **ID**: TD-20260701-print-agent-permission-bridge
- **Area**: Backend / Agents
- **Problem (fact)**: A Kronn-orchestrated Claude Code agent runs in `--print` (non-interactive, `backend/src/agents/runner.rs:1118-1146`). `--dangerously-skip-permissions` is only appended when `full_access == true` (`runner.rs:1129-1131`), and `full_access` defaults to **false** (`config.agents.claude_code.full_access`, resolved via `models/setup.rs:319-329`). Consequences when `full_access` is off:
  1. **Gated tools silently blocked.** In `--print` mode there is NO interactive permission channel, and Kronn does not surface a Claude Code permission request in its own UI. Any tool that would prompt (e.g. the `mcp__kronn-internal__api_call` / `qa_run` **write** calls in the "Apply framing" QP) simply cannot be approved → the run stalls with no visible reason. Read-only calls that happen to be pre-allowed still pass, which masks the issue until a write step is hit.
  2. **The agent hallucinates a popup.** Instead of reporting a denial, the model (trained on interactive Claude Code UX) tells the user "a permission popup will appear, click Allow" — but no popup exists anywhere. Observed live 2026-07-01 in disc `a942306f-450a-40b9-8462-a8ba8c2cf6be` (EW-6518 Apply framing), messages 13→15: the agent repeatedly asked the user to approve a non-existent popup; the user saw nothing to click.
- **Why we can't fix now (constraint)**: The immediate unblock is a config toggle (enable full access for the Claude Code agent → adds `--dangerously-skip-permissions`), so it isn't blocking. A proper fix touches the permission-bridge policy (when to auto-skip vs. pre-allowlist which tools) and the agent's denial-reporting, and shouldn't be bundled with the in-flight encryption-key hardening.
- **Impact**: correctness · UX (orchestrated runs stall invisibly; agent emits false "click Allow" instructions that waste user time and erode trust in the run)
- **Where (pointers)**:
  - `backend/src/agents/runner.rs:1116-1146` — `agent_command` for `ClaudeCode`; the `if full_access { --dangerously-skip-permissions }` gate at L1129-1131. Same pattern for Codex at L1225 / Gemini at L1278.
  - `backend/src/models/setup.rs:319-333` — `full_access_for` / `any_full_access` (source of the flag; default false).
- **Suggested direction (non-binding)**:
  - **Force skip-permissions for `--print` runs**, OR pre-authorize the Kronn tool surface via `--allowedTools "mcp__kronn-internal__*"` (+ the QP's declared side-effecting endpoints), so orchestrated agents can call their own tools without an interactive prompt that can never appear. `--print` is inherently non-interactive → the interactive-permission UX does not apply.
  - **Detect the denial and surface it**: when a gated tool call is refused in headless mode, emit a clear structured error ("tool X blocked: enable full access or allowlist it") instead of letting the model invent a popup. Consider a system-prompt note for Kronn-launched agents: "you are headless; there is no permission popup — never instruct the user to click Allow."
  - Longer term: a per-run permission policy in Kronn (allowlist of tools an orchestrated agent may call unattended), decoupled from the coarse `full_access` boolean.
- **Next step**: create ticket.

## Notes

- Surfaced 2026-07-01 while debugging "the agent tells me to click a popup I don't see" on the EW-6518 framing workflow. Root cause verified in code (the `full_access`-gated skip-permissions flag), not a Jira/Atlassian permission and not an MCP credential issue — the Kronn API broker injects creds server-side; the block is purely Claude Code's own tool-permission layer in `--print` mode.
