# Activity navigation disclosures

## Stable navigation and live counts

Projects and Automation always navigate on the first activation, even while
audits or workflows run. Counts remain passive badges. Separate labelled
activity buttons outside the scrolling tabs disclose the matching non-modal
dialog; opening one closes the other. Tour anchors stay on the navigation tabs.
The disclosure sets `aria-controls` only while its dialog exists and remains
available when polling reaches zero until the user closes or navigates.
[src: file: frontend/src/pages/Dashboard.tsx:725-778#data-tour-id]
[src: file: frontend/src/pages/Dashboard.tsx:784-867#activeAuditsPopoverOpen]

## Overlay and focus ownership

Rendering inside the sticky navigation clipped the tray at 390/1100px and let
the mobile collection sidebar cover its controls. Both popovers now use a body
portal and the same viewport-bounded fixed positioning, updated on scroll and
resize. Coarse-pointer controls have a 44px minimum target.
[src: file: frontend/src/components/ActiveAuditsPopover.tsx:107-116#createPortal]
[src: file: frontend/src/components/workflows/ActiveRunsPopover.tsx:82-90#createPortal]
[src: file: frontend/src/components/workflows/ActiveRunsPopover.css:9-21#wf-active-runs-popover]
[src: file: frontend/src/components/workflows/ActiveRunsPopover.css:179-191#coarse]
[src: file: frontend/src/hooks/useActivityPopover.ts:37-47#position]
[src: file: frontend/src/hooks/useActivityPopover.ts:62-69#position]

Opening focuses the dialog. Escape belongs to it only while focus is in the
dialog or its trigger; capture prevents the underlying collection from also
closing. Close/Escape restores the connected trigger, falling back to its
surviving navigation tab when a zero-count close removes the trigger. Outside
pointer input closes without stealing focus. Row/footer navigation closes and
focuses the destination tab. If a live refresh removes the focused row or Stop
button, a scoped mutation observer restores dialog focus only when it would
otherwise be lost to the document body; intentional outside focus is preserved.
[src: file: frontend/src/hooks/useActivityPopover.ts:62-65#focus]
[src: file: frontend/src/hooks/useActivityPopover.ts:14-21#closeAndRestoreFocus]
[src: file: frontend/src/hooks/useActivityPopover.ts:26-36#contentObserver]
[src: file: frontend/src/hooks/useActivityPopover.ts:48-53#onPointerDown]
[src: file: frontend/src/hooks/useActivityPopover.ts:54-69#onKeyDown]
[src: file: frontend/src/pages/Dashboard.tsx:810-819#projectsTabRef]
[src: file: frontend/src/pages/Dashboard.tsx:852-861#workflowsTabRef]

Cancellation uses a synchronous per-ID ref guard as well as the disabled UI.
Failures show a localized alert, permit an explicit retry and refresh the
parent's snapshot after settlement; they never navigate from the Stop button.
[src: file: frontend/src/components/ActiveAuditsPopover.tsx:83-105#handleCancel]
[src: file: frontend/src/components/ActiveAuditsPopover.tsx:161-173#stopPropagation]
[src: file: frontend/src/components/ActiveAuditsPopover.tsx:180#cancelError]
[src: file: frontend/src/components/workflows/ActiveRunsPopover.tsx:58-80#handleCancel]
[src: file: frontend/src/components/workflows/ActiveRunsPopover.tsx:142#stopPropagation]
[src: file: frontend/src/components/workflows/ActiveRunsPopover.tsx:157#cancelError]

## Regression evidence

The initial navigation regression failed before the recovered changes. Shared
interaction tests then failed eight cases before the portal/focus hookup;
two further regressions reproduced losing focus when the last row disappeared.
All pass after correction. Tests retain exact cancellation IDs, synchronous
duplicate prevention, visible errors/retry and empty-list behavior.
[src: file: frontend/src/pages/__tests__/Dashboard.navigation.test.tsx:100-141#navigates]
[src: file: frontend/src/components/__tests__/ActivityPopover.interactions.test.tsx:15-89#interactions]
[src: file: frontend/src/components/__tests__/ActiveAuditsPopover.test.tsx:144-174#guards]
[src: file: frontend/src/components/workflows/__tests__/ActiveRunsPopover.test.tsx:85-106#lists]
[src: file: frontend/src/components/workflows/__tests__/ActiveRunsPopover.test.tsx:147-173#falls]

Principal Chromium qualification mounts the real Dashboard and lazy pages with
strictly simulated API/WebSocket traffic. At 390px touch and 1100px it covers
containment, pointer actionability, 44px touch targets, Escape isolation,
exclusivity, rows/footers, cancellation error/retry, zero-count focus fallback
and zero serious/critical axe findings on navigation and the open dialogs.
It does not qualify a real backend cancellation, provider, or the future
attention center. See the [release evidence ledger](../releases/0.13.0-checklist.md).
