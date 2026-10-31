# Copyable-ID tour disclosure

The discussion ID pill lives in the side panel's settings tab, not in the
header: `ChatHeader` only carries the single Details button that opens that
panel. [src: file: frontend/src/components/ChatHeader.tsx:153] [src: file: frontend/src/components/DiscussionDetailsSection.tsx:93-98#discussion-id-pill]

The required `copyable-ids` guided-tour step clicks that Details button when
the anchor is absent before it waits for the ID pill. Keep the
step required: the tour provider records a missing required selector as skipped,
not as content that was shown. [src: file: frontend/src/components/tour/tourSteps.ts:273-299#revealCopyableIds] [src: file: frontend/src/components/tour/tourSteps.ts:530-542#copyable-ids] [src: file: frontend/src/components/tour/TourProvider.tsx:178-210]
