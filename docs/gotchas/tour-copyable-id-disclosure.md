# Copyable-ID tour disclosure

On narrow layouts, `ChatHeader` renders the discussion ID pill in its collapsed
details region rather than beside the title. [src: file: frontend/src/components/ChatHeader.tsx:230-245] [src: file: frontend/src/components/ChatHeader.tsx:484-486]

The required `copyable-ids` guided-tour step opens the existing details
disclosure when that anchor is absent before it waits for the ID pill. Keep the
step required: the tour provider records a missing required selector as skipped,
not as content that was shown. [src: file: frontend/src/components/tour/tourSteps.ts:260-285] [src: file: frontend/src/components/tour/TourProvider.tsx:178-210]
