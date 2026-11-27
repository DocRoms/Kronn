# Base UI components — `frontend/src/components/ui/`

KT-1880 (design-system lot 1). These components replace the parallel base
buttons, tab styles, menus and native `title=` tooltips. No screen is migrated
in KT-1880; the screens move to them in lots 2-5. KT-1882 adds the form
components (`Field`, `Select`, `Switch`, `FormFooter`); lot 5 migrates the
forms to them. KT-1883 adds the feedback components (`Notice`,
`EmptyState`, `LoadingState` with its `Skeleton`). Pairs with
[design-tokens.md](design-tokens.md) and [ui-text.md](ui-text.md).

## Where they live

One folder, one file per component, with its stylesheet and its test next to
it (`Button.tsx`, `Button.css`, `Button.test.tsx`). `stylesheets.test.ts`
fails if a component lacks one of the three.
`[src: file: frontend/src/components/ui/stylesheets.test.ts:180]`

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
| `Field` | One form field: `label` above the control and bound to it, `required` (an asterisk) or `optional` (the word), `help` and `error` lines under the control, `disabled`. Wraps one control element, or a render function `control => …` for a control it cannot clone. | the control's own |
| `Select` | The native `<select>`, one look: `options` (an entry or a group), `value`, `onChange(value)`, `placeholder`, `size` sm / md. No label of its own: a `Field` names it, or an `aria-label`. | native |
| `Switch` | An on/off setting that applies at once: a checkbox with `role="switch"`, named by its visible `label` (`hideLabel` keeps it for assistive technology only), `description` under it. | Space; a click on the label |
| `FormFooter` | The end of a form: `blockers` (what still holds the action), then Cancel (`onCancel`), then the action (`submitLabel`, `tone` primary / danger, `busy`), on the right, `pinned` to the bottom of what scrolls (default). Without `onSubmit` the action submits the form around it. | a held action stays focusable |
| `Notice` | A message inside the page it concerns (see [feedback-channels.md](feedback-channels.md)): `tone` info / warning / danger / success, optional `title`, one `action` (`{ label, onClick }`), `onDismiss` adds a cross named "Dismiss". Tone on the icon, the inline-start border and the tint; text in `--kr-text-primary`. `role="alert"` for danger only, `status` otherwise. | native |
| `EmptyState` | `kind` firstUse (its `action`, the primary button, is required), noResult (`onClear`, required, "Clear the filters" unless `clearLabel`), nothingSelected (no action); `title`, `description`, `icon`. The type refuses a case without its way out. | native |
| `LoadingState` | `size` sm (a ring beside the text, inline) / md (the mark over its sentence, a panel; default) / lg (a whole page); `fullscreen` (the window, lg), `phase`, `onRetry`. md and lg keep the cold-start screen of `index.html` instead of drawing a second one, unless `holdsBootScreen={false}` (a loader shown as content); the hold follows the current props, so a loader that turns sm or stops holding releases that screen and draws itself. Same file: `Skeleton`, `kind` list (`rows`) / detail, one named status (not marked busy, so its label is read), shapes hidden from assistive technology. | — |

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

## Form components (KT-1882)

- **A label is always bound.** `Field` gives its control an id (it keeps one
  the control already has) and renders `<label for>` on it, above it; a
  native `input`, `textarea` or `select` also takes the field style
  (`ui-field-control`), a component keeps its own
  `[src: file: frontend/src/components/ui/Field.tsx:39]`.
- **The asterisk means required, nothing else.** `required` prints it
  (hidden from assistive technology, the control gets `aria-required`);
  `optional` writes the word; the props are exclusive at the type level. A
  text label ending in `*` loses it: an asterisk for "advanced options are
  set" (`WorkflowWizard.tsx`) cannot pass through `Field`
  `[src: file: frontend/src/components/ui/Field.tsx:43#plainLabel]`.
- **The error is announced.** The error line sits in a polite live region
  rendered from the start; while there is an error the control is
  `aria-invalid` and its description reads the error first, then the help
  `[src: file: frontend/src/components/ui/Field.tsx:100]`. The error text is
  `--kr-text-primary` (4.5:1 in every theme), its icon carries the danger
  colour.
- **16 px on a coarse pointer.** A phone zooms into a field written under
  16 px when it takes the focus: `ui-field-control` and `Select` write
  `--kr-type-xl` (1rem) under `@media (pointer: coarse)`, and grow to
  `--kr-control-h-touch`
  `[src: file: frontend/src/components/ui/Field.css:106]`.
- **A held action says why.** `FormFooter` holds its action with
  `aria-disabled` (not `disabled`) while there are `blockers` or it is
  `busy`: the keyboard still reaches it, its description is the list of
  reasons, a click or Enter does nothing (and does not submit the form)
  `[src: file: frontend/src/components/ui/FormFooter.tsx:32#FormFooter]`.
- **One switch.** `Switch` is the native checkbox with the `switch` role,
  transparent over its track; the focus ring is drawn on the track. Its off
  thumb and border (`--kr-text-muted`), its on track (`--kr-accent`) and on
  thumb (`--kr-text-on-accent`) hold 3:1 in every theme, which the test
  measures.
- **Axe.** Each of the four has an axe run in its test, in every state, and
  the gallery test runs axe on their four sections.
- **`.ui-visually-hidden`** now lives in `visuallyHidden.css`, imported by
  each component that uses it (it was in `SearchField.css`, which `Switch`
  and `FormFooter` would otherwise depend on).

## Feedback components (KT-1883)

- **Which channel.** A toast answers a gesture just made, errors included;
  the bell holds what lasts or happens in the background; a banner (a
  `Notice`) stays inside the page it concerns, in its one slot under the
  page header. The rule and one example per channel:
  [feedback-channels.md](feedback-channels.md).
- **Notice reads at 4.5:1.** As `StatusBadge`, the tone colour never writes
  the text: the test composites the tint on every surface of the seven
  themes and measures the text on it
  `[src: file: frontend/src/components/ui/Notice.test.tsx:49]`.
- **An empty state always has its way out.** `EmptyState` is a union on
  `kind`: a first use without `action`, a no-result without `onClear`, or a
  nothing-selected with an action do not compile (`@ts-expect-error` lines
  in its test) `[src: file: frontend/src/components/ui/EmptyState.tsx:44#EmptyStateProps]`.
- **One LoadingState.** `components/LoadingState.tsx` re-exports
  `ui/LoadingState` (and `BootHold`) for its ten users; the sizes replace the
  14 sizes of `<Loader2>` as screens move (lots 2-5). Every animation stops
  under `prefers-reduced-motion`, which the test reads from the stylesheet
  `[src: file: frontend/src/components/ui/LoadingState.tsx:43#LoadingState]`.
  A loader shown as content, never waiting for anything (the gallery's
  demos), passes `holdsBootScreen={false}`: mounted for good, md and lg would
  otherwise hold the cold-start screen and leave Settings behind it when it
  is the first page opened
  `[src: file: frontend/src/components/settings/DesignSystemComponents.tsx:479]`.
- **Variants** live in `feedbackKinds.ts` (`NOTICE_TONES`,
  `EMPTY_STATE_KINDS`, `LOADING_SIZES`, `SKELETON_KINDS`), so the component
  files export components only (fast refresh).

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
