# Pull requests that change the UI

The owner's rule (2026-10-31, restated 2026-11-11): a Kronn pull request, or a
comment answering a review on one, that changes the UI carries **before / after
captures**, and the captures are **retaken after every push that changes an
already captured screen**. A reviewer should see the change, not reconstruct it
from the diff.

The rule applies to everyone who opens or updates a PR: contributors, workers
and agents. The PR template has the matching required section
([`.github/pull_request_template.md`](../../.github/pull_request_template.md)).

## When captures are mandatory

Any diff under `frontend/` that changes **what renders**:

- components and pages (`frontend/src/components/`, `frontend/src/pages/`);
- CSS and theme tokens;
- i18n strings that are shown on screen.

Not a UI change: tests only, types only (`generated.ts`), a refactor with no
visible difference, backend-only work. Write `N/A — no UI change` in the PR's
Screenshots section instead of leaving it empty.

If the UI change cannot be captured (a state you cannot reach locally, a screen
behind a paid provider), say which screen and why in the same section. "No
captures" without a reason is not acceptable.

## What to capture

For **each changed screen**:

| Axis | Rule |
|------|------|
| Before | the target branch (`main`), the tree the PR started from |
| After | the PR head, the exact commit the PR currently points at |
| Themes | light **and** dark (`data-theme` set through the `kronn:theme` local-storage key, see `frontend/src/lib/ThemeContext.tsx`) |
| Width | desktop (1440 px wide) always; a mobile width (390 px) too when the change is responsive |
| New screen | no "before": capture "after" only, and say it is new |

Capture the state that shows the change (open the panel, trigger the error, fill
the form), not just the page at rest. Use demo data
([`docs/operations/screenshot-sandbox.md`](../operations/screenshot-sandbox.md)),
never real project names, tickets or secrets.

## Where to publish

Captures live on the `pr-screenshots` branch, which holds images only and is
never merged: one folder per PR, one sub-folder per version when the captures
are retaken (as done on #419 and #438):

```
<pr-number>/<screen>-before-light.png
<pr-number>/<screen>-before-dark.png
<pr-number>/<screen>-after-light.png
<pr-number>/<screen>-after-dark.png
<pr-number>/v2/<screen>-after-light.png      # retaken after a later push
```

Push them there, then embed them in the PR with the raw GitHub URLs of that
branch (`https://raw.githubusercontent.com/<owner>/<repo>/pr-screenshots/<pr-number>/<file>`)
in the Screenshots section. A before/after table, one row per screen and theme,
reads best. Never commit captures to the PR branch itself.

## Retake after each push that changes a captured screen

When a push (review fix, rebase, new commit) changes a screen that already has
captures, the old "after" is stale:

1. retake the affected "after" captures from the new head into the next `vN/`
   folder (the "before" stays valid while `main` has not changed that screen;
   retake it too if it has);
2. post **one comment** on the PR that names the **new head sha** and lists the
   refreshed screens with their images.

A push that changes no captured screen needs nothing. The same applies to a
comment answering a review: if the answer is a UI change, it carries the
captures; if it only explains, it does not need any.

## How to capture locally

Run the PR head and `main` side by side, each on its own Vite port, against a
sandbox backend:

1. start a sandbox backend (`./scripts/seed-demo-fixtures.sh`, port `3145`);
2. check `main` out in a second worktree, and start Vite in each tree on
   another port than the host's `5173`, pointing at that backend:

   ```bash
   cd frontend
   KRONN_VITE_BACKEND_URL=http://127.0.0.1:3145 VITE_DEV_PORT=5174 \
     node ./node_modules/vite/bin/vite.js
   ```

   (`frontend/vite.config.ts` reads `KRONN_VITE_BACKEND_URL` and
   `VITE_DEV_PORT`; the second tree takes `5175`);
3. drive each Vite with the repo's Playwright (`@playwright/test`, see
   [`frontend/e2e/README.md`](../../frontend/e2e/README.md)): a throwaway script
   that sets `localStorage['kronn:theme']` to `light` then `dark`, sets the
   viewport, reaches the state and calls `page.screenshot()`. Playwright's
   browser is the one `pnpm test:e2e` installs.

Never listen on `3140` or `5173`: that is the maintainer's running Kronn.

## Automation

KT-1493 automates these captures in CI. Until it lands, they are taken by hand
or by the agent as above; once it does, the automation fulfils this rule and the
published layout stays the same.

## Agents

Kronn briefs carry this rule:

- the worker brief (`worker_brief_markdown_in`, `backend/src/api/orchestration.rs`;
  text in `backend/src/core/ui_captures.rs`) asks a worker that changes
  `frontend/` to take the captures or state why none;
- the review hand-back (`request_changes`, same text file) asks for the
  captures to be retaken when the fix changes a captured screen;
- the autoCode Issue publish step opens the PR with the Screenshots section
  (`backend/src/workflows/autocode_issue/publish.py`).
