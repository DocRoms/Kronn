# Architecture decisions (why, not what)

> **TEMPLATE FILE.** Filled during the final consolidation, after all specialized audits. Record a rationale only when documented or confirmed by the user.

This file captures **intentional choices** — patterns that look unusual but are deliberate. It prevents agents from "fixing" things that aren't broken.

It is the *positive* counterpart to `inconsistencies-tech-debt.md`: decisions the team made on purpose, NOT problems to fix.

## How to fill (final consolidation)

- **No minimum number of decisions.** If none has an evidenced rationale, say so. Never fill a quota.
- **Every row must cite the rationale**: an ADR, explicit project documentation, or user confirmation. Code proves implementation, not why it was chosen. An unknown rationale remains unknown; ask only when it affects the task.
- **Remove unused rows entirely** — if the project has only one real decision, delete the `{{DECISION_2}}` placeholder row instead of padding.

## Decisions

<!-- Fill during audit. Each row should be traceable to code evidence or user confirmation. -->
| Decision | Why chosen | What NOT to do | Source |
|----------|-----------|---------------|--------|
| {{DECISION_1}} | {{REASON}} | {{ANTI_PATTERN}} | {{FILE_OR_USER}} |
| {{DECISION_2}} | {{REASON}} | {{ANTI_PATTERN}} | {{FILE_OR_USER}} |
