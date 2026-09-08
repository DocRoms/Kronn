# Arbitration form reset during discussion refresh

## Diagnosis (KT-622, September 8, 2026)

On the integrated source at `6503ecd14af145a5f668e1afd845b6f839abc7ef`,
an otherwise unchanged Markdown message remounts its pending arbitration form
when `sources` is replaced with a newly deserialized, equivalent array. The
textarea node is replaced, its draft and radio selection disappear, focus is
lost, and the answer button becomes disabled. This reproduces a mechanism
consistent with the human report; it is not a browser recording of that report.
`[src: commit: 6503ecd14af145a5f668e1afd845b6f839abc7ef]`

At that checkpoint, the discussion page fetched detail every five seconds and replaced its loaded
discussion object. `MessageBubble` passes the received lint report's `sources`
array into Markdown rendering. The old `MarkdownContent` rebuilt its citation
component table on array identity changes, then defined a new inline `pre`
component when that table changed. The arbitration card was below this
component, so changing its identity remounted the form instead of updating props.
The form's draft, selection and in-flight guard are local state/refs.
[src: commit: 6503ecd14af145a5f668e1afd845b6f839abc7ef]
`[src: file: frontend/src/pages/DiscussionsPage.tsx:893-894]`
`[src: file: frontend/src/pages/DiscussionsPage.tsx:1083-1095]`
`[src: file: frontend/src/components/MessageBubble.tsx:1054-1062]`
`[src: file: frontend/src/components/DiscussionQuestionCard.tsx:100-117]`

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
as historical diagnosis, not as a release regression test.

## Authorized correction

The human approved `fix-with-tests` on the durable question
`kt622-authorize-bounded-form-fix` at 16:20:21 UTC on September 8.
The correction keeps the paragraph, list-item and fenced-block component types
stable, and supplies fresh values through a per-message React context. Polling
is unchanged and citation verdicts still update. Question/planning-card keys
include the discussion, message and fence identity, so changing the question
does not transfer the previous draft.
`[src: file: frontend/src/components/MessageBubble.tsx:1693-1731]`
`[src: file: frontend/src/components/MessageBubble.tsx:1756-1797]`
`[src: file: frontend/src/components/MessageBubble.tsx:1830-1851]`

Seven product regressions cover question-store refresh, same-reference and
repeated equivalent sources, changing citation verdicts in paragraphs/lists,
streaming after a question, submission in flight and switching to a different
question. Four failed before the renderer correction; all seven pass after it.
The focused card/rendering/citation suite passed 123 tests. These are component
tests against production rendering with mocked API I/O, not browser end-to-end
evidence. KT-619's independent `MessageBubble` edits must retain this boundary.
`[src: file: frontend/src/components/__tests__/MarkdownContent.questionRefresh.test.tsx:1-143]`

The full frontend suite passed 3,977 tests across 305 files (208.26 seconds,
four workers). Both TypeScript checks, oxlint, i18n, CI ESLint (0 errors and
the unchanged 63 warnings) and the production build passed. No backend source
or polling interval changed, and no backend replay or new coverage report is
attributed to this frontend-only correction.
