---
name: accessibility
description: Use when building or reviewing UI components, forms, navigation, or any user-facing HTML. Ensures WCAG 2.1 AA compliance with semantic HTML, keyboard navigation, and screen reader support.
license: AGPL-3.0
category: business
icon: ♿
builtin: true
---

## Procedure

1. **Check semantic structure**: correct heading hierarchy (single H1), landmark regions (`nav`, `main`, `aside`), `<button>` for actions, `<a>` for navigation.
2. **Verify keyboard access**: tab through the entire flow. Confirm visible focus indicators, logical tab order, no keyboard traps. Every interactive element must be reachable.
3. **Add screen reader support**: `alt` on images, `aria-label` on icon-only buttons, `aria-live` for dynamic content updates.
4. **Validate color contrast**: 4.5:1 for normal text, 3:1 for large text. Never convey meaning through color alone.
5. **Wire up forms**: every input has an associated `<label>`. Error messages linked via `aria-describedby`. Required fields marked. `autocomplete` attributes set.
6. **Respect motion**: check `prefers-reduced-motion`. No auto-playing animations > 5 seconds.

## Gotchas

- `role="button"` on a `<div>` is NOT enough — you also need `tabindex="0"` and `keydown` handlers for Enter/Space. Just use `<button>`.
- `aria-label` overrides visible text for screen readers — don't use it on elements that already have visible text.
- `aria-live="assertive"` interrupts the user. Use `"polite"` unless it's an error or urgent alert.
- Lighthouse accessibility score misses ~60% of real issues. Always complement with manual keyboard + screen reader testing.

## Validation

Run axe-core or Lighthouse. Then manually: Tab through the page, activate every control with keyboard, verify with NVDA/VoiceOver on one key flow.

✓ `<button aria-label="Close dialog">X</button>` with visible focus ring
✗ `<div onclick="close()">X</div>` — no keyboard support, no role, no label
