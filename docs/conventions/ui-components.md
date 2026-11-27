# Base UI components — `frontend/src/components/ui/`

KT-1880 (design-system lot 1). These components replace the parallel base
buttons, tab styles, menus and native `title=` tooltips. No screen is migrated
in KT-1880; the screens move to them in lots 2-5. KT-1882 adds the form
components (`Field`, `Select`, `Switch`, `FormFooter`); lot 5 migrates the
forms to them. KT-1883 adds the feedback components (`Notice`,
`EmptyState`, `LoadingState` with its `Skeleton`). KT-1884 adds the six
header kinds (`ListHeader`, `DetailHeader`, `TabHeader`, `SectionHeader`,
`DrawerHeader`, `CardHeader`), see [header-kinds.md](header-kinds.md). Pairs with
[design-tokens.md](design-tokens.md) and [ui-text.md](ui-text.md).

## Where they live

One folder, one file per component, with its stylesheet and its test next to
it (`Button.tsx`, `Button.css`, `Button.test.tsx`). `stylesheets.test.ts`
fails if a component lacks one of the three.
`[src: file: frontend/src/components/ui/stylesheets.test.ts:182]`

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
  `[src: file: frontend/src/components/settings/DesignSystemComponents.tsx:485]`.
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
(KT-1987): its page header (`ListHeader`, the page's one `h1`) with the way
back to the appearance section of Settings (`/config#settings-appearance`,
where the link sits), an index (the tokens, then one entry per component),
then two tabs. Under the `h1`, each tab's part is an `h2` (visually hidden:
the tab says it), the token groups and the entries are `h3`. Tokens is
`/config/design-system`; Components, `/config/design-system/components`,
shows each component under its own heading, with its variants and states, in
the current theme. An entry's address is its tab plus the heading in the hash
(`/config/design-system/components#ds-c-menu`). The old `/config#design-system`
and the old `/config#ds-…` entry anchors still land on the right tab and entry:
the table is `DESIGN_SYSTEM_ANCHOR_TABS`.
`[src: file: frontend/src/lib/routes.ts:167#DESIGN_SYSTEM_ANCHOR_TABS]`
`[src: file: frontend/src/lib/routes.ts:184#legacyDesignSystemLocation]`
The route is its own chunk, never preloaded, so Settings does not load the
gallery.
`[src: file: frontend/src/routes/lazyRoutes.ts:17#DesignSystemRoute]` Adding a component: add it to `UI_GALLERY_COMPONENTS`, a
demo in `DesignSystemComponents.tsx`, and its `designSystem.caption.<id>` in
the four locales; the gallery test fails if a component of the folder is
missing. A shared component built on those controls (the agent picker,
KT-1885) goes in `SHARED_GALLERY_COMPONENTS`, shown after them.
`[src: file: frontend/src/components/settings/designSystemGallery.ts:19#UI_GALLERY_COMPONENTS]`
`[src: file: frontend/src/components/settings/designSystemGallery.ts:47#SHARED_GALLERY_COMPONENTS]`

## The one agent picker

Every place that chooses an agent uses `AgentSwitchPicker` (KT-1885); its
modes are props, not copies: the trigger `variant` (`chip`, `compact`,
`field`), `multiple` (several targets, the list stays open), `exactModel`
(an agent and a model id, the catalogue's or one typed in the search, or
« = principal », with the principal's own model greyed) and `customModel`
(KT-1994: the full list of a discussion, see below).
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:484#variant]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:507#multiple]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:518#exactModel]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:526#customModel]`
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
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:592#place]`
Availability always goes through
`AgentAvailabilityBadge` (`unavailable` and `disabled` block the choice,
`needs-full-access` and `not-listed` only warn, `catalog` is the neutral
« Détecté », « Cache à revérifier », « Configuration migrée »).
`[src: file: frontend/src/lib/agentAvailability.ts:14#AgentAvailabilityState]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:129#AgentAvailabilityBadge]`

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

With a search text, a mode row is shown only when the text hits the agent's
name, that row's model (id or display name) or the mode's name; other models
are filtered by id or display name; an agent with no visible row is not
listed, and when nothing matches one line says so. The fold controls are
hidden while searching. The trigger writes the model id the configuration
names (or the custom model), whether or not the catalogue is loaded: its text
never changes when the list opens; the display name goes to its title.
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:756#AgentModelList]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:57#modeModelId]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:864]`

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
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:50#OTHER_MODELS_RENDER_LIMIT]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:1304-1305#selectModel]`
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
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:196#AgentPickerTierChoices]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:179#showModel]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:188#tierRowState]`
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
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:295#AgentPickerPalette]`
`[src: file: frontend/src/components/AgentSwitchPicker.tsx:314#boundary]`
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
