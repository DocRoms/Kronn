# Feedback channels — toast, bell, banner

KT-1883 (design-system lot 1). Three ways to tell the user something; each
message goes in exactly one. The components are in
[ui-components.md](ui-components.md) (`Notice`, `EmptyState`,
`LoadingState`).

## The rule

| Channel | What it carries | Lifetime | Where |
|---|---|---|---|
| **Toast** | The answer to a gesture the user **just made** — its success **and its error**. | A few seconds; an error stays until dismissed. | Bottom left of the window from 768 px up, under the app bar below (see [the toast stack](#the-toast-stack)). |
| **Bell** | What **lasts** or what happens **in the background**, without a gesture to answer: an outage, a server error, an update, a key or a token that needs attention, the end of a run the user is not watching. | Until its cause is gone or the user puts it away. | The notification bell of the header. |
| **Banner** (`Notice`) | A state of **the page it concerns**, true as long as the page shows it: read only, a setting that holds a feature, a load that failed. | As long as the state holds (dismissible if the user can live with it). | That page only, in its one notice slot under the page header. |

How to choose:

1. Did the user just click, type or drop something, and is this its result?
   → **toast**, whatever the outcome. An error the user can act on at once
   stays in the toast (persistent, with its copyable details), not in a
   banner elsewhere.
2. Otherwise, is it about the page on screen and only that page? → a
   **banner** (`Notice`) in that page.
3. Otherwise (it outlives the page, or nobody asked) → the **bell**.

Never two channels for one message: a toast does not repeat a banner, and a
bell entry does not toast — except the one short "see in the bell" pointer
below.

## One example per channel

- **Toast — a gesture and its two outcomes.** Copying a discussion id answers
  with a success toast, or an error toast when the clipboard refuses
  `[src: file: frontend/src/components/DiscussionIdPill.tsx:26-36]`.
- **Bell — something that lasts, found in the background.** A newer Kronn
  version is a bell notice of the `update` source, put away for that version
  only `[src: file: frontend/src/hooks/useSystemNotices.ts:81]`. Server errors
  (5xx) and an unreachable backend go to the bell too: a toast for them
  becomes a single "see in the bell" warning, at most once per 30 s
  `[src: file: frontend/src/hooks/useToast.tsx:66]`.
- **Banner — a state of this page.** While run payload retention is off,
  Settings and Workflows show a banner in the page, with a link to the
  setting `[src: file: frontend/src/components/settings/RunRetentionBanner.tsx:30#RunRetentionBanner]`.
  New banners are a `Notice` (`tone` info / warning / danger / success).

## The toast stack

The placement is in a stylesheet, `frontend/src/hooks/toastStack.css`, read
by `frontend/src/hooks/__tests__/toastStack.test.ts` (owner decision of
2026-11-21). The bell and the "Details" button of a detail header sit at the
top right, and a discussion's Send, Add a file and Quick Prompt at the bottom
right of the list column: no corner on the right is free.

- **From 768 px (`--kr-bp-md`) up: bottom left.** That is where the
  discussions list column appears (`useIsMobile()` is `BREAKPOINTS.md`):
  between 640 and 767 px the page is one column and a bottom-left toast would
  sit on the composer. 16 px from the left and the bottom, the stack growing
  upwards, newest at the bottom
  `[src: file: frontend/src/hooks/toastStack.css:14]`. A toast is at most
  320 px wide there, so it stays inside the list column (358 px wide at
  1440 px; the composer's first button starts at x 386)
  `[src: file: frontend/src/hooks/toastStack.css:33]`.
- **Below 768 px: at the top, under the app bar.** Every corner covers
  something there, so the stack sits just under the top bar, full width less
  the 16 px margins, and never over the bar where the bell is
  `[src: file: frontend/src/hooks/toastStack.css:49]`. The bar's height is
  not a token (it follows the text size and the touch controls): Dashboard
  measures it and publishes it as `--app-bar-h`
  `[src: file: frontend/src/pages/Dashboard.tsx:63]`, and so does the
  standalone discussion mosaic for its own toolbar, since it renders its own
  stack outside Dashboard. The Plugins page's stack is a Dashboard route, under
  Dashboard's bar.
- **Never taller than the window.** Persistent toasts are not capped in
  number and an error with a copyable block is up to 240 px high, so the
  stack is capped to the window less its margins (and less the app bar below
  768 px) and scrolls; each new toast is scrolled into view, and only the
  toasts take clicks, not the gaps between them
  `[src: file: frontend/src/hooks/ToastStack.tsx:10#ToastStack]`.
- **A persistent toast is never evicted.** At most three transient toasts
  are on screen, the oldest goes first; a persistent one (an error, by
  default) stays until the user dismisses it, whatever comes after it
  `[src: file: frontend/src/hooks/useToast.tsx:42#withToast]`.

## Notice: one slot per page

A page has one place for its notices: under its header, above its content.
Several states at once are stacked there, the most severe first; a page does
not scatter banners through its sections. A `Notice` is an alert (read at
once) only in the danger tone; the others are a polite status, so a page that
opens with an information banner does not interrupt a screen reader.
