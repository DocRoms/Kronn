// AI Audit pipeline split into one file per concern
// (TD-20260417-audit-monolith). The big static prompt definitions
// (`PROMPT_PREAMBLE`, `ANALYSIS_STEPS`)
// live here because every sub-module reads from them, and they are
// the single source of truth for what "the audit" actually does.
// Sub-modules are re-exported via `pub use *::*` so every existing
// `api::audit::Foo` call site keeps resolving without edits.

use std::convert::Infallible;
use std::pin::Pin;

use axum::response::sse::Event;
use futures::{Stream, StreamExt};

mod agent_launch;
pub mod anti_hallu_enforce;
pub mod anti_hallu_step;
pub mod briefing;
mod document_repair;
pub mod drift;
pub mod full;
pub mod helpers;
pub mod info;
#[cfg(test)]
mod prompt_quality_tests;
pub mod reconciliation;
pub mod redact_artifacts;
pub mod run;
pub mod validate;
pub mod validation;

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
pub(crate) use helpers::{
    check_ai_dir_permissions, detect_project_skill_markers, detect_project_skills,
};

pub(super) type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Poll the producer independently so disconnecting a subscriber cannot stop an audit.
pub(super) fn detach_sse_stream(mut producer: SseStream) -> SseStream {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    tokio::spawn(async move {
        while let Some(item) = producer.next().await {
            let _ = tx.send(item);
        }
    });
    Box::pin(futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|item| (item, rx))
    }))
}

#[cfg(test)]
mod detach_sse_stream_tests {
    use super::{detach_sse_stream, Event, SseStream};
    use futures::StreamExt;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn producer_runs_to_completion_after_its_subscriber_is_dropped() {
        let progress = Arc::new(AtomicUsize::new(0));
        let producer_progress = progress.clone();
        let producer: SseStream = Box::pin(async_stream::try_stream! {
            for i in 1..=5u32 {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                producer_progress.fetch_add(1, Ordering::SeqCst);
                yield Event::default().event("tick").data(i.to_string());
            }
        });

        drop(detach_sse_stream(producer));

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while progress.load(Ordering::SeqCst) < 5 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("producer must finish after its subscriber is dropped");
        assert_eq!(progress.load(Ordering::SeqCst), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn events_reach_a_subscriber_that_stays_attached() {
        let producer: SseStream = Box::pin(async_stream::try_stream! {
            yield Event::default().event("start").data("{}");
            yield Event::default().event("done").data("{}");
        });
        let mut detached = detach_sse_stream(producer);

        let first: Result<Event, Infallible> = detached.next().await.expect("first event");
        assert!(first.is_ok());
        let second: Result<Event, Infallible> = detached.next().await.expect("second event");
        assert!(second.is_ok());
        assert!(detached.next().await.is_none());
    }
}

pub(crate) const PROMPT_PREAMBLE: &str = "\
Rules: Write new content in the documentation language declared by `docs/AGENTS.md` § Project parameters. While Step 1 is still filling that value, preserve the language of existing docs and use English only for a fresh skeleton with no established language. Be factual and concise — this is AI context for coding agents, NOT human documentation.\n\
- Do NOT invent — read `docs/AGENTS.md` § 0 Anti-Hallucination Protocol (STEP 0 of this audit just wrote/refreshed it). The cascade is `read code → existing docs/ → external doc → escalate via TODO marker → never assert without proof`. Every non-trivial fact written into `docs/` MUST carry `[src: file: <path>:<line>]` or `[src: url: …]` ; fabricated citations (path doesn't exist / outside project root) are mechanically checked — `enforce` mode rejects them with a retry, the default `warn` logs them; treat any warning as a defect.\n\
- Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content. {{PLACEHOLDERS}} are literal text markers — replace by editing file content directly.\n\
- Keep the existing file structure and section headings — fill in the blanks, do NOT rewrite the file from scratch.\n\
- If a section does not apply to this project, replace placeholders with 'N/A — not used in this project.' Do not delete the section.\n\
- Any section marked `owner=\"human\"` is owned by a person (legacy `curated=\"human\"` has the same protection): a full OR partial audit must NEVER rewrite, reorder, or delete its body. The pipeline restores attempted changes and records the proposal under `docs/reports/`; do not work around that guard.\n\
- Never force-translate an existing `docs/` file into another language — a doc already written in a different language stays as-is; the docs/tickets language parameter only governs NEW content.\n\
- Plain facts in docs/ files — no opinions, debate or trade-off analysis. One exception: `Suggested direction` in TD detail files (and index remediation lines) may carry a one-line, non-binding fix hint.\n\
- Each section should be self-contained: another AI agent reading just that section should get the full picture.\n\
- Add or remove table rows as needed to match the project. Write fewer entries rather than inventing content to fill slots.\n\
\n\
## MARKER DISCIPLINE (critical — avoid marker overuse)\n\
\n\
Three marker types exist. Each has a STRICT semantic — using the wrong one creates noise the user has to triage later.\n\
\n\
1. **`<!-- TODO: verify -->`** — RESERVED for facts you literally could not check.\n\
   Examples: file lives outside the repo (linked_repos), tool requires credentials not provided, sandbox blocks the read.\n\
   **DO NOT use** when you DID verify (via Glob/Read/ls) — write the conclusion directly:\n\
   - WRONG: `phpstan.neon.dist <!-- TODO: verify — file not present at project root -->`\n\
   - RIGHT: `phpstan.neon.dist (not present at project root)`\n\
   If you Globed for a config and found nothing, that IS the verified answer — don't add a TODO marker.\n\
\n\
2. **`<!-- TODO: ask user -->`** — for facts that require a HUMAN DECISION, not a verification you could do yourself.\n\
   Examples: \"is this rule aspirational or enforced?\", \"which domain is canonical?\", \"is this port intentional or vestigial?\".\n\
   The Phase 2 validation discussion will ask the user these specific questions.\n\
\n\
3. **`<!-- TODO: unknown -->`** — placeholder when a previous validation pass left a question unanswered. Rare. Don't introduce new ones; only preserve existing.\n\
\n\
**Default behaviour**: if you can verify, verify and write the result. Markers are escape hatches, not opt-out tokens. A doc with 25 `TODO: verify` markers from a non-interactive audit usually means the agent gave up on verification — try harder.\n\
\n\
This is an autonomous (non-interactive) pass. Do NOT ask questions inline — use the marker discipline above and move on.";

#[derive(Clone, Copy)]
pub(crate) struct AnalysisStep {
    pub(crate) target_file: &'static str,
    pub(crate) prompt: &'static str,
    /// Source file patterns to hash for drift detection.
    /// Special values: "__GIT_SOURCE_TREE__" (F27 content+structure fingerprint of the
    /// source tree, excluding Kronn outputs — the preferred whole-repo signal);
    /// "__GIT_HEAD__"/"__GIT_LS_FILES__" (legacy, kept for back-compat only).
    /// Glob patterns: "*.json", ".github/workflows/*"
    /// Empty = step is excluded from drift detection.
    pub(crate) sources: &'static [&'static str],
}

/// 0.8.7 — Anti-hallu canonical section body (without the `<!-- kronn:section -->`
/// wrappers, which are written dynamically with the current audit date).
///
/// SOURCE OF TRUTH for the anti-hallucination protocol. Mirrored in
/// `templates/docs/AGENTS.md` (the new-project bootstrap copy) — a test in
/// this module asserts the two stay in sync.
///
/// Lives here (not in `core::anti_halluc`) because the audit step 0 is the
/// only writer ; the runtime `PREAMBLE` now points to this section instead
/// of duplicating it.
pub(crate) const ANTI_HALLU_SECTION_BODY: &str = "\
## 0. Anti-Hallucination Protocol\n\
\n\
Never state a non-trivial technical fact (paths, APIs, config, versions, behaviour) without proof — read the code, then `docs/`, then official docs, then ask; never guess. Cite every assertion as `[src: file: <path>:<line>]` or `[src: url: <url>]`; a citation that doesn't resolve is rejected as fabricated.\n\
\n\
Full grammar and cascade: [`docs/conventions/agents-md-format-v1.md`](conventions/agents-md-format-v1.md).\n\
";

/// 0.8.7 — Audit STEP 0 doctrine (used by the `apply_anti_hallu_section`
/// deterministic function in `anti_hallu_step.rs`, NOT as an LLM prompt).
///
/// The pre-step is implemented in Rust because it's 100% mechanical
/// (read → edit → write). No need to burn tokens or risk hallucination
/// on an idempotent file edit. This const stays as documentation for
/// the case where an external tool wants to invoke an agent on this
/// step or to surface the doctrine via the audit detail UI.
///
/// **Separate const, NOT in `ANALYSIS_STEPS`**, because the existing
/// drift detection API (`drift.rs:131, 211`) uses 1-indexed step numbers
/// that map to `ANALYSIS_STEPS[step - 1]`. Inserting STEP 0 would shift
/// everything.
#[allow(dead_code)] // doc-only constant ; the real work is in anti_hallu_step.rs
pub(crate) const STEP_0_ANTI_HALLU: AnalysisStep = AnalysisStep {
    target_file: "docs/AGENTS.md",
    prompt: "\
This is the FIRST step of the audit. Your job is to ensure `docs/AGENTS.md` carries the canonical anti-hallucination protocol section that every subsequent step (and every future agent reading this file) will rely on.\n\
\n\
## Algorithm\n\
\n\
1. Read `docs/AGENTS.md`.\n\
2. Search for the opening marker `<!-- kronn:section name=\"anti-hallu\"`.\n\
3. **If found** : refresh ONLY the `audit=\"<date>\"` attribute on the opening marker (set it to today's date, ISO YYYY-MM-DD). Do NOT touch the body — `owner=\"audit\"` means Kronn owns the content. Leave the closing `<!-- kronn:section:end -->` marker untouched.\n\
4. **If NOT found** : insert the canonical block (exactly as below) immediately after the file's H1 (`# AI agent context — Entry point`) and BEFORE the next `---` separator. Preserve everything else.\n\
\n\
## Canonical block to write (EXACT, no edits)\n\
\n\
```markdown\n\
<!-- kronn:section name=\"anti-hallu\" curated=\"ai\" owner=\"audit\" audit=\"<TODAY YYYY-MM-DD>\" -->\n\
## 0. Anti-Hallucination Protocol\n\
\n\
Never state a non-trivial technical fact (paths, APIs, config, versions, behaviour) without proof — read the code, then `docs/`, then official docs, then ask; never guess. Cite every assertion as `[src: file: <path>:<line>]` or `[src: url: <url>]`; a citation that doesn't resolve is rejected as fabricated.\n\
\n\
Full grammar and cascade: [`docs/conventions/agents-md-format-v1.md`](conventions/agents-md-format-v1.md).\n\
<!-- kronn:section:end -->\n\
```\n\
\n\
## DO NOT\n\
\n\
- Do NOT touch any OTHER section of `docs/AGENTS.md` in this step (placeholders, tables, etc. are handled by subsequent steps).\n\
- Do NOT add commentary, summary, or step-numbering. Output the file silently after the write.\n\
- Do NOT escalate (no `<!-- TODO: ask user -->`). This step is purely mechanical.",
    sources: &["docs/AGENTS.md"],
};

pub(crate) const ANALYSIS_STEPS: &[AnalysisStep] = &[
    // Step 1: Project analysis + index
    AnalysisStep {
        target_file: "docs/AGENTS.md",
        prompt: "\
Read README.md, package.json (or composer.json, Cargo.toml, go.mod), Makefile, Dockerfile, docker-compose.yml, \
CI configs (discover tracked workflow files, Jenkinsfiles, pipeline/build scripts and workspace manifests before selecting relevant ranges), and main config files.\n\
Determine: tech stack, project structure, build/dev/test commands, key patterns, third-party services, CI/CD pipeline.\n\n\
Then fill docs/AGENTS.md — replace ALL {{PLACEHOLDERS}} in each section:\n\
- {{PROJECT_NAME}} and {{STACK_SUMMARY}}: project name and one-line stack description\n\
- {{PROJECT_LANGUAGE}}: language used for code comments, commit messages, and identifiers\n\
- Common tasks table: replace {{TASK_1}}..{{TASK_4}} with real task→file mappings (add/remove rows to match the project — see the general rule above)\n\
- Prerequisites: retain the pointer to operations/debug-operations.md; Step 7 fills its prerequisites there. Do not copy its commands into this entry point\n\
- DO NOT rules: replace {{DO_NOT_1}}, {{DO_NOT_2}} with project-specific rules (add more rows if warranted)\n\
- Source of truth: retain the repo-map.md pointer. Source locations and code placement belong there. On older templates, replace the source inventory with that pointer, not a duplicate inventory\n\
- Stack: retain the repo-map.md pointer. On older templates, replace the detailed stack table with that pointer. Keep only a one-line STACK_SUMMARY in the header; detailed technologies, versions and roles belong in repo-map.md\n\
- Workflow constraints: replace {{WORKFLOW_CONSTRAINT_1}}, {{WORKFLOW_CONSTRAINT_2}} with project-specific rules\n\
- {{DOCS_LANGUAGE}}: language for new project documentation. If an existing doc is already written in another language, KEEP it as-is — do not force-translate it\n\
- {{TICKET_LANGUAGE}}: language used for tickets\n\
- {{COMMENT_TICKET_POLICY}}: whether code comments may cite a ticket/issue ID\n\
- {{TEST_POLICY}}: this project's test policy (default: every code change needs a test — see docs/testing-quality.md)\n\
- {{DATE}}: set to today's date (YYYY-MM-DD)\n\
\n\
**CLEANUP ONCE FILLED** — the template ships with scaffolding that MUST NOT survive the fill: \
remove the `> **TEMPLATE FILE.** Every {{...}} MUST be filled…` banner block entirely, and strip \
every inline bracketed hint left next to a value (e.g. `[ex: \"French\", \"English\" — …]`). \
A filled file that still shows the banner or hints counts as a FAILED fill.\n\
\n\
**ANTI-HALLU FOR THIS STEP** — anchor each non-trivial project fact once beside the claim using `[src: file: <path>:<line>]`. Versions must cite their manifest/lockfile. Routing links are navigation, not evidence: do not repeat the same citation in every cell or invent anchors for future generated files. If a fact cannot be established, omit it or state the uncertainty. Keep this entry point short: project parameters, critical rules, task routes and pointers; detailed inventories belong in the routed documents. {{DATE}} needs no citation.",
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
- {{STACK_OVERVIEW}}: concise technologies, versions and roles, with manifest/lockfile evidence; this is their canonical home (do not duplicate the table in AGENTS.md)\n\
- Key folders: replace the {{KEY_FOLDERS}} block with every major directory (2-3 levels deep), tree format with annotations\n\
- Entrypoints: replace the {{ENTRYPOINTS}} block with 5-7 key files (config, routes, models, etc.)\n\
- Auto-generated files: replace the {{GENERATED_FILES}} block with files NOT to edit manually",
        sources: &["__GIT_SOURCE_TREE__"],
    },

    // Step 4: Coding rules
    AnalysisStep {
        target_file: "docs/coding-rules.md",
        prompt: "\
Read docs/AGENTS.md for context. Find ALL linter, formatter, and type-checker configs in the repo \
(e.g. .eslintrc, eslint.config.js, prettier, rustfmt.toml, tsconfig.json, phpcs.xml, etc.).\n\n\
Fill docs/coding-rules.md — replace ALL {{PLACEHOLDERS}}:\n\
- Replace {{LANGUAGE_1}}, {{LANGUAGE_2}} with one section per language/framework used in the project (add more sections if the project uses more than two)\n\
- For each language, fill its {{TOOLS_1}}/{{TOOLS_2}} block with the linter, formatter, and type checker — config file and run command for each\n\
- Replace {{CONVENTIONS_1}}, {{CONVENTIONS_2}} with coding conventions OBSERVED in existing code (naming, error handling, imports). Write fewer rather than inventing.\n\
- Replace {{MISTAKES_1}}, {{MISTAKES_2}} with common mistakes to avoid (from linter configs, framework gotchas observed in code)\n\
- If no linter/formatter is configured, write 'Not configured' in its place\n\
- Add or remove language sections as needed to match the actual project stack\n\
\n\
**ANTI-HALLU FOR THIS STEP** — {{CONVENTIONS_*}} and {{MISTAKES_*}} are the #1 vector for training-data leakage into AGENTS.md. Apply this rule strictly:\n\
- Every CONVENTION row MUST cite the rule's source: either a linter config (`[src: file: .eslintrc.json:12]`, `[src: file: rustfmt.toml:3]`) OR at least 3 observed call sites (`[src: file: src/foo.rs:120]`, `[src: file: src/bar.rs:45]`, …).\n\
- Every MISTAKE row MUST cite either a linter rule that enforces the prohibition OR a real bug found in the repo's git history (`[src: commit: <sha>]`).\n\
- A rule you 'know from the language ecosystem' but cannot anchor in this repo is NOT a project convention — do NOT add it. Write fewer rules with proof, not more rules from memory.",
        sources: &["package.json", "Cargo.toml", "tsconfig.json", "rustfmt.toml", "pyproject.toml"],
    },

    // Step 5: Testing & quality
    AnalysisStep {
        target_file: "docs/testing-quality.md",
        prompt: "\
Read docs/AGENTS.md for context. Find test framework configs (jest, vitest, phpunit, pytest, cargo test, bats, etc.) \
and CI quality gates.\n\n\
Fill docs/testing-quality.md — replace ALL {{PLACEHOLDERS}}:\n\
- Build & quality checks: replace the {{BUILD_CHECKS}} block with a table of all quality checks (compile, lint, format, test, build) and their commands\n\
- Test infrastructure: replace the {{TEST_INFRASTRUCTURE}} block with the runner, config file, and Node/runtime version for each language\n\
- Test suites: replace the {{TEST_SUITES}} block with each test file/suite and its approximate count\n\
- Coverage: replace the {{COVERAGE}} block with the current status and target\n\
- Replace {{UNTESTED}} with components that have NO tests\n\
- Smoke checks: replace the {{SMOKE_CHECKS}} block with 3-5 quick pre-commit commands",
        sources: &["package.json", "Cargo.toml", "pyproject.toml"],
    },

    // Step 6: Architecture overview
    AnalysisStep {
        target_file: "docs/architecture/overview.md",
        prompt: "\
Read docs/AGENTS.md and docs/repo-map.md for context. Analyze the high-level architecture.\n\n\
Fill docs/architecture/overview.md — replace ALL {{PLACEHOLDERS}}:\n\
- Apps/services: replace the {{SERVICES}} block with a table of service, port, tech, and role for each service\n\
- Key patterns: replace the {{PATTERNS}} block with 3-5 architectural patterns \
  (API pattern, state management, auth, data flow, caching, etc.) — 2-3 sentences each\n\
- {{SEPARATION_DESCRIPTION}}: how the codebase is organized (by feature, by layer, etc.)\n\
- {{DATA_FLOW_DESCRIPTION}}: 2-3 sentences on how data moves through the system\n\
- Legacy: replace the {{MIGRATIONS}} block with a table of area, current state, and target for any legacy patterns or planned migrations\n\n\
**Mermaid diagrams** — mandatory, REPLACES the old ASCII placeholder:\n\
1. **Architecture overview** — replace `{{ARCHITECTURE_MERMAID}}` with a `flowchart TD` (or `LR` for wide projects) inside a ```mermaid fence. \
Show every service from the table above, the main data flow direction (HTTP, DB, message bus, etc.), and external systems (APIs, third-party providers). \
Use `subgraph` blocks to group related components. If the project warrants it (multi-tier app, hexagonal arch), \
simulate **C4-style layers** with named subgraphs (`Person`, `System`, `Container`, `Component`) — still in Mermaid syntax, no PlantUML/Structurizr.\n\
2. **Sequence diagrams** — for the 2-3 MOST CRITICAL flows you can identify in the code (auth, primary request lifecycle, deploy/CI pipeline, payment, etc.), \
write ONE file per flow under `docs/architecture/sequences/<flow-name>.md` using `sequenceDiagram` Mermaid syntax. \
Hard cap: **3 files maximum**. Quality > quantity — if you only identify ONE clear critical flow, write only one. \
Each file must include a 2-3 sentence intro before the diagram (\"This sequence describes how a user authenticates against the API. It starts when the client POSTs to /auth/login and ends with a JWT in the response cookie.\").\n\n\
**Mermaid sequenceDiagram safety rules** — Mermaid 11.x parser is unforgiving on message strings; respect these or the diagram won't render and the user sees a parse error instead of a flow:\n\
- **Message text is ASCII-only**. Replace `…` with `...`, `→` with `->`, em-dash with `-`. Unicode punctuation routinely trips the parser.\n\
- **Avoid `:` and `;` inside message text**. The first `:` after the arrow is the separator (`A->>B: msg`), but additional `:` / `;` combined with parens or Unicode can confuse the lexer. Rephrase: write `Cache-Control maxage=604800` instead of `Cache-Control: maxage=604800`, `Link rel=preload` instead of `Link: ...; rel=preload`.\n\
- **No literal `(`/`)`/`[`/`]`/`{`/`}` chains** inside a message. Short, declarative prose only: `301 redirect to /a-propos` not `301 Location: /a-propos (set by LocaleRedirectSubscriber)`. If you need the detail, add a `Note over X` block.\n\
- **Keep each line ≤ 100 chars**. Long lines hide parser-state issues. Break into multiple messages or a `Note over` block.\n\
- **Participant/actor aliases must NOT be Mermaid reserved words** (case-insensitive): `alt`, `else`, `end`, `opt`, `loop`, `par`, `and`, `rect`, `note`, `critical`, `break`, `activate`, `deactivate`, `box`. Declaring `participant Alt as AlternateLocaleSubscriber` then writing `Caddy->>Alt: msg` makes the lexer read `Alt` as the start of an `alt` block → parse error. Pick a non-keyword alias (`AltLoc`, `AltSub`, `Alternate`). Same for `flowchart` node ids — never name a node `end`/`alt`/etc.\n\
- Test mentally: would `mermaid.parse` accept this verbatim? If unsure, simplify.\n\n\
**Why Mermaid + file separation**: every viewer (GitHub, GitLab, Obsidian, VS Code) renders Mermaid natively — no external tools. \
Sequence diagrams live in separate files so `docs/AGENTS.md` T0 stays small; an agent only loads them when working on the related flow.",
        sources: &["docker-compose.yml", "src/main.*", "src/lib.*", "src/index.*"],
    },

    // Step 7: Debug operations
    AnalysisStep {
        target_file: "docs/operations/debug-operations.md",
        prompt: "\
Read docs/AGENTS.md for context. Find operational commands from Makefile, package.json scripts, \
docker-compose commands, and any run/build/debug procedures.\n\n\
Fill docs/operations/debug-operations.md — replace ALL {{PLACEHOLDERS}}:\n\
- Prerequisites: fill {{PREREQUISITES}} with runtime/tool versions anchored to manifests, Dockerfiles or CI; this is their canonical home, not AGENTS.md\n\
- Common commands: replace the {{COMMANDS}} block with a table of action → command for start, stop, logs, test, build, deploy\n\
- Docker services: replace the {{DOCKER_SERVICES}} block with a table of service, port, role, and health check for each container\n\
- Troubleshooting: replace the {{TROUBLESHOOTING}} block with 3-5 common issues (symptom, cause, fix)",
        sources: &["docker-compose.yml", "Makefile", "Dockerfile"],
    },

    // Step 8: Inconsistencies & tech debt
    //
    // 0.8.2 — Step 8 is the load-bearing step of the audit. It must be
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
Real issues only, not hypothetical. Read current docs guidance (exclude dated `docs/reports/` snapshots) AND scan source code. \
Scan: entry points, config files, Dockerfiles, CI configs, and 5-10 core source files \
(prioritize auth, data persistence, external input handling).\n\n\
# A. MANDATORY BASELINE CHECKLIST (never skip, never trim)\n\
\n\
These are well-known defaults that silently break security/ops in production. \
For each item, establish applicability and check equivalent controls elsewhere before emitting a TD. \
Record applicable / covered elsewhere / not applicable / not verified with evidence. The applicability/equivalent-control rule governs every emit-TD shorthand below. Unknown deployment context is not evidence of a production defect. Only an established defect produces a TD; these do NOT count \
against the cap (see § D point 5). Be explicit when an item is satisfied (\"verified \
present\", \"verified absent\") so the user can trust the audit.\n\n\
\n\
**If a `Dockerfile` exists** (in any subdirectory of the project root):\n\
- `USER <non-root>` directive present (otherwise: container runs as root — emit TD).\n\
- `display_errors = Off` for prod env (otherwise: stack traces leak in HTTP responses — emit TD).\n\
- `opcache.enable = 1` for PHP projects (otherwise: every request re-compiles — perf disaster — emit TD).\n\
- Health detection: inspect Docker HEALTHCHECK and deployment readiness/liveness probes. A missing Docker directive alone is not a defect; emit a TD only for an evidenced operational gap in the deployed configuration.\n\
- `apt-get install` uses `--no-install-recommends` AND `rm -rf /var/lib/apt/lists/*` (otherwise: image bloat + outdated packages — emit TD if either missing).\n\
- Base image has a tag pinned to a specific digest or version (not `latest` — otherwise: non-reproducible builds — emit TD).\n\
- No `ADD <remote-url>` (only `RUN curl … && verify-checksum`; otherwise: silent supply-chain risk — emit TD).\n\
- Secrets are NOT passed via `ARG` (build args are visible in `docker history` — emit TD if found).\n\n\
\n\
**If a `docker-compose.yml` or `compose.yaml` exists**:\n\
- Resource limits (`mem_limit`/`cpus` or `deploy.resources.limits.*`) on services that can realistically run away (app, workers, DB). Missing limits are `Medium` at most — and NOT a finding when the compose file is clearly dev-only or the platform (Swarm/K8s) owns limits elsewhere; say which case applies.\n\
- `read_only: true` where the service is stateless (otherwise: writable rootfs is a useful RCE escalation surface — emit TD only if clearly stateless).\n\
- No `:latest` image tags (otherwise: non-reproducible — emit TD).\n\
- Credential hygiene of `environment:` blocks: `Not evaluated safely (requires Kronn's local secret scanner)`. Safe signal only: `grep -lE 'secrets:|env_file:' <compose files>` (filename-only) tells you whether the safe mechanisms are used at all.\n\n\
\n\
**If `.github/workflows/*.y(a)ml` OR `.gitlab-ci.yml` OR equivalent CI config exists**:\n\
- No `StrictHostKeyChecking=no` in ssh/scp calls (otherwise: trivial MitM — emit TD).\n\
- Deploy workflow has a quality gate (lint OR test OR static analysis) before pushing to prod (otherwise: broken code can ship — emit TD).\n\
- CI secret sourcing: `Not evaluated safely (requires Kronn's local secret scanner)` — detecting a hardcoded literal means reading the line that contains it. Safe signal only: `grep -l 'secrets.' <workflow files>` (filename-only) shows whether the secure mechanism is used at all.\n\
- No `pull_request_target` without explicit checkout of the base branch's workflow definition (RCE risk — emit TD if pattern found).\n\
- Pinned action versions (`uses: actions/checkout@v4` not `@master`) (emit TD only if drift is dramatic, e.g. pinning to a moving branch).\n\n\
\n\
**If `.env*` files exist at repo root**:\n\
- No `.env` file tracked in git (only `.env.dist` / `.env.example`) — check `git ls-files .env*` (otherwise: emit TD).\n\
- Values inside tracked `.env*` files: `Not evaluated safely (requires Kronn's local secret scanner)` — never open or pattern-scan their content. The tracking status itself (`git ls-files -- '*.env*' '.env*'`, quoted pathspec, filename-only) is the safe signal and already a finding when a real `.env` is tracked.\n\
- Deploy step EXCLUDES `.env*` from the deploy bundle (rsync/scp/tar `--exclude .env*`) — otherwise the tracked `.env.dist` lands in production with its hardcoded defaults (emit TD if not excluded).\n\n\
\n\
**For any web project** (HTML templates detected — Twig / Blade / JSX / Vue / ERB / Razor — > 3 template files):\n\
- Form `<input>` elements have associated `<label>` or `aria-label` (sample 3-5 templates; emit TD if pattern of missing labels).\n\
- `<img>` elements have `alt=` attribute (sample; emit TD if pattern of missing alt).\n\
- No inline `<script>` or `<style>` longer than ~50 lines per template (CSP risk + Asset Mapper bypass — emit TD if pattern).\n\
- Content-Security-Policy header set (check controllers/middleware/web server config — emit TD if absent on a project that touches user input).\n\n\
\n\
**Community standards** — gated on OSS intent. Run this block ONLY with evidenced public/open-source distribution intent. Explicit private/internal scope takes precedence over the signals below: \
(1) project documentation explains a license for public redistribution (a LICENSE file alone is insufficient), \
(2) the project documentation explicitly states public/open-source distribution (a GitHub/GitLab hostname alone proves neither public visibility nor OSS intent), \
(3) the README explicitly invites external public contributions; a keyword such as contributing alone is insufficient. \
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
**Repository hygiene** — universal (not gated on OSS intent). Use `git ls-files --cached` so only tracked/index content is judged; do not flag harmless local/untracked developer files:\n\
- Package-manager caches (`.pnpm-store/`, `.npm/`, `.yarn/cache/`, `.gradle/caches/`, `.cargo/registry/`) are not tracked. They are generated, large and machine-specific (emit TD: Low when tracked), except a documented Yarn zero-install/offline-mirror setup (`enableGlobalCache: false` plus repository documentation).\n\
- Runtime databases/logs/dumps (`*.db`, `*-wal`, `*-shm`, `*.log`, `*.trace`, `*.dump`) are not tracked from the repo root or cache/runtime/state/tmp directories. Test fixtures are legitimate and must not be flagged solely by extension.\n\
- Ad-hoc audit/session handoff notes and generated review/test outputs (`findings/`, `playwright-report/`, `test-results/`) are ignored or stored in the project's documented artifact location, not tracked at the root by an agent run (emit TD: Low when clearly generated).\n\n\
\n\
The 7 categories above are MANDATORY when their gating condition matches. Even if you find 30 other findings, you MUST scan them. \
Repository hygiene is always mandatory. Deterministic evidence is bounded to 15 paths per detector plus an explicit remainder count; the finding cap in § D point 5 applies AFTER these baseline findings are emitted.\n\n\
# B. DIMENSIONAL SCAN — every dimension is ACCOUNTABLE (on top of the baseline)\n\
\n\
This is where audits silently under-deliver: the baseline (§ A) is easy + deterministic, so the model finds it and STOPS. \
You MUST account for EACH of the 10 dimensions below — none may be silently skipped. \
For EVERY dimension, record exactly ONE outcome in the `## Dimension coverage` matrix of the index file:\n\
  - **findings** — one or more TDs (each with `[src: file:…]` evidence), OR\n\
  - **scanned — nothing substantiable** — you actually opened the relevant code and found nothing you can anchor (this is NOT \"I didn't look\"), OR\n\
  - **N/A: <verifiable reason>** — the dimension cannot apply, with a reason a human can CHECK (\"no Dockerfile\", \"no DB/ORM layer\", \"no web surface\", \"no CI config\") — NEVER a vague \"not relevant\".\n\
A dimension absent from the matrix, or marked with an unverifiable reason, = **incomplete audit**: a cheap structural validator FAILS this step (it gets re-run) on a missing / short / ill-formed matrix. The matrix **records** breadth + makes it reviewable (it is a declaration, not proof the scan happened — the detectors will anchor that); the TDs carry the depth.\n\n\
The 10 dimensions:\n\
- **Dependencies**: compare root/workspace manifests, lockfiles, imports, scripts and build/CI tooling. Check undeclared tools, unused/suspect declarations and duplicated workspace declarations; search callers and dynamic/plugin loading before claiming unused. Record versions exactly; freshness/EOL requires current evidence, never memory.\n\
- **Security**: safe credential metadata only (value scans remain excluded), inbound AND outbound authentication/transport, injection vectors (SQL/XSS/command), insecure defaults and debug endpoints. Inspect HTTP/search/database client verification options without exposing credential values; TLS termination on the server does not cover outbound clients.\n\
- **Code quality**: functions >50 lines, god classes, SRP violations, dead code, error swallowing (empty catch / let _ = / unwrap_or_default on Result).\n\
- **Scalability**: N+1 requests/queries, unbounded loops or result accumulation, pagination bounds, cache capacity/eviction and memory growth, indexes on hot queries. HTTP/search clients and in-memory caches count even without an ORM.\n\
- **Maintainability**: tight coupling, circular dependencies, missing critical-path tests, unclear naming, mixed languages. Inspect EACH discovered CI/build entry point for manifest/lockfile/runtime drift and destructive cleanup, including scripts referenced by the workflows; one working CI provider does not establish that another is sound.\n\
- **Accessibility (a11y)**: covered by baseline if web project; on libraries/CLIs surface API a11y (machine-readable output, --quiet flag for scripts, etc.).\n\
- **Observability**: missing logging in hot paths (auth, payment, write endpoints), no error tracker (Sentry/Glitchtip/equivalent), no health/readiness endpoints, no metrics on critical SLI.\n\
- **Compliance**: GDPR issues (external resources, data retention), license incompatibilities (`composer licenses` / `cargo deny` / `license-checker`).\n\
- **Performance**: whenever network, rendering, storage or worker code exists, inspect request timeouts/retries/cancellation, cache bounds and response accumulation. Prioritize using briefing/README; absence of an explicit performance requirement does not make these mechanisms N/A. Web assets and cache headers apply only to the relevant surface.\n\
- **Documentation drift**: cross-check the docs/ files you just wrote (steps 1-7) against the source code. Flag concrete contradictions (e.g. `coding-rules.md` says X is enforced but no linter rule exists, `testing-quality.md` claims N tests but actual count differs, `repo-map.md` points to a path that no longer exists).\n\n\
**Self-critic before you finish § B** (cheap, catches the usual misses — do it, don't skip):\n\
- For every dimension marked \"scanned — nothing substantiable\", name the relevant source scope inspected. A clean result is valid; if the scope was not inspected, mark the gap rather than inventing a finding.\n\
- Any mechanically-obvious signal you passed over? e.g. a committed secret/API key, a `target=\"_blank\"` without `rel=\"noopener\"`, `0` test files for N source files (a distinct TD from \"no CI gate\"), an `unsafe-eval`/`unsafe-inline` CSP, a god-file >500 lines. If it's real → add the TD; if intentional → say so in the matrix evidence column. Don't leave it silently undetected.\n\
- The 10-row coverage matrix is the CONTRACT for this step: a reviewer must be able to see, per dimension, that you looked and what you concluded.\n\n\
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
The pipeline will run a reconciliation pass after Step 8 to classify them definitively.\n\
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
   - **Verified in source**: the cited source establishes the stated defect under the named conditions, after checking relevant counter-evidence. Reading a file or resolving a citation alone is insufficient. This status skips human re-confirmation, not source-quality review; never use it merely as a default.\n\
   - **Inferred**: applicability, impact or behavior remains unverified, including when you read the source but it does not establish the claim. State the missing evidence; do not invent it.\n\
   - **Blocked upstream**: depends on a third-party fix (vendor bug, language version bump, framework deprecation cycle).\n\
   - **Mitigated**: partial fix shipped, residual work tracked.\n\
   - **Confirmed by user**: only set by the validation phase after user confirms.\n\
   - **Rejected**: only set by the validation phase if user rejects — the next audit will NOT recreate this TD.\n\
   - **Accepted decision**: only set by the validation phase when the user records the TD as an intentional trade-off in `docs/decisions.md`.\n\
   - **Deferred**: only set by the validation phase when the user postpones the TD decision.\n\
\n\
5. **Cap**: target 30 findings maximum. Critical + High findings (including all baseline checklist failures) are NEVER trimmed to fit the cap. Trim Medium and Low if needed. If you cannot stay under 30 even after trimming all Low, note the count of trimmed Medium in the index's `## Coverage gaps` section.\n\
\n\
6. **Tracker MCP dedup**: if a ticket tracker MCP is configured (Jira/Linear/GitHub Issues), before creating a NEW TD file, do a read-only search for an existing open ticket with a matching title fragment. If found: set `Next step: link existing ticket <URL>` instead of `create ticket`. Avoid duplicating tracked work.\n\
\n\
7. **No issues found**: single row in the index: `'None identified during initial audit'`. Zero findings is valid when the applicable checks found no substantiated defect; never invent a finding to fill the list.\n\
\n\
8. **Dimension coverage matrix — MANDATORY**: fill the `## Dimension coverage` table in the index — ALL 10 rows, each with an outcome per § B (`findings` / `scanned — nothing substantiable` / `N/A: <verifiable reason>`) and an evidence/reason cell. A blank row, or an `N/A` whose reason a human can't verify, = incomplete audit. **If the `## Dimension coverage` section is absent** (project installed before this section existed), CREATE it with the 10 dimensions. This is the breadth contract that stops the scan from silently skipping dimensions.\n\
\n\
**Detail file format**: use the canonical TD template included below in the shared evidence contract. It is also shipped as `docs/tech-debt/TEMPLATE.md`; keep that reusable template unchanged.\n\
\n\
For UPDATES of existing TDs: APPEND a new entry to `audit_history`, do not replace previous entries. The chronological list is the value.\n\n\
\
**Scope reminder**: this step ONLY fills `docs/inconsistencies-tech-debt.md` + creates/updates `docs/tech-debt/TD-*.md` detail files. The companion `docs/decisions.md` is intentionally filled in the final consolidation (not here), after the specialized audits so the agent has the full audit picture before recording positive choices.",
        sources: &["__GIT_SOURCE_TREE__"],
    },

    // Foundation step 9: consolidation, scheduled last in the full chain
    //
    // Step 9 has a *real* target_file (`docs/decisions.md`) so the
    // validate_step_output guard catches the case where the
    // agent forgets it. The prompt is a TWO-PHASE pass:
    //   (1) Final quality review across current routed docs (reports excluded).
    //   (2) Fill docs/decisions.md with intentional architectural
    //       choices observed during steps 1-8 (kept out of the tech-debt
    //       prompt, where it sat 200 lines deep and was systematically
    //       forgotten).
    //
    // The two phases are deliberately ordered: review first (catches
    // contradictions + cleans up markers), then decisions.md (last
    // free-form write where the agent has the full picture).
    AnalysisStep {
        target_file: "docs/decisions.md",
        prompt: "\
This is the FINAL consolidation, executed after all specialized audits. Execute the two phases in order:\n\
\n\
# PHASE 1 — Targeted quality review (reports excluded)\n\
\n\
Use the run's generated domain indexes and existing detector/known-debt evidence first. List current docs and search for contradictions, duplicate TD root causes, unresolved markers and broken links. Read the affected sections and source only; do not reread every doc or historical TD. EXCLUDE docs/reports/ and preserve human-owned sections.\n\
\n\
- Reconcile the general dimension coverage with specialized findings (including N/A claims). Reuse one TD ID for the same root cause across domains; update references before removing a duplicate generated in this run. Preserve prior/user-owned TDs.\n\
- In inconsistencies-tech-debt.md, add a compact Domain routes table linking each existing specialized index. Every TD must be linked from at least one relevant index. Do not copy every specialized finding into the general index. Known paths may be accessed directly; links do not require loading all targets.\n\
- No duplicated facts across files: keep detailed stack/source inventory in repo-map.md and prerequisites/commands in operations/debug-operations.md. Keep AGENTS.md as essential rules and routing; retain required parameters and protected human sections.\n\
- Run a bounded structural sweep over current docs (exclude reusable TEMPLATE.md/_template.md, conventions examples, reports and protected human sections). List only defects: unresolved {{...}} or bracketed template hints, empty required TD sections, invalid field/history values, broken Markdown links and TDs missing an index link. A bare TD ID is not a link. Inspect and repair the affected files, then rerun the checks; do not claim coverage from prose alone. Fill auxiliary workflow/environment documents from evidence or explicit unknowns, without inventing policy. Keep the generated AGENTS.md within its 800-word budget by moving detail to its canonical file, preserving mandatory rules and human sections.\n\
- Marker discipline: keep <!-- TODO: ask user --> and <!-- TODO: unknown --> for genuine human decisions. For <!-- TODO: verify -->, attempt a targeted source check; resolve it or state the remaining uncertainty and convert to <!-- TODO: ask user --> only if human input is needed. Do not turn an unknown into a factual claim.\n\
- Source-quality review still applies to Verified in source findings. Reconcile coverage gaps and N/A claims with the source inventory; check negative claims against actual client/response/CI code. Read the affected ranges, not whole trees. A valid path/line is not proof of the claim. Correct unsupported assertions and add concrete Verification criteria without invented commands, thresholds or policy. Repair audit-created YAML/history fields together with the body; no external owner will fix malformed audit output.\n\
\n\
# PHASE 2 — Fill docs/decisions.md\n\
\n\
Record intentional architectural choices only when their rationale is explicitly documented (ADR, briefing, project documentation) or user-confirmed. Source code proves implementation, not intent. No inferred rationales or invented prohibitions.\n\
Each row contains Decision, Why chosen, What NOT to do (only if evidenced), and Source with [src: file: path:line] or a user confirmation. Unknown intent remains unknown.\n\
There is no minimum decision count. Zero evidenced decisions is a valid result: state that explicitly and remove unused rows. Never pad to a quota.\n\
\n\
End state: docs/decisions.md contains no {{...}} placeholders; current domain summaries agree with specialized evidence; relevant index links reach every TD without requiring a global flat list. Report what you verified and remaining uncertainty.",
        sources: &["__GIT_SOURCE_TREE__"],
    },
];

// ─── Specialized audit kinds (0.8.2 "Design C") ──────────────────────────
//
// Each focused kind reuses Step 8's prompt scaffolding but narrows the
// scope. The Full foundation pipeline keeps all 9 steps. Specialized kinds skip
// the docs-generation steps (1-8) because the user only wants a focused
// re-scan, and they emit findings into a *named* tech-debt index file
// per kind (e.g. `docs/inconsistencies-security.md`) so they don't
// clobber the Full audit's `docs/inconsistencies-tech-debt.md`.
//
// S2.D3-D5 will replace the placeholder bodies below with vetted
// prompts. The dispatcher (`kind_to_steps`) is stable now so the
// front-end can already wire kind selection.

pub(crate) const DRIFT_STEPS: &[AnalysisStep] = &[AnalysisStep {
    target_file: "REVIEW",
    prompt: "\
DRIFT AUDIT (0.8.2) — placeholder body, content lands in S2.D3.\n\n\
Read docs/checksums.json. For every (file, sha256) recorded there, recompute the sha and \
list the files where the recorded hash no longer matches. Do NOT rewrite any file — \
your output is a short bullet list of stale docs/ files only.",
    sources: &["docs/checksums.json"],
}];

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
- No REAL `.env*` file is tracked (`git ls-files -- '*.env*' '.env*'` — pathspec listing, no grep) — `.env.example`/`.env.dist`/`.env.sample` are legitimate templates and NOT findings. Tracking status and file NAMES are the ONLY safe signals you may collect.\n\
- **Secret VALUES are out of your scope entirely.** You may not open, grep, or otherwise inspect file contents for secret-looking values — any command whose output could echo a matched line would pull the secret into your context, which leaves the machine. For the value-scan sub-dimension, write exactly `Not evaluated safely (requires Kronn's local secret scanner)` — NEVER `verified absent`/`verified clean` on a scan you did not safely run.\n\
- CI YAML / docker-compose credential hygiene: `Not evaluated safely (requires Kronn's local secret scanner)` — proving the absence of a hardcoded credential means reading the very content that could contain it. You may only note the SAFE structural facts: which workflow/compose files exist, and whether they use `secrets:`/`env_file:` — established EXCLUSIVELY via `grep -lE 'secrets:|env_file:' <files>` (filename-only output, zero lines, zero context; `-l` is the ONLY grep flag allowed near credential material).\n\
- No private keys or certs in repo (`*.pem`, `*.key`, `*.p12`, `id_rsa*`).\n\
- `git log --all -S 'BEGIN PRIVATE KEY' --name-only --format='%h %ad'` returns nothing recent (last 12 months) — metadata only, NEVER `-p` (a patch dump would pull the key itself into your context).\n\n\
\
**EXFILTRATION GUARD (absolute)**: you run with filesystem access and your context leaves the machine. NEVER open, quote, print or write the VALUE of any secret, key or token — in your output, in docs/, in TDs, anywhere. Findings carry file + line + pattern type only.\n\n\
\
**Authentication & sessions:**\n\
- Session cookies set with `HttpOnly` AND `Secure` AND `SameSite=Lax|Strict` (grep for `setcookie`, `Set-Cookie`, `withCredentials`).\n\
- Password storage uses a slow hash (`bcrypt`, `argon2`, `scrypt`, `PBKDF2`) — not MD5/SHA1/plain.\n\
- JWT secret sourcing and hardcoded `Bearer` tokens: `Not evaluated safely (requires Kronn's local secret scanner)` — same boundary, the match line would BE the secret. You may note the safe structural fact that a JWT/auth library is configured (dependency name from the manifest) without inspecting its key material.\n\
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
- Outbound HTTP, search and database clients: certificate AND hostname verification, trust configuration and authentication wiring. Look for explicit bypass settings (for example `rejectUnauthorized: false`, `verify=False`, or insecure TLS flags), and trace whether production callers can reach them. Inspect only safe options/call sites, never secret values. A reverse proxy securing inbound traffic does not secure these outbound connections.\n\
- HTTPS enforced (nginx/middleware redirects HTTP → HTTPS, or `secure_origin_whitelist` is empty in prod).\n\
- `Strict-Transport-Security` header set with `max-age >= 15768000` and `includeSubDomains`.\n\
- `Content-Security-Policy` header set (even a permissive one with `default-src 'self'` is better than absent).\n\
- `X-Frame-Options: DENY` or CSP `frame-ancestors 'none'` to block clickjacking.\n\
- `X-Content-Type-Options: nosniff` set.\n\
- CORS allowlist is explicit (not `Access-Control-Allow-Origin: *` for any endpoint that touches credentials).\n\n\
\
**Dependencies & supply chain:**\n\
- Lockfile present and tracked (`package-lock.json`, `pnpm-lock.yaml`, `composer.lock`, `Cargo.lock` for bins, `poetry.lock`, `Gemfile.lock`).\n\
- Dependency risk signals you can VERIFY in the repo: lockfile pins to versions the project's own tooling flags (`cargo audit`/`npm audit`/`composer audit` output or config if present), deps several majors behind their manifest constraint, or abandoned forks. Do NOT assert specific CVEs from memory — model knowledge is stale; if no tooling output exists, record `Inferred` and suggest wiring an audit tool instead.\n\
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
   - `Verified in source` — source evidence and counter-checks establish the defect under its stated conditions.\n\
   - `Inferred` — pattern-matched only; the fix may already be in flight upstream.\n\
   Source access alone never determines status; unresolved applicability or counter-evidence stays Inferred.\n\
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
- **Resource limits**: services that can realistically starve the host (app, workers, DB) carry `mem_limit`/`cpus` or `deploy.resources.limits.*`. Not a finding on clearly dev-only compose files or when the deployment platform owns limits — state which case applies.\n\
- **No `:latest`** image tags (reproducibility).\n\
- **Healthchecks**: critical services (DB, app, gateway) have `healthcheck:` blocks AND `depends_on` uses `condition: service_healthy` instead of bare `depends_on: [svc]`.\n\
- **Secrets handling**: `Not evaluated safely (requires Kronn's local secret scanner)` — never grep for credential literals (the match line would be the secret). Safe signal: `grep -lE 'secrets:|env_file:'` (filename-only) shows whether the safe mechanisms are present.\n\
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
3. **Status taxonomy** (two-tier): `Verified in source` when evidence and counter-checks establish the defect; `Inferred` when applicability or impact remains uncertain.\n\
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
- **Pagination defaults**: follow list parameters through handler, HTTP/search/DB client and response. Check enforced bounds and accumulation over pages; these checks apply without an ORM. Inspect the actual response envelope before claiming metadata is missing.\n\n\
\
# B. CACHING CHECKLIST\n\
\n\
- **Cache headers** on static assets: `Cache-Control: public, max-age=31536000, immutable` for hashed bundles; `no-cache` for HTML shells.\n\
- **CDN / edge cache** strategy documented (in docs/operations/ or in the load balancer config).\n\
- **App-level cache**: inspect TTL, maximum entries/bytes, eviction and key cardinality separately. TTL alone is not a capacity bound. Trace all writes, including appended arrays/buffers; distinguish an evidenced growth path from an unmeasured performance impact.\n\
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
- **Outbound reliability**: inspect client and per-call timeouts, retries/backoff, cancellation and error propagation. Report exact configured values and reachable failure paths; a very short timeout or zero retries is a conditional risk unless requirements/tests establish failure, not automatically a proven outage.\n\
- **Connection pool sizing**: inspect configured bounds and caller concurrency; unknown workload or defaults alone do not establish a defect.\n\
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
        sources: &["__GIT_SOURCE_TREE__"],
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
        sources: &["__GIT_SOURCE_TREE__"],
    },
];

pub(crate) const RGAA_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-rgaa.md",
        prompt: "\
RGAA 4.1 AUDIT (0.8.4) — focused re-scan against the French Référentiel Général d'Amélioration de l'Accessibilité, version 4.1 (106 criteria across 13 topics). Stricter than WCAG 2.1 AA: RGAA mandates specific French-context implementations (e.g. `lang=\"fr\"` propagation, `aria-label` text in French, specific patterns for govt e-forms). Mandatory for French public-service sites + companies > 250 employees (loi du 11 février 2005, décret n° 2019-768).\n\n\
\
Read existing `docs/inconsistencies-rgaa.md` if present (anti-repetition: don't re-discover the same finding under a new slug — refresh / reuse the ID per § F below).\n\n\
\
# A. THÉMATIQUE 1 — IMAGES (8 critères)\n\
- 1.1: Every `<img>` carries a `alt` (decorative ⇒ `alt=\"\"`, informative ⇒ pertinent text in the page's language).\n\
- 1.2: SVG / icons that convey meaning have `<title>` or `aria-label`; decorative SVG → `aria-hidden=\"true\"` + no `tabindex`.\n\
- 1.3: Image captions linked via `<figure>` / `<figcaption>` when applicable.\n\
- 1.6: Map images (graph, chart) have a structured text equivalent (data table or `<details>`).\n\
- 1.8: CAPTCHA: provide an alternative not based on vision.\n\n\
\
# B. THÉMATIQUE 3 — COULEURS (3 critères)\n\
- 3.1: Information NEVER conveyed by color alone (RGAA stricter than WCAG: scan ALL status indicators + form-error styling, flag any color-only signal as critical, not just \"recommended\").\n\
- 3.2: Body text contrast ≥ 4.5:1 ; large text (≥ 24px or ≥ 18.5px bold) ≥ 3:1. Read the design-token file (`tokens.css`, `_colors.scss`, etc.) and compute contrast for the 5 most common pairings. Flag any pair under 4.5:1 as Critical or High.\n\
- 3.3: Non-text contrast (focus rings, form borders, icon-only buttons) ≥ 3:1 against adjacent colors.\n\n\
\
# C. THÉMATIQUE 7 — SCRIPTS (5 critères)\n\
- 7.1: Custom widgets (dropdowns / tabs / accordions / modals) follow ARIA Authoring Practices (correct role + keyboard support).\n\
- 7.3: Each scripted control is keyboard-operable (Tab, Enter, Esc, Arrow keys per pattern).\n\
- 7.4: Status updates use `aria-live` (polite for non-urgent, assertive for errors). Toast/notification components MUST announce.\n\
- 7.5: Time-limited interactions can be paused/extended (auto-dismissing alerts especially).\n\n\
\
# D. THÉMATIQUE 8 — ÉLÉMENTS OBLIGATOIRES (10 critères)\n\
- 8.1: HTML validates (no parse errors that break assistive tech).\n\
- 8.2: `<html lang=\"fr\">` (or appropriate locale) is set at the root.\n\
- 8.4: Page title (`<title>`) is unique and descriptive.\n\
- 8.6: Heading structure: ONE `<h1>`, no skipped levels.\n\
- 8.7: Lists are real `<ul>`/`<ol>`, not stacks of `<div>`.\n\
- 8.9: HTML uses semantic tags appropriately (no `<div onClick>` instead of `<button>`).\n\n\
\
# E. THÉMATIQUE 11 — FORMULAIRES (13 critères)\n\
- 11.1: Every input has an associated `<label for=>` (or wraps the input, or carries `aria-label`/`aria-labelledby`). Placeholder-only is NOT a label.\n\
- 11.2: Form labels are pertinent (in French if the page is French) — e.g. `<label>Adresse e-mail</label>` not `<label>Champ 3</label>`.\n\
- 11.5: Required fields are explicitly marked (visual cue + `aria-required=\"true\"` + announced).\n\
- 11.10: Form errors are bound to the field via `aria-describedby` AND announced via a live region.\n\
- 11.13: Fields collecting personal data have the correct `autocomplete` attribute (RGPD + RGAA — `autocomplete=\"email\"`, `\"family-name\"`, `\"tel\"`, etc.).\n\n\
\
# F. ANTI-REPETITION + OUTPUT FORMAT\n\
\n\
Same rules as the Full audit Step 8: anti-repetition pass + detail-file schema + severity calibration. Index file = `docs/inconsistencies-rgaa.md`. For each finding, the detail file at `docs/tech-debt/TD-<date>-<slug>.md` carries:\n\
- `**Category**: rgaa`\n\
- `**RGAA Criterion**: <thématique>.<critère>` (e.g. `3.2`, `11.10`)\n\
- `**WCAG mapping**: <reference>` if applicable (e.g. `1.4.3`)\n\
- Severity calibration: Critical = bloquant + applicable au scope du décret 2019-768 (public service site or large org); High = compromet l'usabilité pour screen reader / keyboard-only users.\n\n\
\
If no findings, the index reads `'Aucune non-conformité RGAA identifiée lors de cet audit.'`.\n\n\
\
# G. POUR ALLER PLUS LOIN — AUDIT MANUEL + FORMATION (obligatoire)\n\
\n\
**À TOUJOURS ÉCRIRE en fin de `docs/inconsistencies-rgaa.md`**, même quand il n'y a aucune non-conformité automatisée. Section littérale (à reformuler dans le ton de la doc projet, mais le fond doit y être) :\n\n\
\
```\n\
## ⚠️ Cet audit ne remplace PAS un audit complet\n\
\n\
**À lire AVANT de conclure que « le site est conforme ».**\n\
\n\
Cet audit automatique a dépoussiéré une grande partie des problèmes structurels — labels manquants, contrastes faibles, semantic HTML, marqueurs ARIA, attributs `autocomplete`. C'est utile pour rattraper le retard rapide, mais **il y a presque certainement des choses non trouvées** :\n\
\n\
- Beaucoup de critères RGAA (navigation clavier complète, parcours lecteur d'écran, pertinence des alternatives textuelles, gestion du focus dans les widgets complexes, accessibilité cognitive) **ne peuvent PAS être évalués par du tooling**. Le W3C et la DINUM le rappellent explicitement.\n\
- Le tooling automatique couvre **30-40 % des critères au mieux**. Les 60-70 % restants demandent une revue humaine.\n\
\n\
**Deux options non négociables** pour se considérer réellement conforme :\n\
\n\
### 1. Re-tester soi-même avec une grille manuelle\n\
\n\
- Suivre la **grille d'évaluation officielle RGAA 4.1** (DINUM) — 106 critères, page par page, sur un échantillon représentatif du site.\n\
- Compléter avec des outils : Wave, Axe DevTools, **et surtout** un vrai lecteur d'écran (NVDA sous Windows, VoiceOver sous macOS/iOS, TalkBack sous Android) + navigation au clavier seul + simulation de daltonisme.\n\
- Livrable : déclaration d'accessibilité (obligatoire si vous êtes soumis au décret n° 2019-768) + plan d'action pluriannuel + schéma pluriannuel.\n\
- Personne dédiée formée requise (cf. § Formation ci-dessous).\n\
\n\
### 2. Faire appel à un pro\n\
\n\
- **[Access42](https://access42.net)** — la référence française pour l'**audit RGAA officiel et certifiant**. C'est une agence spécialisée 100 % accessibilité : audits d'experts qui produisent la documentation légale opposable, accompagnement de mise en conformité, jurisprudence à jour. **C'est ici qu'on va si on a besoin d'un audit qui tienne devant une demande de la DGCCRF ou un contrôle d'un référent ministériel.**\n\
\n\
### Formation continue (pour ne plus laisser passer)\n\
\n\
Sans compréhension des enjeux, les correctifs sont superficiels et les régressions inévitables à chaque release.\n\
\n\
- **[Access42](https://access42.net)** propose aussi un cursus métier — référent accessibilité numérique, expert RGAA. À privilégier pour un profil dont c'est le cœur du job (référent a11y / lead front).\n\
- **[Opquast](https://www.opquast.com)** — certification « Maîtrise de la qualité en projet web ». Plus large (240 règles : qualité, perf, RGPD, accessibilité, UX, SEO), accessible à tous les profils (devs, designers, PO, chefs de projet, contenu). RGAA y est traité comme un sous-ensemble. La cert reste valide à vie. **À privilégier pour faire monter en compétence toute l'équipe** sur la qualité Web globale, avec un volet a11y solide.\n\
\n\
Vise au minimum **un référent accessibilité Access42-formé par produit** + **toute l'équipe certifiée Opquast** pour que la qualité d'ensemble ne régresse pas entre deux audits.\n\
```\n\n\
\
Apply the marker discipline (`TODO: verify` only if you couldn't check; `TODO: ask user` for human decisions). Do NOT modify source.",
        sources: &[],
    },
];

pub(crate) const DATABASE_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-database.md",
        prompt: "\
DATABASE AUDIT (0.8.4) — focused re-scan on data-layer hygiene. Read existing `docs/inconsistencies-database.md` if present (anti-repetition: don't re-discover the same finding under a new slug — refresh / reuse the ID per § C below).\n\n\
\
# A. SCHEMA + MIGRATIONS\n\
- **Migration safety**: any migration that ALTERs a table > 1M rows must be backed by a non-blocking strategy (online DDL, shadow column + backfill, pt-online-schema-change). Flag `ALTER TABLE ... ADD COLUMN NOT NULL DEFAULT ...` on big tables — locks the whole table on MySQL pre-8.0.\n\
- **Migration reversibility**: each migration file should declare a `down()` or be explicitly marked irreversible. Check the `migrations/` folder (Flyway, Alembic, Diesel, Knex, sqlx, etc.).\n\
- **Schema drift**: compare the live schema (or the dump committed alongside migrations) with what the ORM models declare. Stale columns, missing NOT NULL, missing FK constraints.\n\
- **Charset/collation**: in 2026, only `utf8mb4` is acceptable on MySQL. `utf8` (3-byte) breaks emoji + supplementary planes.\n\n\
\
# B. INDEXES + PERFORMANCE\n\
- Missing indexes on common WHERE clauses (read the ORM hot paths — list-by-user-id, filter-by-status, ordered-by-created-at).\n\
- Over-indexing: tables with > 8 indexes incur write penalty + RAM pressure.\n\
- Compound indexes with wrong column order (low-cardinality column first wastes the index).\n\
- Missing `WHERE clause` in DELETE/UPDATE in app code (PostgreSQL: missing `... WHERE id = $1` on `DELETE FROM users` is catastrophic).\n\n\
\
# C. ORM + N+1\n\
- **N+1 queries**: scan the most-used list / dashboard endpoints. ORM lazy-load on a collection of N parent rows produces N+1 queries.\n\
- **Unbounded SELECT**: queries without LIMIT on user-facing endpoints (`User.all` on the admin page when the table grew to 500k rows).\n\
- **Transaction boundaries**: long-running transactions that hold locks (PostgreSQL `idle in transaction`, MySQL row-locking) blocking other writers. Check that DB calls inside HTTP handlers commit promptly.\n\n\
\
# D. DATA INTEGRITY\n\
- Foreign keys: missing constraints (ORM-level FK only is not enforced at the DB layer if `disable_constraints` was ever set in tests).\n\
- Soft-delete vs hard-delete inconsistency (some queries scope WHERE deleted_at IS NULL, others don't — leak risk).\n\
- Nullable columns that the app treats as required: TypeScript / Rust types may say `string` but the column allows NULL.\n\n\
\
# E. ANTI-REPETITION + OUTPUT FORMAT\n\
\n\
Same rules as the Full audit Step 8 (§ C and § D): scan `docs/tech-debt/` for existing TDs; for each finding, decide REUSE / REFINE / NEW per § C; create `docs/tech-debt/TD-<date>-<slug>.md` for new ones; UPDATE `audit_history` for existing. Detail file schema = the Full audit's. Index file = `docs/inconsistencies-database.md` (not `inconsistencies-tech-debt.md`). Severity calibration is the same.\n\n\
\
Apply the marker discipline (`TODO: verify` only if you couldn't check; `TODO: ask user` for human decisions). If no findings, the index reads `'None identified during this database audit pass'`.",
        sources: &["__GIT_SOURCE_TREE__"],
    },
];

pub(crate) const API_DESIGN_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-api.md",
        prompt: "\
API DESIGN AUDIT (0.8.4) — focused re-scan on the public-facing contract. Read existing `docs/inconsistencies-api.md` if present (anti-repetition rules apply, same as Full audit Step 8).\n\n\
\
# A. CONTRACT CONSISTENCY\n\
- **Naming**: REST endpoints must follow ONE convention (kebab-case, snake_case, camelCase) — flag inconsistencies like `/api/users-list` next to `/api/orderItems`. Same for query params, JSON keys.\n\
- **HTTP semantics**: GET that mutates state (anti-pattern); POST that returns 200 for create (should be 201); DELETE that returns the deleted resource (should be 204 unless explicitly part of the contract); PATCH vs PUT confusion.\n\
- **Error envelope**: ONE shape, used by EVERY endpoint. Flag endpoints returning `{ \"error\": \"...\" }` next to ones returning `{ \"errors\": [...] }`, mixed status codes (400 vs 422 for validation).\n\
- **Status codes**: judge against the API's OWN documented contract first (OpenAPI/docs); flag INTERNAL inconsistency (same operation returning different codes, 200 bodies carrying `{ \"error\": ... }` for failures, 401/403 confusion between authn and authz). 200-on-write is a style choice, not a defect, when the contract says so — flag it as `Low`/`Inferred` only if no contract exists.\n\n\
\
# B. VERSIONING + EVOLUTION\n\
- **Versioning strategy**: header (`Accept: application/vnd.api+json; version=2`) vs path (`/api/v2/`) — pick one, don't mix.\n\
- **Breaking changes**: removing a field, narrowing a type, renaming — without a deprecation window or version bump.\n\
- **Additive changes safety**: new required field on an existing endpoint breaks old clients.\n\n\
\
# C. PAGINATION + LIST RESPONSES\n\
- Unbounded list endpoints (no `?limit=` enforcement) — DoS surface + cursor unsafe at scale.\n\
- Inconsistent pagination shape (cursor on some endpoints, offset on others, or no metadata at all).\n\
- Before a missing-metadata finding, read the response construction and consumer, including nested envelopes and existing equivalent fields. Cite the actual return shape. Do not treat an uninspected response as absent; distinguish missing bounds from missing metadata and check each claim independently.\n\n\
\
# D. AUTHN + AUTHZ + RATE LIMITING\n\
- Endpoints that should require auth but don't (read the routing config — the audit's `repo-map.md` is a primer).\n\
- IDOR risk: endpoints scoped by URL param (`/api/orders/:id`) without ownership check.\n\
- Rate limiting absent on public auth endpoints (`/login`, `/register`, `/password-reset`).\n\
- CSRF tokens absent on state-mutating endpoints when cookies are used for session (SPA + cookie auth).\n\n\
\
# E. DOC DRIFT\n\
- OpenAPI / GraphQL schema in the repo (if any) vs what the endpoints actually accept / return. Flag fields documented but unused, or used but undocumented.\n\
- Examples in the docs that no longer parse (wrong field names, stale auth headers).\n\n\
\
# F. ANTI-REPETITION + OUTPUT FORMAT\n\
\n\
Same rules as the Full audit Step 8: anti-repetition pass + detail-file schema + severity calibration. Index file = `docs/inconsistencies-api.md`. If no findings, the index reads `'None identified during this API design audit pass'`.\n\n\
\
Apply the marker discipline (`TODO: verify` only if you couldn't check; `TODO: ask user` for human decisions).",
        sources: &["__GIT_SOURCE_TREE__"],
    },
];

/// Dispatch table for `LaunchAuditRequest.kind`. Returns the step
/// slice to drive the agent through. For `Custom`, callers should
/// inline the user-supplied prompt rather than going through this fn.
/// 0.9.0 — Code quality & maintainability. Born from the DOCROMS_WEB
/// dogfooding: a Full audit surfaced docs & infra debt but zero findings
/// about the CODE (templates, styles, backend smells) — "un audit qui ne
/// sort pas de soucis de qualité de code, c'est super limité".
pub(crate) const CODE_QUALITY_STEPS: &[AnalysisStep] = &[
    AnalysisStep {
        target_file: "docs/inconsistencies-code-quality.md",
        prompt: "\
You are running a FOCUSED CODE-QUALITY AUDIT. Output ONLY code quality / maintainability \
findings — ignore security, docs drift, infra config (other audits own those).\n\n\
If `docs/briefing.md` exists, read it FIRST: the user's stated quality concerns take priority \
over the generic checklist below.\n\n\
# A. MANDATORY QUALITY CHECKLIST (adapt to the project's actual stack)\n\n\
Walk through every applicable item. If satisfied, write \"verified clean\". \
If NOT, emit a TD finding. Sample REAL files — never assert from file names alone.\n\n\
**Templates / markup (Twig, Blade, JSX, ERB, plain HTML…):**\n\
- Duplication: same block/markup repeated across ≥3 templates instead of a partial/macro/component (diff 3 similar templates).\n\
- Dead templates: files never referenced by any route/controller/include (grep template names against source).\n\
- Inline `style=` / inline `<script>` blocks in templates (grep, count, sample) — belongs in the asset pipeline.\n\
- Oversized templates (> ~300 lines): inspection candidates, not an acceptance threshold. Identify mixed responsibilities or duplication and its concrete maintenance cost; size alone is not a defect. Never prescribe a new line-count policy without project evidence.\n\
- Data/logic in templates: heavy computation, raw SQL-ish calls or business rules inside template files.\n\n\
**Styles (CSS/SCSS):**\n\
- Architecture: is there a discernible convention (BEM, utility, tokens)? Mixed conventions in the same codebase = finding.\n\
- Duplication: same rule blocks repeated (sample the 2 largest files); hardcoded colors/sizes repeated ≥5 times instead of variables.\n\
- Dead selectors: classes defined but absent from every template (sample 20 selectors).\n\
- Single global stylesheet > ~1500 lines with no imports/layers = finding.\n\n\
**Backend code (PHP, TS/JS, Rust, Python… — whatever the project uses):**\n\
- God files: inspect large files for unrelated responsibilities; size is a search heuristic, not proof or a new project limit. Name the actual coupling/duplication and evidence.\n\
- Dead code: unreferenced functions/classes/exports (sample; use grep on symbol names).\n\
- Swallowed errors: empty catch blocks, `catch (e) {}`, `@`-suppression (PHP), `.unwrap()` chains on fallible paths in handlers.\n\
- Copy-paste logic: same function body duplicated across files (sample the controllers/services layer).\n\
- Typing hygiene where the language offers it: untyped public signatures, `any`/`mixed` density.\n\
- TODO/FIXME/HACK density: count them; > 20 in source (not docs) = finding listing the 5 oldest.\n\n\
**Perf & eco hygiene (code-level only — infra belongs to other audits):**\n\
- Assets: images > 300 KB served without modern format (webp/avif); JS/CSS bundles not minified in the build.\n\
- Requests in loops: DB/HTTP calls inside iteration (N+1 shape) — sample controllers/services.\n\
- Caching affordances in code: repeated identical computations/fetches with no memoization where an obvious hot path exists.\n\n\
# B. ANTI-REPETITION (priors)\n\n\
Before writing findings: use the existing domain indexes to locate matching TDs, then read only their detail files. If a finding matches an \
existing one, REUSE its ID (append an `audit_history` entry — never overwrite, never re-slug).\n\n\
# C. OUTPUT FORMAT\n\n\
1. Write `docs/inconsistencies-code-quality.md` with the standard table header:\n\
   `| ID | Problem | Area | Severity |`\n\
2. For each finding, also write `docs/tech-debt/<ID>.md` using the existing TD detail template — \
   add a `**Category**: code-quality` line.\n\
3. Status taxonomy: `Verified in source` only when evidence establishes the defect and its conditions after counter-checks; otherwise `Inferred`. Merely reading a file is insufficient.\n\
4. Cap: 20 findings. High severity exempt from the cap.\n\
5. Each finding: file path + line, one-sentence maintainability impact, one-sentence remediation.",
        sources: &["__GIT_SOURCE_TREE__"],
    },
];

/// Relevance gate prepended to every CHAINED sub-audit step: a dimension
/// that doesn't apply to the audited project must cost one line, not a
/// full agent pass ("si pas utile au projet → on passe").
pub(crate) const CHAINED_STEP_GATE: &str = "\
# RELEVANCE GATE (answer FIRST, before anything else)\n\
This dimension may not apply to this project. Assess applicability in ≤ 5 tool calls \
(e.g. a Docker audit on a project with no Dockerfile/compose, a Database audit on a \
static site with no DB layer). If NOT applicable: write your index file with the single \
line `Not applicable: <one-sentence reason>` and STOP — no findings, no TD files. If any part applies or the inventory is uncertain, continue the applicable checks. No ORM does not exclude HTTP/search pagination, caching or outbound transport. Mark individual inapplicable checks separately; never use one missing technology to skip a whole mixed checklist.\n";

/// A Full run executes eight foundation steps, seven focused sub-audits,
/// then the ninth foundation step (consolidation), so one launch covers everything and
/// the single validation discussion at the end confirms the WHOLE TD set.
/// RGAA stays on-demand (French legal norm, superset of the chained a11y
/// pass). Sub-audits remain individually launchable for surgical re-scans.
/// Which prompt gate a step gets: chained sub-audit steps (past the
/// foundation) open with the relevance gate; foundation steps and
/// standalone sub-audits (deliberately chosen by the user) never do.
pub(crate) fn gate_for_step(step: &AnalysisStep, kind: crate::models::AuditKind) -> &'static str {
    if matches!(kind, crate::models::AuditKind::Full)
        && step.target_file.contains("inconsistencies-")
        && !step.target_file.ends_with("inconsistencies-tech-debt.md")
    {
        CHAINED_STEP_GATE
    } else {
        ""
    }
}

pub(crate) fn assemble_chained_steps(kind: crate::models::AuditKind) -> Vec<AnalysisStep> {
    use crate::models::AuditKind;
    if !matches!(kind, AuditKind::Full) {
        return kind_to_steps(kind).to_vec();
    }
    let (consolidation, foundation) = ANALYSIS_STEPS.split_last().expect("audit foundation");
    let mut chain = foundation.to_vec();
    for sub in [
        AuditKind::Security,
        AuditKind::Docker,
        AuditKind::Performance,
        AuditKind::Accessibility,
        AuditKind::Database,
        AuditKind::ApiDesign,
        AuditKind::CodeQuality,
    ] {
        chain.extend_from_slice(kind_to_steps(sub));
    }
    chain.push(*consolidation);
    chain
}

pub(crate) fn finding_evidence_block(step: &AnalysisStep) -> &'static str {
    if !step.target_file.contains("inconsistencies-") && step.target_file != "docs/decisions.md" {
        return "";
    }
    concat!(
        "\n\n## Finding evidence and output contract\n\
Survey the relevant tracked source families before detailed findings. Use path lists and targeted ranges; expand the initial read batch until the requested checklist is covered. In the domain index, record concise coverage notes: inspected scope and outcome, explicit unchecked gaps, or a supported N/A. Fewer findings is not evidence of better coverage.\n\
For each claim, name conditions, source evidence and the relevant counter-check. For absence claims inspect workspace manifests, callers, response construction or deployment controls; a failed search or missing local directive alone is not proof. Unknown is not absent. Verified in source requires evidence establishing the stated defect; impact can remain explicitly conditional.\n\
Use the canonical TD shape below with NAMED fields. Write direct file content or a named-field serializer, never positional shell arguments for TD fields. Check written YAML, allowed Severity/Status/Effort values, nonempty sections and agreement between body status and latest audit_history. S/M/L/XL are effort values, never statuses. Quote YAML strings. No component outside this audit fixes your generated fields. Preserve prior human decisions.\n\
Verification names an observable pass/fail result, with executed checks distinguished from proposed checks. A fix recipe is not a verification result. Do not invent commands, thresholds or policy. Zero findings is valid after coverage. Reuse one TD ID per root cause. Each index ID must be an actual Markdown link to its detail file; create/check both together.\n\n",
        include_str!("../../../../templates/docs/tech-debt/TEMPLATE.md")
    )
}

/// Step-8 prompt context, shared by the Full and partial pipelines (Codex
/// lot-3 #4): the deterministic detector signals AND the known-debt digest
/// block travel together — a partial re-run of step 8 with only half the
/// inputs would duplicate priors or miss anchored signals.
pub(crate) fn step8_context_block(
    signals: &[crate::core::audit_detectors::DetectedSignal],
    prior_td_digests: &[reconciliation::PriorDigest],
) -> String {
    let mut out = format!(
        "\n\n{}\n",
        crate::core::audit_detectors::render_signals_block(signals)
    );
    if !prior_td_digests.is_empty() {
        out.push_str(&format!(
            "\n\n{}\n",
            reconciliation::render_known_debt_block(prior_td_digests),
        ));
    }
    out
}

/// Whether a chained step can be refreshed via partial-audit: only steps
/// with a REAL docs/ target are verifiable post-run — the synthetic REVIEW
/// pseudo-step (no file) would pass the validator on exit code 0 alone.
pub(crate) fn partial_selectable(step: &AnalysisStep) -> bool {
    step.target_file != "REVIEW"
        && !step.target_file.is_empty()
        && step.target_file.starts_with("docs/")
}

/// Agents eligible to RUN an audit: the pipeline's only deliverable is
/// files written into `docs/`, so an agent that cannot write there can only
/// produce a silent no-op that the validator then flags step by step.
///
/// Two kinds of agent can: a CLI with its own filesystem, and an HTTP agent
/// whose file tools Kronn executes on its behalf (KT-338), scoped to the
/// project by `agent_launch` (KT-924). Ollama and LiteLLM are the HTTP agents
/// admitted: NVIDIA is a hosted service the user has not been asked to send a
/// whole repository to, and `Custom` needs a named connection the audit has no
/// way to select. The refusal therefore stays for an agent with no file tools
/// Kronn can scope to the project, and `audit_refusal_message` names who is
/// accepted.
///
/// ALLOWLIST, not denylist (Codex A4 v2): a new/unknown variant must force
/// a compiler decision here instead of being audit-capable by default —
/// `Custom`'s runner is a bare `echo` that exits 0, the exact silent no-op
/// this gate exists to stop. Mirrored by the UI's `canRunAudit`, enforced
/// here because MCP/bridge callers bypass the UI entirely.
pub(crate) fn agent_can_audit(agent: &crate::models::AgentType) -> bool {
    use crate::models::AgentType;
    match agent {
        AgentType::ClaudeCode
        | AgentType::Codex
        | AgentType::OpenCode
        | AgentType::GeminiCli
        | AgentType::Kiro
        | AgentType::CopilotCli
        | AgentType::Ollama
        | AgentType::LiteLlm => true,
        AgentType::Vibe | AgentType::Nvidia | AgentType::Custom => false,
    }
}

/// The agents `agent_can_audit` admits, for the refusal message. Kept beside the
/// predicate and pinned to it by a test, so the message cannot name an agent the
/// gate refuses or forget one it accepts.
const AUDIT_AGENTS: [crate::models::AgentType; 8] = [
    crate::models::AgentType::ClaudeCode,
    crate::models::AgentType::Codex,
    crate::models::AgentType::OpenCode,
    crate::models::AgentType::GeminiCli,
    crate::models::AgentType::Kiro,
    crate::models::AgentType::CopilotCli,
    crate::models::AgentType::Ollama,
    crate::models::AgentType::LiteLlm,
];

/// Why `agent` was refused, and who would have been accepted. One wording for the
/// Full and the partial launch: the two used to disagree, and neither said which
/// agents to pick instead.
pub(crate) fn audit_refusal_message(agent: &crate::models::AgentType) -> String {
    let accepted = AUDIT_AGENTS
        .iter()
        .map(crate::api::disc_helpers::agent_display_name)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{agent:?} cannot run audits: Kronn has no file tools it can scope to the project for it, so the docs/ deliverables would silently never be written. Pick one of: {accepted}."
    )
}

pub(crate) fn kind_to_steps(kind: crate::models::AuditKind) -> &'static [AnalysisStep] {
    use crate::models::AuditKind;
    match kind {
        AuditKind::Full => ANALYSIS_STEPS,
        AuditKind::Drift => DRIFT_STEPS,
        AuditKind::Security => SECURITY_STEPS,
        AuditKind::Docker => DOCKER_STEPS,
        AuditKind::Performance => PERFORMANCE_STEPS,
        AuditKind::Accessibility => ACCESSIBILITY_STEPS,
        AuditKind::Rgaa => RGAA_STEPS,
        AuditKind::Database => DATABASE_STEPS,
        AuditKind::ApiDesign => API_DESIGN_STEPS,
        AuditKind::CodeQuality => CODE_QUALITY_STEPS,
        // Custom is handled at the call site: it builds a one-off
        // AnalysisStep from req.custom_prompt rather than using a const.
        AuditKind::Custom => &[],
    }
}

#[cfg(test)]
mod kind_dispatch_tests {
    use super::*;
    use crate::models::AuditKind;

    #[test]
    fn public_docs_match_chained_audit_contract() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("backend crate must live below the repository root");
        let readme = std::fs::read_to_string(repo_root.join("README.md"))
            .expect("README.md must be readable");
        let architecture = std::fs::read_to_string(repo_root.join("docs/architecture/overview.md"))
            .expect("architecture overview must be readable");

        for expected in ["Step 8", "CodeQuality", "Rgaa"] {
            assert!(
                readme.contains(expected) || architecture.contains(expected),
                "public docs must name {expected}"
            );
        }
        assert!(readme.contains("9 docs steps"));
        assert!(readme.contains("16 steps"));
        assert!(architecture.contains("9 étapes de fondation"));
        assert!(architecture.contains("chaîne complète de 16 étapes"));

        for stale in [
            "Step 9 now ALWAYS",
            "Step 9 also flags",
            "Full` reste la base (10 steps)",
            "4 checks non-skippables",
            "`Active`/`Reopened`",
        ] {
            assert!(
                !readme.contains(stale) && !architecture.contains(stale),
                "public docs still contain stale audit contract: {stale}"
            );
        }

        let active_contract_sources = [
            include_str!("full.rs"),
            include_str!("run.rs"),
            include_str!("helpers.rs"),
            include_str!("../../models/projects.rs"),
            include_str!("../../db/sql/050_audit_runs.sql"),
            include_str!("../../../../frontend/src/components/ProjectCard.tsx"),
            include_str!("../../../../frontend/src/types/generated.ts"),
        ]
        .join("\n");
        for stale in [
            "run the 9-step audit",
            "run 9-step audit",
            "once the 9-step loop begins",
            "Run 9-step audit",
            "auditing step N/10",
            "Reprendre Step N/10",
            "`last_completed_step` ∈ 1..=9",
            "before reaching step 10",
            "10 = full pipeline ran to end",
            "Step 9's cluster detector",
            "outside the audit's 9-step pipeline",
            "all 9 steps success",
            "before step 10",
            "did we reach step 10?",
            "protocol the 9-step audit",
            "runs the canonical 9-step pipeline",
        ] {
            assert!(
                !active_contract_sources.contains(stale),
                "active source comments still contain stale audit contract: {stale}"
            );
        }
    }

    #[test]
    fn full_kind_returns_canonical_9_step_foundation() {
        let steps = kind_to_steps(AuditKind::Full);
        assert_eq!(
            steps.len(),
            ANALYSIS_STEPS.len(),
            "Full kind must return the canonical step list, not a subset"
        );
        assert_eq!(
            steps.len(),
            9,
            "kind_to_steps(Full) is the 9-step foundation before launch-time chaining"
        );
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
            AuditKind::CodeQuality,
        ] {
            let steps = kind_to_steps(kind);
            assert_eq!(
                steps.len(),
                1,
                "{:?} should expose one focused step in 0.8.2 (S2.D3-D5 fill the body)",
                kind
            );
        }
    }

    #[test]
    fn full_chain_consolidates_after_every_specialized_audit() {
        // "Un seul audit qui envoie du pâté" : Full = 9 foundation steps +
        // the 7 chained dimensions, each single-step. RGAA stays on-demand.
        let chain = assemble_chained_steps(AuditKind::Full);
        assert_eq!(chain.len(), ANALYSIS_STEPS.len() + 7);
        assert_eq!(
            chain[ANALYSIS_STEPS.len() - 1].target_file,
            "docs/inconsistencies-security.md"
        );
        assert_eq!(chain.last().unwrap().target_file, "docs/decisions.md");
        assert!(
            !chain.iter().any(|s| s.target_file.contains("rgaa")),
            "RGAA is legal/on-demand, never chained by default"
        );
    }

    #[test]
    fn non_full_kinds_do_not_chain() {
        assert_eq!(assemble_chained_steps(AuditKind::Security).len(), 1);
        assert_eq!(assemble_chained_steps(AuditKind::CodeQuality).len(), 1);
    }

    #[test]
    fn chained_sub_audits_are_findings_indices_and_drift_trackable() {
        // #8 + F27 — the Full baseline checksums the WHOLE chain, so every
        // chained sub-audit (steps 9..15) is a findings index AND carries a
        // drift source: the source-bearing dimensions (Security/Docker) hash
        // their own file sets, the broad passes (perf/a11y/api-design/
        // code-quality/database) use the F27 `__GIT_SOURCE_TREE__` fingerprint
        // — content+structure of the source tree, excluding Kronn outputs, so
        // tracking them no longer self-drifts on the audit's own commit.
        let chain = assemble_chained_steps(AuditKind::Full);
        let subs = &chain[ANALYSIS_STEPS.len() - 1..chain.len() - 1];
        assert_eq!(
            subs.len(),
            7,
            "7 chained sub-audits after the 9 foundation steps"
        );
        for s in subs {
            assert!(
                s.target_file.contains("inconsistencies-"),
                "{} must be a findings index",
                s.target_file
            );
            assert!(
                !s.sources.is_empty(),
                "{} must carry a drift source (F27 fingerprint or file set)",
                s.target_file
            );
        }
    }

    #[test]
    fn gate_applies_to_chained_steps_only() {
        let chain = assemble_chained_steps(AuditKind::Full);
        for step in &chain {
            let gate = gate_for_step(step, AuditKind::Full);
            let specialized = step.target_file.contains("inconsistencies-")
                && !step.target_file.ends_with("inconsistencies-tech-debt.md");
            assert_eq!(!gate.is_empty(), specialized, "{}", step.target_file);
            assert_eq!(gate_for_step(step, AuditKind::Security), "");
        }
        assert_eq!(gate_for_step(chain.last().unwrap(), AuditKind::Full), "");
    }

    #[test]
    fn chained_gate_orders_the_early_exit() {
        assert!(CHAINED_STEP_GATE.contains("RELEVANCE GATE"));
        assert!(CHAINED_STEP_GATE.contains("Not applicable"));
        assert!(CHAINED_STEP_GATE.contains("STOP"));
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
            (AuditKind::Security, "docs/inconsistencies-security"),
            (AuditKind::Docker, "docs/inconsistencies-docker"),
            (AuditKind::Performance, "docs/inconsistencies-performance"),
            (
                AuditKind::Accessibility,
                "docs/inconsistencies-accessibility",
            ),
            (AuditKind::Rgaa, "docs/inconsistencies-rgaa"),
            (AuditKind::Database, "docs/inconsistencies-database"),
            (AuditKind::ApiDesign, "docs/inconsistencies-api"),
            (AuditKind::CodeQuality, "docs/inconsistencies-code-quality"),
        ] {
            let steps = kind_to_steps(kind);
            assert_ne!(
                steps[0].target_file, canonical,
                "{:?} must NOT write into the Full audit's index",
                kind
            );
            assert!(
                steps[0].target_file.starts_with(expected_prefix),
                "{:?} target_file should start with {expected_prefix}, got {}",
                kind,
                steps[0].target_file
            );
        }
    }

    #[test]
    fn drift_kind_consumes_checksums() {
        // Drift is purely a re-hash of docs/checksums.json — it doesn't
        // emit findings, just reports stale files. The single step
        // therefore lists checksums.json as its source.
        let drift = kind_to_steps(AuditKind::Drift);
        assert_eq!(drift.len(), 1);
        assert!(
            drift[0].sources.contains(&"docs/checksums.json"),
            "Drift step must hash docs/checksums.json to compute drift"
        );
    }

    #[test]
    fn audit_kind_label_round_trip() {
        // The label is what lands in `audit_runs.kind` and powers SSE
        // event filtering on the front-end. Drift in labels would break
        // existing rows after a deploy.
        let expected = [
            (AuditKind::Full, "Full"),
            (AuditKind::Drift, "Drift"),
            (AuditKind::Security, "Security"),
            (AuditKind::Docker, "Docker"),
            (AuditKind::Performance, "Performance"),
            (AuditKind::Accessibility, "Accessibility"),
            (AuditKind::Rgaa, "Rgaa"),
            (AuditKind::Database, "Database"),
            (AuditKind::ApiDesign, "ApiDesign"),
            (AuditKind::Custom, "Custom"),
        ];
        for (kind, label) in expected {
            assert_eq!(kind.as_label(), label, "label drift on {:?}", kind);
        }
    }

    #[test]
    fn audit_kind_display_name_is_user_facing_french() {
        // 0.8.4 (#322 / F2) — `display_name()` is what users actually
        // read (disc titles, log lines). The TitleCase wire labels
        // (`as_label()`) leak as "Rgaa" which reads as a typo — this
        // helper exposes the human form ("RGAA 4.1", "Sécurité").
        use crate::models::AuditKind;
        assert_eq!(
            AuditKind::Rgaa.display_name(),
            "RGAA 4.1",
            "RGAA must keep its uppercase acronym + version"
        );
        assert_eq!(
            AuditKind::Security.display_name(),
            "Sécurité",
            "FR: must say Sécurité not Security"
        );
        assert_eq!(AuditKind::Accessibility.display_name(), "Accessibilité");
        assert_eq!(AuditKind::Database.display_name(), "Base de données");
        assert_eq!(AuditKind::ApiDesign.display_name(), "Design d'API");
        assert_eq!(AuditKind::Full.display_name(), "Audit global");
        // Wire labels stay TitleCase (the disc.kind column round-trips
        // through serde — never break the format).
        assert_eq!(AuditKind::Rgaa.as_label(), "Rgaa");
        assert_eq!(AuditKind::Security.as_label(), "Security");
    }

    #[test]
    fn rgaa_kind_carries_french_criteria_and_distinct_index() {
        // 0.8.4 (#287) — RGAA must check the French norm explicitly,
        // not be a translated copy of the WCAG-flavored Accessibility
        // prompt. Spot-check that the prompt:
        //   1. mentions the RGAA reference + version 4.1;
        //   2. cites concrete criteria numbers (1.x, 11.x);
        //   3. writes to its OWN index file (not the WCAG one).
        let steps = kind_to_steps(AuditKind::Rgaa);
        assert_eq!(steps.len(), 1, "Rgaa is a single-step focused audit");
        let prompt = steps[0].prompt;
        assert!(
            prompt.contains("RGAA"),
            "must reference the French norm by name"
        );
        assert!(
            prompt.contains("4.1"),
            "must pin the RGAA version (4.1 as of 2026)"
        );
        // A handful of canonical criteria — drift in any of these means
        // the prompt was edited to remove the French specificity, which
        // defeats the whole reason this kind exists.
        assert!(
            prompt.contains("11.10"),
            "must cover form-error binding (critère 11.10)"
        );
        assert!(
            prompt.contains("autocomplete"),
            "must cover the RGPD-adjacent autocomplete reqs"
        );
        assert!(
            prompt.contains("contrast") || prompt.contains("contrast"),
            "must cover thématique 3 (couleurs + contraste)"
        );
        assert_eq!(
            steps[0].target_file, "docs/inconsistencies-rgaa.md",
            "RGAA must NOT clobber the WCAG-flavored accessibility index"
        );
        // Manual-audit-is-mandatory section: must educate the user that
        // automation only covers 30-40% of RGAA, and point them to the
        // two French training references (Access42 + Opquast). Without
        // this, the audit ships a false sense of compliance.
        assert!(
            prompt.contains("audit") && prompt.contains("manuel"),
            "must explicitly require the manual-audit section"
        );
        assert!(
            prompt.contains("Access42"),
            "must reference Access42 — the certifying-RGAA reference (audit officiel + expertise)"
        );
        assert!(
            prompt.contains("Opquast"),
            "must reference Opquast — the broader web-quality certification with RGAA coverage"
        );
        assert!(
            prompt.contains("W3C") || prompt.contains("DINUM"),
            "must cite the authority recommending manual audit"
        );
        // Anti-false-sense-of-compliance: explicitly tell the agent to
        // warn the user that automated audits do NOT mean the site is
        // conforming. Without this, users tend to read the empty-findings
        // case as "all good".
        assert!(
            prompt.contains("ne remplace")
                || prompt.contains("non trouvées")
                || prompt.contains("retestent")
                || prompt.contains("appel à un pro"),
            "must explicitly warn against the false sense of compliance"
        );
    }

    #[test]
    fn launch_audit_request_defaults_kind_to_full() {
        // Backwards-compat: clients that don't send `kind` get Full.
        let json = r#"{"agent":"ClaudeCode"}"#;
        let req: crate::models::LaunchAuditRequest =
            serde_json::from_str(json).expect("LaunchAuditRequest must still parse without `kind`");
        assert_eq!(req.kind.unwrap_or_default(), AuditKind::Full);
        assert!(req.custom_prompt.is_none());
        assert!(req.tier.is_none());
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::helpers::{
        build_briefing_prompt, build_sub_audit_validation_prompt, build_validation_prompt,
        run_scope_block,
    };
    use super::*;
    use crate::models::AuditInfo;

    #[test]
    fn code_quality_validation_routes_to_its_own_index() {
        // Codex r6 P0 — the CodeQuality arm was missing from the sub-audit
        // validation builder: it fell into the defensive fallback and told
        // the agent to validate against inconsistencies-tech-debt.md while
        // the audit had written inconsistencies-code-quality.md.
        let p = build_sub_audit_validation_prompt(
            crate::models::AuditKind::CodeQuality,
            "fr",
            false,
            &[],
        );
        assert!(
            p.contains("docs/inconsistencies-code-quality.md"),
            "CodeQuality validation must target its own index"
        );
        assert!(
            !p.contains("docs/inconsistencies-tech-debt.md"),
            "…and never the Full-audit fallback index"
        );
    }

    #[test]
    fn validation_never_confirms_on_silence() {
        // Codex r6 P0 — option (c) auto-confirmed every unselected TD while
        // the same prompt forbade batch confirmation. Silence must never
        // become `Confirmed by user`, in any locale or variant.
        let info = AuditInfo {
            files: vec![],
            todos: vec![],
            tech_debt_items: vec![],
        };
        for lang in ["fr", "en", "es"] {
            let p = build_validation_prompt(lang, &info, false, &[]);
            assert!(
                !p.contains("automatiquement marqu") && !p.contains("automaticamente `Confirmed"),
                "{lang}: no TD may be auto-confirmed"
            );
            assert!(
                !p.contains("default to `Confirmed by user`"),
                "{lang}: unselected TDs must keep their status"
            );
        }
        let sub =
            build_sub_audit_validation_prompt(crate::models::AuditKind::Security, "fr", false, &[]);
        assert!(
            sub.contains("gardent leur statut actuel"),
            "sub-audit variant must state that unselected TDs keep their status"
        );
    }

    #[test]
    fn validation_scopes_to_the_run_td_ids() {
        // Codex r6 P0 — the validation phase read every TD-*.md on disk and
        // could re-open findings settled by previous validations. The scope
        // block pins the exact ids this run touched.
        let ids = vec!["TD-20260720-a".to_string(), "TD-20260720-b".to_string()];
        let block = run_scope_block(&ids, "fr");
        assert!(block.contains("TD-20260720-a, TD-20260720-b"));
        assert!(block.contains("UNIQUEMENT"));
        assert!(
            run_scope_block(&[], "fr").is_empty(),
            "no ids → no scope block (fresh install)"
        );

        let info = AuditInfo {
            files: vec![],
            todos: vec![],
            tech_debt_items: vec![],
        };
        let p = build_validation_prompt("fr", &info, false, &ids);
        assert!(
            p.contains("TD-20260720-a"),
            "Full validation prompt must embed the run scope"
        );
        let sub =
            build_sub_audit_validation_prompt(crate::models::AuditKind::Docker, "fr", false, &ids);
        assert!(
            sub.contains("TD-20260720-b"),
            "sub-audit prompt must embed the run scope"
        );
    }

    #[test]
    fn validation_no_longer_claims_a_ten_step_pipeline() {
        // Codex r6 P1 — the prompt announced "step 10/10" and "9 analysis
        // steps" after the pipeline grew to 16 chained steps.
        let info = AuditInfo {
            files: vec![],
            todos: vec![],
            tech_debt_items: vec![],
        };
        for lang in ["fr", "en", "es"] {
            let p = build_validation_prompt(lang, &info, false, &[]);
            assert!(!p.contains("10/10"), "{lang}: stale step count");
            assert!(
                !p.contains("9 analysis steps")
                    && !p.contains("9 etapes")
                    && !p.contains("9 étapes"),
                "{lang}: stale analysis-step count"
            );
        }
    }

    #[test]
    fn security_prompt_never_pulls_secret_values_into_context() {
        // Codex r7 P0 — the agent runs full-access and its context leaves
        // the machine: the checklist must detect secrets by metadata only.
        // Canary on the assembled step prompt (static — values only ever
        // enter via tool use the prompt now forbids).
        let p = SECURITY_STEPS[0].prompt;
        assert!(
            !p.contains("and read each"),
            "must never instruct reading .env content"
        );
        assert!(
            !p.contains("-p -S") && !p.contains("--all -p"),
            "git history search must never dump patches"
        );
        assert!(p.contains("--name-only"), "history search is metadata-only");
        assert!(
            p.contains("EXFILTRATION GUARD"),
            "the absolute never-transmit rule must be present"
        );
        assert!(p.contains("Findings carry file + line + pattern type only"));
        // Codex r9 — the value-scan is not safely runnable by an LLM agent:
        // the prompt must claim honest non-coverage, never a clean bill.
        assert!(
            p.matches("Not evaluated safely").count() >= 3,
            "each secret-content sub-check must be individually labeled"
        );
        // Codex sync (msg 105) — positive semantic coverage on the FULL
        // pipeline's own secret mentions, not just the Security sub-audit.
        let full: String = ANALYSIS_STEPS.iter().map(|s| s.prompt).collect();
        assert!(
            full.contains("CI secret sourcing: `Not evaluated safely"),
            "Full step 7 CI credential check must be honestly non-evaluated"
        );
        assert!(
            full.contains("Values inside tracked `.env*` files: `Not evaluated safely"),
            "Full step 7 .env value check must be honestly non-evaluated"
        );
        assert!(
            full.contains("git ls-files -- '*.env*' '.env*'"),
            "tracked-.env detection must be the quoted pathspec form"
        );
        // The RULE, over EVERY step prompt of all three lists: any grep
        // command shape near credential material must be filename-only, and
        // no content-inspection recipe may exist anywhere.
        let all_prompts: Vec<&str> = ANALYSIS_STEPS
            .iter()
            .chain(SECURITY_STEPS.iter())
            .chain(DOCKER_STEPS.iter())
            .map(|s| s.prompt)
            .collect();
        for prompt in all_prompts {
            for recipe in [
                "grep -cE",
                "grep -nE",
                "search for `Bearer ",
                "and read each",
                "emit TD if literal credentials found",
                "emit TD if found, this is almost always a leak",
            ] {
                assert!(
                    !prompt.contains(recipe),
                    "content-inspection recipe survived: {recipe}"
                );
            }
            for (i, _) in prompt.match_indices("grep -") {
                let window = &prompt[i..(i + 60).min(prompt.len())];
                let near_secrets = [
                    "secret",
                    "credential",
                    "env_file",
                    "_PASSWORD",
                    "Bearer",
                    ".env",
                ]
                .iter()
                .any(|w| prompt[i.saturating_sub(300)..(i + 200).min(prompt.len())].contains(w));
                if near_secrets {
                    assert!(window.starts_with("grep -l"),
                        "only filename-only grep (-l) allowed near credential material, found: {window}");
                }
            }
        }
    }

    /// Every `AgentType`, with whether an audit may run on it. A new variant is a
    /// compile error in `agent_can_audit`; this table is what pins the decision.
    fn audit_capability_table() -> [(crate::models::AgentType, bool); 11] {
        use crate::models::AgentType;
        [
            (AgentType::ClaudeCode, true),
            (AgentType::Codex, true),
            (AgentType::OpenCode, true),
            (AgentType::GeminiCli, true),
            (AgentType::Kiro, true),
            (AgentType::CopilotCli, true),
            // KT-924 — HTTP agents whose file tools Kronn executes, scoped to the
            // project by `agent_launch`: they no longer lack a filesystem.
            (AgentType::Ollama, true),
            (AgentType::LiteLlm, true),
            // Vibe is API-only with no file tools; NVIDIA is hosted and was never
            // offered the project; Custom needs a connection the audit cannot pick.
            (AgentType::Vibe, false),
            (AgentType::Nvidia, false),
            (AgentType::Custom, false),
        ]
    }

    #[test]
    fn audit_capability_admits_the_agents_that_have_file_tools() {
        // The predicate gates BOTH launch endpoints because MCP callers bypass
        // the UI's canAudit.
        for (agent, expected) in audit_capability_table() {
            assert_eq!(agent_can_audit(&agent), expected, "{agent:?}");
        }
    }

    #[test]
    fn a_refused_agent_is_told_who_is_accepted_and_the_message_matches_the_gate() {
        // The Full and the partial launch share this one wording (KT-924): it
        // must list exactly the agents `agent_can_audit` admits — no more (a
        // refused one named as a fix), no fewer.
        let accepted: Vec<crate::models::AgentType> = audit_capability_table()
            .into_iter()
            .filter_map(|(agent, admitted)| admitted.then_some(agent))
            .collect();
        assert_eq!(
            AUDIT_AGENTS.to_vec(),
            accepted,
            "the list behind the message drifted from the gate"
        );
        for (agent, admitted) in audit_capability_table() {
            if admitted {
                continue;
            }
            let message = audit_refusal_message(&agent);
            assert!(
                message.starts_with(&format!("{agent:?} cannot run audits")),
                "{message}"
            );
            let (_, picks) = message.split_once("Pick one of: ").expect(&message);
            let picks: Vec<&str> = picks.trim_end_matches('.').split(", ").collect();
            let expected: Vec<String> = accepted
                .iter()
                .map(crate::api::disc_helpers::agent_display_name)
                .collect();
            assert_eq!(picks, expected, "{agent:?}");
            assert!(picks.contains(&"Ollama") && picks.contains(&"LiteLLM"));
            // The stale premise, and the stale advice, are gone.
            assert!(!message.contains("no filesystem access"), "{message}");
            assert!(!message.contains("Pick a CLI agent"), "{message}");
        }
    }

    #[test]
    fn partial_validation_prompt_matrix_localized_and_scoped() {
        // Codex lot-2 v3 — full localization matrix × TD presence. The
        // allowlist must be exact, EN/ES must carry no French protocol, and
        // the TD phase must vanish entirely when no TD was touched.
        use super::helpers::build_partial_validation_prompt;
        let files = vec!["docs/inconsistencies-security.md".to_string()];
        let ids = vec!["TD-20260720-x".to_string()];
        for lang in ["fr", "en", "es"] {
            // With TDs: allowlist = index + exact detail file, review phase present.
            let p = build_partial_validation_prompt(&files, &ids, lang);
            assert!(p.contains("docs/inconsistencies-security.md"), "{lang}");
            assert!(
                p.contains("docs/tech-debt/TD-20260720-x.md"),
                "{lang}: the exact TD detail file must be allowlisted"
            );
            assert!(p.contains("TD-20260720-x"), "{lang}: scope block present");
            assert!(p.contains("KRONN:VALIDATION_COMPLETE"), "{lang}");
            // Without TDs: no scope block, no TD phase, explicit no-TD note.
            let p0 = build_partial_validation_prompt(&files, &[], lang);
            assert!(!p0.contains("SCOPE"), "{lang}: empty ids → no scope block");
            assert!(!p0.contains("BULK-FIRST"), "{lang}: no TD review phase");
            assert!(
                !p0.contains("audit_history"),
                "{lang}: no TD instruction at all"
            );
        }
        // EN/ES carry none of the French protocol.
        for lang in ["en", "es"] {
            let p = build_partial_validation_prompt(&files, &ids, lang);
            for french in [
                "Marqueurs",
                "Revue des TDs",
                "gardent leur statut",
                "ré-émis",
            ] {
                assert!(
                    !p.contains(french),
                    "{lang} must not contain French: {french}"
                );
            }
        }
        // FR sanity.
        let fr = build_partial_validation_prompt(&files, &ids, "fr");
        assert!(fr.contains("SURFACE MODIFIABLE"));
    }

    #[test]
    fn preamble_says_autonomous() {
        assert!(
            PROMPT_PREAMBLE.contains("autonomous") || PROMPT_PREAMBLE.contains("non-interactive"),
            "PREAMBLE must instruct the agent not to ask questions"
        );
        assert!(
            PROMPT_PREAMBLE.contains("Do NOT ask questions")
                || PROMPT_PREAMBLE.contains("Do not ask"),
            "PREAMBLE must explicitly forbid questions"
        );
    }

    #[test]
    fn analysis_steps_include_decisions_md() {
        let has_decisions = ANALYSIS_STEPS
            .iter()
            .any(|step| step.prompt.contains("decisions.md"));
        assert!(
            has_decisions,
            "At least one audit step must fill decisions.md"
        );
    }

    #[test]
    fn step1_prompt_orders_template_scaffolding_cleanup() {
        // A filled AGENTS.md kept the "TEMPLATE FILE" banner + inline
        // bracketed hints (observed on the docroms-web run) — the prompt
        // must order their removal explicitly.
        let prompt = ANALYSIS_STEPS[0].prompt;
        assert!(
            prompt.contains("CLEANUP ONCE FILLED"),
            "cleanup section missing"
        );
        assert!(
            prompt.contains("TEMPLATE FILE"),
            "must name the banner to remove"
        );
        assert!(
            prompt.contains("bracketed hint"),
            "must order inline-hint removal"
        );
    }

    #[test]
    fn step6_prompt_enforces_mermaid_safety_rules() {
        // 0.8.3 — user reported DOCROMS_WEB sequence file `page-request.md`
        // wouldn't render: line `FP-->>U: 103 Early Hints (Link: …; rel=preload)`
        // triggered "Parse error on line 50, got NEWLINE expecting arrow".
        // The combination of Unicode `…`, parens, `:` and `;` inside the
        // message text confuses Mermaid 11.x's lexer. The prompt now
        // teaches the agent to write parser-safe messages.
        let arch_step = ANALYSIS_STEPS
            .iter()
            .find(|s| s.target_file == "docs/architecture/overview.md")
            .expect("architecture step must exist");
        let p = arch_step.prompt;
        assert!(
            p.contains("Mermaid sequenceDiagram safety rules"),
            "Step 6 must surface the safety rules section by name"
        );
        // The 4 specific gotchas the user hit:
        assert!(
            p.contains("ASCII-only"),
            "Step 6 must require ASCII-only message text (Unicode … breaks parser)"
        );
        assert!(
            p.contains(": ") && p.contains(";"),
            "Step 6 must call out `:` + `;` inside message text as risky"
        );
        assert!(
            p.contains("Note over"),
            "Step 6 must redirect detailed info to Note blocks"
        );
        assert!(
            p.contains("100 char") || p.contains("≤ 100"),
            "Step 6 must cap line length to surface parser-state issues"
        );
        // 0.8.6 — DOCROMS_WEB's request-lifecycle diagram broke: a participant
        // aliased `Alt` collided with the `alt` block keyword. The prompt must
        // forbid reserved-keyword aliases and offer a safe alternative.
        assert!(p.contains("reserved words") && p.contains("AltLoc"),
            "Step 6 must forbid reserved-keyword participant aliases (alt/else/end/…) with a safe-rename example");
    }

    #[test]
    fn step9_target_is_decisions_md_for_validation_guard() {
        // 0.8.3 FIX — decisions.md was getting forgotten because it was
        // a *secondary* output of Step 8 (tech-debt) buried 200 lines
        // deep in the prompt. `validate_step_output` only
        // checks `target_file`, so a missed decisions.md produced no
        // step_warning. Step 9 now PROMOTES decisions.md to its own
        // target_file so the guard fires when it's not filled, AND
        // the prompt is short + focused (2 phases: review + decisions).
        let step9 = ANALYSIS_STEPS.last().expect("at least one step");
        assert_eq!(
            step9.target_file, "docs/decisions.md",
            "Step 9 must target decisions.md so validate_step_output catches an unfilled file"
        );
        // The prompt must still cover the original "final review" duty.
        assert!(
            step9.prompt.contains("PHASE 1"),
            "Step 9 must keep the final-review phase"
        );
        assert!(
            step9.prompt.contains("PHASE 2"),
            "Step 9 must include the decisions.md fill phase"
        );
        // And explicitly mention all 3 marker types so the agent
        // applies the discipline rules added in #303 (F2).
        for marker in ["TODO: ask user", "TODO: verify", "TODO: unknown"] {
            assert!(
                step9.prompt.contains(marker),
                "Step 9 prompt must mention `{marker}` so the agent disambiguates marker types"
            );
        }
    }

    #[test]
    fn preamble_documents_marker_discipline_three_types() {
        // 0.8.3 FIX — DOCROMS_WEB audit had 26 `<!-- TODO: verify -->` on
        // testing-quality.md alone, most of them on files the agent
        // HAD actually verified (Glob/Read). The pre-fix preamble said
        // "mark unknowns with TODO: verify" with zero discrimination
        // between "I couldn't check" and "I checked and it's missing".
        // The new MARKER DISCIPLINE block names all 3 marker types,
        // gives a WRONG/RIGHT example, and explicitly tells the agent
        // not to fall back to TODO: verify after a successful Glob.
        assert!(PROMPT_PREAMBLE.contains("MARKER DISCIPLINE"),
            "PREAMBLE must surface the marker discipline section by name (it's the regression we're guarding)");
        for marker in ["TODO: verify", "TODO: ask user", "TODO: unknown"] {
            assert!(
                PROMPT_PREAMBLE.contains(marker),
                "PREAMBLE must mention `{marker}` so the agent knows the 3 types"
            );
        }
        // The WRONG/RIGHT pair is what teaches the agent to skip the
        // marker after a confirmed Glob — preserve both labels.
        assert!(
            PROMPT_PREAMBLE.contains("WRONG:") && PROMPT_PREAMBLE.contains("RIGHT:"),
            "PREAMBLE must show the WRONG/RIGHT example pair"
        );
        // Anti-regression: the old terse line "mark unknowns with TODO: verify"
        // was the very pattern that caused the over-use. Make sure the
        // new wording mentions the unverified case explicitly.
        assert!(
            PROMPT_PREAMBLE.contains("could not check")
                || PROMPT_PREAMBLE.contains("couldn't check"),
            "PREAMBLE must qualify TODO: verify as 'could not check', not generic 'unknown'"
        );
    }

    #[test]
    fn audit_step_blocks_do_not_duplicate_doctrine() {
        // 0.8.7 redesign — anti-regression test demanded by the architecte
        // expert (R2 P1-A). The doctrine lives in ONE place :
        // `docs/AGENTS.md` § Anti-Hallucination (written by STEP 0).
        // Step prompts may carry TASK-SPECIFIC reminders (e.g. "Step 4
        // requires 3 observed call sites for each CONVENTION") but must
        // NOT redume the cascade or the citation grammar — that's how
        // the doctrine started drifting in the first place. Pin no step
        // prompt re-inlines the cascade.
        //
        // We allow `[src: file:` to APPEAR in step prompts (steps need
        // to give CONCRETE EXAMPLES of citations they expect in cells),
        // but we forbid the cascade keywords that signal a doctrine
        // re-inline (`READ THE CODE`, `ANTI-HALLUCINATION PROTOCOL`,
        // `NEVER ASSERT WITHOUT PROOF`).
        for (i, step) in ANALYSIS_STEPS.iter().enumerate() {
            let step_num = i + 1;
            assert!(
                !step.prompt.contains("ANTI-HALLUCINATION PROTOCOL"),
                "step {step_num} ({}) re-inlines the protocol header — doctrine must live in docs/AGENTS.md § Anti-Hallucination (written by STEP 0), step prompts only carry task-specific reminders",
                step.target_file,
            );
            assert!(
                !step.prompt.contains("READ THE CODE"),
                "step {step_num} ({}) re-inlines the cascade step 1 — pointer to AGENTS.md § Anti-Hallucination is enough",
                step.target_file,
            );
            assert!(
                !step.prompt.contains("NEVER ASSERT WITHOUT PROOF"),
                "step {step_num} ({}) re-inlines the cascade step 5 — pointer is enough",
                step.target_file,
            );
            // Step prompts can be long (some are 400+ words for legit
            // task-specific reasons like Step 8's TD discipline). The
            // anti-duplication test is structural, not size-based.
        }
    }

    #[test]
    fn preamble_pins_pointer_to_anti_hallu_section() {
        // 0.8.7 redesign — the audit PROMPT_PREAMBLE no longer carries the
        // full doctrine inline. STEP 0 (`anti_hallu_step::apply`) writes
        // the canonical anti-hallucination section into `docs/AGENTS.md`
        // BEFORE the numbered steps run ; PROMPT_PREAMBLE now points at
        // that section + ships the citation grammar inline as a
        // token-budget safety net. Pin both : the pointer + the grammar.
        assert!(
            PROMPT_PREAMBLE.contains("docs/AGENTS.md")
                && PROMPT_PREAMBLE.contains("Anti-Hallucination"),
            "PROMPT_PREAMBLE must point at `docs/AGENTS.md` § Anti-Hallucination (written by STEP 0)",
        );
        // Citation grammar must remain inline so agents writing into
        // `docs/` know the structured form Kronn verifies mechanically.
        assert!(
            PROMPT_PREAMBLE.contains("[src: file:"),
            "[src: file:] grammar missing"
        );
        assert!(
            PROMPT_PREAMBLE.contains("[src: url:"),
            "[src: url:] grammar missing"
        );
        // The TODO: ask user escalation path is still the audit-specific
        // way to surface gaps — distinct from the runtime PREAMBLE's
        // `ASK THE USER` cascade step.
        assert!(
            PROMPT_PREAMBLE.contains("TODO") || PROMPT_PREAMBLE.contains("escalate"),
            "PROMPT_PREAMBLE must reference the escalation marker",
        );
        // Size sanity — PROMPT_PREAMBLE was 450+ words before the
        // redesign. The anti-hallu doctrine block is now a single
        // bullet pointer, but the MARKER DISCIPLINE section (the
        // 3 TODO marker types + WRONG/RIGHT example) is legitimate
        // audit task-specific doctrine that stays. KT-843 added two
        // more bullets (owner="human" preservation + never
        // force-translate an existing doc) — real doctrine that
        // applies to every step of a full OR partial audit, so it
        // belongs here rather than duplicated per-step. Cap raised to
        // stay loose enough to catch a real re-inline of the cascade
        // without flagging today's legitimate doctrine.
        let word_count = PROMPT_PREAMBLE.split_whitespace().count();
        assert!(
            word_count < 650,
            "PROMPT_PREAMBLE ballooned to {word_count} words — anti-hallu cascade should live in docs/AGENTS.md § Anti-Hallucination (written by STEP 0), not re-inlined here",
        );
    }

    #[test]
    fn preamble_forbids_rewriting_human_owned_sections_on_any_audit() {
        // KT-843 (Template v2) — the "Never edit docs/AGENTS.md" blanket
        // ban is gone; ownership is now per-section (`owner="audit"` vs
        // `owner="human"`). PROMPT_PREAMBLE is injected before EVERY
        // step of a Full run (full.rs) AND a Drift/partial run
        // (drift.rs), so this is the one place that guards BOTH: a
        // human-owned section must never be silently rewritten, and a
        // re-audit proposes the change as a diff instead.
        assert!(
            PROMPT_PREAMBLE.contains("owner=\"human\""),
            "PROMPT_PREAMBLE must name the owner=\"human\" ownership marker"
        );
        assert!(
            PROMPT_PREAMBLE.to_lowercase().contains("never rewrite"),
            "PROMPT_PREAMBLE must forbid rewriting a human-owned section"
        );
        assert!(
            PROMPT_PREAMBLE
                .to_lowercase()
                .contains("full or partial audit")
                || PROMPT_PREAMBLE.contains("full OR partial audit"),
            "the rule must explicitly cover BOTH a full and a partial (drift) audit"
        );
        assert!(
            PROMPT_PREAMBLE.contains("diff"),
            "a re-audit must propose the change as a diff, not silently overwrite"
        );
        // The old blanket ban must actually be gone (KT-843 objective).
        assert!(
            !PROMPT_PREAMBLE.contains("Never edit docs/AGENTS.md"),
            "the blanket \"Never edit docs/AGENTS.md\" ban must be replaced by the per-section ownership rule"
        );
    }

    #[test]
    fn full_and_partial_audits_restore_human_sections_before_retry_decisions() {
        for (pipeline, source) in [
            ("full", include_str!("full.rs")),
            ("partial", include_str!("drift.rs")),
        ] {
            let protection = source
                .find("protect_human_owned_sections")
                .unwrap_or_else(|| panic!("{pipeline} audit must enforce section ownership"));
            let retry_gate = source
                .find("evaluate_enforce_gate")
                .unwrap_or_else(|| panic!("{pipeline} audit must contain its retry gate"));
            assert!(
                protection < retry_gate,
                "{pipeline} audit must restore owner=\"human\" content before a retry can start"
            );
        }
    }

    #[test]
    fn docs_agents_md_template_states_project_parameters_and_language_preservation() {
        // KT-843 DoD #3 — docs/tickets language, the comment→ticket
        // policy, and the test policy are project parameters (set once
        // during audit), not fixed doctrine. An existing non-English doc
        // must be preserved, never force-translated.
        let tpl_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("templates/docs/AGENTS.md");
        let body = std::fs::read_to_string(&tpl_path)
            .unwrap_or_else(|e| panic!("read template {}: {e}", tpl_path.display()));
        assert!(
            body.contains("Project parameters"),
            "template must declare a Project parameters block"
        );
        for token in [
            "{{DOCS_LANGUAGE}}",
            "{{TICKET_LANGUAGE}}",
            "{{COMMENT_TICKET_POLICY}}",
            "{{TEST_POLICY}}",
        ] {
            assert!(
                body.contains(token),
                "template must expose {token} as a project parameter"
            );
        }
        assert!(
            body.contains("Ticket IDs in code comments: {{COMMENT_TICKET_POLICY}}"),
            "the comment→ticket policy must be an explicit project parameter"
        );
        assert!(
            body.to_lowercase().contains("never force-translate")
                || body.to_lowercase().contains("never translate")
                || body
                    .to_lowercase()
                    .contains("preserve every existing document in its current language"),
            "template must state that an existing non-English doc is kept as-is, not translated"
        );
        // The step-1 prompt (docs/AGENTS.md is its target_file) must cite
        // the exact same tokens it asks the agent to fill — otherwise a
        // weak model is told to "replace ALL {{PLACEHOLDERS}}" with no
        // concrete pointer to these two, and may skip them.
        let step1 = &ANALYSIS_STEPS[0];
        assert_eq!(step1.target_file, "docs/AGENTS.md");
        for token in [
            "{{DOCS_LANGUAGE}}",
            "{{TICKET_LANGUAGE}}",
            "{{COMMENT_TICKET_POLICY}}",
            "{{TEST_POLICY}}",
        ] {
            assert!(
                step1.prompt.contains(token),
                "step 1 prompt must cite {token} so the agent doesn't skip it"
            );
        }
    }

    #[test]
    fn docs_agents_md_template_routes_workflow_environments_examples_and_reports() {
        // KT-843 DoD #2 — the workflow/, environments.md, examples/ and
        // reports/ cases exist in the template AND are routed from
        // AGENTS.md. `document_optimization::analyze` flags an unrouted
        // doc as an orphan (non-blocking, but still a real drift signal),
        // so this test pins both halves of the contract: the link text
        // in AGENTS.md, and the linked file actually existing.
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let tpl_path = workspace_root.join("templates/docs/AGENTS.md");
        let body = std::fs::read_to_string(&tpl_path)
            .unwrap_or_else(|e| panic!("read template {}: {e}", tpl_path.display()));

        for (link_target, must_exist_at) in [
            ("workflow/README.md", "templates/docs/workflow/README.md"),
            ("workflow/commits.md", "templates/docs/workflow/commits.md"),
            (
                "workflow/pull-requests.md",
                "templates/docs/workflow/pull-requests.md",
            ),
            ("workflow/tickets.md", "templates/docs/workflow/tickets.md"),
            ("workflow/ci-cd.md", "templates/docs/workflow/ci-cd.md"),
            ("environments.md", "templates/docs/environments.md"),
            ("examples/README.md", "templates/docs/examples/README.md"),
            ("reports/README.md", "templates/docs/reports/README.md"),
        ] {
            assert!(
                body.contains(link_target),
                "templates/docs/AGENTS.md must route to `{link_target}`"
            );
            assert!(
                workspace_root.join(must_exist_at).is_file(),
                "`{must_exist_at}` must exist on disk"
            );
        }
        // Reports are explicitly excluded from the tiered routing
        // strategy (dated snapshots, never "always loaded") even though
        // they are still linked (so the audit doesn't flag them as an
        // orphan document).
        assert!(
            body.contains("excluded from T0–T2 routing"),
            "docs/reports/ must be documented as excluded from the tiered routing strategy"
        );
        for level in ["T0 — Router", "T1 — Routed context", "T2 — Search"] {
            assert!(
                body.contains(level),
                "the weak-agent routing map must preserve level `{level}`"
            );
        }
        assert!(
            body.contains("Skills and Kronn resources index:") && body.contains("kronn/INDEX.md"),
            "the T0 router must expose the skills/resources index"
        );
    }

    #[test]
    fn step1_named_placeholders_all_exist_in_the_agents_md_template() {
        // KT-843 DoD #4 — every placeholder the step-1 prompt names
        // explicitly must exist in the template it fills, so a weak
        // model isn't told to "replace {{X}}" in a file that actually
        // shows a differently-named token. Mirrors the same contract
        // `architecture_template_carries_mermaid_placeholder_and_sequences_pointer`
        // already pins for docs/architecture/overview.md.
        let step1 = &ANALYSIS_STEPS[0];
        assert_eq!(step1.target_file, "docs/AGENTS.md");
        let tpl_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("templates/docs/AGENTS.md");
        let body = std::fs::read_to_string(&tpl_path)
            .unwrap_or_else(|e| panic!("read template {}: {e}", tpl_path.display()));
        for token in [
            "{{PROJECT_NAME}}",
            "{{STACK_SUMMARY}}",
            "{{PROJECT_LANGUAGE}}",
            "{{TASK_1}}",
            "{{DO_NOT_1}}",
            "{{DO_NOT_2}}",
            "{{WORKFLOW_CONSTRAINT_1}}",
            "{{WORKFLOW_CONSTRAINT_2}}",
            "{{DOCS_LANGUAGE}}",
            "{{TICKET_LANGUAGE}}",
            "{{COMMENT_TICKET_POLICY}}",
            "{{TEST_POLICY}}",
            "{{DATE}}",
        ] {
            assert!(
                step1.prompt.contains(token),
                "step 1 prompt should cite {token}"
            );
            assert!(
                body.contains(token),
                "template must expose {token}, cited by the step 1 prompt"
            );
        }
    }

    /// Extract every literal (non-wildcard) `{{UPPER_SNAKE}}` token a prompt
    /// cites. `{{FOO_*}}` family references are intentionally excluded —
    /// they name a class of rows the agent may add/remove freely (per
    /// `PROMPT_PREAMBLE`'s "add or remove table rows" rule), not a single
    /// token the template must carry verbatim.
    fn cited_literal_tokens(prompt: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = prompt;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                break;
            };
            let inside = &after[..end];
            let is_literal = !inside.is_empty()
                && !inside.contains('*')
                && inside
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if is_literal {
                out.push(format!("{{{{{inside}}}}}"));
            }
            rest = &after[end + 2..];
        }
        out
    }

    #[test]
    fn every_step_prompt_placeholder_citation_exists_in_its_template() {
        // KT-843 DoD #4 — "chaque placeholder cité par un prompt d'audit
        // existe dans le template". Generalizes
        // `step1_named_placeholders_all_exist_in_the_agents_md_template` to
        // every foundation step with a real shipped template: a prompt
        // that tells the agent to "replace {{X}}" must cite a token the
        // template it fills actually contains, or a weak model is pointed
        // at a placeholder that doesn't exist and may skip the real one.
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        // Generic meta-language ("replace ALL {{PLACEHOLDERS}}") — every
        // step's prompt uses it to mean "every placeholder in the file",
        // not a literal token any template carries under that exact name.
        const META_TOKEN: &str = "{{PLACEHOLDERS}}";

        let mut checked_steps = 0;
        for step in ANALYSIS_STEPS {
            if step.target_file == "REVIEW" || step.target_file.is_empty() {
                continue;
            }
            let tpl_path = workspace_root.join("templates").join(step.target_file);
            if !tpl_path.is_file() {
                continue;
            }
            let body = std::fs::read_to_string(&tpl_path)
                .unwrap_or_else(|e| panic!("read template {}: {e}", tpl_path.display()));
            checked_steps += 1;
            for token in cited_literal_tokens(step.prompt) {
                if token == META_TOKEN {
                    continue;
                }
                assert!(
                    body.contains(&token),
                    "{}: prompt cites {token} but the template doesn't contain it",
                    step.target_file
                );
            }
        }
        assert!(
            checked_steps >= 7,
            "expected every foundation step (1,3-9) to have a shipped template to check against, got {checked_steps}"
        );
    }

    #[test]
    fn step8_does_not_duplicate_decisions_md_instruction() {
        // 0.8.3 FIX — before, Step 8 also had a `# E. ALSO FILL
        // docs/decisions.md` section that the agent routinely
        // forgot (buried in 200 lines). We moved the duty to Step 9
        // and replaced the Step 8 mention with a scope reminder. Pin
        // the no-duplicate state so a future "let's also fill it here"
        // drift gets caught.
        let step8 = ANALYSIS_STEPS
            .iter()
            .find(|s| s.target_file == "docs/inconsistencies-tech-debt.md")
            .expect("Step 8 must exist");
        // The "ALSO FILL" pattern is the regex marker for the bug.
        assert!(
            !step8.prompt.contains("ALSO FILL docs/decisions.md"),
            "Step 8 must NOT instruct decisions.md fill anymore — Step 9 owns it now"
        );
    }

    #[test]
    fn architecture_template_carries_mermaid_placeholder_and_sequences_pointer() {
        // The audit prompt + the template ship together. If one
        // grows a section the other must keep up, otherwise the
        // agent fills `{{ARCHITECTURE_MERMAID}}` into a section
        // that doesn't exist and the placeholder leaks into the
        // final docs/architecture/overview.md.
        let tpl_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("templates/docs/architecture/overview.md");
        let body = std::fs::read_to_string(&tpl_path)
            .unwrap_or_else(|e| panic!("read template {}: {e}", tpl_path.display()));
        assert!(body.contains("{{ARCHITECTURE_MERMAID}}"),
            "template must expose `{{{{ARCHITECTURE_MERMAID}}}}` so the audit step can write into it");
        assert!(
            body.contains("Architecture diagram"),
            "template must have an `Architecture diagram` section header"
        );
        assert!(
            body.contains("Sequence diagrams"),
            "template must have a `Sequence diagrams` section that points to `sequences/`"
        );
        assert!(
            body.contains("sequences/"),
            "template must link to the dedicated sequences subfolder"
        );
        // Sanity: the sequences/ subfolder ships with a README and a
        // TEMPLATE.md so the audit doesn't generate orphan files.
        let seq_readme = tpl_path.parent().unwrap().join("sequences/README.md");
        let seq_tpl = tpl_path.parent().unwrap().join("sequences/TEMPLATE.md");
        assert!(
            seq_readme.exists(),
            "sequences/README.md must ship with the template tree"
        );
        assert!(
            seq_tpl.exists(),
            "sequences/TEMPLATE.md must ship with the template tree"
        );
        let tpl_body = std::fs::read_to_string(&seq_tpl).unwrap();
        assert!(tpl_body.contains("sequenceDiagram"),
            "sequences/TEMPLATE.md must show a Mermaid `sequenceDiagram` so the audit fills follow the same shape");
    }

    #[test]
    fn step6_architecture_step_requires_mermaid_diagrams() {
        // 0.8.3 (#286) — the architecture step ships with a mandatory
        // Mermaid diagram (replacing the legacy ASCII flow) PLUS a
        // bounded sequence-diagram budget (max 3 files under
        // `sequences/`). Lock the contract so a future "let's drop
        // the diagram, agents waste tokens on it" tidy-up gets caught.
        let arch_step = ANALYSIS_STEPS
            .iter()
            .find(|s| s.target_file == "docs/architecture/overview.md")
            .expect("audit must include an architecture step");
        let p = arch_step.prompt;
        assert!(
            p.contains("Mermaid") || p.contains("mermaid"),
            "architecture step must instruct the agent to emit Mermaid syntax"
        );
        assert!(
            p.contains("flowchart"),
            "architecture step must specify a `flowchart` Mermaid block for the overview"
        );
        assert!(
            p.contains("ARCHITECTURE_MERMAID"),
            "the template placeholder must match the prompt's named field"
        );
        assert!(
            p.contains("sequenceDiagram"),
            "architecture step must mention `sequenceDiagram` so per-flow files are also Mermaid"
        );
        assert!(
            p.contains("sequences/"),
            "architecture step must point to the dedicated `sequences/` subfolder"
        );
        assert!(
            p.contains("3 files maximum") || p.contains("max 3") || p.contains("3 maximum"),
            "architecture step must cap sequence diagrams to avoid token explosion on big projects"
        );
    }

    #[test]
    fn step8_baseline_includes_community_standards_gate() {
        // The Step 8 prompt grew an OSS-intent-gated community standards
        // section (LICENSE / README description / issue+PR templates /
        // SECURITY / CONTRIBUTING / CODE_OF_CONDUCT). Regress against
        // any future cleanup that accidentally strips it.
        let step8 = ANALYSIS_STEPS
            .iter()
            .find(|s| s.target_file == "docs/inconsistencies-tech-debt.md")
            .expect("Step 8 prompt must target docs/inconsistencies-tech-debt.md");
        let prompt = step8.prompt;
        assert!(
            prompt.contains("Community standards"),
            "Step 8 must include the 'Community standards' section header"
        );
        assert!(
            prompt.contains("OSS intent"),
            "Section must be gated on OSS intent (private projects skipped)"
        );
        for needle in [
            "LICENSE",
            "ISSUE_TEMPLATE",
            "pull_request_template",
            "SECURITY.md",
            "CONTRIBUTING.md",
            "CODE_OF_CONDUCT.md",
        ] {
            assert!(
                prompt.contains(needle),
                "Community-standards block must check `{}`",
                needle
            );
        }
        for needle in [
            "Repository hygiene",
            "git ls-files --cached",
            "tracked/index content",
            ".pnpm-store/",
            "Runtime databases/logs/dumps",
            "findings/",
            "harmless local/untracked developer files",
            "documented Yarn zero-install/offline-mirror",
            "The 7 categories above",
            "bounded to 15 paths per detector",
        ] {
            assert!(
                prompt.contains(needle),
                "Repository-hygiene baseline must retain `{}`",
                needle
            );
        }
    }

    #[test]
    fn step8_dimensional_scan_forces_per_dimension_coverage() {
        // The drastic improvement: every dimension is ACCOUNTABLE via the
        // `## Dimension coverage` matrix — a dimension can't be silently
        // skipped (the root cause of the "partially complete" audit).
        let step = ANALYSIS_STEPS
            .iter()
            .find(|s| s.target_file == "docs/inconsistencies-tech-debt.md")
            .expect("Step 8 must target docs/inconsistencies-tech-debt.md");
        let p = step.prompt;
        assert!(
            p.contains("Dimension coverage"),
            "Step 8 must require the `## Dimension coverage` matrix"
        );
        assert!(
            p.contains("scanned — nothing substantiable"),
            "must offer the explicit 'scanned, nothing' outcome (≠ 'didn't look')"
        );
        assert!(
            p.contains("verifiable reason"),
            "an N/A dimension must carry a human-verifiable reason"
        );
        assert!(
            p.contains("incomplete audit"),
            "a dimension absent from the matrix must mark the audit incomplete"
        );
        assert!(
            p.contains("Self-critic"),
            "must include the pre-finish self-critic pass"
        );
    }

    #[test]
    fn phase3_ticket_offer_only_with_tracker_mcp() {
        let info = AuditInfo {
            files: vec![],
            todos: vec![],
            tech_debt_items: vec![],
        };
        for lang in ["fr", "en", "es"] {
            let with = build_validation_prompt(lang, &info, true, &[]);
            let without = build_validation_prompt(lang, &info, false, &[]);
            assert!(
                with.to_lowercase().contains("ticket"),
                "{} prompt with tracker MCP must contain the ticket offer",
                lang
            );
            assert!(
                !without.to_lowercase().contains("ticket"),
                "{} prompt without tracker MCP must not contain a ticket offer",
                lang
            );
        }
    }

    #[test]
    fn phases2_and3_use_td_cards_with_high_priority_singles_and_batches_of_eight() {
        for lang in ["fr", "en", "es"] {
            let info = AuditInfo {
                files: vec![],
                todos: vec![],
                tech_debt_items: vec![crate::models::TechDebtItem {
                    id: "TD-20260928-example".into(),
                    problem: "Example".into(),
                    area: "Backend".into(),
                    severity: "High".into(),
                }],
            };
            let prompt = build_validation_prompt(lang, &info, false, &[]);
            for contract in [
                "kronn-question",
                "audit-td:<TD-ID>",
                "audit-td-batch",
                "`items`",
                "`confirm`",
                "`reject`",
                "`accept_decision`",
                "`defer`",
            ] {
                assert!(
                    prompt.contains(contract),
                    "{lang} validation prompt must contain `{contract}`"
                );
            }
            assert!(
                prompt.contains("8 `items`"),
                "{lang} remaining TD batches must be capped at eight items"
            );
            let expected_summary = match lang {
                "en" => "across Phases 2 and 3",
                "es" => "entre las Fases 2 y 3",
                _ => "entre les Phases 2 et 3",
            };
            assert!(
                prompt.contains(expected_summary),
                "{lang} TD summary must cover both card phases"
            );
        }
    }

    #[test]
    fn validation_prompt_has_no_legacy_bulk_first_contradiction() {
        for lang in ["fr", "en", "es"] {
            let info = AuditInfo {
                files: vec![],
                todos: vec![],
                tech_debt_items: vec![],
            };
            let prompt = build_validation_prompt(lang, &info, false, &[]);
            let lower = prompt.to_lowercase();
            assert!(
                !lower.contains("bulk-first")
                    && !lower.contains("tout valider")
                    && !lower.contains("do not batch-confirm")
                    && !lower.contains("no confirmar en lote")
                    && !lower.contains("pas de confirmation en lot"),
                "{} validation prompt must not retain the contradictory legacy protocol",
                lang
            );
        }
    }

    #[test]
    fn validation_prompt_forbids_code_modification() {
        for lang in ["fr", "en", "es"] {
            let info = AuditInfo {
                files: vec![],
                todos: vec![],
                tech_debt_items: vec![],
            };
            let prompt = build_validation_prompt(lang, &info, false, &[]);
            let lower = prompt.to_lowercase();
            assert!(
                lower.contains("modify only `docs/`")
                    || lower.contains("modifica solo `docs/`")
                    || lower.contains("modifie uniquement `docs/`"),
                "Validation prompt ({}) must restrict changes to docs",
                lang
            );
        }
    }

    #[test]
    fn analysis_steps_have_sources_field() {
        let with_sources: Vec<_> = ANALYSIS_STEPS
            .iter()
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
    fn analysis_steps_count_is_9() {
        assert_eq!(
            ANALYSIS_STEPS.len(),
            9,
            "Expected exactly 9 analysis steps, got {}",
            ANALYSIS_STEPS.len()
        );
    }

    #[test]
    fn briefing_notes_injected_into_prompt() {
        let briefing_notes: Option<String> =
            Some("This project uses microservices with gRPC".into());
        let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, ANALYSIS_STEPS[0].prompt);

        if let Some(ref notes) = briefing_notes {
            full_prompt.push_str(&format!(
                "\n\n## Project briefing (from the user)\n{}\n",
                notes
            ));
        }

        assert!(
            full_prompt.contains("## Project briefing (from the user)"),
            "Briefing section header must be present when notes are set"
        );
        assert!(
            full_prompt.contains("microservices with gRPC"),
            "User's briefing content must appear in the prompt"
        );
    }

    #[test]
    fn briefing_notes_not_injected_when_none() {
        let briefing_notes: Option<String> = None;
        let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, ANALYSIS_STEPS[0].prompt);

        if let Some(ref notes) = briefing_notes {
            full_prompt.push_str(&format!(
                "\n\n## Project briefing (from the user)\n{}\n",
                notes
            ));
        }

        assert!(
            !full_prompt.contains("## Project briefing"),
            "Briefing section must NOT appear when notes are None"
        );
    }

    #[test]
    fn briefing_prompt_is_localized() {
        let fr = build_briefing_prompt("fr", None);
        let en = build_briefing_prompt("en", None);
        let es = build_briefing_prompt("es", None);
        assert_ne!(fr, en, "FR and EN briefing prompts must differ");
        assert_ne!(en, es, "EN and ES briefing prompts must differ");
        assert_ne!(fr, es, "FR and ES briefing prompts must differ");
    }

    #[test]
    fn briefing_prompt_forbids_code_reading() {
        let fr = build_briefing_prompt("fr", None);
        let en = build_briefing_prompt("en", None);
        let es = build_briefing_prompt("es", None);
        assert!(
            fr.contains("ne lis PAS"),
            "FR briefing prompt must contain 'ne lis PAS'"
        );
        assert!(
            en.contains("Do NOT read"),
            "EN briefing prompt must contain 'Do NOT read'"
        );
        assert!(
            es.contains("NO leas"),
            "ES briefing prompt must contain 'NO leas'"
        );
    }

    #[test]
    fn briefing_prompt_requires_answers_1_to_5() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang, None);
            assert!(
                prompt.contains("1-5") || prompt.contains("1 a 5") || prompt.contains("1-5"),
                "Briefing prompt ({}) must reference questions 1-5 as required",
                lang
            );
            let lower = prompt.to_lowercase();
            assert!(
                lower.contains("optional")
                    || lower.contains("optionnel")
                    || lower.contains("opcional"),
                "Briefing prompt ({}) must mark Q6 as optional",
                lang
            );
        }
    }

    #[test]
    fn briefing_prompt_says_stack_auto_detected() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang, None);
            let lower = prompt.to_lowercase();
            assert!(
                lower.contains("auto-detect") || lower.contains("auto-detect"),
                "Briefing prompt ({}) must mention stack is auto-detected",
                lang
            );
        }
    }

    #[test]
    fn briefing_prompt_contains_completion_signal() {
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang, None);
            assert!(
                prompt.contains("KRONN:BRIEFING_COMPLETE"),
                "Briefing prompt ({}) must contain KRONN:BRIEFING_COMPLETE",
                lang
            );
        }
    }

    /// 0.8.4 (#320 / B4) — guard against ts-rs export drift on
    /// `LaunchAuditRequest`. ts-rs 12.x has an incremental-compile
    /// quirk: when fields are added to a `#[ts(export)]` struct,
    /// `cargo test` doesn't always re-fire the auto-generated export
    /// test, so the .ts file goes stale and the frontend can compile
    /// against a wrong shape. This test fails loudly when the
    /// declared Rust shape no longer matches what we expect the .ts
    /// file to contain, forcing the maintainer to either run
    /// `touch backend/src/models/projects.rs && cargo test export_bindings`
    /// to regen, or hand-edit `frontend/src/types/LaunchAuditRequest.ts`.
    #[test]
    fn launch_audit_request_shape_pins_kind_and_resume_run_id() {
        // Round-trip JSON to assert the field set hasn't drifted from
        // what the frontend expects. If a new field is added to the
        // Rust struct, this test forces the maintainer to also update
        // the hand-shipped `frontend/src/types/LaunchAuditRequest.ts`
        // (which the ts-rs auto-export sometimes fails to refresh —
        // see B4 in `PLAYWRIGHT_AUDIT_REVIEW.md`).
        use crate::models::{AgentType, AuditKind, LaunchAuditRequest, ModelTier};
        let req: LaunchAuditRequest = serde_json::from_str(
            r#"{
            "agent": "ClaudeCode",
            "tier": "reasoning",
            "kind": "Rgaa",
            "custom_prompt": null,
            "resume_run_id": "run-abc"
        }"#,
        )
        .expect("LaunchAuditRequest must accept the current shape");
        assert!(matches!(req.agent, AgentType::ClaudeCode));
        assert_eq!(req.tier, Some(ModelTier::Reasoning));
        assert_eq!(req.kind, Some(AuditKind::Rgaa));
        assert!(req.custom_prompt.is_none());
        assert_eq!(req.resume_run_id.as_deref(), Some("run-abc"));

        // Backwards compat: a client that only sends `agent` must still
        // parse (the audit pipeline defaults kind=Full, fresh run).
        let legacy: LaunchAuditRequest = serde_json::from_str(r#"{"agent":"ClaudeCode"}"#)
            .expect("minimal shape must still parse");
        assert!(legacy.kind.is_none());
        assert!(legacy.tier.is_none());
        assert!(legacy.resume_run_id.is_none());
    }

    /// 0.8.4 (#320 / B4) — assert the hand-shipped
    /// `frontend/src/types/LaunchAuditRequest.ts` covers ALL fields of
    /// the Rust struct. ts-rs auto-export is unreliable on this struct
    /// in the current setup (cf. `launch_audit_request_shape_pins_kind_and_resume_run_id`),
    /// so we pin the file content here. If a new field is added to
    /// the Rust struct, this test fails until the .ts file is
    /// updated to match — preventing the silent type drift that bit
    /// us during the 0.8.4 sub-audit work.
    #[test]
    fn launch_audit_request_ts_file_covers_all_rust_fields() {
        // The .ts file lives outside the crate; resolve via
        // CARGO_MANIFEST_DIR which points at `backend/`.
        let manifest =
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo");
        let ts_path = std::path::Path::new(&manifest)
            .join("..")
            .join("frontend")
            .join("src")
            .join("types")
            .join("LaunchAuditRequest.ts");
        let content = std::fs::read_to_string(&ts_path).unwrap_or_else(|e| {
            panic!(
                "Cannot read {} — did the file get deleted? ({})",
                ts_path.display(),
                e,
            )
        });

        // Each Rust field must appear in the .ts shape, in some form
        // (with `?` for Option<T>). The test is intentionally loose
        // on the exact spelling — what matters is that the property
        // name is present and the file imports the right enum types.
        for field in ["agent", "tier", "kind", "custom_prompt", "resume_run_id"] {
            assert!(content.contains(field),
                "LaunchAuditRequest.ts is missing field `{}` — update the hand-shipped file to match the Rust struct ({})",
                field, ts_path.display(),
            );
        }
        // The enum-typed fields must import their referenced types so
        // tsc compiles. A regen that strips the import would compile
        // fine in isolation but break consumers.
        assert!(
            content.contains("AgentType"),
            "LaunchAuditRequest.ts must reference AgentType"
        );
        assert!(
            content.contains("AuditKind"),
            "LaunchAuditRequest.ts must reference AuditKind (0.8.4)"
        );
        assert!(
            content.contains("ModelTier"),
            "LaunchAuditRequest.ts must reference ModelTier"
        );
    }

    #[test]
    fn briefing_review_prompt_skips_the_6_q_interrogation() {
        // 0.8.4 UX fix — when the user has already submitted the form,
        // the agent must NOT re-ask the 6 questions. The review prompt
        // embeds the user's answers verbatim and only asks targeted
        // clarifications.
        let prefilled = "## Purpose\nA Kronn-managed audit dashboard.\n\n## Team\nSolo dev.\n";
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang, Some(prefilled));
            assert!(
                prompt.contains(prefilled),
                "Review prompt ({lang}) must echo the user's answers verbatim"
            );
            // Must explicitly forbid re-asking the 6 questions wholesale.
            let lower = prompt.to_lowercase();
            assert!(
                lower.contains("ne repose pas")
                    || lower.contains("not re-ask")
                    || lower.contains("no repreguntes"),
                "Review prompt ({lang}) must forbid re-asking the full 6-question set",
            );
            // Still ends with the completion signal so the audit pipeline
            // can detect readiness.
            assert!(
                prompt.contains("KRONN:BRIEFING_COMPLETE"),
                "Review prompt ({lang}) must keep the completion signal"
            );
            // Must NOT include the legacy 6-question enumeration that the
            // None branch ships — that's the whole point.
            assert!(
                !prompt.contains("STEP 1")
                    && !prompt.contains("ETAPE 1")
                    && !prompt.contains("PASO 1"),
                "Review prompt ({lang}) must NOT re-display the legacy 6-step interrogation header",
            );
        }
    }

    #[test]
    fn briefing_prompt_legacy_mode_unchanged_when_no_prefill() {
        // Sanity check: the original 6-Q prompt is still emitted when
        // the caller passes None (no form submission yet). Without this
        // the audit pipeline that calls start_briefing directly would
        // silently switch to review mode + crash on empty notes.
        for lang in ["fr", "en", "es"] {
            let prompt = build_briefing_prompt(lang, None);
            assert!(
                prompt.contains("STEP 1")
                    || prompt.contains("ETAPE 1")
                    || prompt.contains("PASO 1"),
                "Legacy briefing prompt ({lang}) must still expose the step header when no prefill",
            );
        }
    }
}
