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
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Populate audit_status and ai_todo_count on a project (computed from filesystem)
fn enrich_audit_status(project: &mut Project) {
    project.audit_status = scanner::detect_audit_status(&project.path);
    project.ai_todo_count = scanner::count_ai_todos(&project.path);
}

/// GET /api/projects
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<Project>>> {
    match state.db.with_conn(|conn| crate::db::projects::list_projects(conn)).await {
        Ok(mut projects) => {
            for p in &mut projects {
                enrich_audit_status(p);
            }
            Json(ApiResponse::ok(projects))
        }
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/projects/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Project>> {
    match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(mut p)) => {
            enrich_audit_status(&mut p);
            Json(ApiResponse::ok(p))
        }
        Ok(None) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/projects/scan
pub async fn scan(State(state): State<AppState>) -> Json<ApiResponse<Vec<DetectedRepo>>> {
    let config = state.config.read().await;

    let scan_paths = if config.scan.paths.is_empty() {
        std::env::var("KRONN_HOST_HOME")
            .into_iter()
            .collect::<Vec<_>>()
    } else {
        config.scan.paths.clone()
    };
    let ignore = config.scan.ignore.clone();
    drop(config);

    let existing_paths: Vec<String> = state.db.with_conn(|conn| {
        let projects = crate::db::projects::list_projects(conn)?;
        Ok(projects.into_iter().map(|p| p.path).collect())
    }).await.unwrap_or_default();

    match scanner::scan_paths(&scan_paths, &ignore).await {
        Ok(mut repos) => {
            for repo in &mut repos {
                repo.has_project = existing_paths.iter().any(|p| *p == repo.path);
            }
            Json(ApiResponse::ok(repos))
        }
        Err(e) => Json(ApiResponse::err(format!("Scan failed: {}", e))),
    }
}

/// POST /api/projects
pub async fn create(
    State(state): State<AppState>,
    Json(repo): Json<DetectedRepo>,
) -> Json<ApiResponse<Project>> {
    let now = Utc::now();

    let mut project = Project {
        id: Uuid::new_v4().to_string(),
        name: repo.name.clone(),
        path: repo.path.clone(),
        repo_url: repo.remote_url.clone(),
        token_override: None,
        ai_config: AiConfigStatus {
            detected: !repo.ai_configs.is_empty(),
            configs: repo.ai_configs.clone(),
        },
        audit_status: AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        mcps: vec![], // MCPs now managed via mcp_configs system
        tasks: vec![],
        created_at: now,
        updated_at: now,
    };
    enrich_audit_status(&mut project);

    let p = project.clone();
    match state.db.with_conn(move |conn| {
        crate::db::projects::insert_project(conn, &p)?;
        Ok(())
    }).await {
        Ok(()) => Json(ApiResponse::ok(project)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/projects/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::projects::delete_project(conn, &id)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Project not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

// ─── Template installation ──────────────────────────────────────────────────

/// POST /api/projects/:id/install-template
/// Copies the AI template files into the project directory.
pub async fn install_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<AiAuditStatus>> {
    let project = match state.db.with_conn(move |conn| crate::db::projects::get_project(conn, &id)).await {
        Ok(Some(p)) => p,
        Ok(None) => return Json(ApiResponse::err("Project not found")),
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let project_path = scanner::resolve_host_path(&project.path);
    if !project_path.exists() {
        return Json(ApiResponse::err(format!("Project path not found: {}", project_path.display())));
    }

    // Find templates directory
    let template_dir = resolve_templates_dir();
    if !template_dir.exists() {
        return Json(ApiResponse::err(format!("Templates directory not found: {}", template_dir.display())));
    }

    // Copy ai/ directory (non-destructive: don't overwrite existing files)
    let ai_template = template_dir.join("ai");
    let ai_target = project_path.join("ai");
    if ai_template.is_dir() {
        if let Err(e) = copy_dir_nondestructive(&ai_template, &ai_target) {
            return Json(ApiResponse::err(format!("Failed to copy ai/ template: {}", e)));
        }
    }

    // Copy redirector files (CLAUDE.md, .cursorrules, .windsurfrules, .clinerules)
    for filename in &["CLAUDE.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
        let src = template_dir.join(filename);
        let dst = project_path.join(filename);
        if src.exists() && !dst.exists() {
            if let Err(e) = std::fs::copy(&src, &dst) {
                tracing::warn!("Failed to copy {}: {}", filename, e);
            }
        }
    }

    // Inject bootstrap prompt into ai/index.md
    let index_file = project_path.join("ai/index.md");
    if index_file.exists() {
        inject_bootstrap_prompt(&index_file);
    }

    // Ensure ai/ files are in .gitignore
    crate::core::mcp_scanner::ensure_gitignore_public(&project.path, "ai/var/");

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
}

/// Resolve the templates directory (Docker mount or local)
fn resolve_templates_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("KRONN_TEMPLATES_DIR") {
        return std::path::PathBuf::from(dir);
    }
    // Docker default
    let docker_path = std::path::PathBuf::from("/app/templates");
    if docker_path.exists() {
        return docker_path;
    }
    // Local dev fallback: relative to binary
    std::path::PathBuf::from("templates")
}

/// Recursively copy a directory, skipping files that already exist at the destination.
fn copy_dir_nondestructive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {}", dst.display(), e))?;

    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("read_dir {}: {}", src.display(), e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("entry: {}", e))?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_nondestructive(&src_path, &dst_path)?;
        } else if !dst_path.exists() {
            std::fs::copy(&src_path, &dst_path)
                .map_err(|e| format!("copy {} -> {}: {}", src_path.display(), dst_path.display(), e))?;
        }
    }
    Ok(())
}

/// Inject the bootstrap prompt at the top of ai/index.md
fn inject_bootstrap_prompt(index_file: &std::path::Path) {
    let content = match std::fs::read_to_string(index_file) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Already has bootstrap prompt?
    if content.contains("KRONN:BOOTSTRAP:START") {
        return;
    }

    let prompt = r#"<!-- KRONN:BOOTSTRAP:START -->
<!-- ================================================================
     BOOTSTRAP INSTRUCTIONS — AUTO-GENERATED BY KRONN
     ================================================================
     This block is for AI agents only. It instructs you to analyze
     this repository and fill in the ai/ documentation skeleton.

     After completing the analysis, you MUST delete this entire block
     (from KRONN:BOOTSTRAP:START to KRONN:BOOTSTRAP:END).
     ================================================================ -->

> **FIRST-RUN TASK — Bootstrap ai/ documentation**
>
> This is a fresh `ai/` skeleton. You must analyze the repository and fill in all files.
>
> **Rules:**
> - All `ai/` files MUST be in **English**
> - Content is **AI context** (factual, concise), NOT human documentation
> - Do NOT invent information — mark unknowns with `<!-- TODO: verify with team -->`
>
> **Procedure:**
>
> 1. **Analyze the project** — Read README.md, package.json, composer.json, Cargo.toml,
>    Makefile, Dockerfile, docker-compose.yml, CI configs, main config files.
>    Determine: stack, structure, build/dev commands, key patterns, third parties, testing, CI/CD.
>
> 2. **Fill `ai/index.md`** — Replace all `{{PLACEHOLDERS}}` and `<!-- comments -->`:
>    project name, stack summary, common tasks table, prerequisites, DO NOT rules,
>    workflow constraints, source of truth, code placement, stack facts, date.
>
> 3. **Fill `ai/repo-map.md`** — Map key folders, config files, entry points.
>
> 4. **Fill `ai/glossary.md`** — Extract domain terms, abbreviations, internal names.
>
> 5. **Fill `ai/coding-rules.md`** — Linters, formatters, naming conventions.
>
> 6. **Fill `ai/testing-quality.md`** — Test frameworks, commands, quality gates.
>
> 7. **Fill `ai/architecture/overview.md`** — Services, patterns, separation of concerns.
>
> 8. **Fill `ai/operations/mcp-servers.md`** — Document configured MCP servers if any.
>
> 9. **Fill `ai/operations/debug-operations.md`** — Common commands, troubleshooting.
>
> 10. **Fill `ai/inconsistencies-tech-debt.md`** — Note any inconsistencies, legacy
>     patterns, or tech debt discovered during analysis.
>
> 11. **Review** — Ensure no `{{PLACEHOLDER}}` remains. Remove filled `<!-- comments -->`.
>     Keep `<!-- TODO -->` for items needing human verification.
>
> 12. **DELETE THIS ENTIRE BLOCK** (from `KRONN:BOOTSTRAP:START` to `KRONN:BOOTSTRAP:END`).
>
> When done, summarize: files filled, items needing human input, suggested deep-dives.

<!-- KRONN:BOOTSTRAP:END -->

"#;

    let new_content = format!("{}{}", prompt, content);
    if let Err(e) = std::fs::write(index_file, new_content) {
        tracing::warn!("Failed to inject bootstrap prompt: {}", e);
    }
}

// ─── AI Audit (SSE streaming) ───────────────────────────────────────────────

const PROMPT_PREAMBLE: &str = "Rules: Write in English. Be factual and concise — this is AI context for agents, not human documentation. Do NOT invent information — mark unknowns with <!-- TODO: verify -->. Replace all {{PLACEHOLDERS}} and <!-- fill --> comments with real content. Keep the existing file structure and headings.";

const ANALYSIS_STEPS: &[(&str, &str)] = &[
    ("ai/index.md", "Analyze this repository. Read README.md, package.json or composer.json or Cargo.toml, Makefile, Dockerfile, docker-compose.yml, CI configs (.github/workflows, .gitlab-ci.yml), and main config files. Determine the stack, project structure, build/dev commands, key patterns, third-party services, testing setup, and CI/CD pipeline. Then fill ai/index.md: replace the project name, stack summary, common tasks table, prerequisites, DO NOT rules, workflow constraints, source of truth, code placement rules, and stack facts. Set the date to today."),
    ("ai/repo-map.md", "Read ai/index.md for project context. Explore the directory structure of this repository. Fill ai/repo-map.md: list key folders and what they contain, primary config files, and entry points."),
    ("ai/coding-rules.md", "Read ai/index.md for project context. Find all linter, formatter, and type-checker configs in the repo (e.g. .eslintrc, prettier, phpcs, rustfmt, tsconfig, etc). Fill ai/coding-rules.md with the actual tools, their config file paths, and run commands."),
    ("ai/testing-quality.md", "Read ai/index.md for project context. Find test framework configs (jest, phpunit, pytest, vitest, etc) and CI quality gates. Fill ai/testing-quality.md with test commands, config file locations, and quality gates."),
    ("ai/architecture/overview.md", "Read ai/index.md for project context. Analyze the high-level architecture: services/apps, key design patterns, separation of concerns, and any legacy aspects. Fill ai/architecture/overview.md."),
    ("ai/glossary.md", "Read ai/index.md for project context. Search the codebase for domain-specific terms, abbreviations, internal naming conventions, and project jargon. Fill ai/glossary.md organized by category (architecture, business domain, third parties, etc)."),
    ("ai/operations/debug-operations.md", "Read ai/index.md for project context. Find operational commands from Makefile, package.json scripts, docker-compose commands, and any run/build/debug procedures. Fill ai/operations/debug-operations.md with common commands and troubleshooting steps."),
    ("ai/operations/mcp-servers.md", "Read ai/index.md for project context. Check if .mcp.json or .mcp.json.example exists in the repo. If yes, document the configured MCP servers in ai/operations/mcp-servers.md. If no MCP config exists, replace the file content with: '# MCP Servers\n\nNo MCP servers configured for this project.'"),
    ("ai/inconsistencies-tech-debt.md", "Read ai/index.md and browse the other ai/ files you filled in previous steps. Based on everything you observed during analysis, note any inconsistencies, legacy patterns, outdated configs, or technical debt. Fill ai/inconsistencies-tech-debt.md with concrete, factual entries."),
    ("REVIEW", "Read ALL ai/ files. Check for: remaining {{PLACEHOLDERS}}, unfilled <!-- comments -->, inconsistencies between files, duplicate information. Fix any issues found. Remove any remaining <!-- fill --> comments. Ensure all files are coherent with each other. This is the final quality pass — make the documentation clean and useful."),
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

    let project = project.unwrap();
    let project_path_str = project.path.clone();
    let project_path = scanner::resolve_host_path(&project.path);

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

        for (step_num, (target_file, step_prompt)) in ANALYSIS_STEPS.iter().enumerate() {
            let step = step_num + 1;
            let file_label = if *target_file == "REVIEW" { "Relecture finale" } else { target_file };

            let step_start = serde_json::json!({
                "step": step,
                "total": total_steps,
                "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            let full_prompt = format!("{} {}", PROMPT_PREAMBLE, step_prompt);

            // Always use full_access for audit (agent needs to write files)
            match runner::start_agent(&agent_type, &project_path_str, &full_prompt, &tokens, true).await {
                Ok(mut process) => {
                    while let Some(line) = process.next_line().await {
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }

                    let status = process.child.wait().await;
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

        let done = serde_json::json!({ "status": "complete", "total_steps": total_steps });
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

    let project_path = scanner::resolve_host_path(&project.path);
    let index_file = project_path.join("ai/index.md");

    if !index_file.exists() {
        return Json(ApiResponse::err("ai/index.md not found — run the audit first"));
    }

    let content = match std::fs::read_to_string(&index_file) {
        Ok(c) => c,
        Err(e) => return Json(ApiResponse::err(format!("Cannot read ai/index.md: {}", e))),
    };

    // Already validated?
    if content.contains("KRONN:VALIDATED") {
        let status = scanner::detect_audit_status(&project.path);
        return Json(ApiResponse::ok(status));
    }

    // Inject validation marker at the end of the file
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let marker = format!("\n<!-- KRONN:VALIDATED:{} -->\n", today);
    let new_content = format!("{}{}", content.trim_end(), marker);

    if let Err(e) = std::fs::write(&index_file, new_content) {
        return Json(ApiResponse::err(format!("Failed to write marker: {}", e)));
    }

    let status = scanner::detect_audit_status(&project.path);
    Json(ApiResponse::ok(status))
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
