# Compare panels must show recorded identity, not today's config

`BatchComparePanel`/`BatchCompareDetailsPanel` display a comparison child's
model column. When no message on that child carried a recorded model
(`answer.model`, `discussion.model`, or the last message with a `model`
field), the fallback used to call `modelForAgentTier(discussion.agent, tier,
modelTiers, defaultLabel)` — the SAME helper used by live agent/tier pickers
to resolve the *current* configuration. A comparison run from weeks ago would
then silently display whatever model is configured *today* for that
agent/tier, as if it were the model that actually answered. The fallback now
resolves to the plain generic label only, never a live config lookup.
[src: file: frontend/src/components/BatchComparePanel.tsx:178-181]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:438-441]

Connection identity resolution itself (`compareAgentLabel` /
`externalConnectionForDiscussion`) was already correct: it matches by the
connection's durable `connection_id` from `message_targets`, never by
`display_name`, so two connections sharing the same display name ("homonyms")
still keep their own recorded model and label.
[src: file: frontend/src/lib/externalAgentIdentity.ts:46-77]

## Launch-target suffix must reuse the shared picker resolution

The Compare launch selector in `WorkflowsPage` (QP "compare" mode) used to
pass its own manually computed `suffix` prop to `AgentSwitchPicker`:
`selectedChoice?.modelTiers?.[tier] ?? modelForAgentTier(target.agent, tier,
agentAccess?.model_tiers, defaultLabel)`. For a named HTTP connection with an
unset tier, this discarded the connection's own known configuration (e.g. its
`default_model`) and fell back straight to the generic default label — a
"family" value standing in for the connection's own identity, since
`AgentSwitchPicker`'s own `configuredModel()` already has a same-connection
fallback (`target.modelTiers?.default` for HTTP targets) that the ad-hoc
suffix bypassed entirely. The judge/improver pickers in
`BatchCompareDetailsPanel` never passed a manual `suffix` and were unaffected.
The fix removes the manual `suffix` override so the Compare selector reuses
the same catalogue+identity resolution as every other `AgentSwitchPicker`
caller.
[src: file: frontend/src/pages/WorkflowsPage.tsx:3203-3232]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:124-143]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:330-346]

Regressions: a config-drifted-after-the-run case (no answer, no historical
model, current tier config now non-null) for both compare panels, a
no-response case, and a homonymous-connections case verifying each column
keeps its own recorded model instead of blending. The launch-target fix is
covered by switching a named connection's compare target to a tier it never
configured and asserting its own `default_model` surfaces instead of the
generic label.
[src: file: frontend/src/components/__tests__/BatchComparePanel.test.tsx:476-544]
[src: file: frontend/src/components/__tests__/BatchCompareDetailsPanel.test.tsx:1-189]
[src: file: frontend/src/pages/__tests__/WorkflowsPage.qp-launch.test.tsx:591-643]

These are component-level regressions against mocked APIs; no backend, worker,
or provider execution was exercised.
