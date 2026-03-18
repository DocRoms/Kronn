# AI context — Entry point

> **TEMPLATE FILE.** Every `{{...}}` MUST be filled by the AI audit before use.
> If you see an unfilled `{{...}}`, say `NOT_FOUND` and ask the user — **never guess or invent values**.

**Project:** {{PROJECT_NAME}} — {{STACK_SUMMARY}}.

**Working language:** {{PROJECT_LANGUAGE}} [ex: "French", "English" — language of code comments, commit messages, variable names; distinct from ai/ file language (always English) and agent response language (config.toml)]

> **Rules:** All `ai/` files in English. Never hallucinate — check docs, then ask user. Update `ai/` after learning something new.
> **MCP:** Before calling any MCP tool, read `ai/operations/mcp-servers/<name>.md` if it exists.

---

## 1. Context loading (mandatory)

**Tier 1 — Always:** `ai/index.md` (this file). Sufficient for trivial tasks.

**Common tasks — load exactly:**

| Task | Files |
|------|-------|
| [ex: "Backend API changes"] {{TASK_1}} | `ai/repo-map.md`, `ai/coding-rules.md` |
| [ex: "Fix a test"] {{TASK_2}} | `ai/testing-quality.md` |
| [ex: "New feature"] {{TASK_3}} | `ai/architecture/overview.md`, `ai/repo-map.md` |
| [ex: "Debug / deploy"] {{TASK_4}} | `ai/operations/debug-operations.md` |

**Tier 2 — Max 3 files if above doesn't cover:**

| Need | File |
|------|------|
| Repo structure | `ai/repo-map.md` |
| Testing | `ai/testing-quality.md` |
| Coding rules | `ai/coding-rules.md` |
| Known issues | `ai/inconsistencies-tech-debt.md` |
| Architecture decisions | `ai/decisions.md` |
| Glossary | `ai/glossary.md` |

**Tier 3:** Only if Tier 1+2 insufficient. State which file and why. Never load all files.

---

## 2. DO NOT (critical)

- {{DO_NOT_1}}
- {{DO_NOT_2}}
- **Guess** when info is missing — say `NOT_FOUND` and ask the user.
- **Invent file paths** — if you don't know where code goes, check `ai/repo-map.md` or ask.
- **Guess tool versions** — if prerequisites are not filled below, ask. Do not assume "Node 18" or "Python 3.10".
- **Guess languages or frameworks** — check § 6 Stack. Do not assume Express, Django, or Next.js.
- **Edit auto-generated files** — if a file is marked as generated (e.g., types exported from another language), never edit it by hand.
- **Load all Tier 2 files at once** — max 3, pick what you need.
- **Modify business code** when the task is only about AI context — edit `ai/` only.
- **Skip tests** — every code change requires tests. See § 4.

---

## 3. Prerequisites

<!-- Fill after audit. If empty, ask the user for build/run commands. -->
{{PREREQUISITES}}

---

## 4. Constraints

- If no command output: ask user to paste it.
- {{WORKFLOW_CONSTRAINT_1}}
- {{WORKFLOW_CONSTRAINT_2}}

### Testing rule (mandatory)

**Every code change MUST include tests.** No exceptions. Details and checklist: `ai/testing-quality.md`.

---

## 5. Source of truth

| What | File(s) |
|------|---------|
| AI context | `ai/` |
<!-- Fill after audit: data models, API routes, DB schema, config files -->
{{SOURCES}}

---

## 6. Stack

<!-- Fill after audit. DO NOT guess the stack — ask the user if empty. -->
{{STACK}}

---

## 7. Code placement

New code placement: see `ai/repo-map.md`.

---

## 8. Code generation

- Search repo for similar implementations first.
- Use `ai/repo-map.md` for file placement.
- Missing/ambiguous info → say `NOT_FOUND`, ask. Never guess.
- Large refactor needed → add entry to `ai/inconsistencies-tech-debt.md`.
- **Write tests for every change** — see § 4. No exceptions.
- After task: update `ai/` if you learned something non-obvious.

---

## 9. Multi-agent config

Redirectors: `CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `.kiro/steering/instructions.md`, `.vibe/instructions.md`, `.cursorrules`, `.cursor/rules/repo-instructions.mdc`, `.github/copilot-instructions.md`, `.windsurfrules`, `.clinerules`.

**Maintenance rule**: all content lives in `ai/`. Redirectors contain a summary of critical rules + pointer to `ai/index.md` as source of truth.

---

## 10. Last updated

AI context last reviewed: **{{DATE}}**.
