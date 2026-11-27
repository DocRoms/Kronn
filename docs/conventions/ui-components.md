# Base UI components — `frontend/src/components/ui/`

KT-1880 (design-system lot 1). These components replace the parallel base
buttons, tab styles, menus and native `title=` tooltips. No screen is migrated
in KT-1880; the screens move to them in lots 2-5. Pairs with
[design-tokens.md](design-tokens.md) and [ui-text.md](ui-text.md).

## Where they live

One folder, one file per component, with its stylesheet and its test next to
it (`Button.tsx`, `Button.css`, `Button.test.tsx`). `stylesheets.test.ts`
fails if a component lacks one of the three.
`[src: file: frontend/src/components/ui/stylesheets.test.ts:132]`

| Component | Use | Keyboard |
|---|---|---|
| `Button` | `variant` primary / neutral / ghost / danger, `size` sm / md / lg, optional `icon`. `type="button"` by default. | native |
| `IconButton` | An icon only; `label` is required and is its name and its tooltip. | native; tooltip on focus |
| `Menu` | Menu button. `items`, plus at most one `destructiveItem`, always rendered last after a separator. | Enter / Space / ↓ open on the first item, ↑ on the last; arrows, Home, End, type-ahead; Escape and Tab close and give focus back |
| `StatusBadge` | `tone` success / warning / danger / info / neutral (the `--kr-status-*` colours on the dot, border and tint; the label stays `--kr-text-primary` for 4.5:1), the status always written. | — |
| `Tabs` | `tablist` / `tab` / `tabpanel` roles, `label` names the list; with no `panel` the tabs only report the choice. | ← → move and select, Home / End, a disabled tab is skipped |
| `SearchField` | `type="search"` with a required `label` (hidden unless `showLabel`) and a clear button. | Escape clears a non-empty field and stops there |
| `Tooltip` | Wraps one focusable element; `describe` wires it as `aria-describedby`. | opens on hover and keyboard focus, Escape closes, hoverable (WCAG 1.4.13) |
| `Dialog` | Modal layer, centred. `title` (its name, an `h2`), optional `description`, `footer` for the actions, `size` sm / md / lg (480 / 640 / 960 px), `role` dialog / alertdialog. | focus on the first control of the body (or `initialFocus`), Tab kept inside, Escape / the cross / the scrim close it, focus back to the opener |
| `Drawer` | The same props and keyboard as `Dialog`, as a side panel the full height of the window at its end edge. | as `Dialog` |
| `ConfirmDialog` | In place of `window.confirm`: an alertdialog, `title` is the question, `message` what happens, `confirmLabel` the verb and the object ("Delete 3 projects", never "OK"), `tone` danger (default) or primary. `useConfirm()` gives it as a promise. | focus on Cancel (Enter at once answers no); Escape, the cross and the scrim cancel |

`Menu` and `Tooltip` render into `document.body` at a fixed position
(`anchor.ts`), so a container with `overflow: hidden` does not clip them.
A floating layer never leaves the window: it is measured once rendered,
flips to the side with more room, is clamped `--kr-space-2` from the edges,
and a menu taller than the room scrolls inside a max-height
`[src: file: frontend/src/components/ui/anchor.ts:30#anchoredStyle]`.

## Dialog, Drawer and ConfirmDialog (KT-1881)

`Dialog` and `Drawer` are one frame, `OverlayFrame`, rendered in `<body>`:
a scrim, then a panel with `role`, `aria-modal="true"`, `aria-labelledby` on
its heading, `aria-describedby` on its description, and one cross (an
`IconButton` named "Close") at the top end, at the same size and place on
every dialog and drawer
`[src: file: frontend/src/components/ui/overlay.tsx:47#OverlayFrame]`.
Focus, Tab trap, Escape and focus return come from `useDialogFocus`: one
Escape closes one layer, the innermost first
`[src: file: frontend/src/hooks/useDialogFocus.ts:44#useDialogFocus]`.

- **Layers.** A `Drawer` sits on `--kr-z-drawer`, a `Dialog` on
  `--kr-z-modal`, a `ConfirmDialog` on `--kr-z-modal-nested`, so a
  confirmation opened from a drawer or a dialog is above it.
- **Sizes.** `--kr-overlay-w-sm / -md / -lg` (480 / 640 / 960 px, see
  [design-tokens.md](design-tokens.md)). Under the first breakpoint (639 px
  and less) both fill the screen. A component stylesheet writes no px length
  but a 1 or 2 px line and a `@media (max-width: <bp - 1>px)`.
- **A `ConfirmDialog` stays a card on a phone.** It is a short question, not
  content: under the first breakpoint it does not fill the screen but stays
  centred, as wide as the window minus `--kr-space-4` on each side, as tall
  as its content (never taller than the window minus that margin; a long
  message scrolls), the scrim visible around it. Its two buttons stack full
  width when they do not fit on one line, Cancel first
  `[src: file: frontend/src/components/ui/ConfirmDialog.css:22]`. The
  stylesheet test checks that it takes none of the full-screen rules of
  `Dialog`.
- **Tested once for both.** `describeOverlayContract` runs the same checks
  on `Dialog` and `Drawer` (role and name, initial focus, Tab, Escape, cross,
  scrim, nested layers, axe on the open layer)
  `[src: file: frontend/src/test/overlayContract.tsx:23#describeOverlayContract]`.
  `src/test/axe.ts` runs axe-core in vitest: it is the copy that
  `@axe-core/playwright` (already a devDependency) brings, so no new
  dependency.
- **A confirmation, not `window.confirm`.** Use `useConfirm()`:
  `if (!(await confirm({ title, message, confirmLabel }))) return;`, and
  render its `dialog` once in the component
  `[src: file: frontend/src/components/ui/useConfirm.tsx:27#useConfirm]`.
  `ConfirmDeleteButton` and `StrongConfirmDialog` stay as they are.
- **No new native `confirm()`.** ESLint refuses `confirm(…)` (scope-aware
  `no-restricted-globals`: a local function named `confirm` passes) and
  `window.` / `globalThis.` / `self.confirm(…)` (`no-restricted-properties`),
  in every file but the tests
  `[src: file: frontend/eslint.config.js:75]`. The calls that existed when the
  rule came in are its baseline, `frontend/eslint-suppressions.json` (ESLint
  bulk suppressions, read by `pnpm lint`): a call beyond the baseline fails,
  and once a call is migrated `pnpm lint` fails until its entry goes
  (`pnpm lint --prune-suppressions`). A vitest test checks that the baseline
  matches the calls each file still holds
  `[src: file: frontend/src/lib/__tests__/nativeConfirmLint.test.ts:57]`.
  First screens moved (KT-1456): the promotion of a shadow version
  (`LivePageResources`), the restore of a skill version
  (`SkillHistoryDrawer`), the deletion of a Quick Prompt version
  (`QPHistoryDrawer`).

## Rules

- **An IconButton cannot be nameless.** `label` is required, the empty
  literal `""` does not compile, and `aria-label`, `aria-labelledby` and
  `title` are refused (declared `never`: JSX does not check a hyphenated
  attribute against the props unless the prop is declared). The test file
  holds `@ts-expect-error` lines that `tsc -b` turns into errors if the type
  ever accepts them.
  `[src: file: frontend/src/components/ui/IconButton.tsx:15-18#IconButtonNativeProps]`
- **Tokens only, no theme named.** The stylesheets take colours, text sizes,
  spacing, radii, control heights, durations and z-index from tokens; the
  only px written is a 1 or 2 px line. `stylesheets.test.ts` reads every
  `components/ui/*.css` for it.
- **40 px on a coarse pointer.** Every interactive part grows to
  `--kr-control-h-touch` (40 px) under `@media (pointer: coarse)`; an
  IconButton in width too. The test reads the media blocks, which jsdom
  cannot evaluate.
- **Primary text colour.** An accent fill writes its text in
  `--kr-text-on-accent`, never `--kr-bg-base`: the primary `Button` and the
  shared `.btn-primary` are pinned by `tokens-defined.test.ts`.
- **Text inside a component** goes through `useT()` (`ui.*` keys); the
  caller passes every other label.

## The gallery

Settings → appearance → "Design system" (`/config#design-system`) shows each
component under its own heading, with its variants and states, after the
tokens, in the current theme. An index at the top lists the tokens, then one
entry per component. Adding a component: add it to `GALLERY_COMPONENTS`, a
demo in `DesignSystemComponents.tsx`, and its `designSystem.caption.<id>` in
the four locales; the gallery test fails if a component of the folder is
missing.
`[src: file: frontend/src/components/settings/designSystemGallery.ts:18#GALLERY_COMPONENTS]`
