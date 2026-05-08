# TD-20260510 — A11y form-label associations

- **ID**: TD-20260510-a11y-form-labels
- **Area**: Frontend / accessibility
- **Severity**: Low (degraded screen-reader UX, no functional regression)
- **Status**: 🟡 Partial — audit started 2026-05-10, ~6 inputs labelled

## Problem (fact)

Many `<input>` / `<select>` / `<textarea>` elements in
`SettingsPage.tsx` and other forms have a visually-adjacent
`<label class="set-form-label">…</label>` that **isn't formally
associated** via `htmlFor` / `id` or by wrapping. Screen readers
rely on proximity heuristics that work most of the time but skip
when the visual label sits in a sibling div instead of a parent
`<label>`. ~22 inputs in SettingsPage alone.

Some inputs also lack `aria-label` outright — the operator must
visually scan for the placeholder text to know what the field is.

## Why we didn't fix-all in one pass

Each input needs either:

1. **Wrap fix**: `<label className="set-form-label">…<input/></label>`
   — semantically correct but may need a CSS tweak depending on
   flexbox layout.
2. **htmlFor pair**: add `id="…"` to the input and `htmlFor="…"` to
   the label.
3. **aria-label fallback**: add `aria-label` matching the visible
   label.

Each input is a 1-3 line edit but careful — wrong approach can
break label-row alignment or click-to-focus behaviour. Risk of
visual regressions if done in bulk without manual review per form.

## What's done so far

- Sidebar search clear button → `aria-label`
- Dashboard nav tabs → `aria-current="page"` for active, `aria-label`
  for icon-only mobile mode
- Settings password input (`secretCode`) → `aria-label`
- **2026-05-11 sweep**: 12 more inputs labelled — SettingsPage ×7
  (scan-path input, scan-ignore input, skills form name+category+desc+icon+content,
  directives form name+category+desc+icon+conflicts+content, domain
  input), NewDiscussionForm ×4 (title, prompt textarea, branch-name,
  base-branch, file-attach), WorkflowWizard ×4 (workflow name, project
  picker, agent picker, prompt textarea — Step 0 + 1, the wizard's
  most-trafficked surface). 1660 i18n keys triple-localised.

## Plan

By directory, in priority (operator-facing forms first):

1. **SettingsPage.tsx** — skill creation form (~3 inputs), directive
   creation form (~3 inputs), profile creation (~3 inputs), API key
   input, scan paths textarea. ~12 high-value inputs.
2. **NewDiscussionForm.tsx** — title, agent picker, project picker.
   ~3 inputs.
3. **WorkflowWizard.tsx** — name + steps. Tour-driven onboarding
   surface, high impact.

Per-input pattern: prefer `<label htmlFor=...>` association for
formally-labeled fields. Use `aria-label` only when the visible
label is genuinely absent (icon buttons, search inputs).

## Test surface

- Manual: VoiceOver / NVDA / Orca walkthrough of Settings → Custom
  Skills → "Create skill" flow. Each input should be announced with
  its purpose, not just "edit text" / "combobox".
- Automated: install `eslint-plugin-jsx-a11y` and run as warnings.
  ~30 hits expected on first run.

## Pointers

- `frontend/src/pages/SettingsPage.tsx:355,539,582,645,893,910,914,1123,1139,1144` — known
  unlabelled inputs (grep `<input.*set-input` to refresh).
- `frontend/src/components/Sidebar.tsx` (DiscussionSidebar) — already
  has aria-label on icon-only buttons (post-2026-05-10 audit).
- `frontend/src/pages/Dashboard.tsx:486-520` — nav tabs with
  `aria-current` (post-2026-05-10).

## Next step

Pick one form per session, fix all its inputs together with a manual
voiceover check. Don't bulk-rewrite without reviewing each form's
visual layout.
