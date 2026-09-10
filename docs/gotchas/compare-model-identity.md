# Compare panels must show recorded identity, not today's config

`BatchComparePanel`/`BatchCompareDetailsPanel` display a comparison child's
model column. When no message on that child carried a recorded model
(`answer.model` or the last message with a `model` field), the fallback used
to call `modelForAgentTier(discussion.agent, tier, modelTiers, defaultLabel)`
— the SAME helper used by live agent/tier pickers to resolve the *current*
configuration. A comparison run from weeks ago would then silently display
whatever model is configured *today* for that agent/tier, as if it were the
model that actually answered. The fallback now resolves to the plain generic
label only, never a live config lookup.
[src: file: frontend/src/components/BatchComparePanel.tsx:195-197]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:455-457]

## `discussion.model` is a forward override, never a record of what answered

`discussion.model` (`0.8.10 — explicit model override for this discussion`)
is a mutable target for the NEXT run — it can be edited at any time
independently of what already answered — and used to sit in the same
fallback chain as the message-level `answer.model`/history lookup. That
conflated two unrelated things: a user-editable future setting and a
response-attested historical fact. Both panels now derive the displayed model
only from `answer.model` (normalized, the response that actually produced the
compared answer) or, failing that, the last message in history that recorded
a concrete `model` — never from `discussion.model`.
[src: file: frontend/src/types/generated.ts:1793-1798]
[src: file: frontend/src/components/BatchComparePanel.tsx:195-197]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:455-457]

Both ranking card and main column now badge the model to disclose its
provenance instead of a single generic "inferred from tier" label (a holdover
from the tier-inference removal above, which no longer applied once the badge
started firing on ANY missing `answer.model`, not just a tier guess):
`modelRecorded` when the shown model came from an earlier message rather than
the final answer itself, `modelUnknown` when nothing was ever recorded and the
generic default label is shown, and no badge at all when the final answer
itself carries the model.
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:477-478]
[src: file: frontend/src/components/BatchComparePanel.tsx:233-237]

### `lastRecordedModel` is bounded to strictly before the compared answer

`lastRecordedModel` used to scan the ENTIRE message history backwards with no
regard for the compared answer's own position. A System/recovered-partial
record written AFTER that answer (e.g. a later retry attempt with a different
backend) could therefore win and be shown with the `modelRecorded` badge —
which claims "from an earlier message" — even though it was never known when
the compared answer was produced. Both panels now pass the answer message
itself as a `before` bound: the backward scan stops strictly before that
message's index, so only genuinely earlier records can surface as
`modelRecorded`. When there is no answer at all (`before` is `null`), the scan
stays unbounded — there's no answer's provenance to protect at that point, so
any recorded model (e.g. from a System message documenting an attempted model
before failing) is legitimately shown.
[src: file: frontend/src/components/BatchComparePanel.tsx:43-57]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:47-61]

Both panels also normalize model ids consistently: an empty or
whitespace-only string is treated as absent (trimmed, `''` → `null`), the same
as a `null`/`undefined` field, so stray whitespace from a legacy row cannot
silently pass through as if it were a real recorded model.
[src: file: frontend/src/components/BatchComparePanel.tsx:59-62]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:63-66]

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
[src: file: frontend/src/components/AgentSwitchPicker.tsx:108-123]
[src: file: frontend/src/components/BatchCompareDetailsPanel.tsx:343-354]

Regressions: a config-drifted-after-the-run case (no answer, no historical
model, current tier config now non-null), a no-response case, and a
homonymous-connections case verifying each column keeps its own recorded
model instead of blending, for both compare panels. Provenance regressions
add, per panel: response-attested model taking precedence over an older
recorded one (unbadged), a missing/blank final-answer model falling back to
that older recorded model rather than the live `discussion.model` override
(badged `modelRecorded`), and a no-response case with a `discussion.model`
override still resolving to the honest generic/unknown label (badged
`modelUnknown`). Two further regressions per panel guard the `lastRecordedModel`
bound: a missing-model final answer followed by a later System
attempted-model record must resolve to unknown, never that later record; a
no-answer discussion with only a System-recorded model must still surface it
(badged `modelRecorded`), since there is no answer's provenance to protect.
The launch-target fix is covered by switching a named connection's compare
target to a tier it never configured and asserting its own `default_model`
surfaces instead of the generic label.
[src: file: frontend/src/components/__tests__/BatchComparePanel.test.tsx:476-698]
[src: file: frontend/src/components/__tests__/BatchCompareDetailsPanel.test.tsx:1-370]
[src: file: frontend/src/pages/__tests__/WorkflowsPage.qp-launch.test.tsx:591-643]

These are component-level regressions against mocked APIs; no backend, worker,
or provider execution was exercised.

## Principal integration and validation working directory

The delivered HEAD `1667efc1` was reviewed independently (37 focused tests).
The protected merge candidate `d1e93929` included the principal's `d5a5c70d`
checkpoint. Its configured validation invoked Vitest from the repository root
with `--root frontend`; this did not change the process working directory for
tests reading local files. The persisted gate failed with 24 failed tests and
one failed suite, and the target branch did not advance.

The human approved `principal-takeover` for KT-628/630 in the durable question
`kt628-kt630-validation-cwd-recovery` at 2026-09-10T05:51:28.464773Z. Both
executions were cancelled with preservation at 05:53:50Z. Their commits,
worktrees, manifests, reviews and failed gate remain available; cancellation
did not report confirmed OS termination. No database row or validation result
was rewritten and no replacement worker was launched.
[src: user: 2026-09-10: kt628-kt630-validation-cwd-recovery]

The same candidate, frontend tree `355a5c5e1a665f3bd80ed825bcdb8b233fbf7a66`,
passed the full unfiltered replay from its `frontend` working directory:
313 files / 4,063 tests, 54.17 seconds (run 87630). Native and legacy TypeScript
builds, Oxlint, ESLint (0 errors / 63 existing warnings), i18n parity
(4,481 keys in each of four locales), and Vite (8.08 seconds) passed.
The principal then advanced both local branches by fast-forward to this
candidate. This is independently qualified principal integration, not a
successful automatic execution: its durable execution remains `Cancelled`.
No browser, coverage or provider qualification is implied.
[src: commit: d1e9392944173463966d738327523a5b6b39c348]
