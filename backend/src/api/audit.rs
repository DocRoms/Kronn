use std::convert::Infallible;
use std::pin::Pin;

use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use futures::Stream;
use uuid::Uuid;

use crate::agents::runner;
use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

// ─── AI Audit (SSE streaming) ───────────────────────────────────────────────

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
        target_file: "ai/index.md",
        prompt: "\
Read README.md, package.json (or composer.json, Cargo.toml, go.mod), Makefile, Dockerfile, docker-compose.yml, \
CI configs (.github/workflows, .gitlab-ci.yml), and main config files.\n\
Determine: tech stack, project structure, build/dev/test commands, key patterns, third-party services, CI/CD pipeline.\n\n\
Then fill ai/index.md — replace ALL {{PLACEHOLDERS}} in each section:\n\
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
        target_file: "ai/glossary.md",
        prompt: "\
Read ai/index.md. Search codebase for domain terms, abbreviations, naming conventions.\n\n\
Fill ai/glossary.md — replace ALL {{PLACEHOLDERS}}:\n\
- Categorize: Architecture, Domain, Environments, External, Abbreviations\n\
- Each term: one-line definition + optional ai/ reference\n\
- Unknown domain terms: add `<!-- TODO: ask user -->` after the definition\n\
- Cover: framework terms, model names, services, acronyms in code",
        sources: &[],
    },

    // Step 3: Repo map
    AnalysisStep {
        target_file: "ai/repo-map.md",
        prompt: "\
Read ai/index.md and ai/glossary.md for context. Explore the directory structure (2-3 levels deep).\n\n\
Fill ai/repo-map.md — replace ALL {{PLACEHOLDERS}}:\n\
- {{STACK_OVERVIEW}}: one paragraph summarizing the architecture\n\
- Key folders tree: replace {{FOLDER_*}} with every major directory (2-3 levels deep), tree format with annotations\n\
- Entrypoints table: replace {{ENTRYPOINT_*}} with 5-7 key files (config, routes, models, etc.)\n\
- Auto-generated files: replace {{FILE_PATTERN}} with files NOT to edit manually",
        sources: &["__GIT_LS_FILES__"],
    },

    // Step 4: Coding rules
    AnalysisStep {
        target_file: "ai/coding-rules.md",
        prompt: "\
Read ai/index.md for context. Find ALL linter, formatter, and type-checker configs in the repo \
(e.g. .eslintrc, eslint.config.js, prettier, rustfmt.toml, tsconfig.json, phpcs.xml, etc.).\n\n\
Fill ai/coding-rules.md — replace ALL {{PLACEHOLDERS}}:\n\
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
        target_file: "ai/testing-quality.md",
        prompt: "\
Read ai/index.md for context. Find test framework configs (jest, vitest, phpunit, pytest, cargo test, bats, etc.) \
and CI quality gates.\n\n\
Fill ai/testing-quality.md — replace ALL {{PLACEHOLDERS}}:\n\
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
        target_file: "ai/architecture/overview.md",
        prompt: "\
Read ai/index.md and ai/repo-map.md for context. Analyze the high-level architecture.\n\n\
Fill ai/architecture/overview.md — replace ALL {{PLACEHOLDERS}}:\n\
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
        target_file: "ai/operations/debug-operations.md",
        prompt: "\
Read ai/index.md for context. Find operational commands from Makefile, package.json scripts, \
docker-compose commands, and any run/build/debug procedures.\n\n\
Fill ai/operations/debug-operations.md — replace ALL {{PLACEHOLDERS}}:\n\
- Common commands table: replace {{ACTION_*}} and {{COMMAND_*}} for start, stop, logs, test, build, deploy\n\
- Docker services table: replace {{SERVICE_*}}, {{PORT}}, {{ROLE}}, {{HEALTH}} for each container\n\
- Troubleshooting: replace {{ISSUE_*_TITLE}}, {{SYMPTOM}}, {{CAUSE}}, {{FIX}} with 3-5 common issues",
        sources: &["docker-compose.yml", "Makefile", "Dockerfile"],
    },

    // Step 8: MCP servers overview
    AnalysisStep {
        target_file: "ai/operations/mcp-servers.md",
        prompt: "\
Read ai/index.md for context. Check if .mcp.json or .mcp.json.example or .env.mcp.example exists in the repo.\n\n\
If MCP config exists:\n\
- Document each configured MCP server in ai/operations/mcp-servers.md\n\
- For each server: name, transport type, what it's used for, required env vars\n\
- ONLY create a context file at ai/operations/mcp-servers/<slug>.md if you have \
project-specific rules, constraints, or usage patterns to document for that MCP.\n\
  Do NOT create empty or boilerplate context files — they add no value.\n\
  A context file should contain: purpose in this project, specific rules, and usage examples.\n\n\
If no MCP config exists: replace ai/operations/mcp-servers.md content with:\n\
'# MCP Servers\\n\\nNo MCP servers configured for this project.'",
        sources: &[".mcp.json"],
    },

    // Step 9: Inconsistencies & tech debt
    AnalysisStep {
        target_file: "ai/inconsistencies-tech-debt.md",
        prompt: "\
Real issues only, not hypothetical. Read all ai/ files AND scan source code.\n\
Scan: entry points, config files, Dockerfiles, CI configs, and 5-10 core source files \
(prioritize auth, data persistence, external input handling).\n\n\
Systematically audit across these dimensions:\n\
- Dependencies: EOL/deprecated runtimes, frameworks, packages, or versions significantly behind stable\n\
- Security: hardcoded secrets, missing auth checks, injection vectors (SQL/XSS), insecure defaults, exposed debug endpoints\n\
- Code quality: functions >50 lines, god classes, SRP violations, dead code, error swallowing (empty catch/let _ =)\n\
- Scalability: N+1 queries, unbounded loops, missing pagination, memory leaks, missing indexes\n\
- Maintainability: tight coupling, circular dependencies, missing tests for critical paths, unclear naming\n\
- Compliance: GDPR issues (external resources, data retention), license incompatibilities\n\
- Infrastructure: Docker misconfigs (root user, no resource limits), CI gaps, missing health checks\n\n\
Fill ai/inconsistencies-tech-debt.md — replace ALL {{PLACEHOLDERS}} and <!-- ... --> comments:\n\
1. Outdated prerequisites table: flag EOL/deprecated/behind-stable runtimes, frameworks, packages\n\
2. For each issue found: (a) create `ai/tech-debt/TD-YYYYMMDD-slug.md` (YYYYMMDD=today) first, \
then (b) add the one-line entry to the Current list table. Do both or neither.\n\
   Severity: Critical=security/data loss, High=blocks prod, Medium=dev friction/perf, Low=cosmetic. Cite file paths.\n\
3. Limit to 15-20 most impactful findings. Prioritize Critical and High.\n\
4. No issues found → single row: 'None identified during initial audit'\n\n\
Detail file format:\n\
- **ID**: TD-YYYYMMDD-slug\n\
- **Area**: Backend | Frontend | CI | Infra | Security | Docs\n\
- **Severity**: Critical | High | Medium | Low\n\
- **Problem (fact)**: one-line description\n\
- **Impact**: what goes wrong if not fixed\n\
- **Where (pointers)**: file paths with line numbers\n\
- **Suggested direction**: non-binding fix suggestion\n\
- **Next step**: create ticket\n\n\
Also fill `ai/decisions.md` with intentional architectural choices observed in the code that might look unusual \
to a newcomer (e.g., why a certain pattern was chosen over a simpler one).",
        sources: &["__GIT_HEAD__"],
    },

    // Step 10: Final review
    AnalysisStep {
        target_file: "REVIEW",
        prompt: "\
Read ALL ai/ files. Final quality pass — fix issues directly.\n\n\
Check: no remaining `{{` placeholders · no orphan `<!-- fill -->` comments (keep `<!-- TODO: ask user -->`) \
· no duplicated facts · consistent terminology with glossary · valid cross-references \
· no contradictions · no empty critical sections · clean markdown · each tech-debt entry has a detail file \
· TODO markers are genuine unknowns.\n\n\
Empty sections for missing features → 'N/A — not used'.",
        sources: &[],
    },
];

/// POST /api/projects/:id/ai-audit
/// Runs a 10-step AI audit, streaming progress via SSE.
pub async fn run_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Sse<SseStream> {
    // Look up project
    let project = state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await.ok().flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Project not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    // Safety: early return above guarantees project is Some
    let project = project.expect("project is Some after early return");
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let briefing_notes = super::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);

    // Remove bootstrap prompt before running audit
    let index_file = project_path.join("ai/index.md");
    if index_file.exists() {
        remove_bootstrap_block(&index_file);
    }

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let agent_type = req.agent;
    let total_steps = ANALYSIS_STEPS.len();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        let start = serde_json::json!({ "total_steps": total_steps });
        yield Event::default().event("start").data(start.to_string());

        for (step_num, analysis_step) in ANALYSIS_STEPS.iter().enumerate() {
            let step = step_num + 1;
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            let step_start = serde_json::json!({
                "step": step,
                "total": total_steps,
                "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            // Inject today's date so agents don't have to guess it
            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, analysis_step.prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            // Inject user briefing notes if available
            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
            }

            // No profiles for audit — solo agent mode produces clean factual documentation.
            // Multi-profile debate format would pollute ai/ files with discussion artifacts.

            // Always use full_access for audit (agent needs to write files)
            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None,
            }).await {
                Ok(mut process) => {
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    tracing::debug!("Audit step {}: fix_ownership applied for {}", step, file_label);
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success,
                        "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                    // Continue to next step (same behavior as CLI)
                }
            }
        }

        // Generate checksums for drift detection
        {
            let pp = project_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mappings: Vec<crate::core::checksums::ChecksumMapping> = ANALYSIS_STEPS.iter()
                    .enumerate()
                    .filter(|(_, s)| !s.sources.is_empty())
                    .map(|(i, s)| {
                        let checksums = crate::core::checksums::compute_step_checksums(&pp, s.sources);
                        crate::core::checksums::ChecksumMapping {
                            ai_file: s.target_file.to_string(),
                            audit_step: i + 1,
                            sources: s.sources.iter().map(|p| p.to_string()).collect(),
                            checksums,
                        }
                    })
                    .collect();
                if let Err(e) = crate::core::checksums::write_checksums_file(&pp, &mappings) {
                    tracing::warn!("Failed to write checksums: {}", e);
                } else {
                    tracing::info!("Wrote ai/checksums.json with {} mappings", mappings.len());
                }
            }).await;
        }

        let done = serde_json::json!({ "status": "complete", "total_steps": total_steps });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// GET /api/projects/:id/audit-info
/// Returns the list of filled AI files and remaining TODOs for the validation prompt.
pub async fn audit_info(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AuditInfo>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool to avoid blocking the async runtime
    let result = tokio::task::spawn_blocking(move || {
        compute_audit_info_sync(&project_path_str)
    }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

    Json(ApiResponse::ok(result))
}

/// GET /api/projects/:id/drift
/// Check which ai/ sections are stale based on source file checksums.
/// Pure computation — no LLM tokens consumed.
pub async fn check_drift(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<DriftCheckResponse>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);

    let result = tokio::task::spawn_blocking(move || {
        crate::core::checksums::check_drift(&project_path)
    }).await;

    match result {
        Ok(drift) => {
            let response = DriftCheckResponse {
                audit_date: drift.audit_date,
                stale_sections: drift.stale_sections.into_iter().map(|s| DriftSection {
                    ai_file: s.ai_file,
                    audit_step: s.audit_step,
                    changed_sources: s.changed_sources,
                }).collect(),
                fresh_sections: drift.fresh_sections,
                total_sections: drift.total_sections,
            };
            Json(ApiResponse::ok(response))
        }
        Err(e) => Json(ApiResponse::err(format!("Drift check failed: {}", e))),
    }
}

/// POST /api/projects/:id/partial-audit
/// Re-run only specific audit steps and update checksums (merge, not overwrite).
pub async fn partial_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PartialAuditRequest>,
) -> Sse<SseStream> {
    // Look up project
    let project = state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await.ok().flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Project not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    // Safety: early return above guarantees project is Some
    let project = project.expect("project is Some after early return");
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let briefing_notes = super::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);

    // Validate requested step numbers
    let total_analysis_steps = ANALYSIS_STEPS.len();
    for &step in &req.steps {
        if step < 1 || step > total_analysis_steps {
            let msg = serde_json::json!({
                "error": format!("Invalid step {}: must be between 1 and {}", step, total_analysis_steps)
            });
            let stream: SseStream = Box::pin(futures::stream::once(async move {
                Ok::<_, Infallible>(Event::default().event("error").data(msg.to_string()))
            }));
            return Sse::new(stream);
        }
    }

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let agent_type = req.agent;
    let requested_steps = req.steps;
    let total_requested = requested_steps.len();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        let start = serde_json::json!({ "total_steps": total_requested });
        yield Event::default().event("start").data(start.to_string());

        for (progress_idx, &step) in requested_steps.iter().enumerate() {
            let analysis_step = &ANALYSIS_STEPS[step - 1];
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            let step_start = serde_json::json!({
                "step": step,
                "progress": progress_idx + 1,
                "total": total_requested,
                "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, analysis_step.prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
            }

            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None,
            }).await {
                Ok(mut process) => {
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
                    process.fix_ownership();
                    tracing::debug!("Partial audit step {}: fix_ownership applied for {}", step, file_label);
                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success,
                        "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Partial audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                }
            }
        }

        // Merge checksums: read existing, update only re-run steps, write back
        {
            let pp = project_path.clone();
            let steps_clone = requested_steps.clone();
            let _ = tokio::task::spawn_blocking(move || {
                // Build fresh checksums for re-run steps
                let fresh_mappings: Vec<crate::core::checksums::ChecksumMapping> = steps_clone.iter()
                    .filter_map(|&step_num| {
                        let s = &ANALYSIS_STEPS[step_num - 1];
                        if s.sources.is_empty() {
                            return None;
                        }
                        let checksums = crate::core::checksums::compute_step_checksums(&pp, s.sources);
                        Some(crate::core::checksums::ChecksumMapping {
                            ai_file: s.target_file.to_string(),
                            audit_step: step_num,
                            sources: s.sources.iter().map(|p| p.to_string()).collect(),
                            checksums,
                        })
                    })
                    .collect();

                // Read existing checksums and merge
                let mut merged: Vec<crate::core::checksums::ChecksumMapping> =
                    if let Some(existing) = crate::core::checksums::read_checksums_file(&pp) {
                        // Keep mappings for steps NOT re-run
                        let rerun_steps: std::collections::HashSet<usize> = steps_clone.iter().copied().collect();
                        existing.mappings.into_iter()
                            .filter(|m| !rerun_steps.contains(&m.audit_step))
                            .collect()
                    } else {
                        Vec::new()
                    };

                // Add fresh mappings
                merged.extend(fresh_mappings);
                // Sort by step number for consistency
                merged.sort_by_key(|m| m.audit_step);

                if let Err(e) = crate::core::checksums::write_checksums_file(&pp, &merged) {
                    tracing::warn!("Failed to write checksums after partial audit: {}", e);
                } else {
                    tracing::info!("Updated ai/checksums.json for {} re-run steps", steps_clone.len());
                }
            }).await;
        }

        let done = serde_json::json!({ "status": "complete", "total_steps": total_requested });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// POST /api/projects/:id/validate-audit
/// Marks the audit as validated by injecting a KRONN:VALIDATED marker into ai/index.md.
pub async fn validate_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    // Run filesystem I/O on blocking thread pool
    let validate_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = project_path.join("ai/index.md");

        if !index_file.exists() {
            return Err("ai/index.md not found — run the audit first".into());
        }

        let content = std::fs::read_to_string(&index_file)
            .map_err(|e| format!("Cannot read ai/index.md: {}", e))?;

        if content.contains("KRONN:VALIDATED") {
            return Ok(());
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let marker = format!("\n<!-- KRONN:VALIDATED:{} -->\n", today);
        let new_content = format!("{}{}", content.trim_end(), marker);

        std::fs::write(&index_file, new_content)
            .map_err(|e| format!("Failed to write marker: {}", e))?;

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = validate_result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// POST /api/projects/:id/mark-bootstrapped
/// Marks the project as bootstrapped by injecting a KRONN:BOOTSTRAPPED marker into ai/index.md.
pub async fn mark_bootstrapped(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path_str = project.path.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        let index_file = project_path.join("ai/index.md");

        if !index_file.exists() {
            return Err("ai/index.md not found".into());
        }

        let content = std::fs::read_to_string(&index_file)
            .map_err(|e| format!("Cannot read ai/index.md: {}", e))?;

        if content.contains("KRONN:BOOTSTRAPPED") {
            return Ok(());
        }

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let marker = format!("\n<!-- KRONN:BOOTSTRAPPED:{} -->\n", today);
        let new_content = format!("{}{}", content.trim_end(), marker);

        std::fs::write(&index_file, new_content)
            .map_err(|e| format!("Failed to write marker: {}", e))?;

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = result {
        return Json(ApiResponse::err(e));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Compute audit info (files + TODOs) from the filesystem.
fn compute_audit_info_sync(project_path_str: &str) -> AuditInfo {
    let project_path = scanner::resolve_host_path(project_path_str);
    let ai_dir = project_path.join("ai");

    if !ai_dir.is_dir() {
        return AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] };
    }

    let mut files = Vec::new();
    let mut todos = Vec::new();

    for entry in walkdir::WalkDir::new(&ai_dir).max_depth(4).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        let rel = entry.path().strip_prefix(&project_path).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().to_string();

        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            let is_empty = content.lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#') && !l.starts_with('>') && !l.starts_with("---") && !l.starts_with('|'))
                .count() < 3;

            files.push(AuditFileInfo {
                path: rel_str.clone(),
                filled: !is_empty && !content.contains("{{"),
            });

            for (line_num, line) in content.lines().enumerate() {
                if line.contains("<!-- TODO") {
                    todos.push(AuditTodo {
                        file: rel_str.clone(),
                        line: (line_num + 1) as u32,
                        text: line.trim().to_string(),
                    });
                }
            }
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    // Parse tech-debt items from the "Current list" table in inconsistencies-tech-debt.md
    let mut tech_debt_items = Vec::new();
    let tech_debt_file = ai_dir.join("inconsistencies-tech-debt.md");
    if let Ok(content) = std::fs::read_to_string(&tech_debt_file) {
        // Parse markdown table rows: | ID | Problem | Area | Severity |
        for line in content.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with('|') || trimmed.starts_with("| ID") || trimmed.starts_with("|--") || trimmed.contains("{{") {
                continue;
            }
            let cols: Vec<&str> = trimmed.split('|').map(|c| c.trim()).collect();
            // cols[0] is empty (before first |), cols[1]=ID, cols[2]=Problem, cols[3]=Area, cols[4]=Severity
            if cols.len() >= 5 && cols[1].starts_with("TD-") {
                tech_debt_items.push(TechDebtItem {
                    id: cols[1].to_string(),
                    problem: cols[2].to_string(),
                    area: cols[3].to_string(),
                    severity: cols[4].to_string(),
                });
            }
        }
    }

    AuditInfo { files, todos, tech_debt_items }
}

/// Build the validation discussion prompt with file/TODO/tech-debt enrichment.
/// The prompt follows a strict 4-phase protocol to ensure thorough validation.
pub(crate) fn build_validation_prompt(language: &str, info: &AuditInfo, has_issue_tracker_mcp: bool) -> String {
    let base = match language {
        "en" => {
            let mut s = String::from(concat!(
                "Validate the AI context (ai/ folder). Follow this 4-phase protocol. ",
                "Do NOT emit KRONN:VALIDATION_COMPLETE until ALL phases are done.\n\n",
                "**CRITICAL RULE: You are a DOCUMENTATION auditor, not a code fixer. ",
                "NEVER modify source code, Makefile, configs, or any file outside `ai/`. ",
                "Your ONLY job is to make `ai/` files accurate and complete.**\n\n",
                "## Phase 1 — Auto-fix (autonomous)\n",
                "Read source code to understand the project. Fix ONLY `ai/` files: orphan TODO markers, empty/skeleton files inferable from code, outdated info. ",
                "Update `ai/` files directly. Report fixes. Do NOT touch source code.\n\n",
                "## Phase 2 — Ambiguity questions (interactive)\n",
                "Ask remaining ambiguities **one by one**. After each answer, update the relevant `ai/` file (repo-map, coding-rules, architecture, etc.). ",
                "If the user reports a code issue, document it in `ai/inconsistencies-tech-debt.md` — do NOT fix the code yourself.\n",
                "If user answers 'I don't know' or 'skip', mark as `<!-- TODO: unknown -->` and move on.\n",
                "Phase 2 ends when all TODOs are addressed or explicitly skipped.\n\n",
                "## Phase 3 — Tech debt review (interactive)\n",
                "For each entry in `ai/inconsistencies-tech-debt.md`:\n",
                "1. Read its detail file in `ai/tech-debt/`\n",
                "2. Verify against source code — does the issue still exist? Is the description accurate?\n",
                "3. Present to user one by one (or grouped by area if >10). Ask: confirm/reject? correct severity? priority?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Also ask: create a ticket? (issue tracker available via MCP)\n");
            }
            s.push_str(concat!(
                "Do not batch-confirm. Update/remove `ai/` entries per feedback. Do NOT fix code — only update documentation.\n",
                "Also ask: did the audit miss anything obvious? (security, performance, compliance)\n\n",
                "## Phase 4 — Doc challenge (interactive)\n",
                "Ask 2-3 practical onboarding questions that must be answerable from `ai/` files alone. ",
                "Examples: 'How would a new dev add a new API endpoint?', 'What command runs all tests?', 'Where is the DB schema?'. ",
                "Check if `ai/` docs answer them correctly. Fix gaps in `ai/` files.\n\n",
                "## Completion\n",
                "All phases done → end with exact phrase: \"KRONN:VALIDATION_COMPLETE\". Never emit early.",
            ));
            s
        },
        "es" => {
            let mut s = String::from(concat!(
                "Valida el contexto AI (carpeta ai/). Sigue este protocolo de 4 fases. ",
                "NO emitas KRONN:VALIDATION_COMPLETE hasta completar TODAS las fases.\n\n",
                "**REGLA CRITICA: Eres un auditor de DOCUMENTACION, no un corrector de codigo. ",
                "NUNCA modifiques codigo fuente, Makefile, configs, ni ningun archivo fuera de `ai/`. ",
                "Tu UNICO trabajo: hacer los archivos `ai/` precisos y completos.**\n\n",
                "## Fase 1 — Auto-correccion (autonoma)\n",
                "Lee el codigo para entender el proyecto. Corrige SOLO archivos `ai/`: TODOs huerfanos, archivos esqueleto inferibles del codigo, info obsoleta. ",
                "Actualiza `ai/` directamente. Reporta. NO toques el codigo fuente.\n\n",
                "## Fase 2 — Preguntas (interactiva)\n",
                "Pregunta ambiguedades **una por una**. Tras cada respuesta, actualiza el archivo `ai/` correspondiente (repo-map, coding-rules, architecture, etc.). ",
                "Si el usuario reporta un problema de codigo, documentalo en `ai/inconsistencies-tech-debt.md` — NO corrijas el codigo tu mismo.\n",
                "Si el usuario responde 'no se' o 'saltar', marca como `<!-- TODO: unknown -->` y continua.\n\n",
                "## Fase 3 — Deuda tecnica (interactiva)\n",
                "Para cada entrada en `ai/inconsistencies-tech-debt.md`:\n",
                "1. Lee su archivo detalle en `ai/tech-debt/`\n",
                "2. Verifica contra el codigo fuente — ¿el problema existe? ¿la descripcion es correcta?\n",
                "3. Presenta al usuario una por una (o agrupadas por area si >10). Pregunta: ¿confirmar/rechazar? ¿severidad? ¿prioridad?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Tambien: ¿crear ticket? (gestor de issues disponible via MCP)\n");
            }
            s.push_str(concat!(
                "No confirmar en lote. Actualiza/elimina entradas `ai/` segun feedback. NO corrijas codigo — solo documenta.\n",
                "Tambien pregunta: ¿la auditoria omitio algo obvio? (seguridad, rendimiento, cumplimiento)\n\n",
                "## Fase 4 — Challenge doc (interactiva)\n",
                "Haz 2-3 preguntas practicas de onboarding que deben ser respondibles solo con los archivos `ai/`. ",
                "Ejemplos: '¿Como agregar un endpoint?', '¿Que comando ejecuta los tests?'. Corrige gaps en archivos `ai/`.\n\n",
                "## Fin\n",
                "Todas las fases completas → termina con: \"KRONN:VALIDATION_COMPLETE\". Nunca antes.",
            ));
            s
        },
        _ => {
            let mut s = String::from(concat!(
                "Valide le contexte AI (dossier ai/). Suis ce protocole en 4 phases. ",
                "NE PAS emettre KRONN:VALIDATION_COMPLETE avant la fin des 4 phases.\n\n",
                "**REGLE CRITIQUE : Tu es un auditeur de DOCUMENTATION, pas un correcteur de code. ",
                "NE MODIFIE JAMAIS le code source, Makefile, configs, ou tout fichier hors de `ai/`. ",
                "Ton SEUL travail : rendre les fichiers `ai/` precis et complets.**\n\n",
                "## Phase 1 — Auto-correction (autonome)\n",
                "Lis le code source pour comprendre le projet. Corrige UNIQUEMENT les fichiers `ai/` : TODOs orphelins, fichiers squelettes inferables du code, infos obsoletes. ",
                "Mets a jour `ai/` directement. Rapporte les corrections. NE touche PAS au code source.\n\n",
                "## Phase 2 — Questions (interactif)\n",
                "Pose les ambiguites **une par une**. Apres chaque reponse, mets a jour le fichier `ai/` concerne (repo-map, coding-rules, architecture, etc.). ",
                "Si l'utilisateur signale un probleme de code, documente-le dans `ai/inconsistencies-tech-debt.md` — NE corrige PAS le code toi-meme.\n",
                "Si l'utilisateur repond 'je ne sais pas' ou 'passer', marque `<!-- TODO: unknown -->` et continue.\n\n",
                "## Phase 3 — Dette technique (interactif)\n",
                "Pour chaque entree dans `ai/inconsistencies-tech-debt.md` :\n",
                "1. Lis son fichier detail dans `ai/tech-debt/`\n",
                "2. Verifie dans le code source — le probleme existe-t-il ? La description est-elle exacte ?\n",
                "3. Presente a l'utilisateur un par un (ou par domaine si >10). Demande : confirmer/rejeter ? severite ? priorite ?\n",
            ));
            if has_issue_tracker_mcp {
                s.push_str("Aussi : creer un ticket ? (gestionnaire d'issues dispo via MCP)\n");
            }
            s.push_str(concat!(
                "Pas de confirmation en lot. Mets a jour/supprime les entrees `ai/` selon feedback. NE corrige PAS le code — documente seulement.\n",
                "Demande aussi : l'audit a-t-il rate quelque chose d'evident ? (securite, performance, conformite)\n\n",
                "## Phase 4 — Challenge doc (interactif)\n",
                "Pose 2-3 questions pratiques d'onboarding qui doivent etre couvertes par les fichiers `ai/` seuls. ",
                "Exemples : 'Comment ajouter un endpoint ?', 'Quelle commande lance les tests ?'. Corrige les lacunes dans les fichiers `ai/`.\n\n",
                "## Fin\n",
                "Toutes les phases terminees → termine par : \"KRONN:VALIDATION_COMPLETE\". Jamais avant.",
            ));
            s
        },
    };

    let mut prompt = base;

    // Summary counts only — the agent has filesystem access to read the actual files
    let unfilled_count = info.files.iter().filter(|f| !f.filled).count();
    let total_files = info.files.len();
    if total_files > 0 {
        let summary = match language {
            "en" => format!("{} AI files detected ({} still incomplete). Read `ai/index.md` for the full tree.", total_files, unfilled_count),
            "es" => format!("{} archivos AI detectados ({} aun incompletos). Lee `ai/index.md` para el arbol completo.", total_files, unfilled_count),
            _ => format!("{} fichiers AI detectes ({} encore incomplets). Lis `ai/index.md` pour l'arbre complet.", total_files, unfilled_count),
        };
        prompt.push_str(&format!("\n\n{}", summary));
    }

    if !info.todos.is_empty() {
        let hint = match language {
            "en" => format!("{} remaining TODO markers across AI files. Scan `ai/` for `<!-- TODO` to find them all.", info.todos.len()),
            "es" => format!("{} marcadores TODO restantes en archivos AI. Busca `<!-- TODO` en `ai/` para encontrarlos.", info.todos.len()),
            _ => format!("{} marqueurs TODO restants dans les fichiers AI. Cherche `<!-- TODO` dans `ai/` pour les trouver.", info.todos.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    if !info.tech_debt_items.is_empty() {
        let hint = match language {
            "en" => format!("{} tech debt items to review in Phase 3. Read `ai/inconsistencies-tech-debt.md` and `ai/tech-debt/` for details.", info.tech_debt_items.len()),
            "es" => format!("{} items de deuda tecnica a revisar en Fase 3. Lee `ai/inconsistencies-tech-debt.md` y `ai/tech-debt/` para detalles.", info.tech_debt_items.len()),
            _ => format!("{} items de dette technique a revoir en Phase 3. Lis `ai/inconsistencies-tech-debt.md` et `ai/tech-debt/` pour les details.", info.tech_debt_items.len()),
        };
        prompt.push_str(&format!("\n\n{}", hint));
    }

    prompt
}

// ─── Full audit (unified flow) ──────────────────────────────────────────────

/// POST /api/projects/:id/full-audit
/// Unified endpoint: install template + run 10-step audit + create validation discussion.
pub async fn full_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Sse<SseStream> {
    // Look up project
    let project = state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await.ok().flatten();

    if project.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Project not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    // Safety: early return above guarantees project is Some
    let project = project.expect("project is Some after early return");
    let project_id = project.id.clone();
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);
    let project_default_skill_ids = project.default_skill_ids.clone();
    let briefing_notes = super::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);
    let agent_type = req.agent;

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    let total_steps = ANALYSIS_STEPS.len();
    let db = state.db.clone();
    let audit_tracker = state.audit_tracker.clone();

    // Clear any stale cancellation flag for this project
    if let Ok(mut tracker) = audit_tracker.lock() {
        tracker.cancelled.remove(&project_id);
    }

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // ── Phase 1: Install template if needed ──
        let status = scanner::detect_audit_status(&project_path_str);
        let template_installed = matches!(status, AiAuditStatus::NoTemplate);

        if template_installed {
            let pp = project_path_str.clone();
            let install_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
                let project_path = scanner::resolve_host_path(&pp);
                if !project_path.exists() {
                    return Err(format!("Project path not found: {}", project_path.display()));
                }

                let ai_target = project_path.join("ai");
                if ai_target.exists() {
                    if let Err(e) = check_ai_dir_permissions(&ai_target) {
                        return Err(format!("ai/ permission error: {}", e));
                    }
                }

                let template_dir = super::projects::resolve_templates_dir();
                if !template_dir.exists() {
                    return Err(format!("Templates directory not found: {}", template_dir.display()));
                }

                let ai_template = template_dir.join("ai");
                if ai_template.is_dir() {
                    super::projects::copy_dir_nondestructive(&ai_template, &ai_target)?;
                }

                for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    if src.exists() && !dst.exists() {
                        if let Err(e) = std::fs::copy(&src, &dst) {
                            tracing::warn!("Failed to copy {}: {}", filename, e);
                        }
                    }
                }

                let index_file = project_path.join("ai/index.md");
                if index_file.exists() {
                    super::projects::inject_bootstrap_prompt(&index_file);
                }

                runner::fix_file_ownership(&project_path);
                Ok(())
            }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

            if let Err(e) = install_result {
                let err = serde_json::json!({ "error": e });
                yield Event::default().event("error").data(err.to_string());
                return;
            }

            crate::core::mcp_scanner::ensure_gitignore_public(&project_path_str, "ai/var/");
        }

        let tmpl_event = serde_json::json!({ "installed": template_installed });
        yield Event::default().event("template_installed").data(tmpl_event.to_string());

        // ── Phase 2: Run 10-step audit ──
        // Remove bootstrap prompt
        let index_file = project_path.join("ai/index.md");
        if index_file.exists() {
            remove_bootstrap_block(&index_file);
        }

        let start = serde_json::json!({ "total_steps": total_steps });
        yield Event::default().event("start").data(start.to_string());

        for (step_num, analysis_step) in ANALYSIS_STEPS.iter().enumerate() {
            // Check for cancellation before each step
            if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id)).unwrap_or(false) {
                let cancelled = serde_json::json!({ "status": "cancelled" });
                yield Event::default().event("cancelled").data(cancelled.to_string());
                return;
            }

            let step = step_num + 1;
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            let step_start = serde_json::json!({
                "step": step, "total": total_steps, "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, analysis_step.prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
            }

            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &agent_type, project_path: &project_path_str, work_dir: None,
                prompt: &full_prompt, tokens: &tokens, full_access: true,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: None,
                tier: crate::models::ModelTier::Reasoning, model_tiers: None,
            }).await {
                Ok(mut process) => {
                    // Register the child PID for cancellation
                    if let Some(pid) = process.child.id() {
                        if let Ok(mut tracker) = audit_tracker.lock() {
                            tracker.running_pids.insert(project_id.clone(), pid);
                        }
                    }

                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }
                    let status = process.child.wait().await;
                    process.fix_ownership();

                    // Unregister PID
                    if let Ok(mut tracker) = audit_tracker.lock() {
                        tracker.running_pids.remove(&project_id);
                    }

                    // Check if cancelled during this step
                    if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id)).unwrap_or(false) {
                        let cancelled = serde_json::json!({ "status": "cancelled" });
                        yield Event::default().event("cancelled").data(cancelled.to_string());
                        return;
                    }

                    let success = status.map(|s| s.success()).unwrap_or(false);

                    let step_done = serde_json::json!({
                        "step": step, "success": success, "file": file_label
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());
                }
                Err(e) => {
                    tracing::error!("Audit step {} failed to start: {}", step, e);
                    let err = serde_json::json!({
                        "error": format!("Step {} ({}): {}", step, file_label, e),
                        "step": step
                    });
                    yield Event::default().event("step_error").data(err.to_string());
                }
            }
        }

        // ── Auto-detect project skills ──
        let detected_skill_ids = {
            let p = project_path.clone();
            tokio::task::spawn_blocking(move || detect_project_skills(&p))
                .await.unwrap_or_default()
        };
        let skill_ids_for_disc = if detected_skill_ids.is_empty() {
            project_default_skill_ids.clone()
        } else {
            // Save detected skills to DB
            let pid = project_id.clone();
            let sids = detected_skill_ids.clone();
            let _ = db.with_conn(move |conn| {
                crate::db::projects::update_project_default_skills(conn, &pid, &sids)
            }).await;
            detected_skill_ids
        };

        // ── Phase 3: Create validation discussion ──
        let pp = project_path_str.clone();
        let audit_info = tokio::task::spawn_blocking(move || {
            compute_audit_info_sync(&pp)
        }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

        // Detect if project has an issue tracker MCP (GitHub, GitLab, Jira, Linear, etc.)
        let has_issue_tracker_mcp = detect_issue_tracker_mcp(&project_path);

        let validation_prompt = build_validation_prompt(&language, &audit_info, has_issue_tracker_mcp);

        let now = Utc::now();
        let discussion_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: validation_prompt,
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, author_pseudo: None, author_avatar_email: None,
        };

        let discussion = Discussion {
            id: discussion_id.clone(),
            project_id: Some(project_id.clone()),
            title: "Validation audit AI".to_string(),
            agent: agent_type.clone(),
            language: language.clone(),
            participants: vec![agent_type.clone()],
            messages: vec![initial_message.clone()],
            message_count: 1,
            skill_ids: skill_ids_for_disc.clone(),
            profile_ids: vec![
                "architect".into(),
                "tech-lead".into(),
                "qa-engineer".into(),
                "devils-advocate".into(),
            ],
            directive_ids: vec![],
            tier: crate::models::ModelTier::Default,
            pin_first_message: true,
            archived: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            shared_id: None,
            shared_with: vec![],
            created_at: now,
            updated_at: now,
        };

        let disc = discussion.clone();
        let msg = initial_message;
        let disc_created = db.with_conn(move |conn| {
            crate::db::discussions::insert_discussion(conn, &disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
            Ok(())
        }).await;

        let disc_id = match disc_created {
            Ok(()) => {
                let ev = serde_json::json!({ "discussion_id": discussion_id });
                yield Event::default().event("validation_created").data(ev.to_string());
                Some(discussion_id)
            }
            Err(e) => {
                tracing::error!("Failed to create validation discussion: {}", e);
                let err = serde_json::json!({ "error": format!("Failed to create validation discussion: {}", e) });
                yield Event::default().event("step_error").data(err.to_string());
                None
            }
        };

        // Generate checksums for drift detection
        {
            let pp = project_path.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mappings: Vec<crate::core::checksums::ChecksumMapping> = ANALYSIS_STEPS.iter()
                    .enumerate()
                    .filter(|(_, s)| !s.sources.is_empty())
                    .map(|(i, s)| {
                        let checksums = crate::core::checksums::compute_step_checksums(&pp, s.sources);
                        crate::core::checksums::ChecksumMapping {
                            ai_file: s.target_file.to_string(),
                            audit_step: i + 1,
                            sources: s.sources.iter().map(|p| p.to_string()).collect(),
                            checksums,
                        }
                    })
                    .collect();
                if let Err(e) = crate::core::checksums::write_checksums_file(&pp, &mappings) {
                    tracing::warn!("Failed to write checksums: {}", e);
                } else {
                    tracing::info!("Wrote ai/checksums.json with {} mappings", mappings.len());
                }
            }).await;
        }

        let done = serde_json::json!({
            "status": "complete",
            "total_steps": total_steps,
            "discussion_id": disc_id,
            "template_was_installed": template_installed
        });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// Auto-detect skills from project filesystem (config files, package managers, etc.)
pub(crate) fn detect_project_skills(project_path: &std::path::Path) -> Vec<String> {
    let mut skills: Vec<String> = Vec::new();

    // ── Language detection (from package managers / config files) ──
    if project_path.join("Cargo.toml").exists() {
        skills.push("rust".into());
    }
    if project_path.join("package.json").exists() {
        // Check if TypeScript
        if project_path.join("tsconfig.json").exists()
            || project_path.join("tsconfig.app.json").exists()
        {
            skills.push("typescript".into());
        }
    }
    if project_path.join("requirements.txt").exists()
        || project_path.join("pyproject.toml").exists()
        || project_path.join("setup.py").exists()
    {
        skills.push("python".into());
    }
    if project_path.join("go.mod").exists() {
        skills.push("go".into());
    }
    if project_path.join("composer.json").exists() {
        skills.push("php".into());
    }

    // ── Domain detection ──
    // DevOps: Dockerfile, CI/CD, IaC
    if project_path.join("Dockerfile").exists()
        || project_path.join("docker-compose.yml").exists()
        || project_path.join("docker-compose.yaml").exists()
        || project_path.join(".github").join("workflows").exists()
        || project_path.join(".gitlab-ci.yml").exists()
        || project_path.join("Makefile").exists()
    {
        skills.push("devops".into());
    }

    // Database: migrations, schema files
    if project_path.join("migrations").exists()
        || project_path.join("db").exists()
        || project_path.join("prisma").exists()
        || project_path.join("drizzle").exists()
    {
        skills.push("database".into());
    }

    // Security: auth configs, security headers
    if project_path.join(".env.example").exists()
        || project_path.join("security.yaml").exists()
        || project_path.join("config").join("packages").join("security.yaml").exists()
    {
        skills.push("security".into());
    }

    // ── Business detection ──
    // Web performance: frontend projects with build tools
    if project_path.join("webpack.config.js").exists()
        || project_path.join("vite.config.ts").exists()
        || project_path.join("vite.config.js").exists()
        || project_path.join("next.config.js").exists()
        || project_path.join("next.config.ts").exists()
    {
        skills.push("web-performance".into());
    }

    // SEO: robots.txt, sitemap
    if project_path.join("robots.txt").exists()
        || project_path.join("public").join("robots.txt").exists()
    {
        skills.push("seo".into());
    }

    // Filter to only keep skills that actually exist in the system
    let valid: Vec<String> = skills.into_iter()
        .filter(|id| crate::core::skills::get_skill(id).is_some())
        .collect();

    tracing::info!("Auto-detected skills for {}: {:?}", project_path.display(), valid);
    valid
}

fn detect_issue_tracker_mcp(project_path: &std::path::Path) -> bool {
    let mcp_file = project_path.join(".mcp.json");
    if let Ok(content) = std::fs::read_to_string(&mcp_file) {
        let lower = content.to_lowercase();
        return lower.contains("github") || lower.contains("gitlab")
            || lower.contains("jira") || lower.contains("atlassian")
            || lower.contains("linear") || lower.contains("youtrack");
    }
    false
}

/// Files installed by the audit template (to be removed on cancel).
pub(crate) const AUDIT_REDIRECTOR_FILES: &[&str] = &[
    "CLAUDE.md", "GEMINI.md", "AGENTS.md",
    ".cursorrules", ".windsurfrules", ".clinerules",
    ".github/copilot-instructions.md",
];

/// POST /api/projects/:id/cancel-audit
/// Cancel a running audit and remove ALL files created by the audit.
pub async fn cancel_audit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    // Look up project
    let project = match state.db.with_conn({
        let id = id.clone();
        move |conn| crate::db::projects::get_project(conn, &id)
    }).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_id = project.id.clone();

    // 1. Signal cancellation and kill any running agent process
    {
        let Ok(mut tracker) = state.audit_tracker.lock() else {
            return Json(ApiResponse::err("Internal error: audit tracker lock poisoned"));
        };
        tracker.cancelled.insert(project_id.clone());
        if let Some(pid) = tracker.running_pids.remove(&project_id) {
            tracing::info!("Killing audit agent process (PID {}) for project {}", pid, project_id);
            // Kill the process tree: first try killing the process group, then the process itself
            let _ = sync_cmd("kill")
                .args(["-9", &format!("-{}", pid)]) // negative PID = process group
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            let _ = sync_cmd("kill")
                .args(["-9", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    // Small delay to let the SSE stream detect the cancellation
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // 2. Delete all audit-created files
    let project_path_str = project.path.clone();
    let cleanup_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        if !project_path.exists() {
            return Err(format!("Project path not found: {}", project_path.display()));
        }

        // Remove ai/ directory entirely
        let ai_dir = project_path.join("ai");
        if ai_dir.exists() {
            std::fs::remove_dir_all(&ai_dir)
                .map_err(|e| format!("Failed to remove ai/: {}", e))?;
            tracing::info!("Removed ai/ directory from {}", project_path.display());
        }

        // Remove redirector files (CLAUDE.md, .cursorrules, etc.)
        for filename in AUDIT_REDIRECTOR_FILES {
            let file = project_path.join(filename);
            if file.exists() {
                if let Err(e) = std::fs::remove_file(&file) {
                    tracing::warn!("Failed to remove {}: {}", filename, e);
                } else {
                    tracing::info!("Removed {} from {}", filename, project_path.display());
                }
            }
        }

        Ok(())
    }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

    if let Err(e) = cleanup_result {
        // Clear cancellation flag before returning error
        if let Ok(mut tracker) = state.audit_tracker.lock() {
            tracker.cancelled.remove(&project_id);
        }
        return Json(ApiResponse::err(e));
    }

    // 3. Delete any validation discussion for this project
    let pid = project_id.clone();
    if let Err(e) = state.db.with_conn(move |conn| {
        // Find and delete validation discussions for this project
        let discussions = crate::db::discussions::list_discussions(conn)?;
        for disc in discussions {
            if disc.project_id.as_deref() == Some(&pid) && disc.title == "Validation audit AI" {
                crate::db::discussions::delete_discussion(conn, &disc.id)?;
                tracing::info!("Deleted validation discussion {} for project {}", disc.id, pid);
            }
        }
        Ok(())
    }).await {
        tracing::error!("Failed to delete validation discussions for project: {e}");
    }

    // 4. Clear cancellation flag
    if let Ok(mut tracker) = state.audit_tracker.lock() {
        tracker.cancelled.remove(&project_id);
    }

    // Return updated status (should be NoTemplate now)
    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// GET /api/projects/:id/briefing
/// Returns the briefing notes for a project.
pub async fn get_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Option<String>>> {
    match state.db.with_conn(move |conn| {
        crate::db::projects::get_project_briefing_notes(conn, &id)
    }).await {
        Ok(notes) => Json(ApiResponse::ok(notes)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/projects/:id/briefing
/// Sets or clears the briefing notes for a project.
pub async fn set_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetBriefingRequest>,
) -> Json<ApiResponse<bool>> {
    let notes = req.notes;
    match state.db.with_conn(move |conn| {
        crate::db::projects::update_project_briefing_notes(conn, &id, notes.as_deref())
    }).await {
        Ok(true) => Json(ApiResponse::ok(true)),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/projects/:id/start-briefing
/// Creates a conversational briefing discussion for a project.
pub async fn start_briefing(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<LaunchAuditRequest>,
) -> Json<ApiResponse<StartBriefingResponse>> {
    // 1. Look up the project
    let pid = id.clone();
    let project = state.db.with_conn(move |conn| {
        crate::db::projects::get_project(conn, &pid)
    }).await.ok().flatten();

    let Some(project) = project else {
        return Json(ApiResponse::err("Project not found"));
    };

    // 2. Get language
    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    // 3. Build briefing prompt
    let briefing_prompt = build_briefing_prompt(&language);

    // 4. Create discussion
    let now = Utc::now();
    let discussion_id = Uuid::new_v4().to_string();
    let agent_type = req.agent;

    let initial_message = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: briefing_prompt,
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None, author_pseudo: None, author_avatar_email: None,
    };

    let title = match language.as_str() {
        "en" => "Project Briefing".to_string(),
        "es" => "Briefing del proyecto".to_string(),
        _ => "Briefing projet".to_string(),
    };

    let discussion = Discussion {
        id: discussion_id.clone(),
        project_id: Some(project.id.clone()),
        title,
        agent: agent_type.clone(),
        language: language.clone(),
        participants: vec![agent_type],
        messages: vec![initial_message.clone()],
        message_count: 1,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        tier: crate::models::ModelTier::Default,
        pin_first_message: true,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        shared_id: None,
        shared_with: vec![],
        created_at: now,
        updated_at: now,
    };

    let disc = discussion.clone();
    let msg = initial_message;
    if let Err(e) = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_discussion(conn, &disc)?;
        crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
        Ok(())
    }).await {
        return Json(ApiResponse::err(format!("Failed to create discussion: {}", e)));
    }

    Json(ApiResponse::ok(StartBriefingResponse { discussion_id }))
}

/// Try to detect permission issues on an existing ai/ directory.
/// Returns Ok(()) if all files are accessible, or Err with description if unfixable.
pub(crate) fn check_ai_dir_permissions(ai_dir: &std::path::Path) -> Result<(), String> {
    for entry in walkdir::WalkDir::new(ai_dir).max_depth(5).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => return Err(format!("Cannot traverse ai/ directory: {}", e)),
        };
        let path = entry.path();
        if path.is_file() {
            if let Err(e) = std::fs::read(path) {
                return Err(format!("{}: {}", path.display(), e));
            }
        }
    }
    Ok(())
}

/// Remove the KRONN:BOOTSTRAP block from ai/index.md
fn remove_bootstrap_block(index_file: &std::path::Path) {
    let content = match std::fs::read_to_string(index_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    if !content.contains("KRONN:BOOTSTRAP:START") {
        return;
    }

    // Remove everything between START and END markers (inclusive)
    let mut result = String::new();
    let mut in_block = false;
    for line in content.lines() {
        if line.contains("KRONN:BOOTSTRAP:START") {
            in_block = true;
            continue;
        }
        if line.contains("KRONN:BOOTSTRAP:END") {
            in_block = false;
            continue;
        }
        if !in_block {
            result.push_str(line);
            result.push('\n');
        }
    }

    // Trim leading whitespace from the cleaned content
    let trimmed = result.trim_start().to_string();
    if let Err(e) = std::fs::write(index_file, trimmed) {
        tracing::warn!("Failed to remove bootstrap block: {}", e);
    }
}

/// Build the briefing discussion prompt (conversational pre-audit)
pub(crate) fn build_briefing_prompt(language: &str) -> String {
    match language {
        "en" => concat!(
            "ROLE: You are a project briefing assistant.\n\n",
            "ABSOLUTE RULE: Do NOT read source code, project files, or any file outside ai/. ",
            "Do NOT guess ANYTHING. You ask questions and use ONLY the user's answers.\n\n",
            "IF YOU HAVE FILE SYSTEM ACCESS: do NOT use it for this task. ",
            "No ls, cat, read, glob, grep. The only allowed file operation is the final write of ai/briefing.md.\n\n",
            "NOTE: The tech stack will be auto-detected during the audit (from package.json, Cargo.toml, etc.). No need to ask about it.\n\n",
            "STEP 1 — Ask the following 6 questions IN A SINGLE MESSAGE, then STOP. Wait for answers.\n\n",
            "1. What does this project do? (one sentence — what it does for its users)\n",
            "2. Who works on it? (solo / small team / large team)\n",
            "3. What stage is it at? (prototype, MVP, production, legacy, rewrite...)\n",
            "4. Key external dependencies? Include names/URLs if relevant. (e.g. \"PostgreSQL on AWS RDS\", \"user-service API on gitlab.company.com/org/repo\" — or just \"none\")\n",
            "5. What would a new contributor get wrong on day one? (traps, implicit rules, fragile areas)\n",
            "6. Anything else the audit should know? (optional, keep it short)\n\n",
            "STEP 2 — Check that the user answered questions 1-5. If some are missing, ask ONLY the unanswered ones before proceeding. Q6 is optional. ",
            "Once you have answers 1-5 (or the user explicitly says 'skip' for some), write the file ai/briefing.md with THIS EXACT FORMAT:\n\n",
            "# Project Briefing\n",
            "> Auto-generated by AI briefing. Source: user answers (not code analysis).\n",
            "## Purpose\n[answer Q1]\n",
            "## Team\n[answer Q2]\n",
            "## Maturity\n[answer Q3]\n",
            "## External Dependencies\n[answer Q4 — if none, write \"None.\"]\n",
            "## Traps & Fragile Areas\n[answer Q5 — bullet list if multiple]\n",
            "## Additional Context\n[answer Q6 — if skipped, write \"None.\"]\n\n",
            "Write ai/briefing.md IN ENGLISH even if the conversation is in another language.\n",
            "If the user does not answer a question, write \"Not provided\" — do NOT invent ANYTHING.\n",
            "Do NOT modify ANY other file.\n\n",
            "STEP 3 — After writing the file, end your last message with: KRONN:BRIEFING_COMPLETE",
        ).to_string(),
        "es" => concat!(
            "ROLE: Eres un asistente de briefing de proyecto.\n\n",
            "REGLA ABSOLUTA: NO leas el codigo fuente, los archivos del proyecto, ni ningun archivo fuera de ai/. ",
            "NO adivines NADA. Haces preguntas y usas UNICAMENTE las respuestas del usuario.\n\n",
            "SI TIENES ACCESO AL SISTEMA DE ARCHIVOS: NO lo uses para esta tarea. ",
            "Nada de ls, cat, read, glob, grep. La unica operacion de archivo permitida es la escritura final de ai/briefing.md.\n\n",
            "NOTA: La stack tecnica sera auto-detectada durante la auditoria (desde package.json, Cargo.toml, etc.). No es necesario preguntar por ella.\n\n",
            "PASO 1 — Haz las 6 preguntas siguientes EN UN SOLO MENSAJE, luego PARA. Espera las respuestas.\n\n",
            "1. Que hace este proyecto? (una frase — que hace para sus usuarios)\n",
            "2. Quien trabaja en el? (solo / equipo pequeno / equipo grande)\n",
            "3. En que etapa esta? (prototipo, MVP, produccion, legacy, reescritura...)\n",
            "4. Dependencias externas clave? Incluye nombres/URLs si es relevante. (ej: \"PostgreSQL en AWS RDS\", \"API user-service en gitlab.company.com/org/repo\" — o simplemente \"ninguna\")\n",
            "5. Que haria mal un nuevo contributor el primer dia? (trampas, reglas implicitas, zonas fragiles)\n",
            "6. Algo mas que la auditoria deberia saber? (opcional, breve)\n\n",
            "PASO 2 — Verifica que el usuario respondio las preguntas 1-5. Si faltan algunas, pregunta SOLO las que faltan. La Q6 es opcional. ",
            "Cuando tengas las respuestas 1-5 (o el usuario diga 'saltar'), escribe el archivo ai/briefing.md con ESTE FORMATO EXACTO:\n\n",
            "# Project Briefing\n",
            "> Auto-generated by AI briefing. Source: user answers (not code analysis).\n",
            "## Purpose\n[respuesta Q1]\n",
            "## Team\n[respuesta Q2]\n",
            "## Maturity\n[respuesta Q3]\n",
            "## External Dependencies\n[respuesta Q4 — si ninguna, escribir \"None.\"]\n",
            "## Traps & Fragile Areas\n[respuesta Q5 — lista de puntos si hay varios]\n",
            "## Additional Context\n[respuesta Q6 — si omitida, escribir \"None.\"]\n\n",
            "Escribe ai/briefing.md EN INGLES aunque la conversacion sea en otro idioma.\n",
            "Si el usuario no responde a una pregunta, escribe \"Not provided\" — NO inventes NADA.\n",
            "NO modifiques NINGUN otro archivo.\n\n",
            "PASO 3 — Despues de escribir el archivo, termina tu ultimo mensaje con: KRONN:BRIEFING_COMPLETE",
        ).to_string(),
        _ => concat!(
            "ROLE: Tu es un assistant de briefing projet.\n\n",
            "REGLE ABSOLUE: Tu ne lis PAS le code source, les fichiers du projet, ni aucun fichier en dehors de ai/. ",
            "Tu ne devines RIEN. Tu poses des questions et tu utilises UNIQUEMENT les reponses de l'utilisateur.\n\n",
            "SI TU AS ACCES AU SYSTEME DE FICHIERS: ne l'utilise PAS pour cette tache. ",
            "Pas de ls, cat, read, glob, grep. La seule operation fichier autorisee est l'ecriture finale de ai/briefing.md.\n\n",
            "NOTE: La stack technique sera auto-detectee pendant l'audit (depuis package.json, Cargo.toml, etc.). Inutile d'en parler ici.\n\n",
            "ETAPE 1 — Pose les 6 questions suivantes EN UN SEUL MESSAGE, puis STOP. Attends les reponses.\n\n",
            "1. Que fait ce projet ? (une phrase — ce qu'il fait pour ses utilisateurs)\n",
            "2. Qui travaille dessus ? (solo / petite equipe / grosse equipe)\n",
            "3. A quel stade en est-il ? (prototype, MVP, production, legacy, rewrite...)\n",
            "4. Dependances externes cles ? Inclus les noms/URLs si pertinent. (ex: \"PostgreSQL sur AWS RDS\", \"API user-service sur gitlab.company.com/org/repo\" — ou juste \"aucune\")\n",
            "5. Qu'est-ce qu'un nouveau contributeur ferait mal le premier jour ? (pieges, regles implicites, zones fragiles)\n",
            "6. Autre chose que l'audit devrait savoir ? (optionnel, en bref)\n\n",
            "ETAPE 2 — Verifie que l'utilisateur a repondu aux questions 1-5. S'il en manque, redemande UNIQUEMENT celles qui manquent. La Q6 est optionnelle. ",
            "Une fois les reponses 1-5 obtenues (ou si l'utilisateur dit 'passer'), ecris le fichier ai/briefing.md avec CE FORMAT EXACT :\n\n",
            "# Project Briefing\n",
            "> Auto-generated by AI briefing. Source: user answers (not code analysis).\n",
            "## Purpose\n[reponse Q1]\n",
            "## Team\n[reponse Q2]\n",
            "## Maturity\n[reponse Q3]\n",
            "## External Dependencies\n[reponse Q4 — si aucune, ecrire \"None.\"]\n",
            "## Traps & Fragile Areas\n[reponse Q5 — liste a puces si plusieurs]\n",
            "## Additional Context\n[reponse Q6 — si omise, ecrire \"None.\"]\n\n",
            "Ecris ai/briefing.md EN ANGLAIS meme si la conversation est en francais.\n",
            "Si l'utilisateur ne repond pas a une question, ecris \"Not provided\" — n'invente RIEN.\n",
            "Ne modifie AUCUN autre fichier.\n\n",
            "ETAPE 3 — Apres avoir ecrit le fichier, termine ton dernier message par : KRONN:BRIEFING_COMPLETE",
        ).to_string(),
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

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
