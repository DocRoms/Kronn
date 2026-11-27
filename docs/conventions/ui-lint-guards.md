# UI lint guards (KT-1887)

The previous harmonisations of the UI drifted back because nothing checked the
result. These guards refuse **new** drift. The violations that existed when they
came in are frozen in a baseline, which may only shrink.

See also [design-tokens.md](design-tokens.md) for the scales and
[ui-components.md](ui-components.md) for the components the guards point to.

## The guards

| Guard | Where | What it refuses | Baseline |
|---|---|---|---|
| `ui-guards/no-literal-ui-string` | ESLint | A literal UI string in JSX: text, `aria-label`, `title`, `placeholder`, `alt`, `label` | `frontend/eslint-suppressions.json` |
| `ui-guards/control-has-name` | ESLint | An icon-only `<button>`, or an `<input>`/`<select>`/`<textarea>`, without an accessible name | same |
| `ui-guards/no-adhoc-dialog` | ESLint | A modal (`aria-modal`, `<dialog>`) built outside `components/ui` (use `Dialog`, `Drawer`, `ConfirmDialog`) | same |
| `ui-guards/no-foreign-page-stylesheet` | ESLint | A page or a component importing the stylesheet of another page (`src/pages/<Page>.css`) | same |
| `ui-guards/no-off-scale-style` | ESLint | An inline `fontSize`, radius or colour outside the tokens; a `useIsMobile`/`matchMedia` width outside `BREAKPOINTS` | same |
| `no-restricted-globals` / `-properties` (KT-1881) | ESLint | `window.confirm()` and `confirm()` | same |
| Off-scale stylesheet values | `pnpm lint:theme` | A `font-size`, radius, colour or `@media` width outside the scales, in a `.css` file | `frontend/src/styles/themeAuditBaseline.json` (counts per file and kind) |
| Status colours in every theme | `pnpm lint:theme` | A status colour (`--kr-success`…, `--kr-status-*`) under 4.5:1 on a surface, in any theme, light ones included | same file (`contrast` cells) |
| No warning in `components/ui` | ESLint | Every rule that warns elsewhere is an error under `src/components/ui/**` | none: the folder has no warning |

`[src: file: frontend/scripts/eslint-ui-guards.mjs:111]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:183]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:245]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:279]`
`[src: file: frontend/scripts/eslint-ui-guards.mjs:330]`
`[src: file: frontend/src/styles/themeAudit.ts:389]`
`[src: file: frontend/src/styles/themeAudit.ts:466]`
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
  `[src: file: frontend/scripts/eslint-ui-guards.mjs:95]`
- **Name of a control:** `aria-label`, `aria-labelledby` or `title` that is not
  empty; for a button, text or an expression among its children (an icon
  imported from `lucide-react`, or a component ending in `Icon`, is not text).
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
  `[src: file: frontend/src/styles/themeAudit.ts:321]`
  `[src: file: frontend/src/styles/themeAudit.ts:373]`

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
backend (`scripts/e2e-sandbox-backend.sh`). Two hold existing serious debt,
frozen in the baseline:

| Surface | Rule | Nodes | Where | Leaves the baseline |
|---|---|---|---|---|
| Plugins › add plugin form | color-contrast | 12 | `.mcp-cat-pill-count` of the category pills, `.mcp-origin-badge.mcp-origin-official` of the registry cards, and others | the fix ticket the principal opens |
| Plugins › add plugin form | scrollable-region-focusable | 1 | `.mcp-registry-grid` | the same ticket |
| Workflows › quick prompt form | color-contrast | 1 | `.kr-agent-switch-name`: the agent picker trigger writes its name at 10 px (`--kr-fs-xs`) in the agent colour | KT-1936, which replaces that trigger markup |

The same scan of the quick prompt form also reported button-name (critical) on
the form's icon-only close button. It was fixed in the code instead
(`aria-label={t('common.close')}`, also on `QuickApiForm`), not baselined.
