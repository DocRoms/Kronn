# Agent fallback chain (KT-1518)

When an agent's provider cannot serve a turn — a rate limit, a quota or credits spent, an outage that survived its retries, a CLI that is not signed in — the turn continues on another configured agent instead of stopping. The policy is one module, applied by three surfaces.

## Configuration

**Default chain, per agent.** Settings > Agents shows, on each agent card, the agent's fallback chain as a timeline read left to right — « Claude Code → Codex → Ollama ». Each entry can be changed, given an optional model, moved earlier or later, removed, or made the last one (« stop here »); the whole chain can be cut. For a fallback billed by API key (LiteLLM, NVIDIA) the entry carries the owner's opt-in. It is stored as the ordered list `fallback_chain` on the agent's section of `config.toml` (`[[agents.claude_code.fallback_chain]]`, fields `agent`, `model`, `allow_paid`), served by `GET`/`POST /api/config/agent-fallback` (the POST carries the whole chain; an empty one cuts it). [src: file: backend/src/api/setup.rs:6546]

**Overrides.** With no override, the default chain of the agent a turn starts on applies. A discussion (discussion settings) and a workflow Agent step (step editor) can instead use their own chain, or cancel the fallback (an empty chain). A discussion's override is a row of `discussion_fallback_overrides`, served by `GET`/`PUT /api/discussions/{id}/fallback`; a step's is `agent_settings.fallback` (`None` = default, `Some([])` = no fallback). [src: file: backend/src/api/discussions/agent_fallback_override.rs:1] [src: file: backend/src/models/workflows.rs:1655]

Without a `model`, an entry runs the SAME tier as the interrupted turn (economy stays economy), resolved on its own agent: a model name belongs to one provider, so the failing agent's model is never carried over.

A chain is validated when it is saved — by the UI as the owner edits it, and by the backend, which refuses with the reason and stores nothing: no agent twice or the starting agent in it (a loop), no chain past `MAX_CHAIN_DEPTH` (4 entries), no named connection (`Custom`), no entry with more access (`full_access`) than the agent it replaces, a sane model. [src: file: backend/src/core/agent_fallback.rs:340] It is re-checked at run time, so a loop written by hand in `config.toml` ends in a message, not a walk. A chain is a list of the agent the turn STARTED on, never followed through another agent's own chain: after a hop the walk goes on in the same list, past the entry that just failed, so « Claude Code → Codex → Ollama » goes to Ollama when Codex fails too. [src: file: backend/src/core/agent_fallback.rs:522]

What a turn needs that an entry may lack (MCP tools, workspace writes, image input, a long context) depends on the turn, so it is not refused when saving — a chain may end on a local Ollama on purpose. The timeline shows each entry's gaps as warnings (what the agent it replaces has and the entry lacks); at run time an entry lacking what the turn needs is skipped and the walk goes on. [src: file: backend/src/core/agent_fallback.rs:439]

## What triggers it, and what never does

`classify_trigger` answers `rate_limit`, `quota_exhausted`, `outage` or `not_authenticated` from the provider's own words, and `None` for everything else: a task failure, a refusal, a failing test, and the stall or timeout of the work itself — even when its last output quotes a 429. [src: file: backend/src/core/agent_fallback.rs:101] An outage is retried on the same agent first (the existing overload retry for a saturated provider, one retry for any other outage in a room); only then does the chain take over.

## The three surfaces

| Surface | Where | What happens |
|---|---|---|
| Discussion turn | `intercept_failed_turn` [src: file: backend/src/api/discussions/agent_fallback.rs:236] | The same dispatch is re-queued on the fallback and pinned to it (`dispatch_fallback_pins`: origin agent, target agent, model, tier, hops); the refused reply is replaced by one System line. |
| Workflow Agent step | `continue_after` [src: file: backend/src/workflows/agent_fallback_step.rs:140] | The step is rerun on a copy carrying the fallback agent (tier, budget and prompt kept); the line leads the step's output, and the step's provenance names the agent that really answered. |
| Task worker | `worker_after_quota` / `intercept_failed_turn` | The execution is reassigned to the fallback in its own room and worktree, through the KT-1564 reassignment (same delivery identity rules, origin recorded, journaled; the agent it started on is kept in `execution_fallback_origins`, its chain being the one walked); the parent room gets the line. A campaign's own `quota_fallback` opt-in is asked first for a quota refusal. [src: file: backend/src/api/discussions/agent_fallback.rs:308] |

The line: « Codex (gpt-5.6-sol): quota exhausted until 14:00 UTC -> continued with Claude Code (sonnet) » — from which agent/model, why, until when, to which agent/model (the model is the entry's own, else the tier it keeps: `economy tier`). The discussion shows it as a System line on the turn, the run view on the step (it leads the step's output); both render it as `from → to · reason · until`. When a chain is configured and nothing in it can take over: « Codex: quota exhausted -> no fallback (<why>) ». [src: file: backend/src/core/agent_fallback.rs:610] [src: file: frontend/src/lib/fallbackNotice.ts:1]

## Cooldown and return

The failing agent is left alone until the reset the provider announced (`Retry-After`, `resets 4pm (Europe/Paris)`…), else a short re-try window by trigger (5 minutes for a rate limit or an outage, the KT-1657 re-check interval for a quota, 30 minutes for a missing login). It is a re-try window, not a guess of the provider's schedule: after it the next turn tries the agent again, and a new refusal renews it. [src: file: backend/src/core/agent_fallback.rs:635]

While it lasts, a NEW room turn or workflow Agent step starts straight on the fallback. [src: file: backend/src/api/discussions/agent_fallback_start.rs:20] [src: file: backend/src/core/agent_fallback.rs:736] An expired cooldown is simply not read: the agent is back with nothing to undo. A provider that answers the KT-1657 re-check, a forced readiness check, or a new/changed API key ends the cooldown at once. Settings > Agents shows it on the card.

## Paid fallbacks

An agent billed per call by API key (LiteLLM, NVIDIA, a named connection) is used as a fallback only when the link carries `allow_paid` — the owner's opt-in; otherwise it is skipped and the stop line says so. Subscription CLIs and a local Ollama are not paid. [src: file: backend/src/core/agent_fallback.rs:229]

## The quota-back decision points

When the KT-1657 re-check finds the provider still refusing, the chain is the alternative to waiting: a quota-paused workflow run has its refused step switched to the next usable link (the same gesture as the owner's « switch agent »), and a task execution escalated on an unannounced quota (KT-1664) is reassigned to it. When the provider answers instead, `resume_quota_escalation_after_probe` resumes the original worker as before. [src: file: backend/src/workflows/quota_wait.rs:983] The run's « switch agent » control offers the configured fallback first and preselects it.

## Tests

`core/agent_fallback_test.rs` (triggers, chain, overrides, validation, selection, cooldown, config round trip), `api/discussions/agent_fallback_tests.rs` (rooms), `workflows/agent_fallback_step_tests.rs` (steps), `api/orchestration/tests/agent_fallback_runs.rs` (workers and the KT-1664 decision point); each surface has a no-fallback-on-task-failure test. Frontend: `AgentFallbackControl.test.tsx` (timeline editing, inline refusals, overrides), `fallbackNotice.test.ts`, `QuotaPauseControls.test.tsx`.
