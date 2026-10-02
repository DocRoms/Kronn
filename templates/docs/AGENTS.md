<!-- kronn:doc-version="1.0" -->
<!-- kronn:spec="https://github.com/DocRoms/Kronn/blob/main/docs/conventions/agents-md-format-v1.md" local="docs/conventions/agents-md-format-v1.md" -->
<!-- This file follows the Kronn AGENTS.md convention v1. Sections marked
     curated="ai" carry [src: …] provenance per assertion. Template v2 adds
     owner="audit" / owner="human"; human-owned sections are never rewritten
     by an audit. Legacy curated="human" sections receive the same protection. -->
# AI agent context — Entry point

> **TEMPLATE FILE.** Fill `{{...}}` from evidence. Unknown values: `NOT_FOUND`; never invent.

**Project:** {{PROJECT_NAME}} — {{STACK_SUMMARY}}.

## Project parameters

Set once during audit; apply to every `docs/` file.

- Working language: {{PROJECT_LANGUAGE}}.
- Documentation language: {{DOCS_LANGUAGE}}. This governs new documentation only; preserve every existing document in its current language.
- Ticket language: {{TICKET_LANGUAGE}}.
- Ticket IDs in code comments: {{COMMENT_TICKET_POLICY}}.
- Test policy: {{TEST_POLICY}}.

> **MCP:** Before calling any MCP tool, read [operations/mcp-servers/<name>.md](operations/mcp-servers/) if it exists.
> **Skills and Kronn resources index:** `kronn/INDEX.md` (no-op if the project has none published yet).

---

<!-- kronn:section name="anti-hallu" curated="ai" owner="audit" audit="{{DATE}}" -->
## 0. Anti-Hallucination Protocol

Never state a non-trivial technical fact (paths, APIs, config, versions, behaviour) without proof — read the code, then `docs/`, then official docs, then ask; never guess. Cite every assertion as `[src: file: <path>:<line>]` or `[src: url: <url>]`; a citation that doesn't resolve is rejected as fabricated.

Full grammar and cascade: [`docs/conventions/agents-md-format-v1.md`](conventions/agents-md-format-v1.md).
<!-- kronn:section:end -->

---

## 1. Context loading (mandatory)

**T0 — Router (always loaded):** [docs/AGENTS.md](AGENTS.md) (this file). Sufficient for trivial tasks.

**T1 — Routed context:** read the matching task’s relevant sections. Links are choices, not a reading list.

| Task | Files |
|------|-------|
| [ex: "Backend API changes"] {{TASK_1}} | [repo-map](repo-map.md), [coding-rules](coding-rules.md) |
| [ex: "Fix a test"] {{TASK_2}} | [testing-quality](testing-quality.md) |
| [ex: "New feature"] {{TASK_3}} | [architecture/overview](architecture/overview.md), [repo-map](repo-map.md) |
| [ex: "Debug / deploy"] {{TASK_4}} | [operations/debug-operations](operations/debug-operations.md) |

**T2 — Search:** open known paths directly; otherwise search `docs/`. Start with at most 3 relevant files/sections; expand when evidence requires it.

| Need | File |
|------|------|
| Repo structure | [repo-map](repo-map.md) |
| Testing | [testing-quality](testing-quality.md) |
| Coding rules | [coding-rules](coding-rules.md) |
| Known issues | [inconsistencies-tech-debt](inconsistencies-tech-debt.md) or a known TD directly |
| Known domain | Find `inconsistencies-<domain>.md` (security, api, etc.); open directly if present |
| Architecture decisions | [decisions](decisions.md) |
| Glossary | [glossary](glossary.md) |
| Workflow overview | [workflow/](workflow/README.md) |
| Commit conventions | [workflow/commits](workflow/commits.md) |
| Pull request conventions | [workflow/pull-requests](workflow/pull-requests.md) |
| Ticket/tracker conventions | [workflow/tickets](workflow/tickets.md) |
| CI/CD pipeline | [workflow/ci-cd](workflow/ci-cd.md) |
| Environments (staging, prod, …) | [environments](environments.md) |
| Worked examples | [examples/](examples/README.md) |

State the missing evidence before expanding. Never load every file or reread unchanged instructions already in context.

`docs/reports/` — [dated snapshots](reports/README.md), excluded from T0–T2 routing; read one only when a task names it.

---

## 2. DO NOT (critical)

- {{DO_NOT_1}}
- {{DO_NOT_2}}
- **Guess** when info is missing — say `NOT_FOUND` and ask the user.
- **Invent paths, versions or commands** — inspect [repo-map](repo-map.md) and manifests; ask if still unknown.
- **Edit auto-generated files** by hand.
- **Load all T2 files at once** — select the relevant sections, then expand as needed.
- **Modify business code** when the task is only about project documentation — edit `docs/` only.
- **Skip the test policy** — see § Project parameters above and § 4.

---

## 3. Prerequisites

Runtime/setup: [debug-operations](operations/debug-operations.md). Checks: [testing-quality](testing-quality.md).

---

## 4. Constraints

- If no command output: ask user to paste it.
- {{WORKFLOW_CONSTRAINT_1}}
- {{WORKFLOW_CONSTRAINT_2}}

### Test policy

See the Test policy parameter above (§ Project parameters). Checklist: [testing-quality](testing-quality.md).

---

## 5. Source of truth

| What | File(s) |
|------|---------|
| Project documentation | `docs/` |
| Cross-repo context (companion repos) | `docs/linked-repos.md` — read ONLY when your task references something not in this repo |

Source inventory: [repo-map](repo-map.md).

---

## 6. Stack

Versions and component roles: [repo-map](repo-map.md#stack-overview).

---

## 7. Code placement

New code placement: see [repo-map](repo-map.md).

---

## 8. Code generation

- Search repo for similar implementations first.
- Use [repo-map](repo-map.md) for file placement.
- Large refactor needed → add entry to [inconsistencies-tech-debt](inconsistencies-tech-debt.md).
- Update the relevant document after learning something non-obvious; keep one canonical home per fact.
- `docs/AGENTS.md` sections use `owner="audit"` or `owner="human"` (legacy `curated="human"` remains protected). Never rewrite a human-owned section in a full or partial audit; re-audits put the proposed change in a dated diff under `docs/reports/`.

---

## 9. Multi-agent config

Redirectors at the project root: `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, `.kiro/steering/instructions.md`, `.vibe/instructions.md`, `.cursorrules`, `.cursor/rules/repo-instructions.mdc`, `.github/copilot-instructions.md`, `.windsurfrules`, `.clinerules`.

**Maintenance rule**: all content lives in `docs/`. Redirectors are short stubs that point to [docs/AGENTS.md](AGENTS.md) as source of truth.

---

## 10. Last updated

Project documentation last reviewed: **{{DATE}}**.
