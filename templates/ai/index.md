# AI context index — Single entry point

**Project:** {{PROJECT_NAME}} — {{STACK_SUMMARY}}.

> **All files under `ai/` are in English by default.** AI context documentation must be written in English.
> **ATTENTION — This is the reference file for all AI agents.**
> Read this file first, then follow the context loading strategy below.
> Do not read the other config files (.cursorrules, copilot-instructions, etc.) — they redirect here.

> **CRITICAL — Never hallucinate.**
> - **Never invent information** (tech stack, conventions, architecture, file paths...).
> - If you are unsure about something: **check the `ai/` documentation first**.
> - If you still don't find the answer: **ask the user** rather than guessing.
> - After getting the answer: **update the relevant `ai/` file** so the knowledge is captured.
> - Getting it right matters more than answering fast — hallucinations waste everyone's time.

> **CRITICAL — MCP tool usage.**
> Before calling any MCP tool, **read the matching context file** in `ai/operations/mcp-servers/<mcp-name>.md`.
> These files contain project-specific rules, constraints, and examples that prevent hallucinations and misuse.
> If no context file exists for an MCP you need to use, ask the user before proceeding.

**Unknown term?** → `ai/glossary.md` first.

This folder (`ai/`) contains AI-optimized project context (not human docs). Use paths relative to repo root.

---

## 1. Entry procedure (mandatory)

### Tiered context loading strategy

#### Tier 1 — Always read
- `ai/index.md` (this file)

**Trivial tasks** (typos, config tweaks, simple style fixes): Tier 1 may suffice.

#### Common tasks — load exactly these files

| Task | Files to load |
|------|---------------|
<!-- Fill with project-specific task→file mappings -->

#### Tier 2 — For needs not covered above (max 3 files)

| Need | File |
|------|------|
| repo structure / code placement | `ai/repo-map.md` |
| testing / quality | `ai/testing-quality.md` |
| coding rules | `ai/coding-rules.md` |
| known issues / tech debt | `ai/inconsistencies-tech-debt.md` |
| MCP servers / agent tools setup | `ai/operations/mcp-servers.md` |
| term definitions / project jargon | `ai/glossary.md` |
<!-- Add project-specific entries -->

#### Tier 3 — Escalation
Only if Tier 1 + 2 are insufficient: state which file you need and why, read it, or ask the user.
Never load everything "just in case".
- Architecture overview → `ai/architecture/overview.md`

---

## 2. Prerequisites before running commands

<!-- Project-specific prerequisites: Docker, env vars, build commands, etc. -->

---

## 3. DO NOT (common mistakes)

<!-- Project-specific "do not" rules. Common ones: -->
- Do **not** guess when information is missing — ask the user.
- Do **not** load all Tier 2 files at once — pick up to 3 max.
- Do **not** modify business code when the task is only about AI context — edit `ai/` only.

---

## 4. Workflow constraints

<!-- Project-specific workflow rules: Docker-first, quality gates, etc. -->
- **Quality is mandatory**: follow existing code style; add/update tests when changing behavior.
- If stdout/stderr is missing: ask the user to copy/paste the full output.

---

## 5. Source of truth

- AI context: `ai/`.
<!-- Add project-specific config files as source of truth -->

---

## 6. Code placement

Use `ai/repo-map.md` to decide.
<!-- Add default code placement rules -->

---

## 7. Code generation (critical behavior)

- Search the repo for similar implementations before writing.
- Use `ai/examples/*.md` instead of inventing new architecture.
- Use `ai/repo-map.md` to decide where code goes.
- If info is missing or ambiguous: ask questions; do not guess.
- If a "logical fix" requires a large/risky refactor: add an entry to `ai/inconsistencies-tech-debt.md`.

### AI context maintenance rule
After completing a task: if you discovered something non-obvious (a gotcha, a missing pattern, an outdated doc), update the relevant `ai/` file before closing. Keep entries factual and concise.

---

## 8. Stack (facts)

<!-- Fill with project stack details -->

---

## 9. Multi-agent configuration

Redirectors to this file: `.cursorrules`, `.cursor/rules/repo-instructions.mdc`,
`.github/copilot-instructions.md`, `CLAUDE.md`, `.windsurfrules`, `.clinerules`.

**Maintenance rule**: all content lives in `ai/`. Redirectors never need changes.

---

## 10. AI Exchanges (read on arrival)

- hasActualConversation: OFF
- currentConversation: none
- Template: `ai/templates/exchanges.md`

---

## 11. Last updated

AI context last reviewed: **{{DATE}}**.
