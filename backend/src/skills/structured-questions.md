---
name: structured-questions
description: Teaches agents to ask questions using {{var}}: format so the UI renders a structured form. Also teaches agents to understand var: value replies. Activate when you want clean Q&A exchanges instead of free-form prose.
license: AGPL-3.0
category: domain
icon: 📋
builtin: true
---

## Apply when

- You need to ask the user multiple questions at once
- You want structured, machine-parseable answers (not free-form text)
- The conversation involves a form-like exchange (configuration, preferences, specs, triage)

## Do NOT apply when

- You're having a free-form discussion or brainstorming
- You're delivering a report or analysis (no questions needed)
- The question is a simple yes/no that doesn't need a form

## How to ask questions

When you need information from the user, format EACH question on its own line using this exact syntax:

```
{{variable_name}}: Your question here?
```

Rules:
- `variable_name` must be ASCII alphanumeric + underscores only (no accents, no spaces). The parser uses the same `\w+` grammar as Quick Prompts `{{var}}` templates — `{{priorité}}` is rejected on purpose so the same renderer works on both sides.
- One question per line, each starting with `{{name}}:`
- **Keep the question text on the SAME line as `{{var}}:`** — the parser regex matches `[ \t]*` between the colon and the question (not `\s*`), so a newline between `{{var}}:` and the question body silently drops the entry from the form. If your question is long, write it as a single long line; the form input will wrap it visually.
- **Don't emit empty `{{var}}:` (no body)** — empty entries are skipped silently. If you find yourself writing `{{foo}}:` with no follow-up question text, you've lost a form field.
- Keep variable names short and descriptive: `priority`, `scope`, `deadline`, `language`
- You can add context paragraphs BEFORE the questions — only lines matching `{{var}}: text` become form fields. **First-occurrence wins for duplicate vars** — if you write `{{priority}}: …` twice, the second is ignored.
- The UI will render these as a structured form (`AgentQuestionForm` above the ChatInput) with labeled input fields the user fills directly. Submitting the form produces a single `var: value\n…` reply the parser already understands.

Example — good:

```
I need a few details before starting the analysis.

{{priority}}: What is the priority? (low / medium / high)
{{scope}}: Should this cover only the backend or the full stack?
{{deadline}}: Is there a deadline?
```

Example — bad (don't do this):

```
What is the priority? And also, what's the scope?
```

## How to read answers

The user's reply will come back as `variable_name: value` lines, one per field:

```
priority: high
scope: full-stack
deadline: Friday
```

When you receive this format:
- Parse each line as a key-value pair (split on first `:`)
- The keys match exactly the `{{variable_name}}` you asked
- Empty/missing keys mean the user skipped that question — don't assume a default, ask again if critical
- Proceed with the provided values as if the user had written a normal prose answer

## Validation

- Every multi-question turn uses `{{var}}: question` syntax (not bullet points, not numbered lists)
- Variable names are consistent across the conversation (don't rename `priority` to `prio` mid-thread)
- Answers in `var: value` format are parsed correctly without asking the user to repeat

✓ Agent asks `{{target_lang}}: What language should I translate to?` → user replies `target_lang: Spanish` → agent proceeds.
✗ Agent asks "What language?" in prose → user answers "Spanish" → agent has to guess which question it was for.
