# Activity navigation: the permanent bell

## Stable navigation and live counts

Projects and Automation always navigate on the first activation, even while
audits or workflows run. Their counts remain passive badges. Since KT-1658 there
is ONE activity entry, a labelled bell outside the scrolling tabs, that discloses
the non-modal activity panel (it replaced the per-kind spinners and their two
popovers, so there is no longer a second disclosure to keep exclusive). Tour
anchors stay on the navigation tabs. The bell sets `aria-controls` only while its
dialog exists. See [ui-structure.md](../architecture/ui-structure.md) § Attention
center for what the panel lists.
[src: file: frontend/src/pages/Dashboard.tsx:722-736#data-tour-id]
[src: file: frontend/src/pages/Dashboard.tsx:787-801#ActivityBell]
[src: file: frontend/src/components/activity/ActivityBell.tsx:29-33#ActivityBell]

## Overlay and focus ownership

Rendering inside the sticky navigation clipped the tray at 390/1100px and let the
mobile collection sidebar cover its controls. The panel therefore uses a body
portal and viewport-bounded fixed positioning, updated on scroll and resize.
Coarse-pointer controls have a 44px minimum target.
[src: file: frontend/src/components/activity/ActivityPanel.tsx:102-110#createPortal]
[src: file: frontend/src/components/activity/ActivityPanel.css:264-270#coarse]
[src: file: frontend/src/hooks/useActivityPopover.ts:37-47#position]
[src: file: frontend/src/hooks/useActivityPopover.ts:66-67#position]

Opening focuses the dialog. Escape belongs to it only while focus is in the
dialog or its trigger; capture prevents the underlying collection from also
closing. Close/Escape restores the connected trigger (the bell is permanent, so
the optional fallback target is only for a caller whose trigger can disappear).
Outside pointer input closes without stealing focus. Row navigation closes the
panel. If a live refresh removes the focused row or action button, a scoped
mutation observer restores dialog focus only when it would otherwise be lost to
the document body; intentional outside focus is preserved.
[src: file: frontend/src/hooks/useActivityPopover.ts:64-65#focus]
[src: file: frontend/src/hooks/useActivityPopover.ts:14-21#closeAndRestoreFocus]
[src: file: frontend/src/hooks/useActivityPopover.ts:30-36#contentObserver]
[src: file: frontend/src/hooks/useActivityPopover.ts:48-53#onPointerDown]
[src: file: frontend/src/hooks/useActivityPopover.ts:54-61#onKeyDown]

Actions use a synchronous per-action ref guard as well as the disabled UI.
Failures show a localized alert on the row, permit an explicit retry and refresh
the parent's snapshot after settlement; they never navigate from the action button.
[src: file: frontend/src/components/activity/ActivityPanel.tsx:66-100#perform]

## Regression evidence

Shared interaction tests cover Escape isolation, outside-pointer dismissal without
retained listeners, Escape elsewhere, and viewport bounding after resize and scroll.
The panel tests keep exact action arguments, synchronous duplicate prevention,
visible errors with retry and the empty state; the Dashboard tests keep the
single-entry header and the always-navigating tabs.
[src: file: frontend/src/components/__tests__/ActivityPopover.interactions.test.tsx:16-80#interactions]
[src: file: frontend/src/components/activity/__tests__/ActivityPanel.test.tsx:174-198#double]
[src: file: frontend/src/pages/__tests__/Dashboard.navigation.test.tsx:100-152#bell]
[src: file: frontend/src/pages/__tests__/Dashboard.navigation.test.tsx:215-240#navigates]

The 0.13.0 Chromium qualification (390px touch and 1100px: containment, pointer
actionability, 44px targets, Escape isolation, cancellation error/retry, axe on the
navigation and open dialogs) ran against the two popovers this bell replaced. It is
NOT a qualification of the bell: re-run it before relying on those claims. It never
qualified a real backend cancellation or a provider. See the
[release evidence ledger](../releases/0.13.0-checklist.md).
