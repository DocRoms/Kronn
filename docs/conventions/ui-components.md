# Base UI components — `frontend/src/components/ui/`

KT-1880 (design-system lot 1). These components replace the parallel base
buttons, tab styles, menus and native `title=` tooltips. No screen is migrated
in KT-1880; the screens move to them in lots 2-5. KT-1882 adds the form
components (`Field`, `Select`, `Switch`, `FormFooter`); lot 5 migrates the
forms to them. KT-1883 adds the feedback components (`Notice`,
`EmptyState`, `LoadingState` with its `Skeleton`). KT-1884 adds the six
header kinds (`ListHeader`, `DetailHeader`, `TabHeader`, `SectionHeader`,
`DrawerHeader`, `CardHeader`), see [header-kinds.md](header-kinds.md). KT-1886
adds the dense-data components (`CopyButton`, `Table`, `KeyValueList`,
`StatTile` / `StatGrid`, `ProgressBar`); lots 2-5 migrate the existing tables,
lists, tiles, bars and copy actions to them. Pairs with
[design-tokens.md](design-tokens.md) and [ui-text.md](ui-text.md).

## Where they live

One folder, one file per component, with its stylesheet and its test next to
it (`Button.tsx`, `Button.css`, `Button.test.tsx`). `stylesheets.test.ts`
fails if a component lacks one of the three.
`[src: file: frontend/src/components/ui/stylesheets.test.ts:183]`

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
| `CopyButton` | `text` or `getText` (read at the click), required `label`; `form` icon (an `IconButton`) / text (a `Button`). A copy shows a check for 1.5 s and says "Copied" in a polite live region; a refused clipboard turns danger and says "Copy failed". | native |
| `Table` | Required `caption` (`hideCaption`); `columns` (`header`, `align`, `numeric`, `sortable`, `truncate` ellipsis / wrap, `rowHeader`); controlled `sort` / `onSortChange`; `stickyHeader`, `maxHeight` sm / md / lg, `density` compact / normal, `rowTone`, `hover`, `selection`, `actions`, `onRowOpen`, `empty`, `loading`, `footer`, `narrow` scroll / cards. | sort and open buttons, checkboxes; the bounded area is a Tab stop |
| `KeyValueList` | A `dl`; `items` (`label`, `value`, `mono`, `copyable`, `copyText`, `truncate`), `layout` grid / inline. | the copy buttons |
| `StatTile`, `StatGrid` | `label`, `value` (null: "—" and `unknownReason`), `unit`, `icon`, `hint`, `delta` (`text`, `direction`, `tone`), `loading`, `onClick` (a button) with `selected` (`aria-pressed`), `size` md / lg. `StatGrid` (`label` names the group) wraps its tiles by its own width. | native, when a button |
| `ProgressBar` | Required `label` (`hideLabel`); `value`, `max`, `valueText`; no value is indeterminate; `tone` accent / success / warning / danger / info; `size` sm / md. Stacked: `segments` (`id`, `label`, `value`, `tone`), `max`, `legend`. | — |
| `AgentIdentity` | An agent named the same way everywhere (KT-1936): `name` in its `color` (`agentTextColor`), `model` (a concrete id or an unknown state, always written), `role` as a neutral `StatusBadge`, optional `tier` icon, `size` full / inline / compact, `truncateModel` (17 characters). See below. | none: inline markup, it sits inside the caller's button |
| `ListHeader`, `DetailHeader`, `TabHeader`, `SectionHeader`, `DrawerHeader`, `CardHeader` | One header per kind, one heading each at a fixed level (h1; h2 for detail, drawer, card; h3 for tab, section), slots `leading` / `title` / `meta` / `status` / `description` / `actions`, `compact`; `DrawerHeader` adds `onClose` + `closeLabel` (the last control), `titleId` names a drawer or a card. Which goes where: [header-kinds.md](header-kinds.md). | native |

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
  `[src: file: frontend/src/components/settings/DesignSystemComponents.tsx:499]`.
- **Variants** live in `feedbackKinds.ts` (`NOTICE_TONES`,
  `EMPTY_STATE_KINDS`, `LOADING_SIZES`, `SKELETON_KINDS`), so the component
  files export components only (fast refresh).

## Header kinds (KT-1884)

- **One frame.** The six share `headerFrame.tsx` and `headerFrame.css`; each
  kind's own stylesheet sets only its title sizes (from the text scale,
  smaller when `compact`) and its padding. `describeHeaderContract` runs the
  same checks on each (one heading at its level in both forms, slot order,
  sizes, no `position` or `z-index`, axe)
  `[src: file: frontend/src/test/headerContract.tsx:32#describeHeaderContract]`.
- **Variants** live in `headerKinds.ts` (`HEADER_LEVELS`, `HEADER_KINDS`).
- **In the gallery** each kind shows its normal and compact form (the compact
  one with a long title, to show it ellipsize). A demo renders its real
  heading, so the gallery's own `h3` after an `h1` or `h2` demo is the one
  `heading-order` jump its axe test allows.

## Dense-data components (KT-1886, KT-2004)

- **Narrow by the container, not the window.** `Table` and `KeyValueList`
  are size containers (`container-type: inline-size`) and take their narrow
  form under a width of their own: `@container ui-table (width < 40rem)`
  (40rem is `--kr-bp-sm`), `@container ui-kv (width < 24rem)`. A table in a
  side panel turns into cards on a wide screen too; no `@media` width query.
  A container query cannot read a custom property, so the threshold is
  written in rem, the one length besides a 1 or 2 px line.
  `[src: file: frontend/src/components/ui/Table.css:249]`
  `[src: file: frontend/src/components/ui/KeyValueList.css:91]`
  `StatGrid` needs no query: `repeat(auto-fill, minmax(min(100%, …), 1fr))`
  wraps its tiles by its own width, never a fixed column count.
- **A table's narrow forms.** `narrow="scroll"` (default) keeps the columns
  and scrolls sideways in the wrapper that is always there. `narrow="cards"`
  makes each row a card in which every value follows its column header (a
  label hidden from assistive technology, which already has the header row);
  the header row is visually hidden and shows again when a control in it (a
  sort button, the select-all) takes the focus. Engines differ on whether a
  table keeps its roles once its display changes, so in cards form the roles
  are written out (`table`, `rowgroup`, `row`, `columnheader`, `rowheader`,
  `cell`: the native ones, so the wide rendering is unchanged), the table is
  named by its caption with `aria-labelledby`, and each cell names its column
  header in `headers`.
  `[src: file: frontend/src/components/ui/Table.tsx:183]`
- **A selected row is a checked checkbox.** The row's checkbox is the only
  semantic of the selection: `aria-selected` belongs to a row of a `grid`,
  not of a plain table. The row carries `data-selected="true"` for its colour,
  the accent tint of the selected rows of the collection lists
  (`--kr-accent-bg`), laid over the row as a background image so a toned row
  keeps its tone tint and its inline-start border.
  `[src: file: frontend/src/components/ui/Table.css:160]`
- **A scroll area the keyboard reaches.** The wrapper becomes a `region`
  named by the caption, with `tabindex="0"`, when `maxHeight` bounds it or
  when its content is wider than it (measured with a `ResizeObserver`); a
  table that fits is no Tab stop.
  `[src: file: frontend/src/components/ui/Table.tsx:107#useOverflowsSideways]`
- **Sorting is controlled.** The table never reorders rows: a sortable header
  is a button that calls `onSortChange` with `nextSort` (ascending first,
  then the other way on the same column); `aria-sort` sits on the sorted
  header only. `[src: file: frontend/src/components/ui/dataKinds.ts:38#nextSort]`
- **A row opens from the keyboard.** With `onRowOpen` the first data cell
  (the `rowHeader` column, else the first) holds a real button. A click on
  the rest of the row is a mouse shortcut: it never fires from a control in
  the row (button, link, field, label, checkbox, anything focusable), from a
  portal (a menu, a tooltip, a dialog opened from the row), or when the
  reader is selecting text.
  `[src: file: frontend/src/components/ui/Table.tsx:165]`
- **Empty and loading inside the table.** One row spanning every column:
  `empty` (an `EmptyState`, or "Nothing to show") or a `LoadingState` sm with
  the table `aria-busy`. The select-all is disabled when there is no row.
- **12 px at least.** Both densities write `--kr-type-sm` or more; so do the
  tile labels, the delta, the progress label and its legend, and the lines
  and number gutters of `LogViewer` and `DiffViewer`.
- **A copy says what happened.** `CopyButton` keeps its name (focus does not
  re-announce), changes its icon, and writes "Copied" or "Copy failed" in a
  polite live region next to it, empty again after
  `COPY_FEEDBACK_MS` (1500). No clipboard, a refused permission or a
  `getText` that throws are all a failure.
  `[src: file: frontend/src/components/ui/CopyButton.tsx:42#CopyButton]`
  `[src: file: frontend/src/components/ui/dataKinds.ts:6]`
- **Never colour alone.** A delta's direction is written for assistive
  technology ("Up", "Down", "No change") and its tone colours the arrow
  only, the text stays `--kr-text-secondary`; an unknown value is "—" with
  its reason in a title and in hidden text; a missing key-value is "—" read
  as "No value". A tinted log line also writes its marker in words (the
  translated "stderr", "error" or "warning"), and a diff line writes its
  `+`, `−` or space marker as text in a column of its own.
- **A stacked bar is read once.** Without a legend it is a `role="img"`
  whose name lists every segment, its count and its share (percent in the UI
  locale, "<1 %" for a share that would round to 0). With `legend`, the
  legend is the readable list, named by `label`, and the image is named by
  `label` alone, so the breakdown is not heard twice. Each drawn segment keeps
  `--kr-space-1` of width at least.
  `[src: file: frontend/src/components/ui/ProgressBar.tsx:78]`
  A single bar with no `value` is indeterminate: it slides, and under
  `prefers-reduced-motion: reduce` it stands still, the whole track striped.
  `[src: file: frontend/src/components/ui/ProgressBar.css:130]`
- **One log viewer (`LogViewer`, KT-2004).** The scroll area is a focusable
  (`tabIndex={0}`) `role="log"` named by the required `label`; it sets no
  `aria-live` of its own. A line is a string or `{ text, stream, level, time }`:
  `stderr` and the `error` level are tinted danger, `warn` warning, from the
  status tokens, and each also writes its marker in words (`ui.log.stderr`,
  `ui.log.level.error`, `ui.log.level.warn`); a time is a `<time>` in a column
  of its own. ANSI escape sequences are removed by `stripAnsi`, on screen and
  in the copied text. `follow` (default on) keeps the end in view; scrolling
  up by hand turns it off and shows a "jump to the end" button that resumes
  it and gives the focus back to the log. The wrap toggle (`aria-pressed`) is
  always in the toolbar; copy (`copyable`, a `CopyButton` of the whole text),
  refresh (`onRefresh`) and clear (`onClear`) show only when wired. `note`
  ("last N lines", "truncated") sits above the log, outside it; `empty`
  replaces the default "No output". `lineNumbers` are hidden from assistive
  technology and start at `startNumber` (default 1), so a tail is numbered
  where it starts. `maxHeight` is `sm`, `md` (default), `lg` or `fill`.
  `[src: file: frontend/src/components/ui/LogViewer.tsx:142]`
  `[src: file: frontend/src/components/ui/logKinds.ts:37#stripAnsi]`
- **One diff viewer (`DiffViewer`, KT-2004).** It renders a model, never raw
  text: `parseUnifiedDiff` builds it from a git patch (old and new numbers
  from the hunk headers, renames, binary files), `diffFileFromRows` from the
  positional pairs of `qp-history-diff.ts`, where a `changed` line carries the
  new `text` and the old `otherText`. Unified writes a changed line as two
  rows, marked `−` then `+`, both in the changed tint
  (`--kr-diff-changed-*`); side by side puts the old text left and the new
  right on one row. Side by side shows as unified while the viewer's own
  width is under 40rem (`DIFF_SPLIT_MIN_REM`), measured with a
  `ResizeObserver`; the viewer is no size container, so it never collapses
  in a flex row, and the layout toggle hides while it is narrow. Each file
  has a header (previous path → path, a status badge, +added −removed, and a
  `CopyButton` of the new text of its shown lines), or "binary" /
  "truncated" in place of its lines. `highlight(text, path)` must return
  escaped HTML, since its markup is injected as is; it runs on the new texts
  (context, added, the new side of a changed line) and is skipped for a file
  of more than `highlightLimit` lines (default `DIFF_HIGHLIGHT_LIMIT`, 2000);
  its `hljs-*` classes take the `--kr-code-*` colours in `DiffViewer.css`.
  With `selectable`, each line holds a toggle button with `aria-pressed`,
  named by its number; a removed line says so ("Select removed line 13"),
  because the removed and the added line of a modified hunk often share a
  number. A click selects one line, a click on it again clears it, and
  Shift (with a click or the keyboard) extends the range from the first line
  up to `maxSelection` lines (default `DIFF_MAX_SELECTION`, 50); the caller's
  `selectionToolbar` shows under the last selected line. Extending needs
  Shift: a way without a modifier, for touch, is to be decided when the
  first screen moves to it. `loading` shows a `LoadingState` with the viewer
  `aria-busy`; no file shows "No change" or `empty`. A bounded `maxHeight`
  makes the files a focusable region named by `label`.
  `[src: file: frontend/src/components/ui/DiffViewer.tsx:317-322#DiffViewer]`
  `[src: file: frontend/src/components/ui/DiffViewer.tsx:185]`
  `[src: file: frontend/src/lib/diffModel.ts:82#parseUnifiedDiff]`
  `[src: file: frontend/src/lib/diffModel.ts:193#diffFileFromRows]`
- **Variants** live in `dataKinds.ts` (`TABLE_DENSITIES`,
  `TABLE_MAX_HEIGHTS`, `TABLE_NARROW_FORMS`, `TABLE_ROW_TONES`,
  `KEY_VALUE_LAYOUTS`, `STAT_TILE_SIZES`, `PROGRESS_TONES`,
  `PROGRESS_SIZES`, `nextSort`, `COPY_FEEDBACK_MS`), `logKinds.ts`
  (`LOG_LEVELS`, `LOG_MAX_HEIGHTS`, `stripAnsi`) and `diffKinds.ts`
  (`DIFF_LAYOUTS`, `DIFF_MAX_HEIGHTS`, `DIFF_SPLIT_MIN_REM`,
  `DIFF_HIGHLIGHT_LIMIT`, `DIFF_MAX_SELECTION`, `DIFF_MARKERS`); the model of
  a diff and its parsers are in `lib/diffModel.ts`.
- **In the gallery** each one is shown in a wide container on its own line,
  then its narrow demos together on the next line (`WideAndNarrow`: each
  narrow container is 20rem whatever the window and they wrap under each
  other in a narrow window; the Table shows its cards and its sideways
  scroll there; no padding on the containers), with sample data whose
  numbers are written with `Intl.NumberFormat` in the interface's locale;
  the test is `DesignSystemDataComponents.test.tsx`.
  `[src: file: frontend/src/components/settings/DesignSystemComponents.tsx:807-811#WideAndNarrow]`
- **A size container needs a width of its own.** Inline-size containment
  ignores the content, so a `Table` or `KeyValueList` placed as a flex item
  with no width or flex-basis collapses to 0 px and vanishes. In the gallery
  each one sits directly in a `.ds-demo-container` whose form sets a width
  (`wide`: `width: 100%`; `narrow`: `flex: 0 1 20rem`); the gallery test
  checks both. A screen that hosts them in a flex row does the same.
  `[src: file: frontend/src/components/settings/DesignSystemSection.css:395]`

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

Settings → appearance → "Design system" opens a sub-page of its own
(KT-1987), also linked apart at the foot of the Settings list, for
contributors (KT-1923): its page header (`ListHeader`, the page's one `h1`)
with the way back to the appearance section of Settings
(`/config/you#settings-appearance`, where the link sits), an index (the
tokens, then one entry per component),
then two tabs. Under the `h1`, each tab's part is an `h2` (visually hidden:
the tab says it), the token groups and the entries are `h3`. Tokens is
`/config/design-system`; Components, `/config/design-system/components`,
shows each component under its own heading, with its variants and states, in
the current theme. An entry's address is its tab plus the heading in the hash
(`/config/design-system/components#ds-c-menu`). The old `/config#design-system`
and the old `/config#ds-…` entry anchors still land on the right tab and entry:
the table is `DESIGN_SYSTEM_ANCHOR_TABS`.
`[src: file: frontend/src/lib/routes.ts:425#DESIGN_SYSTEM_ANCHOR_TABS]`
`[src: file: frontend/src/lib/routes.ts:442#legacyDesignSystemLocation]`
The route is its own chunk, never preloaded, so Settings does not load the
gallery.
`[src: file: frontend/src/routes/lazyRoutes.ts:17#DesignSystemRoute]` Adding a component: add it to `UI_GALLERY_COMPONENTS`, a
demo in `DesignSystemComponents.tsx`, and its `designSystem.caption.<id>` in
the four locales; the gallery test fails if a component of the folder is
missing. A shared component built on those controls (the agent picker,
KT-1885) goes in `SHARED_GALLERY_COMPONENTS`, shown after them.
`[src: file: frontend/src/components/settings/designSystemGallery.ts:19#UI_GALLERY_COMPONENTS]`
`[src: file: frontend/src/components/settings/designSystemGallery.ts:56-58#SHARED_GALLERY_COMPONENTS]`

## Settings: five sub-pages, a list and a search (KT-1923)

Settings is five sub-pages — Vous, Agents et modèles, Travail, Sécurité,
Système (`/config/you` … `/config/system`). One table, `SETTINGS_SECTIONS`
in `lib/routes.ts`, says which sections each holds, in order, with the i18n
key of each section's name, the keys of every name it draws (field labels,
group headings, the card's own heading when it differs from the name in the
list; a hint drawn inside a label is matched with it, never shown), its folds
and the anchors inside it: it is the single source of the list, the
section links, the redirects of the old `/config#…` anchors and the search.
A new section, a moved one or a new setting starts there.
`[src: file: frontend/src/lib/routes.ts:168#SETTINGS_SECTIONS]`

- **The list** (`SettingsNav`, a `nav` named « Pages des réglages ») —
  `SearchField` on top, then the five sub-pages with their section count, the
  current one `aria-current="page"`, then the design system apart. It is a
  plain navigation over the existing `.set-nav` frame, so the shell of lot 2
  can take its place without changing an address.
  `[src: file: frontend/src/components/settings/SettingsNav.tsx:49#SettingsNav]`
- **The search** (`searchSettings`) matches every word of the query in the
  sub-page names, the section names and the names of the table, and in the
  names of the agents (a hit lands on the agent's row,
  `agent-config-<agent>`, and opens its card; the local agent lands on the
  local models), in the current locale, accents ignored. A placeholder is
  never indexed: a field is found by its label. The hits list their sub-page and section and announce
  their count (`role="status"`); Down from the field goes to the first hit;
  no hit is an `EmptyState` of kind `noResult`. A hit is a link to the
  section's address.
  `[src: file: frontend/src/lib/settingsSearch.ts:72#searchSettings]`
- **A sub-page** opens on a `DetailHeader` (its name, its sentence) and a row
  of links to its sections (`SettingsSubPageHead`). A section that used to
  sit inside another, or that folds, is a `SettingsListedSection` (a card
  with its anchor and a `SectionHeader`).
  `[src: file: frontend/src/components/settings/SettingsSection.tsx:29#SettingsListedSection]`
- **The band of section links** (KT-2017) — the row of links is a sticky
  band, a direct child of the sub-page's box (`.set-subpage-band`; a sticky
  element only sticks inside its parent), right under the app's top bar
  (`--set-band-top: var(--app-bar-h, 51px)`, the bar's height measured by
  `useAppBarHeight`) from the top to the bottom of the sub-page, at every
  width. It is one line of a fixed height (`--set-band-height`) that scrolls
  sideways in a `HeaderScrollLine` when the links do not fit, on an opaque
  `--kr-bg-sticky` surface, with a line and a shadow once stuck
  (`data-stuck`); its layer (30) is under the top bar and under the agent row
  an open model menu lifts to 40. The name and sentence scroll away.
  `[src: file: frontend/src/components/settings/SettingsSection.tsx:66#SettingsSubPageHead]`
- **The section being read** is the link with `aria-current="location"`
  (filled and underlined, not colour alone): the last section whose top has
  passed under the band, the last one at the bottom of the page; a click
  marks its link at once, holds the mark while its scroll runs and brings the
  link into view in the line. `useSubPageBand` listens only while its
  sub-page is the visible one and reads the boxes once per frame at most. It
  also moves a control the Tab key leaves under the stuck band below it
  (WCAG 2.4.11), outside dialogs, menus and listboxes.
  `[src: file: frontend/src/components/settings/useSubPageBand.ts:69#useSubPageBand]`
- **Folds** (`SettingsFold`: `SectionHeader` + a ghost `Button` with
  `aria-expanded`) — the model catalogue, the usage, the API call log and the
  debug log viewer. The page holds which are open, so an arrival or a hit on
  a folded anchor opens it; the content mounts on the first opening and
  stays mounted.
  `[src: file: frontend/src/components/settings/SettingsFold.tsx:28#SettingsFold]`
- **After a jump** (a section link, a hit, an arrival), the focus goes to the
  heading of the section, without scrolling: the caller scrolls. Every
  element with an id inside Settings stops below the fixed top bar and the
  band through one rule (`.settings-page [id]`, `scroll-margin-top:
  var(--set-anchor-offset)`: the bar, the band and `--kr-space-4`, at every
  width); no card carries its own margin. The targets of the guided tour
  share the rule (`[data-tour-id]`), and the tour counts a target's scroll
  margin as covered, so its spotlight never sits on the band.
  `[src: file: frontend/src/pages/SettingsPage.tsx:158#focusSectionHeading]`
  `[src: file: frontend/src/components/tour/useTourPositioning.ts:95]`
- **A sub-page is drawn on its first visit and kept**, hidden while another
  is shown, so an edit in progress survives a visit elsewhere.
- **No control is lost**: `SettingsPage.subpages.test.tsx` counts, per
  section, the buttons, fields and links against the single page of the base
  commit. It also renders each sub-page with its folds open, reads every
  `label`, `h3` and `h4`, and asserts each gives a hit on the section that
  draws it; the names that are options, not settings (« Toujours »,
  « Désactivé »…), sit in an explicit allow-list in the test. A setting added
  without its key in the table fails it.

## AgentIdentity: an agent, its model and its role (KT-1936)

Wherever an agent is named, its model is written next to it; the role (who
the agent is in the discussion) is a separate chip and never replaces the
model. One component does it, on every surface: the discussion header (the
picker's trigger, its static form too), the message header, the activity
bubbles (pending, streaming, failed), the workflow-step bubble and message,
the participant chips and their details, and the discussion list rows.
`[src: file: frontend/src/components/ui/AgentIdentity.tsx:44#AgentIdentity]`

- **Order:** icon, name, model, tier icon, role chip. Its text is « name ·
  model · role », so the button around it reads the same.
- **The model** is the secondary text role, the code face, never under the
  12 px floor. With no concrete id it writes one of three states, never a
  blank or a tier name: « model not declared » (a joined CLI session that
  declared none), « model not recorded » (a legacy message), « default model »
  (a model nobody can name yet: the provider's default before the first
  reply; the workflow step, which reports none). The agent is not repeated in
  that state: its name is written just before it. `agentModel(id, fallback)`
  picks.
  `[src: file: frontend/src/components/ui/agentIdentityKinds.ts:40#agentModel]`
- **Which model a discussion's agent runs** is resolved one way for the
  header's picker trigger, the discussion list row and the activity bubbles
  (pending, streaming, failed), so the three never disagree: the exact model
  the discussion holds, else the model its mode points to (the family's mode
  models in Settings, or the HTTP connection's own), else the model of the
  agent's last reply on that mode, else « default model ». None of them reads
  the catalogue, which the picker loads only when it opens: their text does
  not change when the list opens.
  `[src: file: frontend/src/lib/agentModelResolution.ts:76#discussionAgentModel]`
  `[src: file: frontend/src/lib/agentModelResolution.ts:30#resolveIdentityModel]`
- **The roles:** main agent, one-off agent, CLI session, workflow step
  (`ui.agentIdentity.role.*`); `roleLabel` writes another word in the chip
  (the picker's caller, the guided-tour demo).
- **Sizes:** full (the participant details), inline (everything else),
  compact (the discussion list row: the model and the role leave the screen
  and stay in the accessible name, the row's title has them too).
- **Cut model:** `truncateModel` (the picker trigger, the participant chip)
  cuts the id past 17 characters with an ellipsis; the full id stays in the
  DOM, so in the accessible name, and the caller writes it in its tooltip with
  `agentIdentityText()`. A button that names itself with `aria-label` (which
  replaces its text) writes its action, then that same identity: the picker
  trigger reads « Changer d'agent ou de mode IA — @opencode ·
  opencode-go/kimi-k3.1-pro · Avancé · agent principal », the mode or
  « Personnalisé » in the place of the tier.
  `[src: file: frontend/src/components/ui/agentIdentityKinds.ts:37#AGENT_IDENTITY_MODEL_MAX_CHARS]`
  `[src: file: frontend/src/components/ui/agentIdentityKinds.ts:74#agentIdentityText]`
  `[src: file: frontend/src/components/AgentSwitchPicker.tsx:1400#identityText]`
- **Colour:** the name takes `agentTextColor()`, a text-safe token per agent
  (`--kr-agent-<agent>-text`) that holds 4.5:1 on every surface of every
  theme; the brand colour (`agentColor()`) stays on dots, borders and icons;
  a colour the user configured is written as it is. See
  [design-tokens.md](design-tokens.md), Contrast.
- **Lint:** `ui-guards/no-adhoc-agent-identity` refuses an element written
  in an agent's colour that also writes a model or a role
  ([ui-lint-guards.md](ui-lint-guards.md)).

**The participants strip** (`ParticipantsStrip`, owner decisions
2026-11-20): each chip is a button that opens the participant's details,
with its presence state, its icon and `AgentIdentity` (the model cut past 17
characters); the main agent first. One line that scrolls sideways, never
wrapped: while chips are past its right edge the edge fades and a « +N »
counter (the chips not in view) opens the full list, whose rows open a
participant's details. The same on a wide screen when the chips do not fit.
The role comes from the participant: a workflow step, the main agent (a
native participant of the discussion's own agent), a one-off agent (another
native one) or a joined CLI session.

- **The full list** is a floating layer built like `Menu`: a portal into
  `document.body` placed by `useAnchoredStyle` (`ui/anchor.ts`), so the
  header's clipping ancestors cannot cut it and it stays inside the window.
  Focus moves to its first row on open, the arrows move between rows, Escape
  closes it and gives focus back to the counter, a press outside closes it.
  The counter's name is a plural form (`tp('disc.participantsMore', n)`),
  right for one participant and for several.
- **In the room header** (review round 2, option A of the owner's mockup),
  the whole agent line is ONE line that scrolls sideways
  (`HeaderScrollLine`): the main agent's trigger first, then its on/off
  toggle, the participant chips, the scope and the trust chips. The « +N »
  counter is pinned at the line's right end, outside what scrolls (the strip
  portals it into the line's end slot, through `headerScrollLineContext`), and
  counts the chips not fully inside the line, on either side. The right edge
  fades while the line can scroll further right. A plain mouse wheel scrolls
  it sideways; a focused control is scrolled into view by the browser. The
  header keeps its height and nothing wraps.
- **The trigger's floor** in that line: its name whole, its mode icon, its
  role chip whole, and its model no shorter than 8 characters before the
  ellipsis (`9ch`; an unknown state follows the same rule). Above the floor the
  model gives way first: its `max-width` is `clamp(9ch, 100cqw - 18rem, 17ch)`,
  the line being the size container. What the floor does not fit, the line
  scrolls (`pages/DiscussionsPage.css`, `components/HeaderScrollLine.css`;
  held by `components/ui/stylesheets.test.ts`).
- **The participant details** opened by a chip are a floating layer too
  (`ParticipantDetailsLayer`: portal and `useAnchoredStyle` under the chip),
  because the scrolling line would clip a layer absolute inside it. A row of
  the full list scrolls its chip into view before opening its details.
- **One keyboard and pointer contract for both layers** (review round 3):
  in `document.body` a layer is not next to its control in the Tab order, so
  the full list and the participant details share `useAnchoredLayer`
  (`components/ui/useAnchoredLayer.ts`, on top of `useDialogFocus`): focus
  moves to the layer's first control once it is placed, Tab and Shift+Tab stay
  between its controls, Escape closes it and gives focus back to the control
  that opened it (the counter, the chip), a press outside that control and the
  layer closes it. A new anchored layer of this kind uses the same hook.
- **The details header** is a flex row: the full-size identity shrinks
  (`min-width: 0`) and wraps rather than overflows, the model is the part cut
  with an ellipsis (its full id stays in the header's tooltip, the layer's
  accessible name and the « declared model » row), and the close button never
  shrinks, so it stays inside the 320px layer at every width
  (`styles/components.css`, held by `components/ui/stylesheets.test.ts`).
`[src: file: frontend/src/components/ui/useAnchoredLayer.ts:31#useAnchoredLayer]`
`[src: file: frontend/src/components/ParticipantsStrip.tsx:48#ParticipantsStrip]`
`[src: file: frontend/src/components/HeaderScrollLine.tsx:27#HeaderScrollLine]`
`[src: file: frontend/src/lib/stripOverflow.ts:5#measureStripOverflow]`
`[src: file: frontend/src/lib/stripOverflow.ts:15#measureChipsOutOfView]`
`[src: file: frontend/src/components/DiscParticipantsHeader.tsx:110#participantRole]`
`[src: file: frontend/src/components/DiscParticipantsHeader.tsx:172#ParticipantDetailsLayer]`

## The one agent picker

Every place that chooses an agent uses `AgentSwitchPicker` (KT-1885); its
modes are props, not copies: the trigger `variant` (`chip`, `compact`,
`field`), `multiple` (several targets, the list stays open) and `exactModel`
(an agent and a model id, the catalogue's or one typed in the search, or
« = principal », with the principal's own model greyed) and `customModel`
(KT-1994: the full list of a discussion, see below).
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:499#variant]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:522#multiple]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:533#exactModel]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:541#customModel]`
Its popover is in `<body>` on the popover layer, above every dialog, and opens
on its search field. It is placed from its rendered size, measured on open,
when the search or the catalogue changes it, and on resize: it stays inside
the window and never covers its trigger; opened above, its bottom is anchored
to the trigger. Never place it from an estimated size. The grouped list is
440 px wide whatever is unfolded (so it never jumps when a long model id
appears), the plain agent lists 480 px at most, the @ menu 520 px (from a
768 px window, up to 760 px when its anchor is that wide); never wider than
the window. The @ menu is also never wider than its anchor (the composer, the
new discussion's prompt field: `100%` in its `min(...)`, no floor), so the
column that clips it (`.disc-messages-col`, overflow hidden) never cuts a
tier: in a narrow column it takes the anchor's width and the three tier
columns hold down to 368 px.
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:607#place]`
Availability always goes through
`AgentAvailabilityBadge` (`unavailable` and `disabled` block the choice,
`needs-full-access` and `not-listed` only warn, `catalog` is the neutral
« Détecté », « Cache à revérifier », « Configuration migrée »).
`[src: file: frontend/src/lib/agentAvailability.ts:14#AgentAvailabilityState]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:144#AgentAvailabilityBadge]`

**The grouped list (disposition A, KT-1994)** is the popover of every picker
that chooses a mode (the discussion header, the new-discussion form, workflow
steps, Quick Prompts, the API assistants) and of the shadow editor's exact
model: one list at every width, `AgentModelList`. Per agent, a header: the dot
and the name in one box (`kr-agent-switch-tier-id`, they never split), its kind
(CLI / Local / API), ONE state badge (the more serious of the agent's own state
and the state more than half of its modes share) and its model count. Then its
three modes, one row each: icon and name in an aligned left column, the model
id that mode runs written in the code face, the display name as a secondary
text only when it says more than the id once both are lowercased and kept to
letters and digits (« Fable », shared by three Claude Code models, stays;
« GPT-6-Luna » next to `gpt-6-luna` does not), the check on the selected row. A mode's model is what the catalogue
resolves for it, else the backend's launch verdict for that tier
(`effective_model`, else `requested_model`); an HTTP connection's modes are its
economy / default / reasoning models. A mode that declares none says « agent
default model » (`wf.shadow.rows.defaultModel`). A mode keeps its own state
under its model only when it differs from the header's, and its full state in
its tooltip and accessible description. The assistants' « agent default » is a
fourth row with the short label `agentPicker.defaultTier` (« Défaut »), its
full name in its tooltip and accessible name; the exact-model mode always has
it (no model = the agent's own default). The current agent comes first, then
the configured order; an unavailable agent stays listed and cannot be chosen,
its reason written in one muted line under its header (the caller's detail,
else the launch verdict's, else what the catalogue says of its models). Nothing
is cut with an ellipsis: text wraps (a Recents row only keeps its agent's name
on one line and ellipsizes its model).

**The agent band (KT-2010).** The header is a band of its own, the full width
of the list: an opaque `--kr-bg-raised` surface (rows scroll under it, so it
cannot be a translucent tint), the name in the primary text role one step
larger (`--kr-type-md`) and semibold, the kind, the state badge and the count
in the tertiary role at 12 px; a state badge keeps the panel colour under its
tint, so it reads as it does on any other row. The band sticks to the top of
the list while its own rows scroll under it (the next agent's band pushes it
out), so the agent of the rows in view is always written. Its padding is the
rows', so the dot and the rows keep today's left edge (no extra indent that
would cut model ids at 400 px). A row that takes the focus (arrows, Home, End,
Tab, a click) is scrolled to just below its band, never left under it, and
unfolding « Autres modèles » puts the control right under the band. Both
measure the band's height (`stickyHeaderHeight`). Band text holds 4.5:1 in
every theme (`agentPickerContrast.test.ts`).
`[src: file: frontend/src/components/AgentSwitchPicker.css:455]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:737#stickyHeaderHeight]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:812#keepFocusClearOfHeader]`

**Free models (KT-2010).** The catalogue entry carries no price, so the
model's own words decide, and providers spell it several ways (owner's
catalogue, 2026-11-22): OpenRouter's variants end with `:free` and its router
is `openrouter/free`, OpenCode's free models end with `-free`
(`opencode-go/beam-free`), and one OpenCode model says it in its display name
only ("OpenCode Zen/Union Alpha Free"). A model is free when its id's last word
is `free` (the trimmed id, case-insensitive, is `free` or ends with `free`
right after `:`, `-`, `_` or `/`; `freedom-1`, `vendor/freeze`, `carefree` and
`model-free-preview` are not), or, where the row has a catalogue entry, when
its display name's last word is `free`, bare or in parentheses ("… Alpha
Free", "… (free)"). The mode rows, the other models and the custom row read
their entry; a recent row reads its group's entry for that id when the
catalogue lists it, and the id alone otherwise. Such a model carries a small
neutral-positive chip « Gratuit » / "Free" / « Gratis » / « 免费 »
(`agentPicker.freeBadge`) right after its id, on every row of the list that
shows it: the mode rows, the other models, the custom model and the recents.
One function decides, `isFreeModel` (with `isFreeDisplayName` for the name),
for any agent or connection (a proxy listing the same ids gets the badge too,
not only a connection named OpenRouter or OpenCode). The word ends the row's accessible name (« OpenRouter · Éco ·
Gratuit »), and the search finds free models by that word in the interface
language as well as by `free`. The chip is `--kr-success-bg` with the
secondary text role (4.5:1 on a row at rest, hovered and current in every
theme). The trigger is unchanged: it already writes the full id. If a provider
later states prices in its listing, that fact moves to the catalogue entry
server-side.
`[src: file: frontend/src/lib/freeModel.ts:35#isFreeModel]`
`[src: file: frontend/src/lib/freeModel.ts:25#isFreeDisplayName]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:57#FreeModelBadge]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:67#withFreeName]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:71#freeSearchTerms]`
`[src: file: frontend/src/components/AgentSwitchPicker.css:614]`

With a search text, a mode row is shown only when the text hits the agent's
name, that row's model (id or display name) or the mode's name; other models
are filtered by id or display name; an agent with no visible row is not
listed, and when nothing matches one line says so. The fold controls are
hidden while searching. The trigger writes the model id the configuration
names (or the custom model), whether or not the catalogue is loaded: its text
never changes when the list opens; the display name goes to its title.
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:782#AgentModelList]`
`[src: file: frontend/src/lib/modelCatalogSelection.ts:58-64#modeModelId]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:911]`

With `customModel` (the discussion header and the new-discussion form) and in
the exact-model mode, each agent's other models (the catalogue's models no
mode points to) follow its modes, folded behind one « Autres modèles » control
per agent: unfolding one agent leaves the others folded, and scrolls the list
so that control sits at the top of its viewport (the rows it reveals are seen;
the focus stays on it). The search finds them without unfolding. An unfolded
agent mounts at most
`OTHER_MODELS_RENDER_LIMIT` (50) of them and counts the rest; the search
reaches every one (577 OpenRouter models are never mounted at once). A model
outside the modes is « Personnalisé » / "Custom", on the trigger in place of
the mode chip and in the list, where it stays visible under its agent while
folded. A typed text is offered « as is » in ONE line at the end of the list,
for the picker's current agent only (the shadow editor: the chosen agent, else
the step's own), naming it (« Identifiant absent du catalogue ? Utiliser « q »
avec Codex »), and not when that agent already lists that id. The discussion saves
a custom choice as `model` and a mode as `tier` alone, which clears the model
server-side (KT-1993, [discussion-agent-routing](../architecture/discussion-agent-routing.md));
a refusal is shown next to the entry and the previous choice stays: the picker
trims, it never re-validates.
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:74-78#OTHER_MODELS_RENDER_LIMIT]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:1376-1377#selectModel]`
`[src: file: frontend/src/components/ChatHeader.tsx:334]`
`[src: file: frontend/src/pages/DiscussionsPage.tsx:2469]`

« Récents » sits on top of those lists: the last three distinct choices
(agent, connection, mode or exact model), most recent first, hidden while
searching. They are written when a choice is applied, in localStorage under
`kronn:agentPicker:recents`, a synced UI preference mirrored server-side like
the collection favourites.
`[src: file: frontend/src/lib/agentPickerRecents.ts:54#readAgentPickerRecents]`
`[src: file: frontend/src/lib/uiPreferences.ts:53]`

**A row with tier choices** (the @ palette: `AgentPickerTierChoices`) is one
line per agent at desktop widths: the agent and ONE state badge on the left,
its tiers on the right on equal tracks shared by every row, each tier its icon
and name on one line with the model id it runs written under it
(`showModel`). That id never breaks inside a token: only after `/` and `:` (a
`<wbr>` between unbroken parts), on two lines at most, then an ellipsis; the
cell's title and accessible name keep it whole. A mode with no declared model
writes the short `agentPicker.defaultShort` (« par défaut »), the full
sentence in its title. The palette measures its own width (a container
query): from a 748 px content box the tiers sit on fixed 192 px tracks
beside the agent. A track leaves a 182 px text box once the cell's border and
padding are taken, so a 25-character id fits on one line (12 px mono, 7.22 px
a character) and every row lines up. The 748 px are the three tracks and
their gaps (584 px), the row's gap and padding (24 px) and an agent column of
at least 140 px; at 760 px that column gets 142 px. Narrower, the tiers go
under the agent in three equal columns. The @ menu offers no other model and
no exact model: nothing exists server-side for a one-off call.
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:211#AgentPickerTierChoices]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:194#showModel]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:198-203#tierRowState]`
`[src: file: frontend/src/lib/agentAvailability.ts:62#sharedTierState]`
`[src: file: frontend/src/lib/agentAvailability.ts:82#rowAvailability]`

A menu a text field drives (the composer's @ menu) renders
`AgentPickerPalette` and keeps its own keys. The palette is a listbox: give it
an `id`, spread `agentPaletteFieldAria` on the field (`aria-controls`,
`aria-expanded`, `aria-activedescendant` while open); it keeps the highlighted
row in view and its tier choices are no Tab stops. Give it a `boundary`: its
height is then measured from its anchored edge to that area's far edge, minus
`--kr-space-2`, and it scrolls inside. The composer's boundary is the message
list (`composerMentionBoundary`), so the menu never slides under the
discussion header or a banner.
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:304-310#AgentPickerPalette]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:329#boundary]`
`[src: file: frontend/src/lib/agentPickerPalette.ts:21#agentPaletteFieldAria]`
`[src: file: frontend/src/lib/agentPickerPalette.ts:11#composerMentionBoundary]`
The API assistants go through `AssistantAgentPicker` (installed agents,
external connections, tiers); a reopened conversation shows the connection
and tier it runs on (`assistantChoiceFromDiscussion`).
`[src: file: frontend/src/components/AssistantAgentPicker.tsx:14#AssistantAgentPicker]`
`[src: file: frontend/src/lib/assistantAgentChoice.ts:32#assistantChoiceFromDiscussion]`
Kept apart on purpose: the audit's card grid (a decision aid), the default-tier
radios of Settings (a preference) and the chips of agents detected in a prompt
(a read-only summary).
