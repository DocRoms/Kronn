# Agent fallback, per mode (KT-1518, redesigned by KT-1693)

When an agent's provider cannot serve a turn — a rate limit, a quota or credits spent, an outage that survived its retries, a CLI that is not signed in — the turn continues on another configured agent instead of stopping. The policy is one module, applied by three surfaces.

## Configuration

**One fallback per agent and per mode.** An agent has up to six modes: `eco`, `standard`, `advanced` (the three text modes, i.e. the economy / default / reasoning tiers), `decision`, `image` and `video`. Settings > Agents shows, on each agent card, one row per mode the agent has — « [current model] → [fallback] → [inherited fallback, dashed] » — and every pill carries its transport badge: **CLI**, **HTTP** or **Local**. There is no « paid » badge and no opt-in: everything but a local model costs something, so enabling a fallback is the consent. A fallback is off by default. « + Add a fallback » opens a menu filtered by kind of mode, grouped by agent, each model shown with its mode and its transport; an agent already in the chain is disabled. A search field at the top filters by agent and model name; the arrows, Enter and Escape drive the menu from the keyboard. The menu is drawn in the page body and placed from its anchor (`placeMenu`: flipped onto the anchor's right edge or above it when needed), so the card never clips it. Clicking a pill changes it, ✕ removes it. [src: file: frontend/src/components/settings/FallbackRow.tsx:298] [src: file: frontend/src/lib/fallbackChain.ts:150]

The fallbacks are stored on the agent's section of `config.toml` as `[[agents.claude_code.mode_fallbacks]]` (`row`, and the `fallback` with its `agent`, optional `model` and optional `mode`) and served by `GET`/`POST /api/config/agent-fallback` (the POST sets or, with `fallback: null`, removes one mode row). [src: file: backend/src/models/setup.rs:993] [src: file: backend/src/api/setup.rs:6878]

**Every installed agent takes part**, OpenCode included: an agent needs no concurrency slot to have rows or to be offered as a target. [src: file: backend/src/core/agent_fallback.rs:344]

**Kind rules.** Text modes fall back to any agent/model of a text mode (eco, standard or advanced); `image` to `image`; `video` to `video`; `decision` to `decision`. An incompatible target is never offered by the menu and is refused by the backend with the reason. A fallback's `mode` is the mode of the model it runs on; without it, the mode of the row it replaces. [src: file: backend/src/core/agent_fallback.rs:462]

**Chaining.** When the fallback is itself unavailable, the fallback set on ITS own mode row applies (the row of the target agent, in the mode the target runs in), at most `MAX_CHAIN_DEPTH` (4) links, an agent never twice and never itself. A row that would make a loop or a chain past the limit is refused when saved; a loop written by hand in `config.toml` is simply not walked. The inherited links are shown dashed, with « set on <agent>'s card »; clicking one opens that card. [src: file: backend/src/core/agent_fallback.rs:389] [src: file: backend/src/core/agent_fallback.rs:534]

**Access.** A fallback never gets more access (`full_access`) than the agent it replaces; this is refused when saved and re-checked when a link is chosen.

**Migration.** The agent-level chain of 0.15.3 (`fallback_chain`) is carried at start-up onto the text rows of that agent: its first link on eco, standard and advanced, the next links on the rows of the link before them, so « A → B → C » is still « A → B → C ». Where a row is already set (an agent's own chain wins), the extra link is dropped; the legacy field is emptied and never written again. A discussion's stored override (one list for the whole discussion) becomes the same override on each text mode, an empty one cutting them. `allow_paid` is read and ignored. [src: file: backend/src/core/agent_fallback.rs:597] [src: file: backend/src/core/config.rs:618]

**Overrides, per mode.** With no override, the row of the agent a turn starts on, for the mode the turn runs in, applies. A discussion (discussion settings, one row per text mode, right under the discussion's agent) and a workflow Agent step (step editor, the row of the step's tier, right under the step's agent and mode picker — visible without opening the step's « advanced » section) can instead use their own fallback, go back to the default, or cut the fallback — the same component as the cards. A discussion's overrides are stored per mode in `discussion_fallback_overrides` and served by `GET`/`PUT /api/discussions/{id}/fallback` (the PUT carries the `mode`); a step's is `agent_settings.fallback` (`None` = default, `Some([])` = cut). An override replaces the first link; the rows of its fallbacks then go on. [src: file: backend/src/api/discussions/agent_fallback_override.rs:67] [src: file: backend/src/models/workflows.rs:1908]

Without a `model`, a link runs the SAME tier as the interrupted turn (economy stays economy), resolved on its own agent: a model name belongs to one provider, so the failing agent's model is never carried over. The menu therefore offers a model-less choice on the row's own mode only.

## What triggers it, and what never does

Unchanged from KT-1518. `classify_trigger` answers `rate_limit`, `quota_exhausted`, `outage` or `not_authenticated` from the provider's own words, and `None` for everything else: a task failure, a refusal, a failing test, and the stall or timeout of the work itself — even when its last output quotes a 429. [src: file: backend/src/core/agent_fallback.rs:110] An outage is retried on the same agent first; only then does the fallback take over.

## The three surfaces

| Surface | Where | What happens |
|---|---|---|
| Discussion turn | `intercept_failed_turn` [src: file: backend/src/api/discussions/agent_fallback.rs:275] | The same dispatch is re-queued on the fallback and pinned to it (`dispatch_fallback_pins`: origin agent, target agent, model, tier, hops); the refused reply is replaced by one System line. |
| Workflow Agent step | `continue_after` [src: file: backend/src/workflows/agent_fallback_step.rs:179] | The step is rerun on a copy carrying the fallback agent (tier, budget and prompt kept); the line leads the step's output, and the step's provenance names the agent that really answered. |
| Task worker | `worker_after_quota` [src: file: backend/src/api/discussions/agent_fallback.rs:340] | The execution is reassigned to the fallback in its own room and worktree, through the KT-1564 reassignment; the agent it started on is kept in `execution_fallback_origins`, its rows being the ones walked; the parent room gets the line. A campaign's own `quota_fallback` opt-in is asked first for a quota refusal. |

The text modes are the ones a turn, a step and a worker run in. The decision, image and video rows are stored, validated, chained and shown like the others; the runtime hook of those step types is not part of this change.

The line: « Codex · gpt-5.6-sol: quota exhausted until 14:00 UTC → continued with Claude Code · sonnet [CLI] » — from which agent/model, why, until when, to which agent/model and over which transport (the model is the link's own, else the mode it keeps: `eco mode`). The discussion shows it as a System line on the turn, the run view on the step (it leads the step's output); both render it as `from → to [transport] · reason · until`. A line written by 0.15.3 (`->`, no transport) is still read. When a fallback is configured and nothing in it can take over: « Codex: quota exhausted → no fallback (<why>) ». [src: file: backend/src/core/agent_fallback.rs:875] [src: file: frontend/src/lib/fallbackNotice.ts:1]

## Cooldown and return

The failing agent is left alone until the reset the provider announced (`Retry-After`, `resets 4pm (Europe/Paris)`…), else a short re-try window by trigger (5 minutes for a rate limit or an outage, the KT-1657 re-check interval for a quota, 30 minutes for a missing login). It is a re-try window, not a guess of the provider's schedule: after it the next turn tries the agent again, and a new refusal renews it. The cooldown is per agent, whatever the mode. [src: file: backend/src/core/agent_fallback.rs:931]

While it lasts, a NEW room turn or workflow Agent step starts straight on the fallback. [src: file: backend/src/api/discussions/agent_fallback_start.rs:20] [src: file: backend/src/core/agent_fallback.rs:1032] An expired cooldown is simply not read: the agent is back with nothing to undo. A provider that answers the KT-1657 re-check, a forced readiness check, or a new/changed API key ends the cooldown at once. Settings > Agents shows it on the card.

## The quota-back decision points

When the KT-1657 re-check finds the provider still refusing, the fallback is the alternative to waiting: a quota-paused workflow run has its refused step switched to the next usable link (the same gesture as the owner's « switch agent »), and a task execution escalated on an unannounced quota (KT-1664) is reassigned to it. When the provider answers instead, `resume_quota_escalation_after_probe` resumes the original worker as before. [src: file: backend/src/workflows/agent_fallback_step.rs:274] The run's « switch agent » control offers the configured fallback first (the standard row of the agent) and preselects it.

## Tests

`core/agent_fallback_test.rs` (triggers, rows, kind rules, chaining and depth, loop refusal, migration, overrides per mode, selection, cooldown, config round trip), `api/discussions/agent_fallback_tests.rs` (rooms and the per-mode override API), `workflows/agent_fallback_step_tests.rs` (steps, chaining through the target's row), `api/orchestration/tests/agent_fallback_runs.rs` (workers and the KT-1664 decision point); each surface has a no-fallback-on-task-failure test. Frontend: `AgentFallbackControl.test.tsx` (rows, pills, transport badges, inherited pill, kind-filtered menu, overrides), `FallbackMenu.test.tsx` (menu placement inside the viewport, search, keyboard), `WorkflowWizard.fallback.test.tsx` and `DiscussionSettingsPanel.test.tsx` (override placement), `fallbackChain.test.ts`, `fallbackNotice.test.ts`, `AgentsSection.actions.test.tsx`, `QuotaPauseControls.test.tsx`.
