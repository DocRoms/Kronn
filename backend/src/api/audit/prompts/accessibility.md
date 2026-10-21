You are running a FOCUSED ACCESSIBILITY AUDIT (0.8.2). Output ONLY a11y findings, scoped to WCAG 2.1 AA where applicable.

# A. SEMANTIC HTML CHECKLIST (sample 5 representative templates/components)

- **Form inputs** have an associated `<label for=>` or wrap the input, OR `aria-label` / `aria-labelledby` is present. Placeholder-only is NOT a label.
- **Buttons vs links**: `<a>` used for navigation, `<button>` for actions. No `<div onClick=...>` masquerading as a clickable.
- **Headings**: exactly one `<h1>` per page; no jumps `<h1>` → `<h3>` (skipped levels).
- **Landmarks**: page uses `<header>`, `<nav>`, `<main>`, `<footer>` (or ARIA roles `banner`, `navigation`, `main`, `contentinfo`).
- **Lists**: groups of similar items use `<ul>` / `<ol>`, not stacks of `<div>`.
- **`<img alt=>`** present on every image; decorative images use `alt=""`. Sample 5+ images.
- **SVG**: `<svg>` that conveys meaning has `<title>` or `aria-label`; decorative SVGs have `aria-hidden="true"`.

# B. KEYBOARD & FOCUS CHECKLIST

- **`tabindex`**: no positive values (breaks natural tab order). `tabindex="-1"` only on programmatic focus targets.
- **Focus visible**: at least one global focus style. Grep for `outline: none` / `outline: 0` without a corresponding `:focus-visible` override.
- **Skip link**: a "Skip to main content" link exists (or filed as TD on a multi-section page).
- **Modals/popovers**: focus is trapped while open AND returns to the trigger on close. Read the modal component once.
- **Custom widgets**: dropdowns/tabs/accordions follow ARIA Authoring Practices (correct roles + keyboard support). Open one custom widget and inspect.

# C. COLOR & CONTRAST CHECKLIST

- **Text contrast**: foreground/background pairs in the theme tokens meet 4.5:1 for body, 3:1 for large text (18pt+ or 14pt bold). Read the design-token file (`tokens.css`, `theme.ts`, `_colors.scss`, etc.) and compute contrast for the 3-5 most common pairings.
- **Non-text contrast**: focus rings, form borders, icon-only buttons hit 3:1 against adjacent colors.
- **Color-only state**: error states have a non-color cue (icon, text, underline) — not just red.
- **Hover state**: not the ONLY way to discover an interactive element (touch + keyboard users have no hover).

# D. ARIA HYGIENE

- **Role conflicts**: no `<button role="link">`, no `<a role="button">`, etc. (use the right tag).
- **Live regions**: status updates use `aria-live="polite"` (or `assertive` for errors). Toast/notification components must announce.
- **`aria-hidden`** not applied to interactive elements (breaks screen readers).
- **Form errors** linked via `aria-describedby` to the input.

# E. MEDIA / DYNAMIC

- **Video**: tracks for captions / transcripts (or TD if absent).
- **Animations**: `prefers-reduced-motion` honored (CSS media query OR JS check).
- **Time limits**: auto-dismissing alerts can be paused/extended.

# F. ANTI-REPETITION (priors)

Read existing `docs/tech-debt/TD-*-{a11y,accessibility,aria,contrast,keyboard,...}*.md`. Match → reuse ID + APPEND `audit_history` entry. Slug churn is the #1 audit anti-pattern.

# G. OUTPUT FORMAT

1. Write `docs/inconsistencies-accessibility.md` with header `| ID | Problem | Area | Severity |`.
2. For each finding, write `docs/tech-debt/<ID>.md` with `**Category**: accessibility` and a `**WCAG**: <criterion>` line (e.g. `1.1.1 Non-text Content`).
3. **Status taxonomy**: `Verified in source` vs `Inferred`.
4. **Cap**: 25 findings max. Critical/High exempt.
5. Each finding: file path + line number, who is impacted (screen reader / keyboard-only / low-vision / cognitive), and one-sentence remediation.

Do NOT modify source. Do NOT touch other docs/ files.