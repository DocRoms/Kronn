// `POST /api/projects/:id/full-audit` — the unified end-to-end pipeline:
// install template if needed → run the 10-step audit → create the
// validation discussion. Plus `POST /api/projects/:id/cancel-audit`
// which kills the running agent process and cleans the docs/ tree
// + redirector files so the project is back to a clean slate.

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::agents::runner;
use crate::core::cmd::sync_cmd;
use crate::core::scanner;
use crate::models::*;
use crate::AppState;

use super::helpers::{
    check_ai_dir_permissions, compute_audit_info_sync, detect_issue_tracker_mcp,
    detect_project_skills, build_validation_prompt, build_sub_audit_validation_prompt, remove_bootstrap_block,
};
use super::{SseStream, ANALYSIS_STEPS, PROMPT_PREAMBLE};

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
    let briefing_notes = crate::api::projects::resolve_briefing_notes(&project_path, &project.briefing_notes);
    let linked_repos_block = crate::api::projects::format_linked_repos_for_prompt(&project.linked_repos);
    // 0.8.3 — candidate pool for companion-repo detection. The agent
    // only suggests links from this finite list (typically 5-20),
    // not "every repo I see on disk" — keeps suggestions scalable
    // for users with hundreds of repos.
    let pid_for_universe = project_id.clone();
    let kronn_projects_universe_block = match state
        .db
        .with_conn(crate::db::projects::list_projects)
        .await
    {
        Ok(all) => crate::api::projects::format_kronn_projects_universe_for_prompt(&all, &pid_for_universe),
        Err(e) => {
            tracing::warn!("Failed to load Kronn projects for companion-detection block: {}", e);
            None
        }
    };
    let agent_type = req.agent;
    let agent_label = format!("{:?}", agent_type);
    // 0.8.3 (#311) — resume support. Caller passes the
    // `last_completed_step` of an interrupted run; we skip steps
    // 1..=resume_from and start at resume_from+1. Clamped to total_steps
    // so a malicious / stale client can't ask us to skip past the
    // pipeline entirely. None / 0 / >= total → fresh run.
    let resume_from: u32 = req.resume_from.unwrap_or(0);

    // 0.8.2 — Resolve specialized audit kind. `Full` is the default
    // (backwards-compat for clients that don't send `kind`). `Custom`
    // requires a body that S2.D3-D5 still need to design — for now
    // surface a clean error rather than silently running an empty loop.
    let kind = req.kind.unwrap_or_default();
    let kind_label = kind.as_label();
    if matches!(kind, crate::models::AuditKind::Custom) {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error")
                    .data("{\"error\":\"AuditKind::Custom is not yet wired (S2.D3-D5)\"}")
            )
        }));
        return Sse::new(stream);
    }
    let steps = super::kind_to_steps(kind);

    let tokens = {
        let config = state.config.read().await;
        config.tokens.clone()
    };

    let language = {
        let config = state.config.read().await;
        config.language.clone()
    };

    let total_steps = steps.len();
    let db = state.db.clone();
    let audit_tracker = state.audit_tracker.clone();

    // Clear any stale cancellation flag for this project
    if let Ok(mut tracker) = audit_tracker.lock() {
        tracker.cancelled.remove(&project_id);
    }

    // 0.8.2 — record this audit invocation in the `audit_runs` table.
    // Inserted with status='Running' here, completed at the end of the
    // pipeline (or marked Failed/Cancelled on abnormal exit). The row
    // powers the health-badge sparkline + audit-history doc.
    let audit_run_id = Uuid::new_v4().to_string();
    let audit_started_at = Utc::now();
    {
        let run_id = audit_run_id.clone();
        let pid = project_id.clone();
        let agent_name = agent_label.clone();
        let _ = db.with_conn(move |conn| {
            crate::db::audit_runs::insert_running(
                conn, &run_id, &pid, kind_label, &agent_name, audit_started_at,
            )
        }).await;
    }

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        // Seed live progress so GET /audit-status can report where we are
        // even when the SSE client (browser tab) went away.
        if let Ok(mut t) = audit_tracker.lock() {
            t.start_progress(&project_id, total_steps as u32, "full_audit");
            // Phase 1 starts here; advance_step will update to "auditing"
            // once the 10-step loop begins. The intermediate installing
            // phase is visible by checking step_index == 0.
            if let Some(entry) = t.progress.get_mut(&project_id) {
                entry.phase = "installing".into();
            }
        }

        // ── Phase 1: Install template if needed ──
        // 0.8.2 — Specialized audit kinds (non-Full) skip template
        // installation. They are focused re-scans that assume Full has
        // already laid down `docs/` for the project. Running them on a
        // bare project would produce findings against a non-existent
        // baseline, which is rarely what the user wants.
        let is_full = matches!(kind, crate::models::AuditKind::Full);
        let status = scanner::detect_audit_status(&project_path_str);
        let template_installed = is_full && matches!(status, AiAuditStatus::NoTemplate);

        if template_installed {
            let pp = project_path_str.clone();
            let install_result = tokio::task::spawn_blocking(move || -> Result<(crate::core::legacy_docs::LegacyMigrationReport,), String> {
                let project_path = scanner::resolve_host_path(&pp);
                if !project_path.exists() {
                    return Err(format!("Project path not found: {}", project_path.display()));
                }

                // 0.7.1 — bootstrap to docs/ (or respect legacy ai/ when
                // present). Permission check runs on whichever the project
                // already uses, then template install lands at docs/ for
                // fresh projects (default).
                let docs_target = scanner::detect_docs_dir(&project_path);
                if docs_target.exists() {
                    if let Err(e) = check_ai_dir_permissions(&docs_target) {
                        return Err(format!("{}/ permission error: {}",
                            docs_target.file_name().and_then(|n| n.to_str()).unwrap_or("docs"), e));
                    }
                }

                // 0.8.3 (#272) — Pre-audit legacy docs migration. If the
                // user pointed Kronn at a project with hand-curated docs
                // (no Kronn signature in docs/AGENTS.md), move everything
                // under docs/legacy/ BEFORE the template install. The
                // audit prompt below is patched to read docs/legacy/**
                // as the PRIMARY source of truth when filling fresh
                // Kronn templates. Idempotent: already-Kronn-managed
                // docs/ → no-op (the signature check returns true).
                let legacy_report = crate::core::legacy_docs::migrate_user_docs_to_legacy(&docs_target)
                    .map_err(|e| format!("legacy docs migration failed: {}", e))?;

                let template_dir = crate::api::projects::resolve_templates_dir();
                if !template_dir.exists() {
                    return Err(format!("Templates directory not found: {}", template_dir.display()));
                }

                let docs_template = template_dir.join("docs");
                if docs_template.is_dir() {
                    crate::api::projects::copy_dir_nondestructive(&docs_template, &docs_target)?;
                }
                crate::api::projects::ensure_agent_writable_subfolders(&docs_target)?;

                // 0.8.4 (#295) — auto-write `docs/linked-repos.md` from
                // the project's linked_repos list. Catch-up for projects
                // created before the push→pull migration: the disc/WF
                // prompts no longer inject the block inline, so the
                // file on disk IS the source of truth. Idempotent +
                // no-op on empty list (file is removed if present).
                if let Err(e) = crate::api::projects::sync_linked_repos_doc_in(&docs_target, &project.linked_repos) {
                    tracing::warn!(
                        "Failed to seed docs/linked-repos.md during audit Phase 1 (project {}): {}",
                        project.name, e
                    );
                }

                // 0.8.3 (#278) — inject the Kronn-managed block into
                // every root agent file. Replaces the pre-0.8.3
                // `copy if !exists` loop that silently skipped user-
                // curated files → the agent never learned that Kronn
                // had put `docs/AGENTS.md` in place. The new helper:
                //   - Creates the file with Kronn block + Kronn template
                //     when the file is missing.
                //   - Prepends just the Kronn block above the user's
                //     existing content when the file exists, byte-
                //     preserving everything the user already wrote.
                //   - Re-renders ONLY the marker zone on a re-audit,
                //     so the user's content never piles up duplicates.
                // Failures on one file don't abort the audit — we log
                // and move on so a single locked / permission-denied
                // file doesn't break the whole install path.
                for filename in crate::core::root_agent_files::KRONN_ROOT_AGENT_FILES {
                    let src = template_dir.join(filename);
                    let dst = project_path.join(filename);
                    let template_body = std::fs::read_to_string(&src).ok();
                    match crate::core::root_agent_files::inject_or_update(
                        &dst,
                        template_body.as_deref(),
                    ) {
                        Ok(outcome) => {
                            tracing::debug!(
                                "Kronn block in {}: {:?}",
                                filename, outcome
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to inject Kronn block in {}: {}",
                                filename, e
                            );
                        }
                    }
                }

                let index_file = project_path.join("docs/AGENTS.md");
                if index_file.exists() {
                    crate::api::projects::inject_bootstrap_prompt(&index_file);
                }

                runner::fix_file_ownership(&project_path);
                Ok((legacy_report,))
            }).await.unwrap_or_else(|e| Err(format!("Task failed: {}", e)));

            let legacy_migration = match install_result {
                Err(e) => {
                    // Install failed — drop progress so the UI stops polling with
                    // a stale "installing" state.
                    if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }
                    let err = serde_json::json!({ "error": e });
                    yield Event::default().event("error").data(err.to_string());
                    return;
                }
                Ok((report,)) => report,
            };

            crate::core::mcp_scanner::ensure_gitignore_public(&project_path_str, "docs/var/");

            // 0.8.3 (#272) — surface the legacy-docs migration so the
            // frontend can show a toast + list of moved entries. The
            // report carries `migrated=false` + a skip_reason when
            // nothing was done (no docs/, already-Kronn-managed,
            // empty docs/). Frontend renders only when migrated=true.
            if legacy_migration.migrated {
                yield Event::default().event("legacy_docs_migrated").data(
                    serde_json::to_string(&legacy_migration).unwrap_or_else(|_| "{}".into())
                );
            }
        }

        let tmpl_event = serde_json::json!({ "installed": template_installed });
        yield Event::default().event("template_installed").data(tmpl_event.to_string());

        // 0.8.3 (#280) — MCP allowlist for the audit. On projects
        // with 10+ MCP servers wired (Fastly, Docker, GitLab, M365,
        // Playwright, …), the agent's system prompt balloons to 12-
        // 15K tokens of tool descriptions BEFORE it starts thinking.
        // The audit doesn't need any of those — it reads local files
        // and fills templates. We swap `.mcp.json` for a filtered
        // version covering only the introspection / reasoning /
        // lookup tools, then restore via RAII Drop when this handler
        // returns (success OR panic). Discussions / workflows that
        // would spawn during this window see the filtered set + a
        // banner explains; trade-off documented in the swap module.
        let _audit_mcp_swap = crate::core::audit_mcp_filter::AuditMcpSwap::install(&project_path)
            .ok()
            .flatten();
        if let Some(ref swap) = _audit_mcp_swap {
            let r = swap.report();
            yield Event::default().event("audit_mcp_filtered").data(
                serde_json::json!({
                    "kept": r.kept,
                    "dropped": r.dropped,
                    "kept_count": r.kept.len(),
                    "dropped_count": r.dropped.len(),
                }).to_string()
            );
        }

        // ── Phase 2: Run 10-step audit ──
        // Remove bootstrap prompt
        let index_file = project_path.join("docs/AGENTS.md");
        if index_file.exists() {
            remove_bootstrap_block(&index_file);
        }

        // 0.8.2 — Snapshot the existing `docs/tech-debt/` directory BEFORE
        // Step 9 has a chance to touch it. Used by the reconciliation
        // pass (after the loop) to classify TDs that the agent did NOT
        // re-emit (Fixed / Stale / Missed / Uncertain). Cheap operation
        // — content-hashes a handful of small markdown files. Survives
        // a fresh project (empty Vec).
        let pre_audit_td_snapshot = super::reconciliation::snapshot_tech_debt_dir(
            &project_path.join("docs"),
        );

        // 0.8.3 (#274) — cumulative token counter surfaced on each
        // `step_done`. Without this, the frontend had to estimate
        // audit cost after-the-fact from the validation discussion
        // (which only exists post-Phase 3) and couldn't show "you've
        // spent X tokens after Y steps, optimise step Z now".
        //
        // The `started_at` we surface in the `start` SSE event is the
        // SAME wallclock taken at the very top of this handler (line
        // ~119, before Phase 1 — template install + legacy migration
        // + bootstrap inject). Re-declaring it here would shift the
        // anchor forward by the Phase 1 duration (~10-30 s on first
        // audits) and the frontend's live-elapsed counter would jump
        // BACK in time once the SSE event lands, leaving the user
        // staring at a counter that re-starts from 0 mid-run.
        let mut total_tokens_so_far: u64 = 0;
        // 0.8.3 (#311) — track resume + completion status. On every
        // successful `step_done` we bump `last_successful_step` AND
        // persist via `update_last_completed_step`. At end-of-stream
        // we use these two locals to decide between:
        //   - `complete()`         (all 10 steps success — happy path)
        //   - `mark_interrupted()` (some step warning OR stream ended
        //                          before step 10 — resumable later)
        //   - `mark_failed()`      (catastrophic failure, e.g. start_agent
        //                          returned Err for every step)
        // The validation discussion is only created on the happy path.
        // 0.8.3 (#311) — when resuming, prime `last_successful_step`
        // with the caller-provided value so the "did we reach step 10?"
        // check at end-of-stream considers the previously-done steps.
        let mut last_successful_step: u32 = resume_from.min(total_steps as u32);
        let mut any_step_warning: bool = false;
        let start = serde_json::json!({
            "total_steps": total_steps,
            "started_at": audit_started_at.to_rfc3339(),
        });
        yield Event::default().event("start").data(start.to_string());

        // 0.8.7 STEP 0 — anti-hallu canonical section maintenance.
        // Runs once at the start of every audit BEFORE the numbered
        // steps. Deterministic Rust function (no LLM call, no token
        // cost). Writes/refreshes the `<!-- kronn:section name=
        // "anti-hallu" -->` block at the top of `docs/AGENTS.md`. The
        // doctrine then lives in the file every subsequent step + every
        // future agent reading the project sees. Idempotent — silently
        // no-op when the file already carries the section at today's
        // date. Cross-cuts resume : even if `resume_from > 0`, we still
        // run STEP 0 because it's cheap and may correct a section that
        // drifted since the interrupted run.
        match super::anti_hallu_step::apply(&project_path) {
            Ok(super::anti_hallu_step::AntiHalluApplyResult::Inserted) => {
                let ev = serde_json::json!({
                    "step": "anti-hallu",
                    "result": "inserted",
                    "file": "docs/AGENTS.md",
                });
                yield Event::default().event("anti_hallu_step").data(ev.to_string());
            }
            Ok(super::anti_hallu_step::AntiHalluApplyResult::Refreshed) => {
                let ev = serde_json::json!({
                    "step": "anti-hallu",
                    "result": "refreshed",
                    "file": "docs/AGENTS.md",
                });
                yield Event::default().event("anti_hallu_step").data(ev.to_string());
            }
            Ok(super::anti_hallu_step::AntiHalluApplyResult::NoOp)
            | Ok(super::anti_hallu_step::AntiHalluApplyResult::FileMissing) => {
                // FileMissing happens on a fresh-bootstrap audit where the
                // template hasn't been copied yet at this call site —
                // unusual but not fatal. Step 1 will create AGENTS.md
                // from the template (which already contains the section).
            }
            Err(e) => {
                // STEP 0 failures are non-fatal — log a warning event and
                // let the audit continue. The subsequent steps still run,
                // they just don't get a fresh section.
                tracing::warn!("STEP 0 anti-hallu apply failed: {e}");
                let ev = serde_json::json!({
                    "step": "anti-hallu",
                    "result": "error",
                    "error": e.to_string(),
                });
                yield Event::default().event("anti_hallu_step").data(ev.to_string());
            }
        }

        for (step_num, analysis_step) in steps.iter().enumerate() {
            // Check for cancellation before each step
            if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id)).unwrap_or(false) {
                if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }
                let run_id = audit_run_id.clone();
                let db_for_cancel = db.clone();
                tokio::spawn(async move {
                    let _ = db_for_cancel.with_conn(move |conn| {
                        crate::db::audit_runs::mark_cancelled(conn, &run_id)
                    }).await;
                });
                let cancelled = serde_json::json!({ "status": "cancelled" });
                yield Event::default().event("cancelled").data(cancelled.to_string());
                return;
            }

            let step = step_num + 1;
            let file_label = if analysis_step.target_file == "REVIEW" { "Final review" } else { analysis_step.target_file };

            // 0.8.3 (#311) — resume support. Skip steps the previous
            // interrupted run already completed. We still advance the
            // tracker's `step_index` so the UI bar shows correct
            // progress, but we don't spawn an agent for these.
            if (step as u32) <= resume_from {
                if let Ok(mut t) = audit_tracker.lock() {
                    t.advance_step(&project_id, step as u32, Some(file_label.to_string()));
                }
                // Surface to the frontend so it can render a "(skipped — already done)"
                // marker on the step instead of pretending we re-did the work.
                yield Event::default().event("step_skipped").data(
                    serde_json::json!({
                        "step": step, "total": total_steps, "file": file_label, "reason": "resume",
                    }).to_string()
                );
                continue;
            }

            if let Ok(mut t) = audit_tracker.lock() {
                t.advance_step(&project_id, step as u32, Some(file_label.to_string()));
                // 0.8.3 — clear per-step ephemeral chips so a stale
                // "🔧 Read" or per-step token count from step N-1
                // doesn't leak into the poll snapshot for step N.
                // total_tokens_so_far stays intact (cumulative).
                t.clear_step_chips(&project_id);
            }

            let step_start = serde_json::json!({
                "step": step, "total": total_steps, "file": file_label
            });
            yield Event::default().event("step_start").data(step_start.to_string());

            // 0.8.4 (#298) — persist per-step metrics for the recap panel.
            // Idempotent on (audit_run_id, step_index) so resuming an
            // interrupted run (#311) that re-fires step_start for the
            // first replayed step doesn't crash on UNIQUE.
            {
                let run_id = audit_run_id.clone();
                let label = file_label.to_string();
                let started = Utc::now();
                let _ = db.with_conn(move |conn| {
                    crate::db::audit_runs::insert_audit_step_start(
                        conn, &run_id, step as u32, &label, started,
                    )
                }).await;
            }

            let today = Utc::now().format("%Y-%m-%d").to_string();
            let today_compact = Utc::now().format("%Y%m%d").to_string();
            let mut full_prompt = format!("{}\n\n{}", PROMPT_PREAMBLE, analysis_step.prompt)
                .replace("YYYYMMDD=today", &format!("YYYYMMDD={}", today_compact))
                .replace("today's date (YYYY-MM-DD)", &today)
                .replace("set to today's date", &format!("set to {}", today));

            if let Some(ref notes) = briefing_notes {
                full_prompt.push_str(&format!("\n\n## Project briefing (from the user)\n{}\n", notes));
            }
            // 0.8.3 (#272) — pre-existing user docs are migrated to
            // docs/legacy/ before the audit runs. Tell the agent to
            // READ them FIRST as the primary source of truth when
            // filling fresh Kronn templates. Only inject the block
            // when docs/legacy/ actually exists (a fresh project or
            // a re-audit on already-Kronn-managed docs has no
            // legacy/ subdir).
            let legacy_dir_exists = project_path.join("docs/legacy").is_dir();
            if legacy_dir_exists {
                full_prompt.push_str("\n\n## Legacy docs (PRIMARY SOURCE for this audit)\nThis project had a hand-curated `docs/` folder BEFORE Kronn was bootstrapped. We moved that content under `docs/legacy/` so the freshly-installed Kronn templates don't collide with it. **READ every `*.md` under `docs/legacy/` BEFORE filling the Kronn templates.** That content is the human-curated knowledge — the README and source code alone would lose 6 months of accumulated context.\n\nWhen filling each Kronn template, cite the legacy source inline (`see docs/legacy/installation.md`, `cf docs/legacy/architecture/overview.md`) so the user can verify the mapping and decide what to keep / discard after the audit. After the audit, the user reviews `docs/legacy/` and either deletes it or migrates remaining pieces into the Kronn structure manually.\n\n**Navigation requirement for `docs/AGENTS.md` ONLY (Step 1):** when filling `docs/AGENTS.md`, add ONE line in the appropriate section (or a small dedicated `## Legacy docs (pre-Kronn snapshot)` section if none fits) that points future agents to `docs/legacy/` — wording like `> Hand-curated docs from before Kronn — see [docs/legacy/README.md](legacy/README.md) for context preserved from the previous structure.` Without this pointer the folder is invisible to anyone re-reading `AGENTS.md` next week. Do NOT add this line to other Kronn templates (`glossary.md`, `repo-map.md`, etc.) — the entry point is enough.\n");
            }
            if let Some(ref block) = linked_repos_block {
                full_prompt.push_str(&format!("\n\n{}\n", block));
            }
            if let Some(ref block) = kronn_projects_universe_block {
                full_prompt.push_str(&format!("\n\n{}\n", block));
            }

            // 0.8.3 (#274) — per-step instrumentation. The audit UI
            // shows a static "Step N/M — file.md" today; users have
            // no signal for how long things take or what tokens are
            // burning. We track:
            //   - step_started_at: wall-clock start (Instant)
            //   - step_tokens:    max(input+output) seen via
            //     `parse_claude_stream_line` — Claude reports
            //     cumulative usage per call, so `.max()` is correct
            //     (NOT a sum, which would double-count).
            //   - total_tokens_so_far: running counter across all
            //     finished steps + the current one's last reading.
            // Surfaced via the enriched `step_done` event below; the
            // frontend then displays per-step + total tokens + a
            // live elapsed counter (computed client-side from the
            // step_started_at wallclock).
            let step_started_at = std::time::Instant::now();
            let mut step_tokens: u64 = 0;

            match runner::start_agent_with_config(runner::AgentStartConfig {
                full_access: true,
                tier: crate::models::ModelTier::Reasoning,
                ..runner::AgentStartConfig::new(&agent_type, &project_path_str, &full_prompt, &tokens)
            }).await {
                Ok(mut process) => {
                    // Register the child PID for cancellation
                    if let Some(pid) = process.child.id() {
                        if let Ok(mut tracker) = audit_tracker.lock() {
                            tracker.running_pids.insert(project_id.clone(), pid);
                        }
                    }

                    let is_stream_json = process.output_mode == runner::OutputMode::StreamJson;
                    // 0.8.3 (#309) — Zombie audit detection.
                    //
                    // The naive `while let Some(line) = process.next_line().await`
                    // blocks indefinitely when the child claude exits but its
                    // stdout pipe is still held open by descendant processes
                    // (npx-launched MCP servers — `sequential-thinking`,
                    // `memory`, `context7` — inherit the stdout fd and don't
                    // release it on parent exit). Result: the audit stays
                    // "auditing step N/10" forever in the tracker, the user
                    // can't proceed, and 100+k tokens are wasted on a run
                    // that's actually dead.
                    //
                    // Fix: `tokio::select!` with a 60s idle timer. Every
                    // 60s without a new line, we check `try_wait()` on the
                    // child. If the child exited cleanly OR was reaped by
                    // an external SIGKILL, we treat the stream as ended
                    // and break out so the loop can emit `step_done` and
                    // move on. 60s is generous enough to absorb long
                    // thinking-only LLM phases without false positives.
                    let mut stream_ended = false;
                    while !stream_ended {
                        let next = tokio::select! {
                            maybe_line = process.next_line() => maybe_line,
                            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {
                                // 60s idle → probe the child. If it's gone,
                                // the open stdout pipe is held by a
                                // descendant; we break with a warning.
                                match process.child.try_wait() {
                                    Ok(Some(status)) => {
                                        tracing::warn!(
                                            "Audit step {} ({}): child claude exited (status: {:?}) but stdout pipe still open after 60s idle — zombie audit detected, breaking SSE loop. Likely cause: descendant MCP processes holding the stdout fd.",
                                            step, file_label, status
                                        );
                                        None
                                    }
                                    Ok(None) => continue,  // child still alive, keep waiting
                                    Err(e) => {
                                        tracing::warn!(
                                            "Audit step {} ({}): try_wait failed ({}); treating as zombie",
                                            step, file_label, e
                                        );
                                        None
                                    }
                                }
                            }
                        };
                        let Some(line) = next else {
                            stream_ended = true;
                            continue;
                        };
                        // Forward the raw line verbatim — frontend
                        // expects the legacy chunk shape for the
                        // streaming text preview.
                        let chunk = serde_json::json!({ "text": line, "step": step });
                        yield Event::default().event("chunk").data(chunk.to_string());

                        // 0.8.3 (#281) — typed-event surfacing for the
                        // live UX. The raw `chunk` above is for the
                        // legacy log preview / fallback; the typed
                        // events below feed the new dedicated chips
                        // (live tokens, current tool name, partial
                        // text). Non-stream-json agents (Vibe direct,
                        // Ollama) skip this branch — their chips stay
                        // empty rather than show stale 0 values.
                        if is_stream_json {
                            match runner::parse_claude_stream_line(&line) {
                                runner::StreamJsonEvent::Usage { input_tokens, output_tokens, .. } => {
                                    step_tokens = step_tokens.max(input_tokens + output_tokens);
                                    let cumulative = total_tokens_so_far.saturating_add(step_tokens);
                                    // 0.8.3 — mirror the live value into
                                    // the AuditTracker so the poll
                                    // endpoint can re-seed the chips
                                    // when SSE buffers / stalls (nginx
                                    // buffering, agent thinking-only
                                    // output, page re-mount).
                                    if let Ok(mut t) = audit_tracker.lock() {
                                        t.update_chips(&project_id, Some(step_tokens), Some(cumulative), None);
                                    }
                                    // Surface tokens-so-far LIVE so the
                                    // frontend chip ticks during the
                                    // step (was previously emitted only
                                    // at `step_done`, leaving the user
                                    // staring at a static counter).
                                    yield Event::default().event("step_progress").data(
                                        serde_json::json!({
                                            "step": step,
                                            "step_tokens": step_tokens,
                                            "total_tokens_so_far": cumulative,
                                        }).to_string()
                                    );
                                }
                                runner::StreamJsonEvent::ToolStart(name) => {
                                    // 0.8.3 — also persist the tool in
                                    // the tracker so the poll re-seeds
                                    // it after a buffer / re-mount.
                                    if let Ok(mut t) = audit_tracker.lock() {
                                        t.update_chips(&project_id, None, None, Some(name.clone()));
                                    }
                                    // The agent is about to call a
                                    // tool (Read, Glob, Bash, MCP,
                                    // …). Surface the name so the UI
                                    // shows "🔧 Reading docs/AGENTS.md"
                                    // — way more informative than a
                                    // spinning loader.
                                    yield Event::default().event("tool_call").data(
                                        serde_json::json!({
                                            "step": step,
                                            "tool": name,
                                        }).to_string()
                                    );
                                }
                                runner::StreamJsonEvent::Text(_)
                                | runner::StreamJsonEvent::ToolInputDelta(_)
                                | runner::StreamJsonEvent::ToolEnd
                                | runner::StreamJsonEvent::Skip => {}
                            }
                        }
                    }
                    let status = process.child.wait().await;
                    process.fix_ownership();

                    // Unregister PID
                    if let Ok(mut tracker) = audit_tracker.lock() {
                        tracker.running_pids.remove(&project_id);
                    }

                    // Check if cancelled during this step
                    if audit_tracker.lock().map(|t| t.cancelled.contains(&project_id)).unwrap_or(false) {
                        if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }
                        let cancelled = serde_json::json!({ "status": "cancelled" });
                        yield Event::default().event("cancelled").data(cancelled.to_string());
                        return;
                    }

                    let cli_success = status.map(|s| s.success()).unwrap_or(false);
                    let duration_ms = step_started_at.elapsed().as_millis() as u64;
                    total_tokens_so_far = total_tokens_so_far.saturating_add(step_tokens);

                    // 0.8.3 — Root-cause guard for the empty-tech-debt
                    // bug on DOCROMS_WEB. The CLI exited 0 (cli_success)
                    // but on a step that targets a real file we ALSO
                    // need to confirm the file was actually written and
                    // looks plausible. Without this, an agent that
                    // crashes mid-Write (or that writes the empty
                    // string in a parse-error fallback) produces a
                    // 0-byte file silently — users only discover the
                    // hole later when validating findings, or never.
                    //
                    // If suspicious: emit `step_warning` (frontend
                    // surfaces a banner), auto-repair from the template
                    // so the user can re-run the step OR resume from a
                    // clean baseline, and report `success: false` in
                    // step_done so the overall audit summary is honest.
                    let (success, warning) = crate::api::audit::validation::validate_and_repair_step_output(
                        cli_success,
                        &project_path,
                        analysis_step.target_file,
                    );

                    if let Some(w) = &warning {
                        tracing::warn!(
                            "Audit step {} ({}) produced suspicious output: {}",
                            step, file_label, w.reason
                        );
                        yield Event::default().event("step_warning").data(
                            serde_json::json!({
                                "step": step,
                                "file": file_label,
                                "reason": w.reason,
                                "repaired_from_template": w.repaired,
                            }).to_string()
                        );
                    }

                    let step_done = serde_json::json!({
                        "step": step,
                        "success": success,
                        "file": file_label,
                        "tokens": step_tokens,
                        "duration_ms": duration_ms,
                        "total_tokens": total_tokens_so_far,
                    });
                    yield Event::default().event("step_done").data(step_done.to_string());

                    // 0.8.4 (#298) — finalize the per-step row. `success`
                    // captures both CLI exit code AND the validation
                    // (#292) result, so a step that wrote a placeholder
                    // file shows up in the recap with cli_success=false
                    // even though the CLI exited 0.
                    {
                        let run_id = audit_run_id.clone();
                        let warn_reason = warning.as_ref().map(|w| w.reason.clone());
                        let repaired = warning.as_ref().map(|w| w.repaired).unwrap_or(false);
                        let ended = Utc::now();
                        let _ = db.with_conn(move |conn| {
                            crate::db::audit_runs::finalize_audit_step(
                                conn,
                                &run_id,
                                step as u32,
                                ended,
                                duration_ms,
                                step_tokens,
                                total_tokens_so_far,
                                success,
                                warn_reason.as_deref(),
                                repaired,
                            )
                        }).await;
                    }

                    // 0.8.3 (#311) — track per-step progress in audit_runs
                    // so an interrupted SSE stream can be resumed at
                    // `last_completed_step + 1` instead of restarting
                    // from step 1. We only update on `success=true`
                    // (no warning, no cli_failure) so a half-baked step
                    // doesn't get treated as done on resume.
                    if success {
                        last_successful_step = step as u32;
                        let run_id = audit_run_id.clone();
                        let step_n = step as u32;
                        let _ = db.with_conn(move |conn| {
                            crate::db::audit_runs::update_last_completed_step(conn, &run_id, step_n)
                        }).await;
                    } else {
                        // Track that something went wrong so the
                        // end-of-stream branch knows to mark the run
                        // as Interrupted rather than Completed and
                        // skip the validation discussion creation
                        // (cf F8c #312 — no validation disc unless all
                        // 10 steps reported success).
                        any_step_warning = true;
                    }
                }
                Err(e) => {
                    tracing::error!("Audit step {} failed to start: {}", step, e);
                    any_step_warning = true;
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

        // ── Phase 2.5: Reconciliation pass (0.8.2) ──
        // Only runs for Full audits — specialized kinds emit findings
        // into their own `inconsistencies-<kind>.md` file and would
        // produce a misleading "all TDs missed" report against the
        // canonical Full TD snapshot.
        // Compute the delta between the pre-audit TD snapshot and the
        // current on-disk state. Classify the TDs that weren't re-emitted
        // (Fixed / Stale / Missed / Uncertain) and write
        // `docs/tech-debt/_reconciliation-<date>.md`. The report keeps
        // the user informed across audits — without this, dropped TDs
        // would vanish silently and the user couldn't tell "fixed" from
        // "missed" between audit runs.
        if is_full && !pre_audit_td_snapshot.is_empty() {
            let project_path_for_recon = project_path.clone();
            let snapshot = pre_audit_td_snapshot.clone();
            let recon_outcome = tokio::task::spawn_blocking(move || {
                use super::reconciliation::{
                    check_signature_in_source, classify, compute_delta, render_report,
                };
                let deltas = compute_delta(&snapshot);
                let project_path_for_check = project_path_for_recon.clone();
                let entries = classify(
                    &deltas,
                    Utc::now(),
                    90,
                    |snap| check_signature_in_source(snap, &project_path_for_check),
                );
                let today = Utc::now().format("%Y-%m-%d").to_string();
                let report = render_report(&entries, &today, "Full");
                let report_path = project_path_for_recon
                    .join("docs/tech-debt")
                    .join(format!("_reconciliation-{}.md", today));
                let candidates = entries
                    .iter()
                    .filter(|e| e.delta != super::reconciliation::DeltaKind::Updated)
                    .count();
                let updated = entries.len() - candidates;
                (report_path, report, candidates, updated)
            })
            .await;

            if let Ok((report_path, report, candidates, updated)) = recon_outcome {
                if let Some(parent) = report_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                match std::fs::write(&report_path, &report) {
                    Ok(()) => {
                        tracing::info!(
                            "Reconciliation report: {} candidates, {} updated. Written to {}",
                            candidates, updated, report_path.display()
                        );
                        let ev = serde_json::json!({
                            "candidates": candidates,
                            "updated": updated,
                            "report_path": format!("docs/tech-debt/{}",
                                report_path.file_name().and_then(|n| n.to_str()).unwrap_or("")),
                        });
                        yield Event::default().event("reconciliation").data(ev.to_string());
                    }
                    Err(e) => {
                        tracing::warn!("Failed to write reconciliation report: {} — audit continues", e);
                    }
                }
            }
        }

        // ── Phase 3: Create validation discussion ──
        // 0.8.2 — Specialized kinds (non-Full) skip the validation
        // discussion: they emit findings into their own index file and
        // the user reviews them directly. The 4-phase validation flow
        // only makes sense after a complete docs/ regeneration.
        //
        // 0.8.3 (#312 F8c) — additional gate: the validation disc is
        // ONLY created when every step reported success AND we made
        // it through all 10 steps. Pre-fix, a rate-limit at step 5
        // produced a validation disc anyway (because the SSE handler
        // reached this code regardless of step outcomes), and the
        // ProjectCard then said "Validation en cours" on an audit
        // that hadn't actually produced anything past step 5.
        let audit_fully_succeeded = last_successful_step == total_steps as u32
            && !any_step_warning;
        if !audit_fully_succeeded {
            tracing::info!(
                "Audit run {} on project {} interrupted at step {}/{} (any_step_warning={}). Skipping validation discussion creation.",
                audit_run_id, project_id, last_successful_step, total_steps, any_step_warning
            );
            // Surface the interrupted state to the frontend so it can
            // show "Reprendre Step N/10" instead of "Validation en cours".
            yield Event::default().event("audit_interrupted").data(
                serde_json::json!({
                    "last_completed_step": last_successful_step,
                    "total_steps": total_steps as u32,
                    "had_warnings": any_step_warning,
                }).to_string()
            );
        }
        // 0.8.4 (#287) — both Full and sub-audits get a validation
        // discussion now. Pre-fix only Full was wired; sub-audits
        // would dump TDs to disk with no human-validation flow.
        let disc_id: Option<String> = if kind.is_validatable() && audit_fully_succeeded {
        if let Ok(mut t) = audit_tracker.lock() { t.mark_validating(&project_id); }

        let pp = project_path_str.clone();
        let audit_info = tokio::task::spawn_blocking(move || {
            compute_audit_info_sync(&pp)
        }).await.unwrap_or_else(|_| AuditInfo { files: vec![], todos: vec![], tech_debt_items: vec![] });

        // Detect if project has an issue tracker MCP (GitHub, GitLab, Jira, Linear, etc.)
        let has_issue_tracker_mcp = detect_issue_tracker_mcp(&project_path);

        // 0.8.4 (#287) — Full keeps the 4-phase protocol; sub-audits
        // get the shorter version scoped to the kind-specific index
        // file + (for RGAA) the explicit manual-audit + Access42 /
        // Opquast reminder.
        let validation_prompt = if kind.is_sub_audit() {
            build_sub_audit_validation_prompt(kind, &language, has_issue_tracker_mcp)
        } else {
            build_validation_prompt(&language, &audit_info, has_issue_tracker_mcp)
        };

        let now = Utc::now();
        let discussion_id = Uuid::new_v4().to_string();
        let initial_message = DiscussionMessage {
            lint_report: None,
            id: Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: validation_prompt,
            agent_type: None,
            timestamp: now,
            tokens_used: 0,
            auth_mode: None,
            model_tier: None, cost_usd: None, author_pseudo: None, author_avatar_email: None, source_msg_id: None, duration_ms: None,
        };

        // 0.8.4 (#287 + #322 / F2) — title carries the audit kind
        // via `display_name()` so the user sees "Validation audit RGAA 4.1"
        // not "Validation audit Rgaa AI" (the TitleCase enum label
        // leaks otherwise). Disc titles are the user-facing surface;
        // wire-level labels stay in `as_label()` for serde stability.
        let disc_title = if kind.is_sub_audit() {
            format!("Validation audit {}", kind.display_name())
        } else {
            "Validation audit AI".to_string()
        };
        let discussion = Discussion {
            id: discussion_id.clone(),
            project_id: Some(project_id.clone()),
            title: disc_title,
            agent: agent_type.clone(),
            language: language.clone(),
            participants: vec![agent_type.clone()],
            messages: vec![initial_message.clone()],
            message_count: 1, non_system_message_count: 1,
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
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
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

        // Generate checksums for drift detection — Full-audit only;
        // specialized kinds don't regenerate the docs/ baseline.
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
                    tracing::info!("Wrote docs/checksums.json with {} mappings", mappings.len());
                }
                if let Err(e) = crate::core::kronn_state::record_audit(&pp, "full") {
                    tracing::warn!("Failed to record audit in .kronn.json: {}", e);
                }
            }).await;
        }
        disc_id
        } else {
            None
        };

        // 0.8.2 — record the audit completion in `audit_runs`. The
        // severity distribution is counted by scanning the freshly-
        // produced TD detail files (cheap: 18-25 small markdown files
        // for a typical project). The reconciliation counts come from
        // the pre-audit snapshot we took earlier.
        let project_path_for_count = project_path.clone();
        let run_id_for_complete = audit_run_id.clone();
        let snapshot_for_count = pre_audit_td_snapshot.clone();
        let db_for_complete = db.clone();
        let ended_at = Utc::now();
        let _ = tokio::task::spawn_blocking(move || {
            let td_dir = project_path_for_count.join("docs/tech-debt");
            let counts = count_td_severities(&td_dir);
            let (resolved, new, carried) = compute_reconciliation_counts(
                &snapshot_for_count, &td_dir,
            );
            let score = crate::models::compute_health_score(
                counts.critical, counts.high, counts.medium, counts.low,
            );
            // 0.8.2 — Step 10 cluster detector. Surfaces "you have 4
            // docker findings, consider a focused Docker audit" cards
            // on the dashboard. Empty Vec is fine — UI hides the card.
            let recs = compute_cluster_recommendations(&td_dir);
            let recs_json = if recs.is_empty() {
                None
            } else {
                serde_json::to_string(&recs).ok()
            };
            (counts, resolved, new, carried, score, recs_json)
        })
        .await
        .map(|(counts, resolved, new, carried, score, recs_json)| {
            let run_id = run_id_for_complete.clone();
            let succeeded = audit_fully_succeeded;
            let last_step = last_successful_step;
            tokio::spawn(async move {
                let _ = db_for_complete.with_conn(move |conn| {
                    if succeeded {
                        crate::db::audit_runs::complete(
                            conn, &run_id, ended_at, "Completed",
                            counts.critical, counts.high, counts.medium, counts.low,
                            resolved, new, carried,
                            score, None, recs_json.as_deref(),
                        )
                    } else {
                        // 0.8.3 (#311) — mark as Interrupted (not Completed,
                        // not Failed): the resume mechanism will pick this
                        // row up via `latest_resumable` and the frontend
                        // shows "Reprendre Step N/10".
                        crate::db::audit_runs::mark_interrupted(
                            conn,
                            &run_id,
                            &format!("interrupted after step {last_step}/10 (warning or stream-end)"),
                        )
                    }
                }).await;
            });
        });

        // Audit fully complete — drop progress so UI polling can stop and
        // `GET /audit-status` reports `None`.
        if let Ok(mut t) = audit_tracker.lock() { t.clear_progress(&project_id); }

        let done = serde_json::json!({
            "status": "complete",
            "total_steps": total_steps,
            "discussion_id": disc_id,
            "template_was_installed": template_installed,
            "audit_run_id": audit_run_id,
        });
        yield Event::default().event("done").data(done.to_string());
    });

    Sse::new(stream)
}

/// Decision returned by [`classify_docs_dir_for_cancel`] — does the
/// cancel handler wipe the docs/ directory or preserve it ?
///
/// Introduced in 0.8.6 phase 4 hotfix to fix the data-loss bug where
/// `cancel_audit` did a blind `remove_dir_all(docs/)` regardless of
/// what content was there. Centralised here so the unit tests pin
/// the exact rule set the cancel handler relies on.
#[derive(Debug, PartialEq, Eq)]
pub enum DocsCancelAction {
    /// Keep the directory intact. `reason` is logged so the operator
    /// can audit the decision after the fact.
    Preserve { reason: &'static str },
    /// Safe to remove — empty directory with no audit history.
    Wipe,
}

/// Decide whether `dir` (one of `docs/`, `doc/`, `ai/` under the
/// project root) should be wiped on audit cancel.
///
/// Conservative by design : any signal of pre-existing content
/// (prior `.kronn.json.audits`, user-written file, sub-directory)
/// flips the decision to `Preserve`. Only a genuinely empty
/// directory (or one that holds nothing but a bare `.kronn.json`
/// with zero audits) is `Wipe`-eligible.
///
/// Pure — no logging here so tests can call it without setting up a
/// tracing subscriber. The caller logs the rationale.
pub fn classify_docs_dir_for_cancel(
    project_path: &std::path::Path,
    dir: &std::path::Path,
) -> DocsCancelAction {
    // Signal 1 — any recorded audit in `.kronn.json`. Even ONE prior
    // audit means the directory holds legitimate user-visible content
    // and the wipe is hostile.
    let prior_state = crate::core::kronn_state::read(project_path);
    if prior_state.as_ref().map(|s| s.has_any_audit()).unwrap_or(false) {
        return DocsCancelAction::Preserve { reason: "prior audit recorded in .kronn.json" };
    }

    // Signal 2 — any non-state-file entry in the directory. Hand-written
    // notes, leftover files from a previous (un-recorded) Kronn run,
    // even a stray README.md the user dropped in — all flip Preserve.
    // We tolerate a lonely `.kronn.json` (it's a Kronn-management
    // artifact, not user content) ; everything else stops the wipe.
    let has_user_content = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok()).any(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            name_str != crate::core::kronn_state::KRONN_STATE_FILENAME
        }))
        .unwrap_or(false);
    if has_user_content {
        return DocsCancelAction::Preserve { reason: "user content present (non-state file in dir)" };
    }

    DocsCancelAction::Wipe
}

/// 0.8.6 phase 4 hotfix — pure cleanup logic extracted from `cancel_audit`'s
/// `spawn_blocking` closure so unit tests can pin the data-loss-safety
/// rules (audit gap #4, 2026-05-22).
///
/// Contract (NEW behaviour, post-hotfix) :
///   - Docs folders (`docs/`, `doc/`, `ai/`) : preserved when prior content
///     exists ([`classify_docs_dir_for_cancel`] decides) ; only wiped when
///     the directory is genuinely empty (or holds only a 0-audit
///     `.kronn.json`).
///   - `ai/` symlink (migration artifact) : removed unconditionally — pure
///     Kronn-managed, no user content lives in a symlink.
///   - **Root redirector files** (`CLAUDE.md`, `AGENTS.md`, `.cursorrules`,
///     …) : NEVER deleted on cancel. User-edited content there would be
///     destroyed otherwise. Pre-fix behaviour was to wipe them blindly.
///
/// Returns `Err` only on missing project path (caller decides what to do) ;
/// individual filesystem errors are surfaced for the cancel handler to
/// report. Tests use this directly with a `tempdir` root.
pub fn cleanup_audit_files(project_path: &std::path::Path) -> Result<(), String> {
    if !project_path.exists() {
        return Err(format!("Project path not found: {}", project_path.display()));
    }

    // Check each candidate docs folder. Skip the wipe when ANY of :
    //   - the dir hosts a `.kronn.json` with finished audits
    //   - the dir hosts ANY non-empty, non-hidden file (could be
    //     hand-written content predating Kronn).
    // The combined check is intentionally pessimistic — false negatives
    // (skip a dir we could have wiped) are harmless, false positives
    // (wipe a dir we shouldn't) are catastrophic.
    for folder in ["docs", "doc", "ai"] {
        let dir = project_path.join(folder);
        if !dir.exists() || !dir.is_dir() || dir.is_symlink() {
            continue;
        }
        match classify_docs_dir_for_cancel(project_path, &dir) {
            DocsCancelAction::Preserve { reason } => {
                tracing::warn!(
                    "Audit cancel : SKIPPING wipe of {}/ — {} — preserving existing content",
                    folder, reason,
                );
            }
            DocsCancelAction::Wipe => {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| format!("Failed to remove {}/: {}", folder, e))?;
                tracing::info!(
                    "Removed {}/ directory (was empty / no audit history) from {}",
                    folder, project_path.display(),
                );
            }
        }
    }
    // Drop a `ai` symlink if one was left over from a migration. Symlinks
    // are 100% Kronn-managed (migration-time artifact), safe to remove
    // regardless of `docs/` content.
    let ai_link = project_path.join("ai");
    if ai_link.is_symlink() {
        let _ = std::fs::remove_file(&ai_link);
    }

    // 0.8.6 phase 4 hotfix (2026-05-22) — redirector files at the project
    // root (`CLAUDE.md`, `AGENTS.md`, `.cursorrules`, …) are NO LONGER
    // auto-deleted on cancel.
    //
    // Rationale : the original "always wipe" assumed these files are
    // 100% Kronn-templated. In practice users routinely edit them
    // (AGENTS.md is a vendor-neutral convention — any project may have
    // hand-written content) and a blind `remove_file` is the same
    // data-loss bug the docs/ fix above addresses, just at the project
    // root instead of in a subdir.
    //
    // Safe trade-off : a cancelled greenfield audit leaves the freshly-
    // installed (placeholder-filled) redirectors on disk. Cost = the
    // operator sees a stale `CLAUDE.md` they didn't ask for ; one `rm`
    // away to clean. Compared to "Kronn deleted my hand-written AGENTS.md"
    // the asymmetry is overwhelming.
    //
    // Future deeper fix (not in this hotfix) : track file creation events
    // during the audit run, only delete files demonstrably created on
    // this session. Add TD-20260522-audit-cancel-track-fs.

    Ok(())
}

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
        // Drop live progress so GET /audit-status stops reporting "running"
        // even if the SSE stream is slow to notice the cancellation flag.
        tracker.clear_progress(&project_id);
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

    // 2. Delete audit-created files — DATA-LOSS GUARDS via
    // `should_preserve_docs_folder()`.
    //
    // 0.8.6 phase 4 hotfix (2026-05-22) — CRITICAL DATA-LOSS BUG :
    // pre-fix, this handler did `remove_dir_all(docs/)` unconditionally
    // on every cancel. A user who re-launched an audit on a project
    // that ALREADY had legitimate audited (or hand-written) docs/
    // content would lose ALL of it on cancel. Reported live by user
    // 2026-05-22.
    //
    // New behaviour : NEVER wipe docs/ when there is prior content.
    // We detect prior content via two paths :
    //
    //   1. `.kronn.json.audits` non-empty → at least one finished
    //      audit, the directory holds legitimate audited content.
    //   2. No `.kronn.json` but the directory still has files →
    //      hand-written docs that pre-date Kronn audits. Equally
    //      sacred.
    //
    // Only when the directory is provably empty (or missing) do we
    // proceed with the legacy clean-slate logic — and even then we
    // only touch directories that the audit demonstrably populated
    // on this session.
    //
    // Redirector files (CLAUDE.md, .cursorrules, ...) are always safe
    // to remove since their content is 100% kronn-templated and
    // overwritten on every audit run anyway. The operator never edits
    // them by hand.
    let project_path_str = project.path.clone();
    let cleanup_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        let project_path = scanner::resolve_host_path(&project_path_str);
        cleanup_audit_files(&project_path)
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

// ─── 0.8.2 helpers for `audit_runs` completion ─────────────────────────

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SeverityCounts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
}

/// Scan every `TD-*.md` file in the tech-debt directory (skipping
/// scaffolding/reconciliation reports) and tally findings by severity.
///
/// The severity is matched from the line shape:
///   `- **Severity**: Critical | High | Medium | Low`
/// We accept variations in casing/spacing but only on the canonical
/// four values — anything else (e.g. an agent-emitted `Severe`) doesn't
/// count. Better to under-count than mis-categorize.
pub(crate) fn count_td_severities(td_dir: &std::path::Path) -> SeverityCounts {
    let mut counts = SeverityCounts::default();
    let Ok(entries) = std::fs::read_dir(td_dir) else { return counts };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        // Skip scaffolding + reconciliation reports — same filter as
        // `count_tech_debt` in scanner.rs to stay consistent.
        if !name.ends_with(".md")
            || name.starts_with('_')
            || matches!(name, "README.md" | "TEMPLATE.md" | "_template.md")
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        // Pick the FIRST Severity line — multiple lines in the same
        // file would be a malformed TD anyway.
        let Some(sev_line) = content.lines().find(|l| {
            let lc = l.to_ascii_lowercase();
            lc.contains("**severity**:") || lc.starts_with("severity:")
        }) else { continue };
        let sev = sev_line
            .split(':')
            .nth(1)
            .map(|s| s.trim().trim_start_matches('*').trim().to_ascii_lowercase())
            .unwrap_or_default();
        match sev.as_str() {
            s if s.starts_with("critical") => counts.critical += 1,
            s if s.starts_with("high")     => counts.high     += 1,
            s if s.starts_with("medium")   => counts.medium   += 1,
            s if s.starts_with("low")      => counts.low      += 1,
            _ => {} // unknown — skip silently
        }
    }
    counts
}

/// Reconciliation counters fed into `audit_runs`:
///   - `td_resolved_since_last` = TDs that existed before and were
///     deleted or whose source signature is gone (we approximate with
///     "file gone" — the full classification lives in the reconciliation
///     module and may bucket some Unchanged-with-signature-gone as
///     Fixed too, but the cheap proxy is good enough for the badge).
///   - `td_new_since_last` = TD files that exist now but weren't in
///     the pre-audit snapshot.
///   - `td_carried_over` = TDs present in both (Unchanged or Updated).
pub(crate) fn compute_reconciliation_counts(
    snapshot: &[super::reconciliation::TdSnapshot],
    td_dir: &std::path::Path,
) -> (u32, u32, u32) {
    use std::collections::HashSet;
    let snap_ids: HashSet<&str> = snapshot.iter().map(|s| s.id.as_str()).collect();

    let mut current_ids: HashSet<String> = HashSet::new();
    if let Ok(entries) = std::fs::read_dir(td_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.ends_with(".md")
                || name.starts_with('_')
                || matches!(name, "README.md" | "TEMPLATE.md" | "_template.md")
            {
                continue;
            }
            current_ids.insert(name.trim_end_matches(".md").to_string());
        }
    }

    let resolved = snap_ids
        .iter()
        .filter(|id| !current_ids.contains(**id))
        .count() as u32;
    let new = current_ids
        .iter()
        .filter(|id| !snap_ids.contains(id.as_str()))
        .count() as u32;
    let carried = current_ids
        .iter()
        .filter(|id| snap_ids.contains(id.as_str()))
        .count() as u32;
    (resolved, new, carried)
}

/// Bucket a TD into a specialized audit kind based on (1) an explicit
/// `**Category**: <kind>` line in the file body (written by S2.D3-D4
/// specialized audits) and (2) heuristic slug-matching on the filename
/// as a fallback for TDs produced by the Full audit (which doesn't
/// write a Category line).
///
/// Returns `None` when nothing matches — those TDs don't drive
/// progressive-disclosure recommendations.
pub(crate) fn classify_td_cluster(filename: &str, body: &str) -> Option<&'static str> {
    // 1) Explicit category line wins. Format: `- **Category**: docker`
    let lower_body = body.to_ascii_lowercase();
    for line in lower_body.lines() {
        let trimmed = line.trim().trim_start_matches('-').trim();
        if let Some(rest) = trimmed.strip_prefix("**category**:") {
            let cat = rest.trim().to_string();
            return match cat.as_str() {
                "docker"        => Some("Docker"),
                "security"      => Some("Security"),
                "performance"   => Some("Performance"),
                "accessibility" => Some("Accessibility"),
                "database"      => Some("Database"),
                "api"           => Some("ApiDesign"),
                _               => None,
            };
        }
    }
    // 2) Slug heuristics on the filename (post `TD-YYYYMMDD-`).
    let slug = filename.to_ascii_lowercase();
    let m = |needles: &[&str]| needles.iter().any(|n| slug.contains(n));
    // Note: "compose-" (hyphen suffix) on purpose — bare "compose" would
    // false-match "composer-install-no-checksum" (a supply-chain finding
    // that happens to live in a Dockerfile but isn't a Docker-class TD).
    if m(&["docker", "dockerfile", "compose-", "image-tag", "layer"]) {
        return Some("Docker");
    }
    // Pull "composer-install" / "supply-chain" / "unverified-download"
    // into Security since the impact (silently swapping the installer
    // binary) is a supply-chain risk, not an image-config issue.
    if m(&["composer-install", "supply-chain", "unverified-download"]) {
        return Some("Security");
    }
    if m(&["secret", "credential", "auth", "csrf", "xss", "sql-injection",
           "cors", "csp", "jwt", "rce", "host-key", "strict-host",
           "apikey", "api-key", "hardcoded-key", "leaked-key"]) {
        return Some("Security");
    }
    if m(&["perf", "n-plus-one", "n+1", "missing-index", "cache-",
           "bundle-size", "slow-query", "memory-leak"]) {
        return Some("Performance");
    }
    if m(&["a11y", "accessibility", "aria", "contrast",
           "keyboard", "alt-attr", "missing-alt", "wcag"]) {
        return Some("Accessibility");
    }
    if m(&["migration", "schema", "orm", "foreign-key", "missing-fk"]) {
        return Some("Database");
    }
    if m(&["api-design", "openapi", "swagger", "endpoint-shape",
           "rest-", "pagination"]) {
        return Some("ApiDesign");
    }
    None
}

/// Threshold above which a cluster of TDs in the same dimension
/// triggers a progressive-disclosure recommendation card on the
/// project dashboard. 3 is the minimum that signals \"pattern\" vs
/// \"one-off\" (a single docker finding doesn't justify launching a
/// focused Docker audit; three or more does).
pub(crate) const CLUSTER_RECOMMENDATION_THRESHOLD: u32 = 3;

/// Scan the TD directory and return per-kind cluster recommendations
/// for the audit_runs row. The JSON shape is `[{\"kind\":..., \"reason\":..., \"cluster_size\":N}]`
/// matching the `AuditRecommendation` struct in `models/projects.rs`.
pub(crate) fn compute_cluster_recommendations(td_dir: &std::path::Path) -> Vec<crate::models::AuditRecommendation> {
    use std::collections::HashMap;
    let mut counts: HashMap<&'static str, u32> = HashMap::new();
    let Ok(entries) = std::fs::read_dir(td_dir) else { return Vec::new() };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".md")
            || name.starts_with('_')
            || matches!(name, "README.md" | "TEMPLATE.md" | "_template.md")
        {
            continue;
        }
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(kind) = classify_td_cluster(name, &body) {
            *counts.entry(kind).or_insert(0) += 1;
        }
    }
    let mut recs: Vec<crate::models::AuditRecommendation> = counts
        .into_iter()
        .filter(|(_, n)| *n >= CLUSTER_RECOMMENDATION_THRESHOLD)
        .map(|(kind, n)| crate::models::AuditRecommendation {
            kind: kind.to_string(),
            reason: format!(
                "{} {}-related findings detected — a focused audit will catch siblings the Full pass missed.",
                n, kind.to_lowercase()
            ),
            // u32 → u8 safe: clusters that big are absurd; saturate to 255.
            cluster_size: n.min(u8::MAX as u32) as u8,
        })
        .collect();
    // Stable order: largest cluster first so the UI surfaces the most
    // impactful recommendation at the top of the card.
    recs.sort_by(|a, b| b.cluster_size.cmp(&a.cluster_size).then_with(|| a.kind.cmp(&b.kind)));
    recs
}

#[cfg(test)]
mod cluster_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn classify_uses_explicit_category_line_first() {
        // Even if the filename SAYS "docker", an explicit Category: security wins.
        let kind = classify_td_cluster(
            "TD-20260512-docker-base-image.md",
            "# Foo\n- **Category**: security\n- **Severity**: High\n",
        );
        assert_eq!(kind, Some("Security"));
    }

    #[test]
    fn classify_falls_back_to_slug_when_no_category_line() {
        assert_eq!(classify_td_cluster("TD-20260512-docker-no-user.md", ""), Some("Docker"));
        assert_eq!(classify_td_cluster("TD-20260512-cors-wildcard.md", ""), Some("Security"));
        assert_eq!(classify_td_cluster("TD-20260512-n-plus-one-orders.md", ""), Some("Performance"));
        assert_eq!(classify_td_cluster("TD-20260512-missing-alt-attr.md", ""), Some("Accessibility"));
    }

    #[test]
    fn apikey_in_template_classifies_as_security() {
        // Real DOCROMS_WEB TD: TD-20260512-here-maps-apikey-in-template.
        // Before the fix, "apikey" wasn't in the Security heuristic, so
        // the cluster detector ignored a legit secret-leak finding.
        assert_eq!(
            classify_td_cluster("TD-20260512-here-maps-apikey-in-template.md", ""),
            Some("Security"),
        );
        assert_eq!(
            classify_td_cluster("TD-20260512-aws-hardcoded-key.md", ""),
            Some("Security"),
        );
    }

    #[test]
    fn composer_install_is_security_not_docker() {
        // Regression: bare "compose" substring matched "composer-install-no-checksum"
        // on DOCROMS_WEB 2026-05-12 audit (false-positive Docker cluster of 5).
        // The fix uses "compose-" with hyphen suffix + a dedicated supply-chain
        // bucket. composer-install belongs to Security (supply-chain).
        assert_eq!(
            classify_td_cluster("TD-20260512-composer-install-no-checksum.md", ""),
            Some("Security"),
        );
        // Real compose findings still match Docker.
        assert_eq!(
            classify_td_cluster("TD-20260512-compose-no-resource-limits.md", ""),
            Some("Docker"),
        );
    }

    #[test]
    fn classify_returns_none_for_unmatched() {
        assert_eq!(classify_td_cluster("TD-20260512-misc-bug.md", ""), None);
        assert_eq!(classify_td_cluster("TD-20260512-spelling-mistake.md", ""), None);
    }

    #[test]
    fn cluster_below_threshold_yields_no_recommendation() {
        let dir = tempdir().unwrap();
        // 2 docker TDs — under the threshold of 3.
        fs::write(dir.path().join("TD-20260512-docker-1.md"), "x").unwrap();
        fs::write(dir.path().join("TD-20260512-docker-2.md"), "x").unwrap();
        let recs = compute_cluster_recommendations(dir.path());
        assert!(recs.is_empty(), "2 hits is below the 3-cluster threshold");
    }

    // ─── 0.8.6 phase 4 hotfix — cancel_audit data-loss guard ──────
    //
    // Pre-fix, `cancel_audit` blindly `remove_dir_all(docs/)` on every
    // cancel call. A user who re-launched an audit on a project with
    // legitimate prior docs (audited or hand-written) lost all that
    // content on cancel — reported live by user 2026-05-22.
    //
    // These tests pin the safety rules of the new
    // `classify_docs_dir_for_cancel` helper. A regression here means
    // the cancel handler is back to data-destruction territory ; the
    // bugs they prevent are NEAR-IMPOSSIBLE to recover from once they
    // ship (operator's local git might not have committed the docs/
    // changes yet).

    #[test]
    fn classify_docs_preserves_when_prior_audit_recorded() {
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();
        // Record a finished audit. This is the exact scenario the user
        // hit : a successful audit, then a relaunch + cancel.
        crate::core::kronn_state::record_audit(project, "full").unwrap();

        let action = classify_docs_dir_for_cancel(project, &docs);
        assert_eq!(
            action,
            DocsCancelAction::Preserve {
                reason: "prior audit recorded in .kronn.json"
            },
        );
    }

    #[test]
    fn classify_docs_preserves_when_user_content_present() {
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();
        // Hand-written file — no `.kronn.json`, no prior audit, but
        // the user wrote their own notes there. Wiping would be
        // catastrophic.
        fs::write(docs.join("AGENTS.md"), "# My agent\nHand-written content").unwrap();

        let action = classify_docs_dir_for_cancel(project, &docs);
        match action {
            DocsCancelAction::Preserve { reason } => {
                assert!(reason.contains("user content"), "got reason: {}", reason);
            }
            DocsCancelAction::Wipe => panic!("MUST preserve user content"),
        }
    }

    #[test]
    fn classify_docs_preserves_when_subdir_present() {
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(docs.join("tech-debts")).unwrap();
        // A sub-directory counts as user content — could be Kronn's
        // tech-debts/ from a prior audit OR something the operator
        // dropped manually. Either way, hands off.

        let action = classify_docs_dir_for_cancel(project, &docs);
        match action {
            DocsCancelAction::Preserve { reason: _ } => {}
            DocsCancelAction::Wipe => panic!("MUST preserve a dir holding a subdir"),
        }
    }

    #[test]
    fn classify_docs_wipes_when_truly_empty() {
        // ONLY case where the wipe proceeds : the directory exists but
        // contains nothing at all. This is the original "greenfield
        // audit aborted before writing a single file" case.
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();

        let action = classify_docs_dir_for_cancel(project, &docs);
        assert_eq!(action, DocsCancelAction::Wipe);
    }

    #[test]
    fn classify_docs_wipes_when_only_state_file_with_zero_audits() {
        // Defensive : a fresh `.kronn.json` was written but no audit
        // ever completed (e.g. a previous run-and-cancel cycle). The
        // state file alone is not user content — we can wipe.
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();
        // Bare state file with no audits.
        let state = crate::core::kronn_state::KronnState::default();
        let json = serde_json::to_string(&state).unwrap();
        fs::write(docs.join(".kronn.json"), json).unwrap();

        let action = classify_docs_dir_for_cancel(project, &docs);
        assert_eq!(action, DocsCancelAction::Wipe);
    }

    #[test]
    fn classify_docs_prior_audit_beats_empty_dir() {
        // Edge case : `.kronn.json` records prior audits even though
        // the actual files have been deleted by hand. We still
        // preserve — the state file is the historical record and a
        // surprise wipe would clear it too.
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();
        crate::core::kronn_state::record_audit(project, "full").unwrap();
        // Remove every file in docs/ except .kronn.json.
        for entry in fs::read_dir(&docs).unwrap().filter_map(|e| e.ok()) {
            let n = entry.file_name();
            if n.to_string_lossy() != ".kronn.json" {
                let _ = fs::remove_file(entry.path());
            }
        }

        let action = classify_docs_dir_for_cancel(project, &docs);
        match action {
            DocsCancelAction::Preserve { reason } => {
                assert!(reason.contains("prior audit"), "got: {}", reason);
            }
            DocsCancelAction::Wipe => panic!("prior audit must win over empty dir"),
        }
    }

    // ─── 0.8.6 phase 4 audit gap #4 (2026-05-22) — cleanup_audit_files
    //     end-to-end safety, including the "root redirectors NOT deleted"
    //     contract that the unit-fn `classify_docs_dir_for_cancel` doesn't
    //     itself cover. ──────────────────────────────────────────────

    #[test]
    fn cleanup_preserves_root_redirectors_unconditionally() {
        // CRITICAL : a regression that re-adds the redirector wipe loop
        // would silently delete CLAUDE.md / AGENTS.md / .cursorrules on
        // every cancel — same data-loss bug we just fixed. This test
        // pins the "redirectors are NEVER touched" contract from
        // cleanup_audit_files.
        let dir = tempdir().unwrap();
        let project = dir.path();
        // Hand-written root redirectors with valuable content.
        fs::write(
            project.join("CLAUDE.md"),
            "# My personal Claude context\nHand-written rules, do not delete",
        ).unwrap();
        fs::write(project.join("AGENTS.md"), "# Vendor-neutral agent context").unwrap();
        fs::write(project.join(".cursorrules"), "personal cursor config").unwrap();
        fs::write(project.join(".windsurfrules"), "windsurf config").unwrap();
        fs::write(project.join(".clinerules"), "cline config").unwrap();

        // Run the cleanup (cancel-audit equivalent).
        cleanup_audit_files(project).expect("cleanup succeeded on empty project");

        // Every root file MUST still exist. Failure = data loss.
        for filename in ["CLAUDE.md", "AGENTS.md", ".cursorrules", ".windsurfrules", ".clinerules"] {
            let file = project.join(filename);
            assert!(
                file.exists(),
                "Root redirector {} was deleted ! Data loss. {}",
                filename,
                "Reverting to the pre-hotfix behaviour is a critical regression.",
            );
        }
        // Content untouched too.
        let claude_after = fs::read_to_string(project.join("CLAUDE.md")).unwrap();
        assert!(claude_after.contains("Hand-written rules"));
    }

    #[test]
    fn cleanup_preserves_docs_with_user_content_and_root_redirectors() {
        // Belt-and-braces : the user case from 2026-05-22. Re-launching
        // + cancelling on a project with both docs/ content AND root
        // redirectors. Nothing should be deleted.
        let dir = tempdir().unwrap();
        let project = dir.path();
        fs::create_dir_all(project.join("docs")).unwrap();
        fs::write(project.join("docs/AGENTS.md"), "# Project context").unwrap();
        fs::write(project.join("docs/architecture.md"), "# Arch overview").unwrap();
        crate::core::kronn_state::record_audit(project, "full").unwrap();
        fs::write(project.join("CLAUDE.md"), "redirector").unwrap();

        cleanup_audit_files(project).unwrap();

        // docs/ + all its content survives.
        assert!(project.join("docs").is_dir());
        assert!(project.join("docs/AGENTS.md").exists());
        assert!(project.join("docs/architecture.md").exists());
        // .kronn.json survives.
        assert!(project.join("docs/.kronn.json").exists());
        // Root redirector survives.
        assert!(project.join("CLAUDE.md").exists());
    }

    #[test]
    fn cleanup_does_wipe_genuinely_empty_docs_dir() {
        // Inverse of preservation : an audit that created docs/ and
        // crashed before writing anything (or was cancelled mid-creation)
        // leaves an empty dir. THIS we still clean up — original intent
        // of the wipe loop, kept intact.
        let dir = tempdir().unwrap();
        let project = dir.path();
        fs::create_dir_all(project.join("docs")).unwrap();
        // No content, no .kronn.json.

        cleanup_audit_files(project).unwrap();

        assert!(!project.join("docs").exists(), "Empty docs/ should be removed on cancel");
    }

    #[test]
    fn cleanup_handles_missing_project_path() {
        // Defensive : project path doesn't exist (deleted between
        // cancel-request and cleanup). Returns Err, doesn't panic.
        let result = cleanup_audit_files(std::path::Path::new("/nonexistent/totally-fake-path"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn classify_docs_preserves_with_both_signals_active() {
        // Belt-and-braces case : both prior audit AND user content.
        // Either signal alone is enough ; both together must still
        // preserve. Tests the OR semantic.
        let dir = tempdir().unwrap();
        let project = dir.path();
        let docs = project.join("docs");
        fs::create_dir_all(&docs).unwrap();
        crate::core::kronn_state::record_audit(project, "full").unwrap();
        fs::write(docs.join("AGENTS.md"), "...").unwrap();

        let action = classify_docs_dir_for_cancel(project, &docs);
        match action {
            DocsCancelAction::Preserve { reason: _ } => {}
            DocsCancelAction::Wipe => panic!("OR semantic — any signal preserves"),
        }
    }

    #[test]
    fn cluster_at_threshold_surfaces_recommendation() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("TD-20260512-docker-1.md"), "x").unwrap();
        fs::write(dir.path().join("TD-20260512-docker-2.md"), "x").unwrap();
        fs::write(dir.path().join("TD-20260512-docker-no-user.md"), "x").unwrap();
        let recs = compute_cluster_recommendations(dir.path());
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].kind, "Docker");
        assert_eq!(recs[0].cluster_size, 3);
    }

    #[test]
    fn multiple_clusters_sorted_by_size_descending() {
        let dir = tempdir().unwrap();
        // 5 security, 3 docker, 1 perf
        for i in 0..5 { fs::write(dir.path().join(format!("TD-20260512-jwt-{i}.md")), "x").unwrap(); }
        for i in 0..3 { fs::write(dir.path().join(format!("TD-20260512-docker-{i}.md")), "x").unwrap(); }
        fs::write(dir.path().join("TD-20260512-perf-single.md"), "x").unwrap();

        let recs = compute_cluster_recommendations(dir.path());
        let kinds: Vec<&str> = recs.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, vec!["Security", "Docker"],
            "perf cluster of 1 is below threshold; security (5) should outrank docker (3)");
    }

    #[test]
    fn underscore_and_readme_files_are_skipped() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("_reconciliation-20260512.md"), "x").unwrap();
        fs::write(dir.path().join("README.md"), "x").unwrap();
        fs::write(dir.path().join("TEMPLATE.md"), "x").unwrap();
        fs::write(dir.path().join("TD-20260512-docker-1.md"), "x").unwrap();
        let recs = compute_cluster_recommendations(dir.path());
        assert!(recs.is_empty(),
            "only 1 real docker TD after filter, below threshold");
    }

    // ── Extra coverage on category-line + slug classification ──────────

    #[test]
    fn classify_explicit_category_database() {
        let body = "# X\n- **Category**: database\n- **Severity**: High\n";
        assert_eq!(classify_td_cluster("TD-X.md", body), Some("Database"));
    }

    #[test]
    fn classify_explicit_category_api() {
        let body = "# X\n- **Category**: api\n";
        assert_eq!(classify_td_cluster("TD-X.md", body), Some("ApiDesign"));
    }

    #[test]
    fn classify_explicit_category_performance() {
        let body = "- **Category**: performance\n";
        assert_eq!(classify_td_cluster("TD-X.md", body), Some("Performance"));
    }

    #[test]
    fn classify_explicit_category_accessibility() {
        let body = "- **Category**: accessibility\n";
        assert_eq!(classify_td_cluster("TD-X.md", body), Some("Accessibility"));
    }

    #[test]
    fn classify_explicit_category_unknown_yields_none() {
        // Explicit category that's not in the canonical set → None.
        let body = "- **Category**: refactoring\n";
        assert_eq!(classify_td_cluster("TD-X.md", body), None);
    }

    #[test]
    fn classify_slug_database_keywords() {
        for slug in &[
            "TD-20260528-migration-missing-rollback.md",
            "TD-20260528-orm-n1.md",
            "TD-20260528-schema-drift.md",
            "TD-20260528-missing-fk-cascade.md",
            "TD-20260528-foreign-key-stale.md",
        ] {
            assert_eq!(classify_td_cluster(slug, ""), Some("Database"),
                "slug {slug} should be Database");
        }
    }

    #[test]
    fn classify_slug_api_design_keywords() {
        for slug in &[
            "TD-X-openapi-spec-missing.md",
            "TD-X-swagger-out-of-sync.md",
            "TD-X-endpoint-shape-mismatch.md",
            "TD-X-rest-handler-untyped.md",
            "TD-X-pagination-leak.md",
        ] {
            assert_eq!(classify_td_cluster(slug, ""), Some("ApiDesign"),
                "slug {slug} should be ApiDesign");
        }
    }

    #[test]
    fn classify_slug_security_keywords_comprehensive() {
        for slug in &[
            "TD-X-secret-leak.md",
            "TD-X-credential-in-yml.md",
            "TD-X-csrf-bypass.md",
            "TD-X-xss-in-template.md",
            "TD-X-sql-injection-orm.md",
            "TD-X-cors-wildcard.md",
            "TD-X-csp-missing.md",
            "TD-X-jwt-no-expiry.md",
            "TD-X-rce-eval.md",
            "TD-X-host-key-hardcoded.md",
            "TD-X-strict-host-disabled.md",
            "TD-X-apikey-in-template.md",
            "TD-X-api-key-frontend.md",
            "TD-X-hardcoded-key-prod.md",
            "TD-X-leaked-key-git.md",
        ] {
            assert_eq!(classify_td_cluster(slug, ""), Some("Security"),
                "slug {slug} should be Security");
        }
    }

    #[test]
    fn classify_slug_perf_keywords_comprehensive() {
        for slug in &[
            "TD-X-perf-hotspot.md",
            "TD-X-n-plus-one-orders.md",
            "TD-X-cache-stampede.md",
            "TD-X-bundle-size-blow.md",
            "TD-X-slow-query-orders.md",
            "TD-X-memory-leak-loop.md",
        ] {
            assert_eq!(classify_td_cluster(slug, ""), Some("Performance"),
                "slug {slug} should be Performance");
        }
    }

    #[test]
    fn classify_slug_a11y_keywords_comprehensive() {
        for slug in &[
            "TD-X-a11y-form.md",
            "TD-X-aria-missing-role.md",
            "TD-X-contrast-warn-too-low.md",
            "TD-X-keyboard-trap.md",
            "TD-X-missing-alt.md",
            "TD-X-wcag-violation.md",
        ] {
            assert_eq!(classify_td_cluster(slug, ""), Some("Accessibility"),
                "slug {slug} should be Accessibility");
        }
    }

    #[test]
    fn classify_explicit_category_overrides_slug() {
        // Slug says docker but explicit Category says security → security wins.
        let body = "- **Category**: security\n";
        assert_eq!(classify_td_cluster("TD-20260528-docker-image-no-scan.md", body), Some("Security"));
    }

    #[test]
    fn classify_unknown_slug_returns_none() {
        // No keyword match → None (not a cluster candidate).
        assert!(classify_td_cluster("TD-X-random-naming-issue.md", "").is_none());
        assert!(classify_td_cluster("TD-X-misc.md", "").is_none());
    }

    #[test]
    fn cluster_recommendations_skip_underscore_files() {
        let dir = tempdir().unwrap();
        for i in 0..5 {
            fs::write(dir.path().join(format!("_internal-{i}.md")), "").unwrap();
        }
        let recs = compute_cluster_recommendations(dir.path());
        assert!(recs.is_empty(), "underscore-prefixed files must all be skipped");
    }

    #[test]
    fn cluster_recommendations_skip_template_aliases() {
        let dir = tempdir().unwrap();
        for name in &["README.md", "TEMPLATE.md", "_template.md"] {
            fs::write(dir.path().join(name), "TD-like body").unwrap();
        }
        let recs = compute_cluster_recommendations(dir.path());
        assert!(recs.is_empty(), "scaffolding aliases must be excluded");
    }

    #[test]
    fn cluster_recommendations_skip_non_md_files() {
        let dir = tempdir().unwrap();
        // 5 docker findings but in .txt — must not count.
        for i in 0..5 {
            fs::write(dir.path().join(format!("TD-docker-{i}.txt")), "").unwrap();
        }
        let recs = compute_cluster_recommendations(dir.path());
        assert!(recs.is_empty(), "non-.md files should be ignored");
    }

    #[test]
    fn cluster_recommendations_secondary_sort_is_alphabetical_on_tie() {
        // 3 docker + 3 security → both pass threshold, sorted by name when
        // cluster_size matches.
        let dir = tempdir().unwrap();
        for i in 0..3 {
            fs::write(dir.path().join(format!("TD-X-docker-{i}.md")), "").unwrap();
            fs::write(dir.path().join(format!("TD-Y-secret-{i}.md")), "").unwrap();
        }
        let recs = compute_cluster_recommendations(dir.path());
        assert_eq!(recs.len(), 2);
        // Both have cluster_size=3 → alphabetical: Docker < Security.
        assert_eq!(recs[0].kind, "Docker");
        assert_eq!(recs[1].kind, "Security");
    }

    #[test]
    fn cluster_recommendations_empty_dir_returns_empty() {
        let dir = tempdir().unwrap();
        let recs = compute_cluster_recommendations(dir.path());
        assert!(recs.is_empty());
    }

    #[test]
    fn cluster_recommendations_missing_dir_returns_empty() {
        let recs = compute_cluster_recommendations(std::path::Path::new("/does/not/exist"));
        assert!(recs.is_empty());
    }
}

#[cfg(test)]
mod severity_tests {
    use super::*;

    #[test]
    fn count_td_severities_tallies_canonical_values() {
        let tmp = std::env::temp_dir().join("kronn-test-sev-count");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("TD-001.md"),
            "# X\n- **Severity**: Critical\n",
        ).unwrap();
        std::fs::write(
            tmp.join("TD-002.md"),
            "# Y\n- **Severity**: High\n",
        ).unwrap();
        std::fs::write(
            tmp.join("TD-003.md"),
            "# Z\n- **Severity**: high\n", // lowercase still counts
        ).unwrap();
        std::fs::write(
            tmp.join("TD-004.md"),
            "# W\n- **Severity**:    Medium\n",
        ).unwrap();
        std::fs::write(
            tmp.join("TD-005.md"),
            "# V\n- **Severity**: Low\n",
        ).unwrap();
        std::fs::write(
            tmp.join("TD-bad.md"),
            "# Bad\n- **Severity**: Severe\n", // unknown — must NOT count
        ).unwrap();
        std::fs::write(tmp.join("README.md"), "scaffolding").unwrap();
        std::fs::write(tmp.join("_reconciliation-2026-01-01.md"), "skip me").unwrap();

        let counts = count_td_severities(&tmp);
        assert_eq!(counts.critical, 1);
        assert_eq!(counts.high, 2);
        assert_eq!(counts.medium, 1);
        assert_eq!(counts.low, 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn count_td_severities_zero_on_missing_dir() {
        let counts = count_td_severities(std::path::Path::new("/does/not/exist"));
        assert_eq!(counts.critical, 0);
        assert_eq!(counts.high, 0);
        assert_eq!(counts.medium, 0);
        assert_eq!(counts.low, 0);
    }

    #[test]
    fn reconciliation_counts_compute_resolved_new_carried() {
        use super::super::reconciliation::TdSnapshot;
        use std::path::PathBuf;

        let tmp = std::env::temp_dir().join("kronn-test-recon-counts");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Pre-audit snapshot: 3 TDs (A, B, C).
        let snapshot = vec![
            TdSnapshot { path: PathBuf::from("/x/TD-A.md"), id: "TD-A".into(), content_hash: "h".into(), mtime: chrono::Utc::now() },
            TdSnapshot { path: PathBuf::from("/x/TD-B.md"), id: "TD-B".into(), content_hash: "h".into(), mtime: chrono::Utc::now() },
            TdSnapshot { path: PathBuf::from("/x/TD-C.md"), id: "TD-C".into(), content_hash: "h".into(), mtime: chrono::Utc::now() },
        ];
        // Post-audit state: A is gone (resolved), B is kept (carried),
        // C is kept (carried), D is brand new (new), E too (new).
        std::fs::write(tmp.join("TD-B.md"), "x").unwrap();
        std::fs::write(tmp.join("TD-C.md"), "x").unwrap();
        std::fs::write(tmp.join("TD-D.md"), "x").unwrap();
        std::fs::write(tmp.join("TD-E.md"), "x").unwrap();
        // Scaffolding must not count anywhere.
        std::fs::write(tmp.join("README.md"), "x").unwrap();
        std::fs::write(tmp.join("_reconciliation-2026-05-13.md"), "x").unwrap();

        let (resolved, new, carried) = compute_reconciliation_counts(&snapshot, &tmp);
        assert_eq!(resolved, 1, "A is gone");
        assert_eq!(new, 2, "D + E are new");
        assert_eq!(carried, 2, "B + C carry over");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
