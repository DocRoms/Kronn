# Copyable-ID tour anchor

The copyable id is **never** behind « Détails » (KT-1530; KT-1344 had moved it
into the side panel and the owner reverted that). It sits right after the title
on the discussion, automation (workflow) and Artifact headers, so people can
click it to mention the object to an agent.

- Discussion: `DetailHeaderShell` renders the `idPill` slot after the title;
  `ChatHeader` fills it with `DiscussionIdPill`, which carries the
  `data-tour-id="discussion-id-pill"` anchor.
  [src: file: frontend/src/components/ChatHeader.tsx:262] [src: file: frontend/src/components/DiscussionIdPill.tsx:18]
- Artifact: the same slot holds a `CopyIdPill`.
  [src: file: frontend/src/pages/PagesPage.tsx:1119]
- Workflow: `.wf-id-pill` next to the `<h2>` in `WorkflowDetail`.

The required `copyable-ids` guided-tour step therefore opens nothing: the anchor
is already on screen, and `revealCopyableIds` only brings the latest message id
into view for the second spotlight. Keep the step required — the tour provider
records a missing required selector as skipped, not as content that was shown.
[src: file: frontend/src/components/tour/tourSteps.ts:273-290#revealCopyableIds] [src: file: frontend/src/components/tour/tourSteps.ts:523-534#copyable-ids]

Do not move the id back into the « Détails » panel: `DiscussionDetailsSection`
and the Artifact « Détails » tab deliberately do not render it.

## Header button pair

Both detail headers share `DetailHeaderShell`: the page's primary action (green,
`disc-chat-header-btn--primary`: « Inviter » / « Synchroniser ») and « Détails »
(`PanelRightOpen` icon) sit side by side at the far right of the title row, same
size (`.disc-chat-header-btn`), `--kr-` tokens only. « Détails » opens the
page's vertical side panel; on an Artifact that panel has two tabs (Sync /
Details), there is no folded third row and no `localStorage` memory.
