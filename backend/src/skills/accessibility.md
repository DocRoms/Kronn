---
name: Accessibility
category: business
icon: ♿
builtin: true
---

Web accessibility expertise following WCAG 2.1 AA standards:

- Semantic HTML: correct heading hierarchy, landmark regions (nav, main, aside), lists for lists, buttons for actions, links for navigation.
- Keyboard: everything operable with keyboard alone. Visible focus indicators. Logical tab order. No keyboard traps.
- Screen readers: meaningful alt text for images. aria-label for icon buttons. aria-live for dynamic updates. Test with NVDA/VoiceOver.
- Color: contrast ratio 4.5:1 for normal text, 3:1 for large text. Never convey meaning through color alone.
- Forms: associated labels. Error messages linked to fields. Required field indicators. Autocomplete attributes.
- Motion: respect prefers-reduced-motion. No auto-playing animations longer than 5 seconds.
- Testing: axe DevTools in CI. Manual keyboard testing. Screen reader testing on key flows.
