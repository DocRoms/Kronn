// AI Audit pipeline split into one file per concern
// (TD-20260417-audit-monolith). The big static prompt definitions
// (`PROMPT_PREAMBLE`, `ANALYSIS_STEPS`, `AUDIT_REDIRECTOR_FILES`)
// live here because every sub-module reads from them, and they are
// the single source of truth for what "the audit" actually does.
// Sub-modules are re-exported via `pub use *::*` so every existing
// `api::audit::Foo` call site keeps resolving without edits.

use std::convert::Infallible;
use std::pin::Pin;

use axum::response::sse::Event;
use futures::Stream;

pub mod briefing;
pub mod drift;
pub mod full;
pub mod helpers;
pub mod info;
pub mod reconciliation;
pub mod run;
pub mod validate;

pub use briefing::*;
pub use drift::*;
pub use full::*;
pub use info::*;
pub use run::*;
pub use validate::*;
// Selective re-export of `pub(crate)` helpers actually consumed from
// outside this module — sibling `api::projects::*` calls these via
// `crate::api::audit::Foo`. The remaining `pub(crate)` helpers
// (`build_validation_prompt`, `build_briefing_prompt`) stay
// `super::helpers::name`-reachable for sub-modules without leaking.
pub(crate) use helpers::{check_ai_dir_permissions, detect_project_skills};

pub(super) type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

pub(crate) const PROMPT_PREAMBLE: &str = "\
Rules: Write in English. Be factual and concise — this is AI context for coding agents, NOT human documentation.\n\
- Do NOT invent information — mark unknowns with `<!-- TODO: verify -->`.\n\
- Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content. {{PLACEHOLDERS}} are literal text markers — replace by editing file content directly.\n\
- Keep the existing file structure and section headings — fill in the blanks, do NOT rewrite the file from scratch.\n\
- If a section does not apply to this project, replace placeholders with 'N/A — not used in this project.' Do not delete the section.\n\
- Write plain facts, not opinions or recommendations. No debate, no trade-offs analysis.\n\
- Each section should be self-contained: another AI agent reading just that section should get the full picture.\n\
- Add or remove table rows as needed to match the project. Write fewer entries rather than inventing content to fill slots.\n\
This is an autonomous (non-interactive) pass. Do NOT ask questions — mark unknowns with `<!-- TODO: verify -->` and move on.";

pub(crate) struct AnalysisStep {
    pub(crate) target_file: &'static str,
    pub(crate) prompt: &'static str,
    /// Source file patterns to hash for drift detection.
    /// Special values: "__GIT_HEAD__" (git commit hash), "__GIT_LS_FILES__" (directory structure).
    /// Glob patterns: "*.json", ".github/workflows/*"
    /// Empty = step is excluded from drift detection.
    pub(crate) sources: &'static [&'static str],
}

pub(crate) const ANALYSIS_STEPS: &[AnalysisStep] = &[
    // Step 1: Project analysis + index
    AnalysisStep {
        target_file: "docs/AGENTS.md",
        prompt: "\
Read README.md, package.json (or composer.json, Cargo.toml, go.mod), Makefile, Dockerfile, docker-compose.yml, \
CI configs (.github/workflows, .gitlab-ci.yml), and main config files.\n\
Determine: tech stack, project structure, build/dev/test commands, key patterns, third-party services, CI/CD pipeline.\n\n\
Then fill docs/AGENTS.md — replace ALL {{PLACEHOLDERS}} in each section:\n\
- {{PROJECT_NAME}} and {{STACK_SUMMARY}}: project name and one-line stack description\n\
- Common tasks table: replace {{TASK_EXAMPLE_*}} with 5-7 real task→file mappings\n\
- Prerequisites table: replace {{PREREQ_*}} with Docker, language versions, build commands\n\
- DO NOT rules: replace {{DO_NOT_*}} with 3+ project-specific rules\n\
- Source of truth table: replace {{SOURCE_*}} with key config files (models, routes, DB schema, types)\n\
- Code placement table: replace {{CODE_TYPE_*}} with where to put new endpoints, pages, tests\n\
- Stack table: replace {{TECH_*}} with all major technologies, versions, and roles\n\
- Workflow constraints: replace {{WORKFLOW_CONSTRAINT_*}} with project-specific rules\n\
- {{DATE}}: set to today's date (YYYY-MM-DD)",
        sources: &["README.md", "package.json", "Cargo.toml", "composer.json", "go.mod", "docker-compose.yml", "Makefile", "Dockerfile"],
    },

    // Step 2: Glossary (early — defines vocabulary for subsequent steps)
    AnalysisStep {
        target_file: "docs/glossary.md",
        prompt: "\
Read docs/AGENTS.md. Search codebase for domain terms, abbreviations, naming conventions.\n\n\
Fill docs/glossary.md — replace ALL {{PLACEHOLDERS}}:\n\
- Categorize: Architecture, Domain, Environments, External, Abbreviations\n\
- Each term: one-line definition + optional docs/ reference\n\
- Unknown domain terms: add `<!-- TODO: ask user -->` after the definition\n\
- Cover: framework terms, model names, services, acronyms in code",
        sources: &[],
    },

    // Step 3: Repo map
    AnalysisStep {
        target_file: "docs/repo-map.md",
        prompt: "\
Read docs/AGENTS.md and docs/glossary.md for context. Explore the directory structure (2-3 levels deep).\n\n\
Fill docs/repo-map.md — replace ALL {{PLACEHOLDERS}}:\n\
- {{STACK_OVERVIEW}}: one paragraph summarizing the architecture\n\
- Key folders tree: replace {{FOLDER_*}} with every major directory (2-3 levels deep), tree format with annotations\n\
- Entrypoints table: replace {{ENTRYPOINT_*}} with 5-7 key files (config, routes, models, etc.)\n\
- Auto-generated files: replace {{FILE_PATTERN}} with files NOT to edit manually",
        sources: &["__GIT_LS_FILES__"],
    },

    // Step 4: Coding rules
    AnalysisStep {
        target_file: "docs/coding-rules.md",
        prompt: "\
Read docs/AGENTS.md for context. Find ALL linter, formatter, and type-checker configs in the repo \
(e.g. .eslintrc, eslint.config.js, prettier, rustfmt.toml, tsconfig.json, phpcs.xml, etc.).\n\n\
Fill docs/coding-rules.md — replace ALL {{PLACEHOLDERS}}:\n\
- Replace {{LANGUAGE_*}} with one section per language/framework used in the project\n\
- For each language, fill the Tools table: {{CONFIG}} and {{COMMAND}} for linter, formatter, type checker\n\
- Replace {{CONVENTION_*}} with coding conventions OBSERVED in existing code (naming, error handling, imports). Write fewer rather than inventing.\n\
- Replace {{MISTAKE_*}} with common mistakes to avoid (from linter configs, framework gotchas observed in code)\n\
- If no linter/formatter is configured, write 'Not configured' in the Config column\n\
- Add or remove language sections as needed to match the actual project stack",
        sources: &["package.json", "Cargo.toml", "tsconfig.json", "rustfmt.toml", "pyproject.toml"],
    },

    // Step 5: Testing & quality
    AnalysisStep {
        target_file: "docs/testing-quality.md",
        prompt: "\
Read docs/AGENTS.md for context. Find test framework configs (jest, vitest, phpunit, pytest, cargo test, bats, etc.) \
and CI quality gates.\n\n\
Fill docs/testing-quality.md — replace ALL {{PLACEHOLDERS}}:\n\
- Build & quality checks table: replace {{CHECK_*}} and {{COMMAND}} with all quality checks (compile, lint, format, test, build)\n\
- Test infrastructure table: replace {{LANG_*}}, {{RUNNER}}, {{CONFIG}} for each language\n\
- Test suites table: replace {{SUITE_*}} with test files/suites and approximate counts\n\
- Coverage: replace {{COVERAGE_STATUS}} and {{COVERAGE_TARGET}} with current status and targets\n\
- Replace {{UNTESTED_*}} with components that have NO tests\n\
- Fast smoke checks table: replace {{COMMAND_*}} with 3-5 quick pre-commit commands",
        sources: &["package.json", "Cargo.toml", "pyproject.toml"],
    },

    // Step 6: Architecture overview
    AnalysisStep {
        target_file: "docs/architecture/overview.md",
        prompt: "\
Read docs/AGENTS.md and docs/repo-map.md for context. Analyze the high-level architecture.\n\n\
Fill docs/architecture/overview.md — replace ALL {{PLACEHOLDERS}}:\n\
- Apps/services table: replace {{SERVICE_*}}, {{PORT}}, {{TECH}}, {{ROLE}} for each service\n\
- Key patterns: replace {{PATTERN_*_NAME}} and {{PATTERN_*_DESCRIPTION}} with 3-5 architectural patterns \
  (API pattern, state management, auth, data flow, caching, etc.) — 2-3 sentences each\n\
- {{SEPARATION_DESCRIPTION}}: how the codebase is organized (by feature, by layer, etc.)\n\
- Data flow: replace {{DATA_FLOW_DIAGRAM}} with ASCII flow diagram and {{DATA_FLOW_DESCRIPTION}}\n\
- Legacy table: replace {{AREA}}, {{CURRENT}}, {{TARGET}} for any legacy patterns or planned migrations",
        sources: &["docker-compose.yml", "src/main.*", "src/lib.*", "src/index.*"],
    },

    // Step 7: Debug operations
    AnalysisStep {
        target_file: "docs/operations/debug-operations.md",
        prompt: "\
Read docs/AGENTS.md for context. Find operational commands from Makefile, package.json scripts, \
docker-compose commands, and any run/build/debug procedures.\n\n\
Fill docs/operations/debug-operations.md — replace ALL {{PLACEHOLDERS}}:\n\
- Common commands table: replace {{ACTION_*}} and {{COMMAND_*}} for start, stop, logs, test, build, deploy\n\
- Docker services table: replace {{SERVICE_*}}, {{PORT}}, {{ROLE}}, {{HEALTH}} for each container\n\
- Troubleshooting: replace {{ISSUE_*_TITLE}}, {{SYMPTOM}}, {{CAUSE}}, {{FIX}} with 3-5 common issues",
        sources: &["docker-compose.yml", "Makefile", "Dockerfile"],
    },

    // Step 8: MCP servers overview + capability introspection
    AnalysisStep {
        target_file: "docs/operations/mcp-servers.md",
        prompt: "\
Read docs/AGENTS.md for context. Check if .mcp.json or .mcp.json.example or .env.mcp.example exists in the repo.\n\n\
If MCP config exists:\n\n\
### A) Document each server in docs/operations/mcp-servers.md\n\
Fill the table with: name, package, purpose, key capabilities (3-5 main tools), credentials.\n\n\
### B) Introspect capabilities\n\
For each MCP server that has tools available, use the MCP tools to discover:\n\
1. **Tool inventory**: list the main tools exposed (via tools/list if available). \
For each tool: name, one-line description, whether it is read-only or mutating, and a use-case.\n\
2. **Project context**: call read-only tools to discover real project data. Examples:\n\
   - Jira/Linear: project keys, open ticket count, labels in use, sprint cadence\n\
   - GitHub/GitLab: repo names, open PRs count, branch strategy, CI status\n\
   - Slack: channel names relevant to the project\n\
   - CloudWatch: log groups, active alarms\n\
   - Confluence: spaces, recent pages\n\
   Only call read-only/list operations — never create, update, or delete anything.\n\
   If a tool call fails or credentials are missing, note it and move on.\n\n\
### C) Create per-MCP context files\n\
For each server where you discovered capabilities or project context, \
create docs/operations/mcp-servers/<slug>.md following the TEMPLATE.md structure:\n\
- Fill the Capabilities table with discovered tools\n\
- Fill Project context with real identifiers found via introspection\n\
- Add rules and gotchas based on what you observed\n\
Do NOT create empty or boilerplate context files — only if you have real data.\n\n\
### D) Fill the workflow automation hints table\n\
Based on the MCP combinations present, suggest 2-5 possible automated workflows.\n\
For each: which MCPs are combined, what the workflow does, and who benefits (Dev/PM/Ops).\n\
Only suggest workflows where all required MCPs are actually configured.\n\n\
If no MCP config exists: replace docs/operations/mcp-servers.md content with:\n\
'# MCP Servers\\n\\nNo MCP servers configured for this project.'",
        sources: &[".mcp.json"],
    },

    // Step 9: Inconsistencies & tech debt
    //
    // 0.8.2 — Step 9 is the load-bearing step of the audit. It must be
    // intransigent on baseline issues (the 0.8.1 regression on DOCROMS_WEB
    // missed 5 well-known Docker/CI security defaults because the agent
    // prioritized novelty over baseline coverage). The prompt below is
    // organized as:
    //   A. Mandatory baseline checklist (Docker/compose/deploy/CI) — NEVER
    //      skipped, NEVER counts against the finding cap.
    //   B. Dimensional scan (10 dimensions, free-form).
    //   C. Anti-repetition rules (read existing TDs as priors, reuse IDs).
    //   D. Output format (severity calibration, two-tier Status, detail
    //      file schema with audit_history YAML frontmatter).
    //   E. Reconciliation hint (list dropped TDs for the pipeline to
    //      handle in a separate reconciliation pass).
    AnalysisStep {
        target_file: "docs/inconsistencies-tech-debt.md",
        prompt: "\
Real issues only, not hypothetical. Read all docs/ files AND scan source code. \
Scan: entry points, config files, Dockerfiles, CI configs, and 5-10 core source files \
(prioritize auth, data persistence, external input handling).\n\n\

# A. MANDATORY BASELINE CHECKLIST (never skip, never trim)\n\
\n\
These are well-known defaults that silently break security/ops in production. \
Each item that is NOT satisfied MUST produce a TD finding — these do NOT count \
against the cap (see § D point 5). Be explicit when an item is satisfied (\"verified \
present\", \"verified absent\") so the user can trust the audit.\n\n\
\n\
**If a `Dockerfile` exists** (in any subdirectory of the project root):\n\
- `USER <non-root>` directive present (otherwise: container runs as root — emit TD).\n\
- `display_errors = Off` for prod env (otherwise: stack traces leak in HTTP responses — emit TD).\n\
- `opcache.enable = 1` for PHP projects (otherwise: every request re-compiles — perf disaster — emit TD).\n\
- `HEALTHCHECK` directive present (otherwise: container orchestrator can't detect zombie processes — emit TD).\n\
- `apt-get install` uses `--no-install-recommends` AND `rm -rf /var/lib/apt/lists/*` (otherwise: image bloat + outdated packages — emit TD if either missing).\n\
- Base image has a tag pinned to a specific digest or version (not `latest` — otherwise: non-reproducible builds — emit TD).\n\
- No `ADD <remote-url>` (only `RUN curl … && verify-checksum`; otherwise: silent supply-chain risk — emit TD).\n\
- Secrets are NOT passed via `ARG` (build args are visible in `docker history` — emit TD if found).\n\n\
\n\
**If a `docker-compose.yml` or `compose.yaml` exists**:\n\
- `mem_limit` (or `deploy.resources.limits.memory`) set on every service (otherwise: runaway process risk — emit TD).\n\
- `cpus` (or `deploy.resources.limits.cpus`) set on every service (same rationale).\n\
- `read_only: true` where the service is stateless (otherwise: writable rootfs is a useful RCE escalation surface — emit TD only if clearly stateless).\n\
- No `:latest` image tags (otherwise: non-reproducible — emit TD).\n\
- No secrets in `environment:` block (use `secrets:` or `env_file:` — emit TD if literal credentials found).\n\n\
\n\
**If `.github/workflows/*.y(a)ml` OR `.gitlab-ci.yml` OR equivalent CI config exists**:\n\
- No `StrictHostKeyChecking=no` in ssh/scp calls (otherwise: trivial MitM — emit TD).\n\
- Deploy workflow has a quality gate (lint OR test OR static analysis) before pushing to prod (otherwise: broken code can ship — emit TD).\n\
- Secrets are sourced from `${{ secrets.* }}` (or equivalent), not hardcoded (emit TD if literal credentials found).\n\
- No `pull_request_target` without explicit checkout of the base branch's workflow definition (RCE risk — emit TD if pattern found).\n\
- Pinned action versions (`uses: actions/checkout@v4` not `@master`) (emit TD only if drift is dramatic, e.g. pinning to a moving branch).\n\n\
\n\
**If `.env*` files exist at repo root**:\n\
- No `.env` file tracked in git (only `.env.dist` / `.env.example`) — check `git ls-files .env*` (otherwise: emit TD).\n\
- No real-looking secrets (32+ hex chars, base64-like strings ≥ 24 chars, vendor-prefixed tokens like `sk-`, `xoxb-`, `p8e-`, `AIzaSy…`) inside ANY tracked `.env*` file (emit TD if found, this is almost always a leak).\n\
- Deploy step EXCLUDES `.env*` from the deploy bundle (rsync/scp/tar `--exclude .env*`) — otherwise the tracked `.env.dist` lands in production with its hardcoded defaults (emit TD if not excluded).\n\n\
\n\
**For any web project** (HTML templates detected — Twig / Blade / JSX / Vue / ERB / Razor — > 3 template files):\n\
- Form `<input>` elements have associated `<label>` or `aria-label` (sample 3-5 templates; emit TD if pattern of missing labels).\n\
- `<img>` elements have `alt=` attribute (sample; emit TD if pattern of missing alt).\n\
- No inline `<script>` or `<style>` longer than ~50 lines per template (CSP risk + Asset Mapper bypass — emit TD if pattern).\n\
- Content-Security-Policy header set (check controllers/middleware/web server config — emit TD if absent on a project that touches user input).\n\n\
\n\
**Community standards** — gated on OSS intent. Run this block ONLY if at least one of these holds: \
(1) a `LICENSE` / `LICENSE.md` / `LICENSE.txt` file exists at repo root, \
(2) `git remote -v` (or the `.git/config` you can read) points to a public host (`github.com`, `gitlab.com`, `codeberg.org`, `git.sr.ht`), \
(3) the `README` body mentions \"contribute\", \"contribution\", \"contributing\", \"open source\", \"OSS\", or similar. \
Otherwise this entire block is skipped (private/internal projects do not need community-standards scaffolding). \
For OSS-intent projects, each missing item below emits a TD at **Low or Medium** severity — these are project-health, not security:\n\
- `LICENSE` (or `LICENSE.md`/`LICENSE.txt`) file at repo root with a recognized license body (MIT / Apache-2.0 / GPL-3.0 / BSD / MPL-2.0 / Unlicense). Without one, downstream users have no legal permission to use the code (emit TD: Medium).\n\
- `README.md` has a one-paragraph description after the title (not just `# Project Name` followed by sections). GitHub shows it next to the repo name in search (emit TD: Low if README is title-only).\n\
- `.github/ISSUE_TEMPLATE/*.md` directory exists with at least one template, OR a top-level `.github/issue_template.md`. Without one, every issue is ad-hoc structure and audits like this one push unstructured tickets (emit TD: Low).\n\
- `.github/pull_request_template.md` (or `PULL_REQUEST_TEMPLATE.md`) present. Without it PR descriptions drift across contributors (emit TD: Low).\n\
- `SECURITY.md` (or `.github/SECURITY.md`) — tells researchers how to report a vulnerability responsibly (emit TD: Medium — silence here pushes researchers to public disclosure).\n\
- `CONTRIBUTING.md` (or `.github/CONTRIBUTING.md`) — onboarding for external contributors (emit TD: Low — informational).\n\
- `CODE_OF_CONDUCT.md` (or `.github/CODE_OF_CONDUCT.md`) — required by GitHub Community Standards checklist (emit TD: Low).\n\n\
\n\
The 6 categories above are MANDATORY when their gating condition matches. Even if you find 30 other findings, you MUST scan them. \
The cap in § D point 5 applies AFTER these baseline findings are emitted.\n\n\

# B. DIMENSIONAL SCAN (free-form, on top of the baseline)\n\
\n\
Systematically audit across these 10 dimensions:\n\
- **Dependencies**: EOL/deprecated runtimes, frameworks, packages, or versions significantly behind stable.\n\
- **Security**: hardcoded secrets (regex: API keys, tokens, OAuth client_secret), missing auth checks, injection vectors (SQL/XSS/command), insecure defaults, exposed debug endpoints, default credentials.\n\
- **Code quality**: functions >50 lines, god classes, SRP violations, dead code, error swallowing (empty catch / let _ = / unwrap_or_default on Result).\n\
- **Scalability**: N+1 queries, unbounded loops, missing pagination, memory leaks, missing indexes on hot queries.\n\
- **Maintainability**: tight coupling, circular dependencies, missing tests for critical paths, unclear naming, mixed languages in comments/strings.\n\
- **Accessibility (a11y)**: covered by baseline if web project; on libraries/CLIs surface API a11y (machine-readable output, --quiet flag for scripts, etc.).\n\
- **Observability**: missing logging in hot paths (auth, payment, write endpoints), no error tracker (Sentry/Glitchtip/equivalent), no health/readiness endpoints, no metrics on critical SLI.\n\
- **Compliance**: GDPR issues (external resources, data retention), license incompatibilities (`composer licenses` / `cargo deny` / `license-checker`).\n\
- **Performance** (if perf-sensitive per `docs/briefing.md` or repo README): CWV regression risk, bundle size, image optim missing, cache headers weak, no CDN.\n\
- **Documentation drift**: cross-check the docs/ files you just wrote (steps 1-8) against the source code. Flag concrete contradictions (e.g. `coding-rules.md` says X is enforced but no linter rule exists, `testing-quality.md` claims N tests but actual count differs, `mcp-servers.md` lists a server not in `.mcp.json`).\n\n\

# C. ANTI-REPETITION (priors from previous audits)\n\
\n\
Before emitting findings, list existing files under `docs/tech-debt/` (excluding README.md, TEMPLATE.md, _template.md, and any file starting with `_`). \
These are TDs from previous audits. For each finding you would emit:\n\
\n\
1. **Same root cause as an existing TD** (same file + same anti-pattern, regardless of date suffix in slug):\n\
   → REUSE the existing TD ID. UPDATE the detail file in place (refresh `Where` pointers if line numbers shifted, add a new entry to the `audit_history` YAML block — see § D format).\n\
   → Do NOT create a new `TD-YYYYMMDD-` file with a different slug for the same root cause. The user already saw and possibly confirmed the existing one.\n\
\n\
2. **Similar but refined** (same area, more precise scope or new evidence):\n\
   → REUSE the existing TD ID but update the title/description. Note `\"refined from previous audit\"` in the audit_history entry.\n\
\n\
3. **Genuinely new** (different root cause, different files):\n\
   → Create a new `TD-YYYYMMDD-slug.md` file with today's date.\n\
\n\
4. **Previously-existing TDs you did NOT re-emit** (because the problem is gone or not visible):\n\
   → LIST them in your output as a separate `## Reconciliation candidates` markdown section. \
For each: TD ID + your best guess (`fixed in commit X`, `no longer visible in source`, `out of scope this audit`, `uncertain`). \
The pipeline will run a reconciliation pass after Step 9 to classify them definitively.\n\
\n\
This rule is critical: the user wiped TDs once to evaluate a fresh audit, but most users don't. Re-discovery under a new slug breaks their workflow.\n\n\

# D. OUTPUT FORMAT\n\
\n\
Fill `docs/inconsistencies-tech-debt.md` — replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comments. For every finding:\n\
\n\
1. **Outdated prerequisites table** in the index file: flag EOL/deprecated/behind-stable runtimes, frameworks, packages with the actual reality column.\n\
\n\
2. **For each issue**: (a) create or update the TD detail file at `docs/tech-debt/TD-<date>-<slug>.md` first, then (b) add a one-line entry to the `Current list` table in the index. Do both or neither. For UPDATES of existing TDs (per § C), keep the original file name and date.\n\
\n\
3. **Severity calibration** (use concrete examples to avoid over-classification as Medium):\n\
   - **Critical**: production down, data leak, exploitable flaw — e.g. hardcoded prod API key in repo, SQL injection in a public endpoint, secret in a logged response.\n\
   - **High**: blocks release or a supported environment — e.g. test suite red on main, build fails on the documented version, auth bypass under specific conditions, baseline checklist failures touching prod safety (display_errors=On in prod, root container).\n\
   - **Medium**: daily dev friction or measurable perf hit — e.g. test suite >30s, N+1 in the dashboard page, manual repetitive setup, broken hot reload, missing CI quality gate.\n\
   - **Low**: cosmetic or minor improvement — e.g. inconsistent variable naming in one module, missing JSDoc, an unused dependency.\n\
\n\
4. **Two-tier Status** (the value matters — the validation phase skips findings already verified):\n\
   - **Verified in source**: you confirmed the problem by reading the actual source code (file:line cited). This is the default for baseline findings (§ A) and most dimensional findings (§ B) — the validation phase will SKIP these from re-confirmation, saving the user time.\n\
   - **Inferred**: you extrapolated from docs/ or briefing — needs user confirmation in validation. Use only when you couldn't reach the actual source (e.g. it lives in a linked repo, or the docs claim a behavior you couldn't verify directly).\n\
   - **Blocked upstream**: depends on a third-party fix (vendor bug, language version bump, framework deprecation cycle).\n\
   - **Mitigated**: partial fix shipped, residual work tracked.\n\
   - **Confirmed by user**: only set by the validation phase after user confirms.\n\
   - **Rejected**: only set by the validation phase if user rejects — the next audit will NOT recreate this TD.\n\
\n\
5. **Cap**: target 30 findings maximum. Critical + High findings (including all baseline checklist failures) are NEVER trimmed to fit the cap. Trim Medium and Low if needed. If you cannot stay under 30 even after trimming all Low, note the count of trimmed Medium in the index's `## Coverage gaps` section.\n\
\n\
6. **Tracker MCP dedup**: if a ticket tracker MCP is configured (Jira/Linear/GitHub Issues), before creating a NEW TD file, do a read-only search for an existing open ticket with a matching title fragment. If found: set `Next step: link existing ticket <URL>` instead of `create ticket`. Avoid duplicating tracked work.\n\
\n\
7. **No issues found**: single row in the index: `'None identified during initial audit'`. (Almost never happens on a real repo — even a green-field has baseline checklist gaps.)\n\
\n\
**Detail file format** (YAML frontmatter + markdown sections, ENFORCE THIS SHAPE):\n\
```\n\
---\n\
name: td-<date>-<slug>\n\
description: One-line summary (≤ 160 chars).\n\
metadata:\n\
  type: tech-debt\n\
  audit_history:\n\
    - date: YYYY-MM-DD\n\
      status: Verified in source | Inferred | Blocked upstream | Mitigated | Confirmed by user | Rejected\n\
      reviewer: <audit kind, e.g. \"Full audit\" or \"Security audit\">\n\
      note: optional — what changed since previous entry (e.g. \"line numbers shifted\", \"severity bumped to High\")\n\
---\n\
\n\
# TD-<date>-<slug>\n\
\n\
- **Area**: Backend | Frontend | CI | Infra | Security | A11y | Observability | Database | ApiDesign | Docs | Other\n\
- **Severity**: Critical | High | Medium | Low\n\
- **Status**: <current value, mirrors latest audit_history entry>\n\
- **Effort**: S (< 1h) | M (1-4h) | L (1+ day) | XL (multi-day / cross-team)\n\
- **Blast radius**: local (1-2 files) | module (5-10 files) | cross-cutting (50+ files or new pattern)\n\
\n\
## Problem (fact)\n\
<one or two sentences, factual — what is, not what should be>\n\
\n\
## Impact\n\
<what goes wrong if not fixed — concrete, e.g. \"production crashes on Monday morning when peak traffic …\">\n\
\n\
## Where (pointers)\n\
- `path/to/file.rs:42` — <one-line context>\n\
- `path/to/other.toml:7-15` — <one-line context>\n\
\n\
## Suggested direction\n\
<non-binding fix suggestion — what would a senior do? Skeleton, NOT full implementation>\n\
\n\
## Next step\n\
`create ticket` OR `link existing ticket <URL>` (see point 6).\n\
```\n\
\n\
For UPDATES of existing TDs: APPEND a new entry to `audit_history`, do not replace previous entries. The chronological list is the value.\n\n\

# E. ALSO FILL docs/decisions.md\n\
\n\
Intentional architectural choices observed in the code that might look unusual to a newcomer (e.g., why a certain pattern was chosen over a simpler one). \
This is NOT a tech-debt file — it's a positive record. Examples: \"Single Mutex on SQLite (rationale: single-writer model)\", \"No ORM (rationale: pure SQL is faster for our access pattern)\".",
        sources: &["__GIT_HEAD__"],
    },

    // Step 10: Final review
    AnalysisStep {
        target_file: "REVIEW",
        prompt: "\
Read ALL docs/ files. Final quality pass — fix issues directly.\n\n\
Check: no remaining `{{` placeholders · no orphan `<!-- fill -->` comments (keep `<!-- TODO: ask user -->`) \
· no duplicated facts · consistent terminology with glossary · valid cross-references \
· no contradictions · no empty critical sections · clean markdown · each tech-debt entry has a detail file \
· TODO markers are genuine unknowns.\n\n\
Empty sections for missing features → 'N/A — not used'.",
        sources: &[],
    },
];

// ─── Specialized audit kinds (0.8.2 "Design C") ──────────────────────────
//
// Each focused kind reuses Step 9's prompt scaffolding but narrows the
// scope. The Full pipeline keeps all 10 steps. Specialized kinds skip
// the docs-generation steps (1-8) because the user only wants a focused
// re-scan, and they emit findings into a *named* tech-debt index file
// per kind (e.g. `docs/inconsistencies-security.md`) so they don't
// clobber the Full audit's `docs/inconsistencies-tech-debt.md`.
//
// S2.D3-D5 will replace the placeholder bodies below with vetted
// prompts. The dispatcher (`kind_to_steps`) is stable now so the
// front-end can already wire kind selection.

pub(crate) const DRIFT_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "REVIEW",
        prompt: "\
DRIFT AUDIT (0.8.2) — placeholder body, content lands in S2.D3.\n\n\
Read docs/checksums.json. For every (file, sha256) recorded there, recompute the sha and \
list the files where the recorded hash no longer matches. Do NOT rewrite any file — \
your output is a short bullet list of stale docs/ files only.",
        sources: &["docs/checksums.json"],
    },
];

pub(crate) const SECURITY_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-security.md",
        prompt: "\
You are running a FOCUSED SECURITY AUDIT (0.8.2). Output ONLY security findings — \
ignore unrelated bugs, perf issues, documentation drift, etc.\n\n\
\
# A. MANDATORY SECURITY CHECKLIST (never skip, never trim)\n\
\n\
Walk through every item below. If satisfied, write \"verified present\" or \"verified absent\". \
If NOT satisfied, emit a TD finding. These do NOT count against any cap.\n\n\
\
**Secrets & credentials in source:**\n\
- No real-looking secrets in tracked `.env*` files (32+ hex chars, base64 ≥ 24 chars, vendor prefixes like `sk-`, `xoxb-`, `p8e-`, `AIzaSy…`). Run `git ls-files | grep -E '\\.env'` and read each.\n\
- No credentials hardcoded in CI YAML (`.github/workflows/*`, `.gitlab-ci.yml`) — must source from `${{ secrets.* }}` or equivalent.\n\
- No real credentials in docker-compose `environment:` blocks — must use `secrets:` or `env_file:`.\n\
- No private keys or certs in repo (`*.pem`, `*.key`, `*.p12`, `id_rsa*`).\n\
- `git log --all -p -S 'BEGIN PRIVATE KEY'` returns nothing recent (last 12 months).\n\n\
\
**Authentication & sessions:**\n\
- Session cookies set with `HttpOnly` AND `Secure` AND `SameSite=Lax|Strict` (grep for `setcookie`, `Set-Cookie`, `withCredentials`).\n\
- Password storage uses a slow hash (`bcrypt`, `argon2`, `scrypt`, `PBKDF2`) — not MD5/SHA1/plain.\n\
- JWT secret is sourced from env, not from code; verify alg is RS256 or HS256 (never `none`).\n\
- No `Authorization: Bearer <hardcoded>` in source (search for `Bearer ` literal).\n\
- Password reset tokens are single-use AND time-limited.\n\n\
\
**Input validation & injection:**\n\
- SQL: every query uses parameterized binding (no raw string concat with user input). Grep for patterns like `query(\"...\" + ` or `query(f\"...{user_var}\"` or PHP `\".$user.\"`.\n\
- Shell: no `exec`, `system`, `passthru`, `subprocess.run(..., shell=True)`, Rust `std::process::Command` spawning `sh`/`bash`/`cmd.exe` with `-c`/`/c` + user input.\n\
- File paths: no user-controlled string flowing into `fs::open`, `open()`, `file_get_contents()` without `realpath`/`canonicalize` + allowlist root.\n\
- Templates: no user input in `dangerouslySetInnerHTML`, `v-html`, `{!! $x !!}` (Blade), `{{ x | safe }}` (Jinja). Sample 3 templates.\n\
- Deserialization: no `pickle.loads`, `unserialize`, `yaml.load` (without `SafeLoader`), `Marshal.load` on untrusted input.\n\n\
\
**Transport & headers:**\n\
- HTTPS enforced (nginx/middleware redirects HTTP → HTTPS, or `secure_origin_whitelist` is empty in prod).\n\
- `Strict-Transport-Security` header set with `max-age >= 15768000` and `includeSubDomains`.\n\
- `Content-Security-Policy` header set (even a permissive one with `default-src 'self'` is better than absent).\n\
- `X-Frame-Options: DENY` or CSP `frame-ancestors 'none'` to block clickjacking.\n\
- `X-Content-Type-Options: nosniff` set.\n\
- CORS allowlist is explicit (not `Access-Control-Allow-Origin: *` for any endpoint that touches credentials).\n\n\
\
**Dependencies & supply chain:**\n\
- Lockfile present and tracked (`package-lock.json`, `pnpm-lock.yaml`, `composer.lock`, `Cargo.lock` for bins, `poetry.lock`, `Gemfile.lock`).\n\
- No known-vulnerable versions in lockfile (cross-check a sample of the top 5 deps against public CVE lists via your knowledge; flag only when severity is clearly High/Critical).\n\
- CI runs a dependency audit step (`npm audit`, `pnpm audit`, `cargo audit`, `composer audit`, `pip-audit`, etc.) or has a Dependabot/Renovate config.\n\
- No `npm install` / `pip install` of git URLs with `master`/`main` branches (must be tagged or pinned commit).\n\n\
\
**SSH/CI deploy:**\n\
- No `StrictHostKeyChecking=no` in deploy scripts.\n\
- Deploy SSH keys have a clear rotation procedure documented (search for `rotat` keyword in deploy docs).\n\
- `pull_request_target` workflows do NOT check out the PR branch's workflow definition (RCE vector).\n\n\
\
# B. ANTI-REPETITION (priors)\n\
\n\
Before writing findings: read existing `docs/tech-debt/TD-*-{auth,security,secrets,xss,sql,cors,csp,...}*.md`. \
If a finding you'd write matches an existing one, **REUSE its ID** (e.g. update the matching `TD-20260315-jwt-secret-hardcoded.md` in place — APPEND a new `audit_history` entry to the YAML frontmatter, never overwrite). \
A new audit run does NOT mean new slugs. Slug churn is the #1 audit anti-pattern.\n\n\
\
# C. OUTPUT FORMAT\n\
\n\
1. Write `docs/inconsistencies-security.md` with the standard table header:\n\
   `| ID | Problem | Area | Severity |`\n\
2. For each finding, also write `docs/tech-debt/<ID>.md` using the existing TD detail template — \
   add a `**Category**: security` line so a future Full-audit reconciliation can dedupe by category.\n\
3. **Status taxonomy** (two-tier):\n\
   - `Verified in source` — you opened the source file and confirmed the issue still exists at the cited path.\n\
   - `Inferred` — pattern-matched only; the fix may already be in flight upstream.\n\
   Use the first one whenever you actually read source.\n\
4. **Cap**: 25 findings max from § A. Critical and High are exempt — if your scan finds 6 Critical, you emit all 6 even if the cap is hit.\n\
5. Each finding has: file path + line number, one-sentence impact (what an attacker gains), one-sentence remediation. \
   No long essays — this is a triage list, not a security report.\n\n\
\
Do NOT modify source code. Do NOT touch other docs/ files. Your scope is only the two output files above.",
        sources: &[
            "docker-compose.yml", "Dockerfile", ".github/workflows",
            ".gitlab-ci.yml", "package.json", "composer.json", "Cargo.toml",
            ".env", ".env.dist", ".env.example",
        ],
    },
];

pub(crate) const DOCKER_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-docker.md",
        prompt: "\
You are running a FOCUSED DOCKER & CONTAINER AUDIT (0.8.2). Output ONLY \
container/image/orchestration findings — ignore unrelated bugs.\n\n\
\
# A. DOCKERFILE CHECKLIST (per Dockerfile in repo — root + subdirs)\n\
\n\
For EACH `Dockerfile`, `*.dockerfile`, or `*/Dockerfile` (use `git ls-files | grep -i dockerfile`):\n\
- **Base image is pinned** to a specific tag (not `:latest`, not just `python` — must be `python:3.11.7-slim` or a digest).\n\
- **`USER <non-root>` present** before the `CMD`/`ENTRYPOINT` (root inside a container is a privilege-escalation jump pad).\n\
- **`HEALTHCHECK` directive present** (or documented absent because the orchestrator handles it).\n\
- **Layer hygiene**: `apt-get install` chains `--no-install-recommends && rm -rf /var/lib/apt/lists/*` in the same RUN. Equivalent for `yum`, `apk`, `dnf`.\n\
- **No secrets via `ARG`** (they leak via `docker history`). Multi-stage `--mount=type=secret` is fine.\n\
- **No `ADD <remote-url>`** without checksum verification — prefer `RUN curl ... && sha256sum -c`.\n\
- **Multi-stage** used when the final image carries a build toolchain (Node, Cargo, Go, JDK) it doesn't need at runtime.\n\
- **`COPY --chown`** used when copying app files (otherwise root-owned files under a non-root USER).\n\
- **`.dockerignore` present** at the directory the Dockerfile sits in, excluding at minimum `.git`, `node_modules`, `target`, `vendor`, `.env*`.\n\
- **PHP-specific** (if the image runs PHP): `display_errors = Off`, `expose_php = Off`, `opcache.enable = 1` for prod; verify `php.ini`/`*.ini` overrides.\n\
- **Node-specific**: `NODE_ENV=production` set, no `npm install` (must be `npm ci` for reproducibility).\n\n\
\
# B. COMPOSE CHECKLIST (per `docker-compose*.yml` / `compose*.yaml`)\n\
\n\
- **Resource limits**: every service has `mem_limit` + `cpus` OR `deploy.resources.limits.{memory,cpus}` set (without limits, one runaway container can starve the host).\n\
- **No `:latest`** image tags (reproducibility).\n\
- **Healthchecks**: critical services (DB, app, gateway) have `healthcheck:` blocks AND `depends_on` uses `condition: service_healthy` instead of bare `depends_on: [svc]`.\n\
- **Secrets handling**: no literal credentials in `environment:` (use `env_file:` or `secrets:` block). Grep for `_PASSWORD=`, `_TOKEN=`, `_KEY=` followed by a non-empty literal.\n\
- **Read-only rootfs**: stateless services have `read_only: true` (or note exceptions). Worth a TD only when the service is obviously stateless (gateway, static-asset proxy).\n\
- **Restart policy**: services have `restart: unless-stopped` or `restart: always` (otherwise a crash stops them silently).\n\
- **Logging cap**: `logging.options.max-size` + `max-file` set somewhere — either per-service or via a default driver — otherwise the host disk fills up.\n\
- **Networks**: services that don't need each other use separate networks (defense-in-depth — emit TD only for obvious cases like a DB sharing the same network as an outbound-facing proxy).\n\
- **Volume mounts**: no `${HOME}:/host-home:rw` style host-root mounts on a service that doesn't need them. Read-only is OK; rw on `/`, `/home`, `/var` is a red flag.\n\n\
\
# C. CI/IMAGE-BUILD CHECKLIST\n\
\n\
- CI step that builds the image runs a vulnerability scanner (`trivy`, `grype`, `snyk container test`) OR a TD is filed acknowledging the gap.\n\
- Image push uses a tag derived from git sha or release version, never just `:latest` alone (multi-tag with `:latest` is fine).\n\
- Cache layers: COPY commands ordered cheapest-changing → most-changing (lockfiles before source).\n\n\
\
# D. ANTI-REPETITION (priors)\n\
\n\
Before writing findings: read existing `docs/tech-debt/TD-*-{docker,compose,dockerfile,image,layer,...}*.md`. \
If a finding matches, reuse its ID (APPEND an `audit_history` entry to the YAML frontmatter — do not overwrite). \
Slug churn is the #1 audit anti-pattern.\n\n\
\
# E. OUTPUT FORMAT\n\
\n\
1. Write `docs/inconsistencies-docker.md` with header `| ID | Problem | Area | Severity |`.\n\
2. For each finding, write `docs/tech-debt/<ID>.md` with a `**Category**: docker` line in addition to the standard TD schema.\n\
3. **Status taxonomy** (two-tier): `Verified in source` when you opened the file and confirmed; `Inferred` for pattern-match only.\n\
4. **Cap**: 25 findings max. Critical/High exempt.\n\
5. Each finding: file path + line number, one-sentence impact (outage/security/perf cost), one-sentence remediation.\n\n\
\
Do NOT modify source. Do NOT touch other docs/ files.",
        sources: &["Dockerfile", "docker-compose.yml", "docker-compose.yaml", "compose.yaml", "compose.yml", ".dockerignore"],
    },
];

pub(crate) const PERFORMANCE_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-performance.md",
        prompt: "\
You are running a FOCUSED PERFORMANCE AUDIT (0.8.2). Output ONLY perf findings.\n\n\
\
# A. DATA-ACCESS CHECKLIST\n\
\n\
- **N+1 queries**: look for ORM patterns where a list iteration triggers per-row queries. \
  Specifically: Laravel/Doctrine `->load()` missing inside a `foreach`, Django `.get()` inside a `for` loop, \
  Rails `.each do |x| x.assoc...` without `includes(:assoc)`, Sequelize `Model.findAll()` followed by per-record `await user.getProfile()`. \
  Sample the top 5 controllers/services.\n\
- **Missing indexes**: cross-check WHERE/ORDER BY/JOIN columns against migration files (`migrations/*`, `db/migrate/*`, `*.sql`). \
  Any column used in WHERE/ORDER BY without an index = TD (unless table is provably tiny).\n\
- **`SELECT *`** in hot paths — flag when a query joins large tables.\n\
- **Synchronous DB calls in async handlers**: `await db.query()` is fine; `db.query_sync()` inside an async fn is not.\n\
- **Pagination defaults**: list endpoints have a hard cap (e.g. `LIMIT 100`); unbounded queries are a TD.\n\n\
\
# B. CACHING CHECKLIST\n\
\n\
- **Cache headers** on static assets: `Cache-Control: public, max-age=31536000, immutable` for hashed bundles; `no-cache` for HTML shells.\n\
- **CDN / edge cache** strategy documented (in docs/operations/ or in the load balancer config).\n\
- **App-level cache** TTLs are explicit (no `cache.forever()` or `expiry: -1` without justification).\n\
- **Cache stampede protection**: hot keys use SWR / single-flight / dogpile-style locking (or filed as TD if absent).\n\n\
\
# C. FRONTEND / RENDER CHECKLIST (when a frontend exists)\n\
\n\
- **Bundle size**: build output present? Check `dist/`, `build/`, `.next/` sizes from CI logs or `package.json` build script. Flag chunks > 500 KB gzipped.\n\
- **Code-splitting**: routes lazy-loaded? Look for dynamic `import()` or `React.lazy()`.\n\
- **Images**: `loading=\"lazy\"` on offscreen `<img>`. Sample 3 templates/components.\n\
- **Long lists**: virtualization used for lists > 100 items? Look for `react-window`, `virtual-scroll`, etc.\n\
- **Re-render hotspots**: `useEffect` deps include large objects/arrays not memoized? Sample 3 components.\n\n\
\
# D. BACKEND / WORKER CHECKLIST\n\
\n\
- **Sync I/O in async handlers**: file reads, HTTP calls without `await`/non-blocking client.\n\
- **Blocking the event loop**: heavy CPU in JS handlers (large JSON parse, sync crypto, regex catastrophic backtracking). Tokio: blocking work outside `spawn_blocking`.\n\
- **Connection pool sizing**: DB pool size set explicitly in config? Or relying on driver defaults (often too low or unbounded).\n\
- **Background jobs** queued but never processed (orphan queues) — check for unused queue names in code vs worker config.\n\n\
\
# E. OBSERVABILITY (gap → can't measure → can't fix)\n\
\n\
- **APM / tracing**: at least one tool wired (OpenTelemetry, Datadog APM, NewRelic, Sentry Performance, etc.) — TD if none.\n\
- **Slow query log**: enabled in DB or surfaced via APM.\n\
- **Front-end RUM**: any tool collecting Core Web Vitals from real users.\n\n\
\
# F. ANTI-REPETITION (priors)\n\
\n\
Read existing `docs/tech-debt/TD-*-{perf,n-plus-one,index,cache,bundle,...}*.md` before writing. \
Match → reuse ID + APPEND `audit_history` entry. Slug churn is the #1 audit anti-pattern.\n\n\
\
# G. OUTPUT FORMAT\n\
\n\
1. Write `docs/inconsistencies-performance.md` with header `| ID | Problem | Area | Severity |`.\n\
2. For each finding, write `docs/tech-debt/<ID>.md` with `**Category**: performance`.\n\
3. **Status taxonomy**: `Verified in source` vs `Inferred` (cf. Full audit).\n\
4. **Cap**: 25 findings max. Critical/High exempt.\n\
5. Each finding: file path + line number, one-sentence impact (latency / throughput cost — be quantitative when possible), one-sentence remediation.\n\n\
\
Do NOT modify source. Do NOT touch other docs/ files.",
        sources: &[],
    },
];

pub(crate) const ACCESSIBILITY_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-accessibility.md",
        prompt: "\
You are running a FOCUSED ACCESSIBILITY AUDIT (0.8.2). Output ONLY a11y findings, \
scoped to WCAG 2.1 AA where applicable.\n\n\
\
# A. SEMANTIC HTML CHECKLIST (sample 5 representative templates/components)\n\
\n\
- **Form inputs** have an associated `<label for=>` or wrap the input, OR `aria-label` / `aria-labelledby` is present. Placeholder-only is NOT a label.\n\
- **Buttons vs links**: `<a>` used for navigation, `<button>` for actions. No `<div onClick=...>` masquerading as a clickable.\n\
- **Headings**: exactly one `<h1>` per page; no jumps `<h1>` → `<h3>` (skipped levels).\n\
- **Landmarks**: page uses `<header>`, `<nav>`, `<main>`, `<footer>` (or ARIA roles `banner`, `navigation`, `main`, `contentinfo`).\n\
- **Lists**: groups of similar items use `<ul>` / `<ol>`, not stacks of `<div>`.\n\
- **`<img alt=>`** present on every image; decorative images use `alt=\"\"`. Sample 5+ images.\n\
- **SVG**: `<svg>` that conveys meaning has `<title>` or `aria-label`; decorative SVGs have `aria-hidden=\"true\"`.\n\n\
\
# B. KEYBOARD & FOCUS CHECKLIST\n\
\n\
- **`tabindex`**: no positive values (breaks natural tab order). `tabindex=\"-1\"` only on programmatic focus targets.\n\
- **Focus visible**: at least one global focus style. Grep for `outline: none` / `outline: 0` without a corresponding `:focus-visible` override.\n\
- **Skip link**: a \"Skip to main content\" link exists (or filed as TD on a multi-section page).\n\
- **Modals/popovers**: focus is trapped while open AND returns to the trigger on close. Read the modal component once.\n\
- **Custom widgets**: dropdowns/tabs/accordions follow ARIA Authoring Practices (correct roles + keyboard support). Open one custom widget and inspect.\n\n\
\
# C. COLOR & CONTRAST CHECKLIST\n\
\n\
- **Text contrast**: foreground/background pairs in the theme tokens meet 4.5:1 for body, 3:1 for large text (18pt+ or 14pt bold). \
  Read the design-token file (`tokens.css`, `theme.ts`, `_colors.scss`, etc.) and compute contrast for the 3-5 most common pairings.\n\
- **Non-text contrast**: focus rings, form borders, icon-only buttons hit 3:1 against adjacent colors.\n\
- **Color-only state**: error states have a non-color cue (icon, text, underline) — not just red.\n\
- **Hover state**: not the ONLY way to discover an interactive element (touch + keyboard users have no hover).\n\n\
\
# D. ARIA HYGIENE\n\
\n\
- **Role conflicts**: no `<button role=\"link\">`, no `<a role=\"button\">`, etc. (use the right tag).\n\
- **Live regions**: status updates use `aria-live=\"polite\"` (or `assertive` for errors). Toast/notification components must announce.\n\
- **`aria-hidden`** not applied to interactive elements (breaks screen readers).\n\
- **Form errors** linked via `aria-describedby` to the input.\n\n\
\
# E. MEDIA / DYNAMIC\n\
\n\
- **Video**: tracks for captions / transcripts (or TD if absent).\n\
- **Animations**: `prefers-reduced-motion` honored (CSS media query OR JS check).\n\
- **Time limits**: auto-dismissing alerts can be paused/extended.\n\n\
\
# F. ANTI-REPETITION (priors)\n\
\n\
Read existing `docs/tech-debt/TD-*-{a11y,accessibility,aria,contrast,keyboard,...}*.md`. \
Match → reuse ID + APPEND `audit_history` entry. Slug churn is the #1 audit anti-pattern.\n\n\
\
# G. OUTPUT FORMAT\n\
\n\
1. Write `docs/inconsistencies-accessibility.md` with header `| ID | Problem | Area | Severity |`.\n\
2. For each finding, write `docs/tech-debt/<ID>.md` with `**Category**: accessibility` and a `**WCAG**: <criterion>` line (e.g. `1.1.1 Non-text Content`).\n\
3. **Status taxonomy**: `Verified in source` vs `Inferred`.\n\
4. **Cap**: 25 findings max. Critical/High exempt.\n\
5. Each finding: file path + line number, who is impacted (screen reader / keyboard-only / low-vision / cognitive), and one-sentence remediation.\n\n\
\
Do NOT modify source. Do NOT touch other docs/ files.",
        sources: &[],
    },
];

pub(crate) const DATABASE_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-database.md",
        prompt: "\
DATABASE AUDIT (0.8.2) — placeholder body, content lands in S2.D4-5.\n\n\
Scope: schema drift, missing indexes, unsafe migrations, ORM lazy-load surprises, \
transaction boundaries. Same TD detail-file schema.",
        sources: &[],
    },
];

pub(crate) const API_DESIGN_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-api.md",
        prompt: "\
API DESIGN AUDIT (0.8.2) — placeholder body, content lands in S2.D4-5.\n\n\
Scope: REST/RPC consistency, error envelope, versioning, pagination, contract drift \
vs documentation. Same TD detail-file schema.",
        sources: &[],
    },
];

/// Dispatch table for `LaunchAuditRequest.kind`. Returns the step
/// slice to drive the agent through. For `Custom`, callers should
/// inline the user-supplied prompt rather than going through this fn.
pub(crate) fn kind_to_steps(kind: crate::models::AuditKind) -> &'static [AnalysisStep] {
    use crate::models::AuditKind;
    match kind {
        AuditKind::Full          => ANALYSIS_STEPS,
        AuditKind::Drift         => DRIFT_STEPS,
        AuditKind::Security      => SECURITY_STEPS,
        AuditKind::Docker        => DOCKER_STEPS,
        AuditKind::Performance   => PERFORMANCE_STEPS,
        AuditKind::Accessibility => ACCESSIBILITY_STEPS,
        AuditKind::Database      => DATABASE_STEPS,
        AuditKind::ApiDesign     => API_DESIGN_STEPS,
        // Custom is handled at the call site: it builds a one-off
        // AnalysisStep from req.custom_prompt rather than using a const.
        AuditKind::Custom        => &[],
    }
}

/// Files installed by the audit template (to be removed on cancel).
pub(crate) const AUDIT_REDIRECTOR_FILES: &[&str] = &[
    "CLAUDE.md", "GEMINI.md", "AGENTS.md",
    ".cursorrules", ".windsurfrules", ".clinerules",
    ".github/copilot-instructions.md",
];

#[cfg(test)]
mod kind_dispatch_tests {
    use super::*;
    use crate::models::AuditKind;

    #[test]
    fn full_kind_returns_canonical_10_steps() {
        let steps = kind_to_steps(AuditKind::Full);
        assert_eq!(steps.len(), ANALYSIS_STEPS.len(),
            "Full kind must return the canonical step list, not a subset");
        assert_eq!(steps.len(), 10, "Full audit is the documented 10-step pipeline");
    }

    #[test]
    fn specialized_kinds_return_focused_single_step() {
        // Every specialized kind ships with exactly one step in 0.8.2.
        for kind in [
            AuditKind::Drift,
            AuditKind::Security,
            AuditKind::Docker,
            AuditKind::Performance,
            AuditKind::Accessibility,
            AuditKind::Database,
            AuditKind::ApiDesign,
        ] {
            let steps = kind_to_steps(kind);
            assert_eq!(steps.len(), 1,
                "{:?} should expose one focused step in 0.8.2 (S2.D3-D5 fill the body)", kind);
        }
    }

    #[test]
    fn custom_kind_returns_empty_slice() {
        // Custom is handled at the call site (caller builds an ad-hoc
        // step from req.custom_prompt). The dispatch table is empty so
        // a copy-paste mistake at the call site fails loudly.
        assert_eq!(kind_to_steps(AuditKind::Custom).len(), 0);
    }

    #[test]
    fn specialized_index_files_are_distinct_from_full() {
        // Each specialized kind must write to its own index file so it
        // doesn't clobber `docs/inconsistencies-tech-debt.md`.
        let canonical = "docs/inconsistencies-tech-debt.md";
        for (kind, expected_prefix) in [
            (AuditKind::Security,      "docs/inconsistencies-security"),
            (AuditKind::Docker,        "docs/inconsistencies-docker"),
            (AuditKind::Performance,   "docs/inconsistencies-performance"),
            (AuditKind::Accessibility, "docs/inconsistencies-accessibility"),
            (AuditKind::Database,      "docs/inconsistencies-database"),
            (AuditKind::ApiDesign,     "docs/inconsistencies-api"),
        ] {
            let steps = kind_to_steps(kind);
            assert_ne!(steps[0].target_file, canonical,
                "{:?} must NOT write into the Full audit's index", kind);
            assert!(steps[0].target_file.starts_with(expected_prefix),
                "{:?} target_file should start with {expected_prefix}, got {}", kind, steps[0].target_file);
        }
    }

    #[test]
    fn drift_kind_consumes_checksums() {
        // Drift is purely a re-hash of docs/checksums.json — it doesn't
        // emit findings, just reports stale files. The single step
        // therefore lists checksums.json as its source.
        let drift = kind_to_steps(AuditKind::Drift);
        assert_eq!(drift.len(), 1);
        assert!(drift[0].sources.contains(&"docs/checksums.json"),
            "Drift step must hash docs/checksums.json to compute drift");
    }

    #[test]
    fn audit_kind_label_round_trip() {
        // The label is what lands in `audit_runs.kind` and powers SSE
        // event filtering on the front-end. Drift in labels would break
        // existing rows after a deploy.
        let expected = [
            (AuditKind::Full,          "Full"),
            (AuditKind::Drift,         "Drift"),
            (AuditKind::Security,      "Security"),
            (AuditKind::Docker,        "Docker"),
            (AuditKind::Performance,   "Performance"),
            (AuditKind::Accessibility, "Accessibility"),
            (AuditKind::Database,      "Database"),
            (AuditKind::ApiDesign,     "ApiDesign"),
            (AuditKind::Custom,        "Custom"),
        ];
        for (kind, label) in expected {
            assert_eq!(kind.as_label(), label, "label drift on {:?}", kind);
        }
    }

    #[test]
    fn launch_audit_request_defaults_kind_to_full() {
        // Backwards-compat: clients that don't send `kind` get Full.
        let json = r#"{"agent":"ClaudeCode"}"#;
        let req: crate::models::LaunchAuditRequest = serde_json::from_str(json)
            .expect("LaunchAuditRequest must still parse without `kind`");
        assert_eq!(req.kind.unwrap_or_default(), AuditKind::Full);
        assert!(req.custom_prompt.is_none());
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::helpers::{build_briefing_prompt, build_validation_prompt};
    use super::*;
    use crate::models::AuditInfo;

    #[test]
    fn preamble_says_autonomous() {
        assert!(PROMPT_PREAMBLE.contains("autonomous") || PROMPT_PREAMBLE.contains("non-interactive"),
            "PREAMBLE must instruct the agent not to ask questions");
        assert!(PROMPT_PREAMBLE.contains("Do NOT ask questions") || PROMPT_PREAMBLE.contains("Do not ask"),
            "PREAMBLE must explicitly forbid questions");
    }

    #[test]
    fn analysis_steps_include_decisions_md() {
        let has_decisions = ANALYSIS_STEPS.iter().any(|step| step.prompt.contains("decisions.md"));
        assert!(has_decisions, "At least one audit step must fill decisions.md");
    }

    #[test]
    fn step9_baseline_includes_community_standards_gate() {
        // The Step 9 prompt grew an OSS-intent-gated community standards
        // section (LICENSE / README description / issue+PR templates /
        // SECURITY / CONTRIBUTING / CODE_OF_CONDUCT). Regress against
        // any future cleanup that accidentally strips it.
        let step9 = ANALYSIS_STEPS.iter()
            .find(|s| s.target_file == "docs/inconsistencies-tech-debt.md")
            .expect("Step 9 prompt must target docs/inconsistencies-tech-debt.md");
        let prompt = step9.prompt;
        assert!(prompt.contains("Community standards"),
            "Step 9 must include the 'Community standards' section header");
        assert!(prompt.contains("OSS intent"),
            "Section must be gated on OSS intent (private projects skipped)");
        for needle in ["LICENSE", "ISSUE_TEMPLATE", "pull_request_template",
                       "SECURITY.md", "CONTRIBUTING.md", "CODE_OF_CONDUCT.md"] {
            assert!(prompt.contains(needle),
                "Community-standards block must check `{}`", needle);
        }
    }

    #[test]
    fn phase3_template_check_only_with_tracker_mcp() {
        // The "before pushing tickets, check issue templates" nudge is
        // gated on `has_issue_tracker_mcp` because it only makes sense
        // when we're actually going to push tickets. Without a tracker
        // the nudge is noise.
        let info = AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] };
        for lang in ["fr", "en", "es"] {
            let with    = build_validation_prompt(lang, &info, true);
            let without = build_validation_prompt(lang, &info, false);
            assert!(with.contains(".github/ISSUE_TEMPLATE"),
                "{} prompt with tracker MCP must contain the template check", lang);
            assert!(!without.contains(".github/ISSUE_TEMPLATE"),
                "{} prompt WITHOUT tracker MCP must NOT contain the template check (noise)", lang);
        }
    }

    #[test]
    fn validation_prompt_forbids_code_modification() {
        for lang in ["fr", "en", "es"] {
            let info = AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] };
            let prompt = build_validation_prompt(lang, &info, false);
            let lower = prompt.to_lowercase();
            assert!(lower.contains("never modify") || lower.contains("ne modifie jamais") || lower.contains("nunca modifiques"),
                "Validation prompt ({}) must forbid code modification", lang);
        }
    }

    #[test]
    fn analysis_steps_have_sources_field() {
        let with_sources: Vec<_> = ANALYSIS_STEPS.iter()
            .filter(|step| !step.sources.is_empty())
            .collect();
        assert!(
            with_sources.len() >= 5,
            "At least 5 analysis steps should have non-empty sources, got {}",
            with_sources.len()
        );
    }

    #[test]
    fn analysis_steps_no_duplicate_target_files() {
        let mut seen = std::collections::HashSet::new();
        for step in ANALYSIS_STEPS {
            assert!(
                seen.insert(step.target_file),
                "Duplicate target_file found: {}",
                step.target_file
            );
        }
    }

    #[test]
    fn analysis_steps_count_is_10() {
        assert_eq!(
            ANALYSIS_STEPS.len(),
            10,
            "Expected exactly 10 analysis steps, got {}",
            ANALYSIS_STEPS.len()
        );
    }

    #[test]
    fn briefing_notes_injected_into_prompt() {
        let briefing_notes: Option<String> = Some("This project uses microservices with gRPC".into());
        let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, ANALYSIS_STEPS[0].prompt);

        if let Some(ref notes) = briefing_notes {
            full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
        }

        assert!(full_prompt.contains("## Project briefing (from the user)"),
            "Briefing section header must be present when notes are set");
        assert!(full_prompt.contains("microservices with gRPC"),
            "User's briefing content must appear in the prompt");
    }

    #[test]
    fn briefing_notes_not_injected_when_none() {
        let briefing_notes: Option<String> = None;
        let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, ANALYSIS_STEPS[0].prompt);

        if let Some(ref notes) = briefing_notes {
            full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
        }

        assert!(!full_prompt.contains("## Project briefing"),
            "Briefing section must NOT appear when notes are None");
    }

    #[test]
    fn briefing_prompt_is_localized() {
        let fr = build_briefing_prompt("fr");
        let en = build_briefing_prompt("en");
        let es = build_briefing_prompt("es");
        assert_ne!(fr, en, "FR and EN briefing prompts must differ");
        assert_ne!(en, es, "EN and ES briefing prompts must differ");
        assert_ne!(fr, es, "FR and ES briefing prompts must differ");
    }

    #[test]
    fn briefing_prompt_forbids_code_reading() {
        let fr = build_briefing_prompt("fr");
        let en = build_briefing_prompt("en");
        let es = build_briefing_prompt("es");
        assert!(fr.contains("ne lis PAS"),
            "FR briefing prompt must contain 'ne lis PAS'");
        assert!(en.contains("Do NOT read"),
            "EN briefing prompt must contain 'Do NOT read'");
        assert!(es.contains("NO leas"),
            "ES briefing prompt must contain 'NO leas'");
    }

    #[test]
    fn briefing_prompt_requires_answers_1_to_5() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang);
            assert!(prompt.contains("1-5") || prompt.contains("1 a 5") || prompt.contains("1-5"),
                "Briefing prompt ({}) must reference questions 1-5 as required", lang);
            let lower = prompt.to_lowercase();
            assert!(lower.contains("optional") || lower.contains("optionnel") || lower.contains("opcional"),
                "Briefing prompt ({}) must mark Q6 as optional", lang);
        }
    }

    #[test]
    fn briefing_prompt_says_stack_auto_detected() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang);
            let lower = prompt.to_lowercase();
            assert!(lower.contains("auto-detect") || lower.contains("auto-detect"),
                "Briefing prompt ({}) must mention stack is auto-detected", lang);
        }
    }

    #[test]
    fn briefing_prompt_contains_completion_signal() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang);
            assert!(prompt.contains("KRONN:BRIEFING_COMPLETE"),
                "Briefing prompt ({}) must contain KRONN:BRIEFING_COMPLETE", lang);
        }
    }
}
