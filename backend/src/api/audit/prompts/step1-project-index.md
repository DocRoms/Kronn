Read README.md, package.json (or composer.json, Cargo.toml, go.mod), Makefile, Dockerfile, docker-compose.yml, CI configs (.github/workflows, .gitlab-ci.yml), and main config files.
Determine: tech stack, project structure, build/dev/test commands, key patterns, third-party services, CI/CD pipeline.

Then fill docs/AGENTS.md — replace ALL {{PLACEHOLDERS}} in each section:
- {{PROJECT_NAME}} and {{STACK_SUMMARY}}: project name and one-line stack description
- {{PROJECT_LANGUAGE}}: language used for code comments, commit messages, and identifiers
- Common tasks table: replace {{TASK_1}}..{{TASK_4}} with real task→file mappings (add/remove rows to match the project — see the general rule above)
- Prerequisites: replace the {{PREREQUISITES}} block with a table of Docker, language versions, build commands
- DO NOT rules: replace {{DO_NOT_1}}, {{DO_NOT_2}} with project-specific rules (add more rows if warranted)
- Source of truth: replace the {{SOURCES}} block with a table of key config files (models, routes, DB schema, types). Code placement (where new endpoints/pages/tests go) belongs in repo-map.md, NOT here — do not duplicate it
- Stack: replace the {{STACK}} block with a table of all major technologies, versions, and roles
- Workflow constraints: replace {{WORKFLOW_CONSTRAINT_1}}, {{WORKFLOW_CONSTRAINT_2}} with project-specific rules
- {{DOCS_LANGUAGE}}: language for new project documentation. If an existing doc is already written in another language, KEEP it as-is — do not force-translate it
- {{TICKET_LANGUAGE}}: language used for tickets
- {{COMMENT_TICKET_POLICY}}: whether code comments may cite a ticket/issue ID
- {{TEST_POLICY}}: this project's test policy (default: every code change needs a test — see docs/testing-quality.md)
- {{DATE}}: set to today's date (YYYY-MM-DD)

**CLEANUP ONCE FILLED** — the template ships with scaffolding that MUST NOT survive the fill: remove the `> **TEMPLATE FILE.** Every {{...}} MUST be filled…` banner block entirely, and strip every inline bracketed hint left next to a value (e.g. `[ex: "French", "English" — …]`). A filled file that still shows the banner or hints counts as a FAILED fill.

**ANTI-HALLU FOR THIS STEP** — every cell in the Stack / Source-of-truth / Code-placement / Prerequisites / Common-tasks tables MUST be anchored. Append a citation at the end of each cell value using the EXACT bracket form the mechanical linter recognizes: `[src: file: <path>:<line>]`. For the Stack table specifically: a version cell MUST cite the lockfile or the manifest line where the version is pinned (e.g. `Rust 1.78 [src: file: Cargo.toml:5]`, `Symfony 7.3.6 [src: file: composer.lock:2884]`). If a row cannot be anchored to a real file/line, do NOT add it — Quality > quantity. {{DATE}} is the only exception.