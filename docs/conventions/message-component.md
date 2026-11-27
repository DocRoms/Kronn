# Discussion messages — one `Message` component (KT-1902)

Every message the discussion page shows renders through `Message`
(`frontend/src/components/Message.tsx`), in one of its variants: `user`,
`agent`, `system`, `error`, `orchestrator`, `streaming`, `queued`, `note`,
`deleted`, `media`. `[src: file: frontend/src/components/messageKinds.ts:4-14#MessageVariant]`
Do not write `disc-msg-row` / `disc-msg-bubble` markup by hand: add a variant
(or a `tone`, the finer `data-variant` the stylesheet reads) instead. A test
fails on hand-written bubble markup in the page and its message components.
`[src: file: frontend/src/components/__tests__/Message.test.tsx:392]`

## Anatomy

- **Header**: author, agent and model once, then the header controls and the
  time (`time` prop). `[src: file: frontend/src/components/Message.tsx:206-246#Message]`
- **Body** (`children`): the only part that folds. `lead` comes before it
  (in-reply-to, expanded summary), `trail` after it (attachments, action cards,
  calls to act). These two never fold, so a decision is never hidden.
- **Footer**: the facts on the left (tokens, duration, tier, access badges,
  lint pills, « Répondu par … » links), the actions on the right.
- **After**: what the footer opens (the citation report).

## Actions

One fixed order: Reply, Copy, listen (TTS), Edit or Retry, Delete. An action
that does not apply is left out, the others never move.
`[src: file: frontend/src/components/messageKinds.ts:21]`
Each is an `IconButton` named by its label. Under the first breakpoint
(640 px) they all go into one « … » `Menu` (keyboard reachable, Escape closes
it and focus returns to the trigger, Delete last). The page decides once with
`useIsMobile(BREAKPOINTS.sm)` and provides `MessageCompactContext`; a message
does not listen to the viewport itself.
`[src: file: frontend/src/pages/DiscussionsPage.tsx:324]`
`[src: file: frontend/src/components/messageKinds.ts:55]`

## Fold

A body taller than `MESSAGE_FOLD_THRESHOLD_PX` (rendered height, measured by
one shared `ResizeObserver`) folds behind a « Show more » button with
`aria-expanded`. `[src: file: frontend/src/components/messageKinds.ts:46]`
The unfolded ids live in `MessageFoldContext`, a `Set` the page renews per open
discussion: kept while the thread stays open, never stored.
`[src: file: frontend/src/pages/DiscussionsPage.tsx:336]`
A streaming reply has no fold; a message being edited, an orchestrator notice
or one carrying a `kronn-question` / `kronn-plan-action` / `kronn-important`
fence never folds; a search hit or an anchor jump (`isSearchCurrent`)
unfolds it for good.

## Code blocks

Fenced code renders through `CodeBlock`: language label when the fence names
one, a `CopyButton` whose name includes the language, horizontal scroll inside
the block, highlight.js classes coloured by the `--kr-code-*` tokens (KT-1878).
`[src: file: frontend/src/components/CodeBlock.tsx:10]`
Highlighting stops above `CODE_BLOCK_HIGHLIGHT_LIMIT` characters.
`[src: file: frontend/src/lib/diff-syntax.ts:175]`
