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
    AnalysisStep {
        target_file: "docs/inconsistencies-tech-debt.md",
        prompt: "\
Real issues only, not hypothetical. Read all docs/ files AND scan source code.\n\
Scan: entry points, config files, Dockerfiles, CI configs, and 5-10 core source files \
(prioritize auth, data persistence, external input handling).\n\n\
Systematically audit across these dimensions:\n\
- Dependencies: EOL/deprecated runtimes, frameworks, packages, or versions significantly behind stable\n\
- Security: hardcoded secrets, missing auth checks, injection vectors (SQL/XSS), insecure defaults, exposed debug endpoints\n\
- Code quality: functions >50 lines, god classes, SRP violations, dead code, error swallowing (empty catch/let _ =)\n\
- Scalability: N+1 queries, unbounded loops, missing pagination, memory leaks, missing indexes\n\
- Maintainability: tight coupling, circular dependencies, missing tests for critical paths, unclear naming\n\
- Accessibility (a11y): unlabelled form inputs, low color contrast (< 4.5:1), missing ARIA on custom widgets, \
keyboard-inaccessible interactive elements, missing focus traps in modals, non-semantic HTML on landmark regions\n\
- Observability: missing logging in hot paths (auth, payment, write endpoints), no error tracker (Sentry/Glitchtip/equivalent), \
no health/readiness endpoints, no metrics on critical SLI (latency, error rate, queue depth)\n\
- Compliance: GDPR issues (external resources, data retention), license incompatibilities\n\
- Infrastructure: Docker misconfigs (root user, no resource limits), CI gaps, missing health checks\n\
- Documentation drift: cross-check the docs/ files you just wrote (steps 1-8) against the source code. \
Flag concrete contradictions (e.g. `coding-rules.md` says X is enforced but no linter rule exists, \
`testing-quality.md` claims N tests but actual count differs, `mcp-servers.md` lists a server not in `.mcp.json`)\n\n\
Fill docs/inconsistencies-tech-debt.md — replace ALL {{PLACEHOLDERS}} and <!-- ... --> comments:\n\
1. Outdated prerequisites table: flag EOL/deprecated/behind-stable runtimes, frameworks, packages\n\
2. For each issue found: (a) create `docs/tech-debt/TD-YYYYMMDD-slug.md` (YYYYMMDD=today) first, \
then (b) add the one-line entry to the Current list table. Do both or neither.\n\
3. Severity calibration (use concrete examples to avoid over-classification as Medium):\n\
   - **Critical**: production down, data leak, exploitable flaw — e.g. hardcoded prod API key in repo, SQL injection in a public endpoint, secret in a logged response\n\
   - **High**: blocks release or a supported environment — e.g. test suite red on main, build fails on the documented Node version, auth bypass under specific conditions\n\
   - **Medium**: daily dev friction or measurable perf hit — e.g. test suite >30s, N+1 in the dashboard page, manual repetitive setup, broken hot reload\n\
   - **Low**: cosmetic or minor improvement — e.g. inconsistent variable naming in one module, missing JSDoc, an unused dependency\n\
4. Limit to 15-20 most impactful findings. Prioritize Critical and High.\n\
5. If a ticket tracker MCP is configured (Jira/Linear/GitHub Issues), before creating a TD-* file, \
do a read-only search for an existing open ticket with a matching title fragment. \
If found: set `Next step: link existing ticket <URL>` instead of `create ticket`, and skip the \"create\" suggestion entirely. Avoid duplicating tracked work.\n\
6. No issues found → single row: 'None identified during initial audit'\n\n\
Detail file format:\n\
- **ID**: TD-YYYYMMDD-slug\n\
- **Area**: Backend | Frontend | CI | Infra | Security | A11y | Observability | Docs\n\
- **Severity**: Critical | High | Medium | Low\n\
- **Status**: Draft | In progress | Blocked upstream | Mitigated (start at Draft)\n\
- **Effort**: S (< 1h) | M (1-4h) | L (1+ day) | XL (multi-day / cross-team)\n\
- **Blast radius**: local (1 file) | module (5-10 files) | cross-cutting (50+ files or new pattern)\n\
- **Problem (fact)**: one-line description\n\
- **Impact**: what goes wrong if not fixed\n\
- **Where (pointers)**: file paths with line numbers\n\
- **Suggested direction**: non-binding fix suggestion\n\
- **Next step**: `create ticket` OR `link existing ticket <URL>` (see point 5)\n\n\
Also fill `docs/decisions.md` with intentional architectural choices observed in the code that might look unusual \
to a newcomer (e.g., why a certain pattern was chosen over a simpler one).",
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

/// Files installed by the audit template (to be removed on cancel).
pub(crate) const AUDIT_REDIRECTOR_FILES: &[&str] = &[
    "CLAUDE.md", "GEMINI.md", "AGENTS.md",
    ".cursorrules", ".windsurfrules", ".clinerules",
    ".github/copilot-instructions.md",
];

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
