# TD-20260724-workflow-agent-fallback

- **ID**: TD-20260724-workflow-agent-fallback
- **Area**: Backend | Workflows | Agents
- **Problem (fact)**: An Agent workflow step stores one `AgentType`. When that
  provider is rate-limited, unavailable, or temporarily unhealthy, the step
  follows its normal retry/failure path with the same agent. Kronn has no
  ordered fallback-agent policy.
- **Why we can't fix now (constraint)**: Safe failover needs structured error
  classification (quota/rate limit/transient outage versus invalid prompt,
  permissions, missing tools, or deterministic failure), capability checks,
  an explicit fallback order, and protection against replaying side effects.
  The run history must also record which agent was attempted and why routing
  changed.
- **Impact**: correctness | operational friction | unattended-run reliability
- **Where (pointers)**:
  - `backend/src/models/workflows.rs` (`WorkflowStep`, `RetryConfig`)
  - `backend/src/workflows/steps.rs` (Agent step execution and retries)
  - `backend/src/agents/runner.rs` (provider process errors)
  - `frontend/src/components/workflows/WorkflowDetail.tsx` (manual agent switch)
- **Suggested direction (non-binding)**: Add an opt-in per-step routing policy
  with an ordered fallback list, capability validation, provider cooldowns,
  retry/failover budgets, and structured run events. Preserve the step's
  abstract model tier when switching providers unless an explicit per-provider
  override exists.
- **Next step**: Write an ADR for failure taxonomy and routing semantics, then
  add runner-level regression tests for rate limit, unavailable binary,
  permission failure, and side-effect replay prevention.
