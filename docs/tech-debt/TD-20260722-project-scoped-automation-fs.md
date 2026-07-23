# TD-20260722-project-scoped-automation-fs

- **ID**: TD-20260722-project-scoped-automation-fs
- **Area**: Backend / Skills / Workflows / MCP
- **Problem (fact)**:
  - Skills, profiles and directives remain DB-only. Their MCP list tools are
    intentionally compact, while full bodies are now readable through
    `skill_get` / `profile_get` / `directive_get` (quick win shipped
    2026-07-24). The remaining problem is the lack of a project filesystem
    source of truth and diffable cross-primitive dependency graph.
  - The same asymmetry exists conceptually for the other automation primitives: skills, workflows, Quick Prompts and Quick APIs each live only in the DB, are edited only through their own MCP tools, and have **no source-of-truth file** in the project repo. They are not versioned with the code they automate, not diffable in a PR, and not portable between machines/projects except via the whole-DB export.
  - Cross-primitive links are implicit only: a workflow Agent step references `skill_ids`, a step can be a QP or an ApiCall, but there is no first-class, declarable dependency graph a human can read in one place.
- **Why we can't fix now (constraint)**:
  - Touches the storage model of four primitives at once (skills, workflows, QPs, QAs) plus their MCP surfaces and the frontend editors. Needs a design ADR first (canonical source: repo file vs DB; conflict/merge semantics; secrets stay server-side).
  - Secrets must never land in repo files (Quick APIs carry `api_config_id` + injected auth). A file-backed representation has to reference configs by id, not value.
- **Impact**: dev friction | portability | reviewability (automation changes invisible in PRs)
- **Where (pointers)**:
  - MCP Agent-library reads: `skill_get`, `profile_get`, and `directive_get`
    in `backend/scripts/disc-introspection-mcp.py`.
  - Backend skill/workflow/QP/QA models + DB layer.
  - Whole-DB export already ships most of these (`DbExport`) — see `TD-20260626-export-residuals` for fidelity gaps.
- **Suggested direction (non-binding)**:
  - **Shipped quick win (2026-07-24)**: full MCP getters for skills, profiles
    and directives; `qp_get` and `workflow_get` already existed.
  - **Target vision (Romuald, 2026-07-22)**: project-scoped automation as a filesystem convention Kronn loads/syncs:
    - `/skills/` — project skills (markdown), loadable per project.
    - `/automation/workflows/` — workflow definitions.
    - `/automation/prompts/` — Quick Prompts.
    - `/automation/api/` — Quick API declarations (reference configs by id, never secrets).
    - Cross-references resolvable **by slug across folders**: a workflow YAML/JSON can name a skill, a QP step, and an API step and Kronn wires them. → automation versioned with the code, diffable in PRs, portable, self-documenting. "That would simplify everything."
  - Likely a two-way sync (DB ⇆ files) with the file as source of truth when present, mirroring the `docs/`-dir convention work (`TD-20260627-configurable-docs-dir`).
- **Next step**: create ticket — design ADR (file-vs-DB source of truth, sync direction, secret handling, cross-slug resolver) before any filesystem-backed implementation.
