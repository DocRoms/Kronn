# Arbitration form reset during discussion refresh

## Diagnosis (KT-622, September 8, 2026)

On the integrated source at `6503ecd14af145a5f668e1afd845b6f839abc7ef`,
an otherwise unchanged Markdown message remounts its pending arbitration form
when `sources` is replaced with a newly deserialized, equivalent array. The
textarea node is replaced, its draft and radio selection disappear, focus is
lost, and the answer button becomes disabled. This reproduces a mechanism
consistent with the human report; it is not a browser recording of that report.
`[src: commit: 6503ecd14af145a5f668e1afd845b6f839abc7ef]`

The discussion page fetches detail every five seconds and replaces its loaded
discussion object. `MessageBubble` passes the received lint report's `sources`
array into Markdown rendering. `MarkdownContent` rebuilds its citation component
table on array identity changes, then defines a new inline `pre` component when
that table changes. The arbitration card is below this component, so changing
its component identity remounts the form rather than merely updating its props.
The form's draft, selection and in-flight guard are local state/refs.
`[src: file: frontend/src/pages/DiscussionsPage.tsx:893-894]`
`[src: file: frontend/src/pages/DiscussionsPage.tsx:1083-1095]`
`[src: file: frontend/src/components/MessageBubble.tsx:1054-1062]`
`[src: file: frontend/src/components/MessageBubble.tsx:1737-1754]`
`[src: file: frontend/src/components/MessageBubble.tsx:1797-1817]`
`[src: file: frontend/src/components/MessageBubble.tsx:1848]`
`[src: file: frontend/src/components/DiscussionQuestionCard.tsx:103-117]`

## Deterministic local evidence

Three diagnostic cases ran against the unchanged production components, with
only the questions API and translation hook mocked (Vitest/happy-dom):

1. Refresh the questions store with a fresh but equivalent durable question:
   the input node, draft, selection and focus remain intact.
2. Rerender the same Markdown with the same `sources` reference:
   the input node, draft and focus remain intact.
3. Rerender identical Markdown with another empty `sources` array:
   the old input disconnects, a new empty input appears, selection/focus are
   lost and the answer button is disabled.

All three diagnostic assertions passed. The third asserts the observed defect,
not desired behavior; it must not be represented as a successful product
regression gate. No backend mutation, real answer submission or provider call
was used. The local diagnostic harness is retained outside the release gates
until a correction is authorized.

## Proposed bounded correction (not yet implemented)

Keep Markdown renderer component identities stable while supplying fresh
discussion/message/fence/citation values through ordinary props or context.
Do not disable polling or hide genuinely updated citation verdicts. Validate
draft/selection/focus and the in-flight submission guard across equivalent and
changed reports, plus streamed content updates and other interactive fences.
Coordinate `MessageBubble` integration with KT-619's separate worktree. The
diagnostic alone does not authorize modifying the production renderer.
