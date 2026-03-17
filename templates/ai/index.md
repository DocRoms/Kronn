# AI context — Entry point

**Project:** {{PROJECT_NAME}} — {{STACK_SUMMARY}}.

> **Rules:** All `ai/` files in English. Never hallucinate — check docs, then ask user. Update `ai/` after learning something new.
> **MCP:** If `ai/operations/mcp-servers/<name>.md` exists for an MCP you're about to use, read it first.

**Unknown term?** → `ai/glossary.md`.

---

## 1. Context loading (mandatory)

**Tier 1 — Always:** `ai/index.md` (this file). Sufficient for trivial tasks.

**Common tasks — load exactly:**

| Task | Files |
|------|-------|
| {{TASK_EXAMPLE_1}} | `ai/repo-map.md`, `ai/coding-rules.md` |
| {{TASK_EXAMPLE_2}} | `ai/testing-quality.md` |
| {{TASK_EXAMPLE_3}} | `ai/architecture/overview.md`, `ai/repo-map.md` |
| {{TASK_EXAMPLE_4}} | `ai/operations/debug-operations.md` |
| {{TASK_EXAMPLE_5}} | `ai/glossary.md`, `ai/architecture/overview.md` |

**Tier 2 — Max 3 files if above doesn't cover:**

| Need | File |
|------|------|
| Repo structure | `ai/repo-map.md` |
| Testing | `ai/testing-quality.md` |
| Coding rules | `ai/coding-rules.md` |
| Known issues | `ai/inconsistencies-tech-debt.md` |
| MCP setup | `ai/operations/mcp-servers.md` |
| Glossary | `ai/glossary.md` |

**Tier 3:** Only if Tier 1+2 insufficient. State which file and why. Never load all files.

---

## 2. Prerequisites

| Prerequisite | Command / Version | Notes |
|-------------|-------------------|-------|
| {{PREREQ_1}} | {{COMMAND_OR_VERSION}} | {{NOTES}} |
| {{PREREQ_2}} | {{COMMAND_OR_VERSION}} | {{NOTES}} |

---

## 3. DO NOT

- Guess when info is missing — ask the user.
- Load all Tier 2 files at once — max 3.
- Modify business code for AI context tasks — edit `ai/` only.
- {{DO_NOT_1}}
- {{DO_NOT_2}}
- {{DO_NOT_3}}

---

## 4. Constraints

- Quality: follow code style, add/update tests when changing behavior.
- If no command output: ask user to paste it.
- {{WORKFLOW_CONSTRAINT_1}}
- {{WORKFLOW_CONSTRAINT_2}}

---

## 5. Source of truth

| What | File(s) |
|------|---------|
| AI context | `ai/` |
| {{SOURCE_1}} | {{FILE_PATH}} |
| {{SOURCE_2}} | {{FILE_PATH}} |

---

## 6. Code placement

Use `ai/repo-map.md`. New code goes:

| Type | Location |
|------|----------|
| {{CODE_TYPE_1}} | {{LOCATION_1}} |
| {{CODE_TYPE_2}} | {{LOCATION_2}} |
| {{CODE_TYPE_3}} | {{LOCATION_3}} |

---

## 7. Code generation

- Search repo for similar implementations first.
- Use `ai/repo-map.md` for placement.
- Missing/ambiguous info → ask, don't guess.
- Large refactor needed → add to `ai/inconsistencies-tech-debt.md`.
- After task: update `ai/` if you learned something non-obvious.

---

## 8. Stack

| Technology | Version | Role |
|-----------|---------|------|
| {{TECH_1}} | {{VERSION}} | {{ROLE}} |
| {{TECH_2}} | {{VERSION}} | {{ROLE}} |
| {{TECH_3}} | {{VERSION}} | {{ROLE}} |

---

## 9. Multi-agent config

Redirectors: `CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `.kiro/steering/instructions.md`, `.cursorrules`, `.github/copilot-instructions.md`, `.windsurfrules`, `.clinerules`. All content lives in `ai/` — redirectors never need changes.

---

## 10. AI Exchanges

- hasActualConversation: OFF
- currentConversation: none
- Template: `ai/templates/exchanges.md`

---

## 11. Last updated

AI context last reviewed: **{{DATE}}**.
