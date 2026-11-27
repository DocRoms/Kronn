# Header kinds — which header goes where

KT-1884 (design-system lot 1). Six kinds of header, one component each in
`frontend/src/components/ui/`, all on one frame: identity on the left
(`leading`, `title`, `meta`, `status`, `description`), `actions` on the
right, a `compact` form. Pairs with [ui-components.md](ui-components.md) and
[design-tokens.md](design-tokens.md).
`[src: file: frontend/src/components/ui/headerFrame.tsx:38#HeaderFrame]`

## The rule

| Kind | Component | Heading | Goes | Normal / compact title |
|---|---|---|---|---|
| list | `ListHeader` | `h1` | top of a list page (and of Settings): the page title, its count under it, the page actions (create, import) | `--kr-type-2xl` / `-xl` |
| detail | `DetailHeader` | `h2` | top of the object a list opens: its name, its id (`meta`), its state (`status`), what it is (`description`), the actions on it | `-xl` / `-lg` |
| drawer | `DrawerHeader` | `h2` | top of a side panel or a drawer: names the layer (`titleId` → `aria-labelledby`), carries its status badges, ends with its one close button | `-xl` / `-lg` |
| card | `CardHeader` | `h2` | top of one card of a grid or a list: names the card (`titleId`), its state as `StatusBadge`s, one action or a menu | `-lg` / `-md` |
| tab | `TabHeader` | `h3` | under the tab bar of a detail: what the tab holds, its count, the actions on the whole tab | `-lg` / `-md` |
| section | `SectionHeader` | `h3` | a group inside a detail, a tab or a drawer: its name, a hint, one or two actions | `-lg` / `-md` |

`[src: file: frontend/src/components/ui/headerKinds.ts:9#HEADER_LEVELS]`

- **One heading, one level.** Each kind renders exactly one heading, at the
  level of the table; there is no prop to change it. The levels follow where
  a kind sits, so a page read heading by heading never skips one:
  list (1) > detail, drawer, card (2) > tab, section (3). A card is an `h2`
  because it most often sits right under the page's `h1`.
- **Never stacked.** One header per surface. A view inside a side panel draws
  no header of its own: it hands its count and its actions to the panel's
  header (below). Two headers on top of each other are a bug.
- **Compact** is for dense places (a side panel, a narrow column, a card in a
  dense grid): smaller title, tighter padding, the description on one line.
  That line ends with an ellipsis when it does not fit: a one-line block
  (`text-overflow` does nothing on a flex box), so give it text or inline
  elements; a box inside it would be clipped mid-letter
  `[src: file: frontend/src/components/ui/headerFrame.css:82]`.
- **An empty slot is no slot.** `null`, `undefined`, `false` and `''` (and
  `true`) draw no wrapper and no gap, so `status={shared && <StatusBadge … />}`
  is fine; one helper decides it for every slot
  `[src: file: frontend/src/components/ui/headerKinds.ts:31#hasSlot]`.
- **Plain content.** A header is never fixed, sticky or above the page; where
  it sits is its container's layout. The stylesheets hold no `position` and no
  `z-index`, which the shared test reads. A gallery demo is plain content.
- **Long titles ellipsize.** The identity shrinks to nothing before the
  actions do: the actions stay on screen at 400 px.
- **Not a landmark.** The header is a `div`, so a page keeps its one banner.
- **A sub-page of Settings is a page.** It opens on a `ListHeader` (its one
  `h1`), its way back in `leading`, pointing at the Settings section that
  links to it rather than at the top of Settings; its own headings follow
  from `h2`. A `SectionHeader` there would leave the page without an `h1`.
  The design system is the first (KT-1987)
  `[src: file: frontend/src/pages/DesignSystemPage.tsx:72]`.
- **Tokens only**, as every stylesheet of `components/ui/`.

## The discussion side panel (the one screen moved in KT-1884)

`SidePanelShell` draws one compact `DrawerHeader` above its rail and its
view: the view's way back if it has one (Git's diff: back, on the left, first
in the tab order), the discussion's title (`h2`), under it the active view's
name and what the view hands over (the plan's progress, the count of files or
notes, the open diff's path), then the view's own actions (Git: expand,
refresh), the « ⋯ » `Menu` and the close button, last
`[src: file: frontend/src/components/SidePanelShell.tsx:143]`.
A view gives its `leading`, `meta` and `actions` through
`SidePanelHeaderSlot`; outside a shell (its own tests) it draws its own header
instead
`[src: file: frontend/src/components/SidePanelHeaderSlot.tsx:21#SidePanelHeaderSlot]`.

- **Every view's meta is inline text on the view name's line, in its size**:
  the shell resets what a view hands over (no box, no margin, no size of its
  own, two classes so it wins over the view's rule), so the line ends with
  « … » like any compact description
  `[src: file: frontend/src/components/SidePanelShell.css:56]`
  `[src: file: frontend/src/components/__tests__/SidePanelShell.test.tsx:320]`.

- **The menu renders in `<body>`** (the shared `Menu`, `--kr-z-popover`), so
  no view's header can sit over its entries (the « copy the id » entry under
  `.disc-tool-panel-header` of 2026-11-21).
- **A view cannot cover the rail or the header.** The body is positioned and
  isolated, and the shell's rule on a view resets its placement with two
  classes, above any one-class rule of a view at any width: the Files view
  was `position: absolute; inset: 0; z-index: modal` under 768 px, over the
  whole shell `[src: file: frontend/src/components/SidePanelShell.css:200]`.
  A test reads every stylesheet of `src/` for a rule that places a view root
  and checks the shell's rule wins
  `[src: file: frontend/src/components/__tests__/SidePanelShell.test.tsx:245]`.

## Moving later

`DetailHeaderShell` (the discussion and the artifact pages) and the other
details, tabs and list pages move to these kinds in lots 2-5; no other screen
changes in KT-1884. `DetailHeaderShell` is `@deprecated` for `DetailHeader`,
and a test fails when a file other than its two current users
(`ChatHeader.tsx`, `pages/PagesPage.tsx`) imports it
`[src: file: frontend/src/components/ui/DetailHeader.test.tsx:25]`.
