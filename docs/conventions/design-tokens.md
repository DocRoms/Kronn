# Design tokens — the scales new screens pick from

KT-1878 (design-system lot 1). The screens move to these scales in the later
lots (KT-1880 to KT-1886); this lot only defines and shows them.

## Where they live

- **Values:** `frontend/src/styles/tokens.css`, `:root` block "Design scales"
  and "Semantic colours". Light themes (`light`, `sakura`, `euronews`) override
  the code and diff text colours in one shared block.
  `[src: file: frontend/src/styles/tokens.css:111-171]`
  `[src: file: frontend/src/styles/tokens.css:293-330]`
- **Typed table:** `frontend/src/styles/designTokens.ts` lists every token of
  a scale with the i18n key of its use line (`designSystem.use.<name>`).
  `[src: file: frontend/src/styles/designTokens.ts:46-128#DESIGN_TOKEN_GROUPS]`
- **In the app:** Settings → appearance panel → "Design system", a sub-page
  of its own since KT-1987 (`/config/design-system`), lazy-loaded, not in the
  section index. Its Tokens tab reads each value with `getComputedStyle` on
  `<html>` and re-reads it when `data-theme` changes. Since KT-1880 the page
  also shows the base components, in a second tab
  (`/config/design-system/components`), under one index of its headings: see
  [ui-components.md](ui-components.md).
  `[src: file: frontend/src/components/settings/DesignSystemTokens.tsx:63#DesignSystemTokens]`
  `[src: file: frontend/src/pages/DesignSystemPage.tsx:39#DesignSystemPage]`

## The scales

| Scale | Tokens | Note |
|---|---|---|
| Text | `--kr-type-sm` … `--kr-type-2xl` (12, 13, 14, 16, 20 px) | rem on a 16 px root; 0.75rem is the floor |
| Spacing | `--kr-space-1` … `--kr-space-8` | 4 px grid |
| Radius | `--kr-radius-control`, `-surface`, `-overlay` | 4, 8, 12 px; a pill stays `--kr-radius-pill` |
| Breakpoint | `--kr-bp-sm`, `-md`, `-lg` | 640, 768, 900 px (count below) |
| Duration | `--kr-duration-fast`, `-base`, `-slow` | 120, 150, 300 ms |
| Weight | `--kr-weight-regular` … `-bold` | 400–700, the faces `reset.css` loads |
| Control height | `--kr-control-h-sm`, `-md`, `-touch`, `-lg` | 28, 34, 40, 44 px, used as `min-height`; `-touch` (KT-1880) is the least size of a control on a coarse pointer |
| Overlay width | `--kr-overlay-w-sm`, `-md`, `-lg` | 480, 640, 960 px: the three sizes of `Dialog` and `Drawer` (KT-1881); `OVERLAY_WIDTHS` holds them for scripts |
| Text roles | `--kr-text-primary`, `-secondary`, `-muted` | existing tokens, distinct in every theme |
| Semantic colours | `--kr-status-*`, `--kr-diff-{added,removed}-{bg,border,text}`, `--kr-code-*` | no component names a theme |

## Rules

- **Adding a token to a scale:** add it to `tokens.css`, to the table in
  `designTokens.ts` and a `designSystem.use.*` line in the four locales.
  `designTokens.test.ts` fails if `tokens.css` declares a token matching a
  scale's prefix that the table omits, and the component test fails if the
  page does not show it.
  `[src: file: frontend/src/styles/__tests__/designTokens.test.ts:43]`
- **Breakpoints in `@media`:** a custom property cannot be read inside a
  media query. A breakpoint is the first width of the wider layout, as in
  `useIsMobile(bp)`: a stylesheet writes `@media (max-width: <bp - 1>px)`,
  a script uses `BREAKPOINTS`; `useIsMobile()` defaults to `BREAKPOINTS.md`.
  `[src: file: frontend/src/styles/designTokens.ts:137#BREAKPOINTS]`
  `[src: file: frontend/src/hooks/useMediaQuery.ts:5#useIsMobile]`
- **Why 640 / 768 / 900** (count of 2026-11-21, all `@media (max-width)`, no `min-width`; `useIsMobile` arguments): small widths — 640 px ×13 + `useIsMobile(641)` ×1, 560 ×11, 700 ×12, 720 ×6, 680 ×4; middle — 760 px ×15 and the `useIsMobile()` default 768 ×8 call sites + 767 px ×1; large — 900 px ×10 + `useIsMobile(900)` ×1, then 980/1050 ×2, `useIsMobile(1024/1100)` ×1.
  640 and 900 are the most used in their range. 760 and 768 are one threshold 8 px apart (24 uses); 768 is kept because the hook default can then read the token with no behaviour change, while 760 would leave CSS and JS split.
  This lot moves no screen: the 15 rules at 760 px and the 13 at 640 px move to `<bp - 1>px` with their screens (KT-1880 to KT-1886).
- **Legacy names stay valid.** `--kr-sp-*` and `--kr-r-*` that sit on a scale
  are mapped to it (`--kr-sp-4: var(--kr-space-2)`); the others, and every
  `--kr-fs-*` (px), keep their values. A test pins them.
- **Focus ring:** one global `*:focus-visible` rule, in `tokens.css`
  (`--kr-accent-ink` + halo). `reset.css` no longer has one; a test counts them.
  `[src: file: frontend/src/styles/__tests__/designTokens.test.ts:182]`
- **Contrast:** code and diff text colours hold 4.5:1 on `--kr-code-surface`
  and `--kr-bg-elevated` in every theme (diff text measured on its tinted row).
  The same 4.5:1 floor holds for the status colours used as text: a label on
  a `--kr-status-*` tint is measured on that tint over each opaque surface, in
  the seven themes. Several themes print `--kr-status-*` under 4.5:1 there, so
  `StatusBadge` writes its label in `--kr-text-primary` and leaves the status
  colour to the dot, the border and the tint
  `[src: file: frontend/src/components/ui/StatusBadge.test.tsx:40]`.
