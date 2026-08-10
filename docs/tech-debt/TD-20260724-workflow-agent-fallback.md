# TD-20260724-workflow-agent-fallback

- **ID**: TD-20260724-workflow-agent-fallback
- **Area**: Backend | Workflows | Discussions | Agents | Frontend | Config
- **Status**: Open — product and safety design captured; implementation must be
  split into focused changes.
- **Impact**: correctness | operational friction | unattended-run reliability |
  cost attribution

## Problem and intended product contract

Kronn binds an execution to one `AgentType`. A workflow Agent step therefore
keeps retrying the same provider and eventually fails when that provider is
rate-limited, out of quota, unavailable, or unhealthy. The user then has to
notice the failed run, edit Claude to Codex (for example), and replay it by hand.
[src: file: backend/src/models/workflows.rs:264-308]
[src: file: backend/src/workflows/steps.rs:213-246]
[src: user: 2026-08-10: unattended workflow steps should continue on an
explicit fallback and make the switch visible]

The target feature is an **automatic cross-agent fallback policy** with this
contract:

- it ships **disabled by default** so an upgrade cannot silently change the
  provider, data boundary, or cost of existing automation;
- once the user enables the global policy, workflow steps inherit it by
  default; a workflow or individual step may explicitly disable or replace the
  inherited chain;
- the abstract tier is preserved by default: a failed Claude `reasoning` run
  can route to the configured Codex `reasoning` model, not reuse the
  Claude-specific concrete model name;
- an optional `prefer_local_when_compatible` setting moves a healthy local
  target ahead of cloud candidates, but never bypasses required capabilities;
- every fallback is durable and visible while the run is live and after reload:
  requested target, attempted targets, reason, final target, tokens, and cost;
- fallback is fail-closed when Kronn cannot prove that replay or continuation is
  safe.

“Enabled by default for my steps” therefore means: the user enables one global
switch, and every supported step displays **Fallback active · inherited**. It
does not mean enabling the feature without consent on a fresh installation.

## Why this is not a model-name substitution

An agent start contains the provider/transport, filesystem and MCP context,
skills/directives/profiles, tool executor, tier, explicit model override, and
discussion identity. Switching Claude to Codex or Ollama changes more than the
`--model` flag. [src: file: backend/src/agents/runner.rs:410-458]

Kronn already has the useful abstraction needed for routing: `ModelTier` plus a
per-agent `ModelTierConfig`. Concrete models are resolved independently for
Claude, Codex, Gemini, Copilot, Ollama, and LiteLLM. An explicit per-step model
currently wins over the tier. [src: file: backend/src/models/setup.rs:415-470]
[src: file: backend/src/agents/runner.rs:539-650]

Consequently, a fallback target must be represented as:

```text
FallbackTarget {
  agent_type,
  tier,              // normally inherited from the requested run
  model_override?,   // belongs to this target, never copied across providers
}
```

“Installed” is not enough to select that target. Current detection separately
tracks installed, enabled, runtime-available, authentication-ready, and runtime
warning states. HTTP providers additionally need endpoint health, model
availability, and model capability checks. [src: file:
backend/src/models/setup.rs:495-548]

## Policy and precedence

Suggested non-binding configuration shape:

```text
AgentFallbackConfig {
  enabled: false,
  prefer_local_when_compatible: false,
  max_total_attempts: 3,
  tiers: {
    economy:   [FallbackTarget...],
    default:   [FallbackTarget...],
    reasoning: [FallbackTarget...],
  }
}

FallbackOverride = inherit | disabled | custom_chain
```

Resolution order:

1. Resolve the requested agent, abstract tier, and requested concrete model.
2. Apply the existing same-provider retry policy only when it can help. A hard
   quota, explicit rate limit, or known unavailable runtime/model should move
   immediately to the next target; an ambiguous 5xx/network transient may get
   one cheap same-provider retry before fallback.
3. Classify the terminal attempt and its effect state.
4. If fallback is enabled and allowed for that failure/effect pair, build the
   eligible candidate list.
5. Apply the explicit tier chain. `prefer_local_when_compatible` may promote a
   local candidate inside that eligible set; it must not invent an unconfigured
   model or route.
6. Enforce one total attempt budget, exclude already attempted targets, and
   honor provider cooldowns / `Retry-After` to prevent loops and retry storms.
7. Persist the failed attempt and emit the fallback event **before** starting
   the next target.

The fallback budget and the existing retry budget must be distinct. Otherwise
three candidates each configured with three retries silently turn one step into
nine paid attempts.

## Typed failure taxonomy

The execution boundary currently exposes strings for start failures and only
three coarse discussion outcomes (`Finished { success }`, `PreflightFailed`,
`RuntimeUnavailable`). The richer rate-limit/auth/server detection is a
presentation helper that scans raw output after execution; it is not a routing
contract. [src: file: backend/src/agents/runner.rs:653-669]
[src: file: backend/src/api/discussions/streaming.rs:230-245]
[src: file: backend/src/api/discussions/orchestration.rs:1226-1307]

Fallback first needs a runner-level, provider-normalized error such as:

```text
AgentFailure {
  kind: RateLimited | QuotaExhausted | ProviderUnavailable |
        RuntimeUnavailable | ModelUnavailable | Authentication |
        Network | Timeout | ContextOverflow | InvalidRequest |
        Permission | CapabilityMismatch | ToolFailure | Cancelled | Unknown,
  retry_after?,
  phase: Preflight | Starting | Running | Finalizing,
  emitted_output: bool,
  observed_tool_activity: bool,
  effect_state: None | ReadOnly | MutatedWorkspace | ExternalEffect | Unknown,
  raw_detail,
}
```

The raw provider output remains useful for diagnostics, but routing decisions
must use the typed value. Unknown errors are not fallback-eligible.

### Default decision matrix

| Failure | Automatic fallback | Notes |
|---|---:|---|
| Rate limit / quota / usage cap | Yes | Cool down the failed provider; do not hide the warning. |
| Provider overload / 5xx / connection failure | Yes | Only if the effect state permits another attempt. |
| Runtime or configured model unavailable | Yes | Candidate must pass readiness and model checks first. |
| Authentication unavailable/expired | No by default | Keep the provider visibly broken; a later explicit policy may permit provider-scoped auth failover. |
| Stall / timeout | No in the first release | Silence does not prove that the agent performed no tool or file effect. Resume-aware routing may support it later. |
| Context overflow | No in the first release | A later policy may allow a target whose context capacity is verified. |
| Invalid request / invalid prompt | No | Another provider can fail differently and obscure the actual bug. |
| Permission / access-policy denial | No | Fallback must not bypass a user or project security boundary. |
| Missing required MCP/tool/capability | No | Fix configuration or select a capability-compatible explicit target. |
| Safety refusal | No | Never use provider switching to bypass a refusal. |
| Invalid structured output | No, not availability fallback | This is quality escalation/repair and needs a separate explicit policy. |
| User cancellation | No | Cancellation always wins. |
| Unknown | No | Fail closed and show the unclassified error. |

Kronn already has an Ollama `TypedSchema` quality escalation that retries on
Claude after local validation and repair fail. That behavior is adjacent, but
semantically different from availability fallback and should eventually use an
explicit escalation policy rather than be folded into this router. [src: file:
backend/src/workflows/steps.rs:253-385]

## Workflow replay and continuation safety

This is the hardest part. An Agent step can modify the worktree or call tools
before a provider error. The current executor streams output and tool markers,
then only returns success or a string error; it does not produce a durable
effect boundary suitable for safe cross-agent replay. [src: file:
backend/src/workflows/steps.rs:727-793]
[src: file: backend/src/workflows/steps.rs:822-903]

The router must distinguish these cases:

| Attempt state | Safe behavior |
|---|---|
| Failure before process/request start | Start the next eligible target with the original prompt. |
| Rate limit/provider rejection before output or tools | Start the next target with the original prompt. |
| Read-only reasoning emitted partial text, no tools/effects | First release: stop, because Kronn cannot yet prove the absence of effects across every CLI. Later: replay or provide bounded partial output as handoff context. |
| Workspace files changed, no external effect | Do **not** blindly replay. Start a resume-aware target only after it inspects the current worktree and receives the failed attempt's bounded handoff. |
| External tool/API side effect observed | Stop unless the tool supplied an idempotency key or a durable exactly-once receipt. |
| Effect state unknown | Stop and require explicit operator confirmation. |

The existing crash-resume guard for deterministic workflow steps already
refuses to replay an uncertain external effect without explicit confirmation;
the fallback router should reuse the same principle. Agent and Batch Quick
Prompt steps are currently outside that uncertain-effect journal, so they need
an equivalent durable attempt intent before automatic fallback can be
restart-safe. [src: file: backend/src/workflows/runner.rs:107-151]
[src: file: backend/src/workflows/runner.rs:2689-2716]

Additional workflow invariants:

- fallback happens inside the current step and worktree; it must not create a
  second workflow run;
- `RetryConfig` retries the current target; fallback advances to the next
  target only after the current retry decision is complete;
- `on_timeout`, `on_result`, rollback, `Goto`, and loop counters evaluate only
  the final step outcome, not every failed provider attempt;
- all attempt tokens/costs count, including failed providers;
- the workflow LLM-call budget is checked before every physical invocation
  (retry, repair, review, quality escalation, or fallback), rather than once per
  logical Agent step;
- a Batch Quick Prompt applies policy independently to each child execution;
  one child's fallback must not reroute the other children;
- multi-agent review/debate roles each have their own requested target and
  trace; a reviewer fallback must not overwrite the author's provenance;
- schema repair and quality escalation attempts remain distinguishable from
  availability fallback attempts;
- process cancellation, workflow stop, and application shutdown cancel the
  whole chain and cannot trigger the next provider.
- candidate permissions are recomputed for the actual agent and capped by the
  original step's access intent; fallback can never increase privileges.

## Capability-compatible local preference

Local-first is a ranking preference, not a guarantee. At minimum every
candidate must prove:

- runner/endpoint reachable, enabled, and authenticated where applicable;
- selected model exists and is healthy;
- sufficient context window and requested output format support;
- required filesystem, MCP, Kronn-tool, vision/file, and permission semantics;
- compatibility with the run's data boundary and project access policy.

This matters today because HTTP agents have a different execution path and
workflow Agent steps do not yet receive the Kronn tool executor. A local Ollama
model therefore cannot safely replace every CLI step merely because the model
is pulled. [src: file: backend/src/agents/runner.rs:510-536]
[src: file: backend/src/workflows/steps.rs:759-779]
[src: file: docs/tech-debt/TD-20260808-http-agents-no-tool-calling.md:67-76]

No-capability declaration should mean “unknown/ineligible”, not “probably
compatible”. This keeps `prefer_local_when_compatible` honest.

## Execution surfaces and rollout boundary

A single execution router should ultimately serve all Kronn-owned agent runs;
adding ad-hoc fallback loops at each call site would produce different safety
and audit behavior.

The current spawn audit found these distinct Kronn-owned paths:

| Current spawn path | What it covers |
|---|---|
| Discussion streaming | Normal replies, manual/forced runs, durable dispatch, and Quick Prompt / Batch Quick Prompt child discussions. HTTP tools are attached only here. [src: file: backend/src/api/discussions/streaming.rs:1004-1028] |
| Debate pre-summary | Compresses the shared context before debate. [src: file: backend/src/api/discussions/orchestration.rs:337-427] |
| Debate participants | Starts every participant/round independently. [src: file: backend/src/api/discussions/orchestration.rs:432-580] |
| Debate final synthesis | Starts the primary synthesis agent. [src: file: backend/src/api/discussions/orchestration.rs:592-660] |
| Automatic discussion summary | Economy-tier background summarisation. [src: file: backend/src/api/discussions/orchestration.rs:916-960] |
| Explicit discussion summary | User-triggered summary path. [src: file: backend/src/api/discussions/orchestration.rs:1107-1175] |
| Workflow Agent helper | Normal attempts, retries, structured-output repair, current Ollama quality escalation, multi-agent review turns, and rollback Agent steps. [src: file: backend/src/workflows/steps.rs:213-408] [src: file: backend/src/workflows/steps.rs:727-794] [src: file: backend/src/workflows/runner.rs:2047-2134] |
| Full audit | Reasoning-tier filesystem audit. [src: file: backend/src/api/audit/full.rs:1006-1031] |
| Drift audit | Reasoning-tier partial audit. [src: file: backend/src/api/audit/drift.rs:468-485] |

Audits currently forbid HTTP/Vibe/Custom targets because those paths cannot
perform the required filesystem work. This is a concrete example of why local
or merely installed candidates cannot be universally eligible. [src: file:
backend/src/api/audit/mod.rs:1186-1205]

| Surface | Desired behavior | Initial rollout |
|---|---|---:|
| Normal workflow Agent step | Inherit global policy; show live and durable step fallback | Yes — priority case |
| Workflow repair / local quality escalation | Separate attempt purpose and policy | Foundation only |
| Multi-agent review/debate | Route author and reviewer independently | Later |
| Batch Quick Prompt children | Route each child independently | Later |
| Sub-workflow | Child workflow resolves its own inherited policy | Later |
| Normal discussion reply | Visible System notice plus final message provenance | After workflow MVP |
| Discussion orchestration / synthesis / summary | Explicit per-role semantics; no silent extra provider | Later |
| Full/drift audits and project AI helpers | Use shared router after they can persist attempts | Later |
| Joined external CLI peers | Out of scope: Kronn does not own or restart their process | Never automatically |

## Durable audit model and UI

`DiscussionMessage` already persists the actual `agent_type`, `model_tier`, and
concrete `model`. `StepResult` already has `step_agent` and `step_model`, and
Run Detail renders them. [src: file: backend/src/models/workflows.rs:1115-1169]
[src: file: frontend/src/components/workflows/RunDetail.tsx:1030-1045]

That is not sufficient for fallback because failed attempts disappear and a
workflow result currently stamps the agent/model from the configured step,
not from an execution trace. [src: file:
backend/src/workflows/runner.rs:2865-2899]

Persist an `AgentAttempt` record (table or versioned run JSON) with:

```text
id, execution_kind, owner_id, workflow_step_id?, attempt_index, purpose,
requested_agent/tier/model, actual_agent/tier/model,
started_at, ended_at, status, failure_kind, retry_after,
emitted_output, effect_state, tokens, cost_usd, raw_detail
```

`purpose` distinguishes `primary`, `same_provider_retry`,
`availability_fallback`, `schema_repair`, `quality_escalation`, `reviewer`, and
`synthesis`. The successful `StepResult`/message keeps the actual final target,
while the attempt timeline preserves the request and every failure.

Required UI:

- Settings → Agents: master switch (off by default), `prefer local when
  compatible`, maximum attempts, and an ordered chain per tier;
- candidate chips: installed, enabled, authenticated, endpoint healthy, model
  available, and capability-compatible; invalid entries explain why they will
  be skipped;
- Workflow editor: `inherit / disabled / custom chain`, with **Fallback active
  · inherited** visible on Agent step cards when the global switch is on;
- live run event before the new attempt, for example:
  `Fallback: Claude · opus → Codex · gpt-5.6-sol — rate limit (2/3)`;
- Run Detail: persistent warning/badge and expandable attempt timeline;
- usage/cost totals include every attempt and can attribute wasted failed
  attempts separately;
- discussion fallback is a System event plus a badge on the final answer, not
  an ephemeral toast.

Workflow runs currently persist token totals but no first-class workflow cost
field. Exact cross-provider cost display therefore needs additive persistence,
not only a frontend estimate. [src: file:
backend/src/models/workflows.rs:994-1003]

The current workflow budget also increments once per logical Agent step, even
though retry, repair, escalation, or debate can perform more model invocations.
Fallback must fix this accounting at the physical-attempt boundary. [src: file:
backend/src/workflows/runner.rs:1498-1531]

Do not reuse the existing `native_fallback` name: that field describes the
separate discussion-routing case where Kronn's native agent answers an
untargeted turn when no external CLI peer accepts it. [src: file:
backend/src/db/sql/105_user_turn_catchup.sql:1-10]

## Delivery plan

1. **ADR and typed foundation** — define failure/effect taxonomy, routing
   precedence, capability contract, and attempt purposes. Add provider parser
   fixtures; keep automatic fallback disabled.
2. **Observability without rerouting** — persist attempts and render requested
   versus actual provenance/cost. This is useful immediately and proves the
   event model before it changes execution.
3. **Workflow MVP** — global opt-in policy inherited by normal Agent steps;
   support preflight and pre-effect rate-limit/quota/provider failures only.
   Emit live and durable fallback events.
4. **Resume-aware workflow fallback** — add effect receipts and bounded handoff
   for partial workspace work; keep unknown/external effects fail-closed.
5. **Batch, debate, repair, and sub-workflow semantics** — preserve separate
   roles/purposes and aggregate all costs correctly.
6. **Discussions, audits, and helpers** — move remaining Kronn-owned execution
   surfaces to the shared router.
7. **Health-aware local preference** — model/capability probes, provider
   cooldown/circuit breaker, and validated local promotion.

## Required regression coverage

- disabled-by-default and `inherit / disabled / custom` precedence;
- abstract-tier preservation and per-target model overrides;
- installed-but-disabled, unauthenticated, unreachable, missing-model, and
  capability-incompatible candidates are skipped with an audited reason;
- rate limit, quota, overload, network, missing runtime/model, auth, context
  overflow, permission, invalid request, cancellation, and unknown errors;
- same-provider retry occurs before fallback without multiplying the global
  attempt budget;
- provider cooldown and `Retry-After`; no candidate loops;
- fallback before output succeeds; partial output is not duplicated;
- workspace mutation resumes rather than blind-replays; external/unknown effect
  blocks fallback;
- stop/cancel/shutdown never starts the next target;
- rollback, `Goto`, `on_timeout`, schema repair, quality escalation, debate,
  Batch Quick Prompt, and sub-workflow accounting;
- one Batch Quick Prompt child may fall back without recreating sibling
  discussions or incrementing the parent completion count twice;
- a crash between durable attempt intent and result never auto-replays an
  effect-unknown Agent attempt;
- failed-attempt and successful-attempt tokens/costs survive reload/export;
- SSE ordering: failure event persisted, fallback event emitted, next start;
- legacy workflow runs without attempt traces still render correctly.

## Difficulty and next step

The configuration UI alone is straightforward. A workflow-only MVP limited to
pre-effect provider failures is medium complexity. Safe fallback across every
execution surface, with partial-work continuation, capability-aware local
routing, exact cost attribution, and restart-safe audit is **high complexity**
(roughly 8/10) because the current execution paths and outcomes are fragmented
and side-effect state is not yet a shared contract.

This should be several reviewable changes, not a last-minute 0.9.4 addition.
The next implementation step is an ADR plus a no-rerouting `AgentAttempt`/
`AgentFailure` foundation; then ship the normal workflow Agent-step MVP behind
the default-off master switch.
