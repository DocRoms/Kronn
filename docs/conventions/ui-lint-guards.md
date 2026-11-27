# UI lint guards (KT-1887)

The previous harmonisations of the UI drifted back because nothing checked the
result. These guards refuse **new** drift. The violations that existed when they
came in are frozen in a baseline, which may only shrink.

See also [design-tokens.md](design-tokens.md) for the scales and
[ui-components.md](ui-components.md) for the components the guards point to.

## The guards

| Guard | Where | What it refuses | Baseline |
|---|---|---|---|
| `ui-guards/no-literal-ui-string` | ESLint | A literal UI string in JSX: text, a literal in braces or as a branch of a child expression (`{ok ? 'Saved' : 'Failed'}`, `{empty && 'None'}`, `{a \|\| 'Untitled'}`, `{a ?? 'Untitled'}`), a template or a concatenation with text; in the nine attributes `aria-label`, `aria-description`, `aria-placeholder`, `aria-roledescription`, `aria-valuetext`, `title`, `placeholder`, `alt`, `label`; and in the text props of a component (`description`, `help`, `error`, `message`, `confirmLabel`, `content`) | `frontend/eslint-suppressions.json` |
| `ui-guards/control-has-name` | ESLint | An icon-only `<button>` or native element with `role="button"`, or an `<input>`/`<select>`/`<textarea>`, without an accessible name | same |
| `ui-guards/no-adhoc-dialog` | ESLint | A modal (`aria-modal`, `<dialog>`) built outside `components/ui` (use `Dialog`, `Drawer`, `ConfirmDialog`) | same |
| `ui-guards/no-foreign-page-stylesheet` | ESLint | A page or a component importing the stylesheet of another page (`src/pages/<Page>.css`) | same |
| `ui-guards/no-off-scale-style` | ESLint | An inline `fontSize`, radius or colour outside the tokens, each branch of a `? :`, `&&`, `\|\|`, `??` value read, and a literal colour in a `--custom` key; a `useIsMobile`/`matchMedia` width outside `BREAKPOINTS` | same |
| `no-restricted-globals` / `-properties` (KT-1881) | ESLint | `window.confirm()` and `confirm()` | same |
| Off-scale stylesheet values | `pnpm lint:theme` | A `font-size` (the size inside a `font:` shorthand too), radius, colour, or `@media`/`@container` width (range syntax included) outside the scales, in a `.css` file, nested rules included | `frontend/src/styles/themeAuditBaseline.json` (counts per file and kind) |
| Status colours in every theme | `pnpm lint:theme` | A status colour (`--kr-success`…, `--kr-status-*`) under 4.5:1 on a surface, in any theme, light ones included | same file (`contrast` cells) |
| No warning in `components/ui` | ESLint | Every rule that warns elsewhere is an error under `src/components/ui/**` | none: the folder has no warning |
| `ui-guards/no-adhoc-agent-identity` (KT-1936) | ESLint | An element written in an agent's colour (`style.color` from `agentTextColor()` / `agentColor()`) whose content, or whose siblings (the other children of its nearest JSX parent, through a `cond &&` or a `? :`), also write a model (a `.model`, `model_tier`, `*Model` expression) or a role (`t('disc.target…')`, `t('ui.agentIdentity.role.…')`): render `AgentIdentity` | none: no violation when it came in; off in `components/ui` |
| Agent colours as text (KT-1936) | `pnpm lint:theme` | A `--kr-agent-<agent>-text` token under 4.5:1 on a surface, in any theme | none: no cell is allowed under its floor |

`[src: file: frontend/scripts/eslint-ui-guards.mjs:179#noLiteralUiString]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:72#UI_TEXT_ATTRS]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:109#COMPONENT_TEXT_PROPS]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:589#noAdhocAgentIdentity]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:579#jsxContainerOf]`
`[src: file: frontend/src/lib/__tests__/uiGuardsLint.test.ts:217]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:291#controlHasName]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:358#noAdhocDialog]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:392#noForeignPageStylesheet]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:457#noOffScaleStyle]`
`[src: file: frontend/src/styles/themeAudit.ts:431#RADIUS_PROP]`
`[src: file: frontend/src/styles/themeAudit.ts:570#checkBaseline]`
`[src: file: frontend/eslint.config.js:22]`

## How a baseline moves

- **ESLint**: a file may hold as many violations of a rule as
  `eslint-suppressions.json` records. One more fails `pnpm lint`. Once the count
  is exceeded, ESLint lists every violation of that rule in the file, the
  baselined ones too. One fewer also fails, and `pnpm lint --prune-suppressions`
  lowers the entry.
- **`pnpm lint:theme`**: the same contract, per file and kind
  (`font-size`, `radius`, `colour`, `breakpoint`), plus the list of contrast cells
  allowed under their floor (printed with `~` in the table).
  `node scripts/theme-audit.mjs --prune-baseline` lowers counts and drops cells
  that pass now. It never raises a count or adds a cell: adding debt is a
  hand-made, reviewed change.
- Test files (`*.test.*`, `test/`, `__tests__/`) are outside the ESLint guards:
  they render fixtures.

## What counts

- **Literal string:** text with a letter in it, unless every word is a path,
  URL, file name or key (it holds `/ . _ $ ~ = @ { }`), a kebab-case identifier
  or flag, an acronym or symbol of six characters or fewer with no lower-case
  letter (GET, PDF, MCP), a unit (`ms`, `px`, `tok`), or a product name
  (`PRODUCT_NAMES`). Text inside `<code>`, `<kbd>`, `<pre>`, `<samp>`, `<var>` is
  code.
  `[src: file: frontend/scripts/eslint-ui-guards.mjs:97#isUntranslatable]`
  KT-2016: the same test applies to every piece of literal text an expression
  renders, in a JSX child or in one of the attributes and props above: a string
  in braces, each branch of `? :`, the right of `&&`, both sides of `||` and `??`
  (one report per branch), and a template or a concatenation, whose literal
  parts are read together (`` `Delete ${name}` `` → « Delete »; `` `${m}m${s}s` ``
  is units only). The component props (`description`, `help`, `error`,
  `message`, `confirmLabel`, `content`) are read on a component, a name with a
  capital, not on a native element (`<meta content>`).
  `[src: file: frontend/scripts/eslint-ui-guards.mjs:132#literalPieces]`
- **Name of a control:** `aria-label`, `aria-labelledby` or `title` that is not
  empty; for a button, or a native element with `role="button"` (KT-2016), text
  among its children. An icon imported from `lucide-react`, or a component
  ending in `Icon`, `Glyph` or `Spinner`, is not text. An expression among the
  children is read (KT-2016): JSX in it is read like a child, each branch of
  `? :` and the right of `&&` must carry text for it to name the button
  (`{open ? <ChevronUp/> : <ChevronDown/>}` names nothing), and a variable or a
  property named `icon` or `…Icon` holds an icon; any other expression (a
  variable, a call) still counts as text. A component with `role="button"` is
  left alone: its props may name it.
  `[src: file: frontend/scripts/eslint-ui-guards.mjs:235#expressionCarriesText]`
  For a field: a wrapping `<label>`, being the direct child of `Field`, a
  `<label htmlFor>` with the same id in the file. A `placeholder` does not
  count, although HTML-AAM computes a name from it and axe's `label` rule
  accepts it: it disappears as soon as the user types, so it is not a label
  (WCAG 3.3.2). New code names its fields; the placeholder-only fields that
  exist are in the baseline. Spread props, `hidden`, `aria-hidden="true"`,
  `display: none` and the `hidden/submit/reset/button/image` input types are left
  alone.
- **Ad-hoc dialog:** `<dialog>` or `aria-modal` that is not a literal `false`,
  outside `src/components/ui/`. A non-modal popover with `role="dialog"`
  (`ContextHelp`, pickers) is a different pattern and is not reported.
- **Off scale:** a font size other than `var(--kr-type-*)`, the legacy
  `var(--kr-fs-*)` or a keyword (`inherit`, `1em`, `100%`…); a radius other than
  `0`, `50%` or `var(--kr-radius-*)`/`var(--kr-r-*)` (a pill is
  `--kr-radius-pill`); a hex colour, a colour function on numbers or a colour name
  (token fallbacks, `rgba(var(--kr-x-rgb), a)` and a fully transparent black
  aside), except in `tokens.css` and the theme layers (`atmosphere.css`,
  `neon.css`, `theme-effects.css`); a `@media (max-width: N px)` where N + 1 is not
  640/768/900, or a `min-width` that is not one of them (design-tokens.md:
  `<bp - 1>px`). A literal equal to a token's value (`font-size: 12px`) is still
  off scale: the token is not read.
  `[src: file: frontend/src/styles/themeAudit.ts:366#OffScaleKind]`
  `[src: file: frontend/src/styles/themeAudit.ts:412#withoutTokens]`
  KT-2016: the size inside a `font:` shorthand is a font size (the part before
  `/line-height`, or the first length, size keyword or type token; `font:
  inherit` and system fonts have none). A rule that holds nested rules is read
  with its own declarations, those after a nested rule included, and each
  nested rule on its own. A width in range syntax counts as its breakpoint:
  `width <= N` as `max-width: N` (N + 1), `width < N` and `width >= N` as N,
  `width > N` as N + 1, `N < width` turned around; an `@container` width is held
  to the same breakpoints as an `@media` one.
  `[src: file: frontend/src/styles/themeAudit.ts:478#fontShorthandSize]`
  `[src: file: frontend/src/styles/themeAudit.ts:82#cssRules]`
  `[src: file: frontend/src/styles/themeAudit.ts:457#queryWidths]`
  An inline style value behind a condition is read branch by branch (each
  branch of `? :`, the right of `&&`, both sides of `||`/`??`), a style object
  cast `as CSSProperties` too.
- **Ad-hoc agent identity (KT-1936):** the "name" is an element whose inline
  `style.color` is `agentTextColor(…)` or `agentColor(…)`; "next to" is its own
  content and its siblings, the other children of its nearest JSX parent
  (through `{cond && …}` and `{a ? … : …}`), searched to any depth, skipping
  `AgentIdentity`. Two hand-made headers were found when the siblings came in
  (2026-11-22) and migrated: the compare column header (`BatchComparePanel`,
  now `AgentIdentity` with its « recorded » / « unknown » provenance note
  below) and the message header (`MessageBubble`, whose agent colour sat on
  the whole header row, the footer's tier among its siblings: the colour now
  sits on the identity's line only).
  What it still cannot see:
  - a name coloured otherwise: a class (`.my-agent-name { color: … }`), a CSS
    variable set inline (`--agent-tone`), a colour from another helper
    (`externalAgentColor`, a user's mention colour read directly), or a name
    that is not coloured at all;
  - a model or a role further away than the parent: a sibling of the
    parent, another component the parent renders (`<ModelLine />` is not
    opened), a `.map()` callback's markup returned from a function defined
    elsewhere;
  - a model or a role that does not look like one: a variable not named
    `model`, `model_tier`, `currentModel`, `suffix` or `…Model` (`id`,
    `label`), a role written as literal text or through another key than
    `disc.target…` / `ui.agentIdentity.role.…`;
  - a string built for a `title` or an `aria-label` (« name · model »): the
    guard reads JSX children, not attributes.
  `[src: file: frontend/scripts/eslint-ui-guards.mjs:534#MODEL_NAME]`

## What the guards cannot see (written limits)

These are limits of a rule that reads one file's JSX or one stylesheet at a
time. They are not chased; review covers them.

- **A value held outside the JSX.** A string or a style object kept in a
  constant, a variable, a function's return or a `.map()` callback defined
  elsewhere (`const LABEL = 'Save'; … {LABEL}`, `const s = { fontSize: 11 };
  … style={s}`) is not read: the guards read literals written in the JSX
  expression itself.
- **An agent and its model written with no agent colour.** The identity rule
  keys on the colour (`agentTextColor()` / `agentColor()` in `style.color`),
  by design: a name written next to its model in another colour, or in none,
  is not reported (see « Ad-hoc agent identity » above).
- **A swap inside a file.** A baseline is a count per file and rule (or file and
  kind): fixing one baselined violation and adding a new one in the same file
  keeps the count, and nothing fails. Review reads the diff of every file the
  baselines list.
- **`components/ui`.** `no-adhoc-dialog` and `no-adhoc-agent-identity` are off
  in the whole folder, where `Dialog` and `AgentIdentity` are built.
- **A dialog without `aria-modal`.** `role="dialog"` or `role="alertdialog"`
  with no `aria-modal` is the documented inline pattern (popovers, inline
  confirmations), not a hole.

## KT-2016 — what the extensions added to the baselines (2026-11-23)

Each count was raised by hand to the exact number of reports the extension
adds, never by regenerating the file.

- `eslint-suppressions.json`, `no-literal-ui-string`: +58 in 17 files
  (literal branches, literals in braces, templates and concatenations, all of
  them hard-coded strings). `control-has-name`: +0: the two icon-only buttons
  the extension found (`GitPanel` branch creation, `PlanningProposalReview`
  reject confirmation) are named, and the three `role="button"` places
  (`AiDocViewer` tree rows, `ProjectCard` header) are named by their text.
  `no-off-scale-style`: +0.
- `themeAuditBaseline.json`: `font-size` +22 (`font:` shorthands with a px
  size), `breakpoint` +8 (`@container` widths); the nested rules of
  `styles/neon.css` add nothing.

## Measured before blocking (2026-11-22, base 428316a53)

Each guard was run on the whole of `frontend/src` and its hits reviewed. The
first-run false positives led to the exclusions above. The rate is what the guard
reports now that it blocks.

| Guard | First run | Now (baselined) | False positives now |
|---|---|---|---|
| no-literal-ui-string | 355 in 65 files | 223 in 40 files | ≈13 (≈6 %): commands and values (`aws`, `cd`, `null`, `string`/`number`/`boolean` options, two `cloudwatch …` and one `install …` argument lists) |
| control-has-name | 124 in 36 files | 110 in 34 files: 32 unnamed controls and 78 fields named only by a placeholder (the placeholder stopped counting after review) | 0 found in a sample of 28 of the 110 (Field children, hidden file inputs and one `aria-hidden` input were the first-run false positives). The two close buttons of `QuickPromptForm` and `QuickApiForm` are fixed, not baselined |
| no-adhoc-dialog | 109 in 59 files (one per attribute) | 40 elements in 37 files | 0: every hit is a hand-built modal (`aria-modal`); the non-modal popovers were removed from the rule |
| no-foreign-page-stylesheet | 36 in 35 files | 36 in 35 files | 0: 14 `DiscussionsPage.css` (the ticket's 14), 16 `SettingsPage.css`, 3 `WorkflowsPage.css`, 2 `Dashboard.css`, 1 `StandaloneLivePageMosaic.css` |
| no-off-scale-style | 138 in 27 files | 137 in 27 files | 0 found (a radius token with a px fallback was the one first-run false positive; computed template sizes are skipped) |
| Off-scale stylesheet values | 1078 (font-size 385, radius 325, colour 284, breakpoint 84) | 915 in 70 files (349, 325, 157, 84) | 0 in a sample of about 25 per kind. The first run read token fallbacks as font sizes and token names (`--kr-purple`) as colour names |
| Status colours in light themes | — | 58 cells under 4.5:1 (light 9, sakura 23, euronews 26), none in a dark theme | Not false positives: real misses, frozen until the colours are retuned. `--kr-bg-raised` is where most of them sit |
| components/ui warnings | 5 (DiffViewer 3, LogViewer 2) | 0 | — |

The light-theme status misses are why `StatusBadge` writes its label in
`--kr-text-primary` ([design-tokens.md](design-tokens.md), Contrast). Retuning
the colours is a design decision, outside this lot.

## Axe on open dialogs and forms

`frontend/e2e/specs/a11y-axe.spec.ts` opens one dialog and one form on each of
the seven routes and scans only the surface that opened (baseline key
`<Page> › <state>` in `e2e/fixtures/a11y-baseline.json`). On Pages, the import
dialog holds the route's only data-independent form, so it covers both. On
Planning and Settings, the dialog is the `?` help popover (`ContextHelp`,
`role="dialog"`).

Each state opens on a fresh backend, with no project, discussion or plugin (the
state CI runs on). On Plugins, that is why the spec opens the import dialog:
the export button only renders once a plugin is configured
`[src: file: frontend/src/components/plugins/PluginToolbar.tsx:112]`.

The principal measured the open surfaces on 2026-11-22 on a fresh sandbox
backend (`scripts/e2e-sandbox-backend.sh`). Two held existing serious debt,
frozen in the baseline. KT-1936 removed the second one, `Workflows › quick
prompt form` (color-contrast on `.kr-agent-switch-name`, the picker trigger's
agent name at 10 px in the brand colour): the trigger now renders
`AgentIdentity` at 12 px through `agentTextColor`, whose token holds 4.5:1 in
every theme. The one entry left:

| Surface | Rule | Nodes | Where | Leaves the baseline |
|---|---|---|---|---|
| Plugins › add plugin form | color-contrast | 12 | `.mcp-cat-pill-count` of the category pills, `.mcp-origin-badge.mcp-origin-official` of the registry cards, and others | the fix ticket the principal opens |
| Plugins › add plugin form | scrollable-region-focusable | 1 | `.mcp-registry-grid` | the same ticket |

The same scan of the quick prompt form also reported button-name (critical) on
the form's icon-only close button. It was fixed in the code instead
(`aria-label={t('common.close')}`, also on `QuickApiForm`), not baselined.
