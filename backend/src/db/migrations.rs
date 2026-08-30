use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Run all migrations in order. Each migration is idempotent.
/// If `db_path` points to an existing file and there are pending migrations,
/// a backup is created at `<db_path>.backup` before applying them.
pub fn run(conn: &Connection) -> Result<()> {
    run_with_backup(conn, None)
}

/// All schema migrations in order (name → SQL). Extracted as a module const so
/// test helpers (`run_through`) can build an older schema, seed pre-migration
/// data, then run the remaining migrations through the real path.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001_initial", include_str!("sql/001_initial.sql")),
    ("002_mcp_redesign", include_str!("sql/002_mcp_redesign.sql")),
    ("003_workflows", include_str!("sql/003_workflows.sql")),
    (
        "004_token_tracking",
        include_str!("sql/004_token_tracking.sql"),
    ),
    (
        "005_discussion_archive",
        include_str!("sql/005_discussion_archive.sql"),
    ),
    (
        "006_discussion_skills",
        include_str!("sql/006_discussion_skills.sql"),
    ),
    (
        "007_project_skills",
        include_str!("sql/007_project_skills.sql"),
    ),
    (
        "008_discussions_index",
        include_str!("sql/008_discussions_index.sql"),
    ),
    ("009_profiles", include_str!("sql/009_profiles.sql")),
    ("010_directives", include_str!("sql/010_directives.sql")),
    (
        "011_multi_profiles",
        include_str!("sql/011_multi_profiles.sql"),
    ),
    ("012_mcp_general", include_str!("sql/012_mcp_general.sql")),
    (
        "013_discussion_worktrees",
        include_str!("sql/013_discussion_worktrees.sql"),
    ),
    (
        "014_summary_cache",
        include_str!("sql/014_summary_cache.sql"),
    ),
    ("015_model_tier", include_str!("sql/015_model_tier.sql")),
    (
        "016_message_model_tier",
        include_str!("sql/016_message_model_tier.sql"),
    ),
    (
        "017_message_count",
        include_str!("sql/017_message_count.sql"),
    ),
    (
        "018_briefing_notes",
        include_str!("sql/018_briefing_notes.sql"),
    ),
    (
        "019_pin_first_message",
        include_str!("sql/019_pin_first_message.sql"),
    ),
    (
        "020_fix_worktree_paths",
        include_str!("sql/020_fix_worktree_paths.sql"),
    ),
    (
        "021_message_identity",
        include_str!("sql/021_message_identity.sql"),
    ),
    ("022_contacts", include_str!("sql/022_contacts.sql")),
    (
        "023_shared_discussions",
        include_str!("sql/023_shared_discussions.sql"),
    ),
    ("024_message_cost", include_str!("sql/024_message_cost.sql")),
    (
        "025_context_files",
        include_str!("sql/025_context_files.sql"),
    ),
    // 026: idempotent column addition (handled below, not via SQL file)
    (
        "027_quick_prompts",
        include_str!("sql/026_quick_prompts.sql"),
    ),
    (
        "028_quick_prompt_descriptions",
        include_str!("sql/027_quick_prompt_descriptions.sql"),
    ),
    (
        "029_batch_workflow_runs",
        include_str!("sql/028_batch_workflow_runs.sql"),
    ),
    (
        "030_workflow_run_parent",
        include_str!("sql/030_workflow_run_parent.sql"),
    ),
    (
        "031_partial_response",
        include_str!("sql/031_partial_response.sql"),
    ),
    (
        "032_partial_response_started_at",
        include_str!("sql/032_partial_response_started_at.sql"),
    ),
    (
        "033_discussion_pinned",
        include_str!("sql/033_discussion_pinned.sql"),
    ),
    (
        "034_test_mode_fields",
        include_str!("sql/034_test_mode_fields.sql"),
    ),
    (
        "035_mcp_server_api_spec",
        include_str!("sql/035_mcp_server_api_spec.sql"),
    ),
    (
        "036_mcp_host_sync",
        include_str!("sql/036_mcp_host_sync.sql"),
    ),
    (
        "037_mcp_host_sync_backfill",
        include_str!("sql/037_mcp_host_sync_backfill.sql"),
    ),
    (
        "038_mcp_host_sync_collapse",
        include_str!("sql/038_mcp_host_sync_collapse.sql"),
    ),
    (
        "039_workflow_guards",
        include_str!("sql/039_workflow_guards.sql"),
    ),
    (
        "040_workflow_artifacts",
        include_str!("sql/040_workflow_artifacts.sql"),
    ),
    (
        "041_workflow_on_failure",
        include_str!("sql/041_workflow_on_failure.sql"),
    ),
    (
        "042_workflow_run_state",
        include_str!("sql/042_workflow_run_state.sql"),
    ),
    (
        "043_workflow_exec_allowlist",
        include_str!("sql/043_workflow_exec_allowlist.sql"),
    ),
    (
        "044_workflow_variables",
        include_str!("sql/044_workflow_variables.sql"),
    ),
    ("045_quick_apis", include_str!("sql/045_quick_apis.sql")),
    (
        "046_workflow_run_produced_branches",
        include_str!("sql/046_workflow_run_produced_branches.sql"),
    ),
    (
        "047_discussion_summary_strategy",
        include_str!("sql/047_discussion_summary_strategy.sql"),
    ),
    (
        "048_disc_summary_ranges",
        include_str!("sql/048_disc_summary_ranges.sql"),
    ),
    (
        "049_introspection_call_count",
        include_str!("sql/049_introspection_call_count.sql"),
    ),
    ("050_audit_runs", include_str!("sql/050_audit_runs.sql")),
    (
        "051_agent_decisions",
        include_str!("sql/051_agent_decisions.sql"),
    ),
    (
        "052_project_linked_repos",
        include_str!("sql/052_project_linked_repos.sql"),
    ),
    (
        "053_audit_runs_last_completed_step",
        include_str!("sql/053_audit_runs_last_completed_step.sql"),
    ),
    (
        "054_cross_agent_memory",
        include_str!("sql/054_cross_agent_memory.sql"),
    ),
    (
        "055_audit_run_steps",
        include_str!("sql/055_audit_run_steps.sql"),
    ),
    (
        "056_qp_qa_profile_directive_binding",
        include_str!("sql/056_qp_qa_profile_directive_binding.sql"),
    ),
    (
        "057_message_duration",
        include_str!("sql/057_message_duration.sql"),
    ),
    (
        "058_qp_versions_and_lineage",
        include_str!("sql/058_qp_versions_and_lineage.sql"),
    ),
    (
        "059_qp_versions_backfill",
        include_str!("sql/059_qp_versions_backfill.sql"),
    ),
    (
        "060_discussion_sessions",
        include_str!("sql/060_discussion_sessions.sql"),
    ),
    (
        "061_api_call_logs",
        include_str!("sql/061_api_call_logs.sql"),
    ),
    (
        "062_message_lint_report",
        include_str!("sql/062_message_lint_report.sql"),
    ),
    (
        "063_continual_learning",
        include_str!("sql/063_continual_learning.sql"),
    ),
    (
        "064_discussion_session_last_seen",
        include_str!("sql/064_discussion_session_last_seen.sql"),
    ),
    (
        "065_reap_abandoned_sessions",
        include_str!("sql/065_reap_abandoned_sessions.sql"),
    ),
    (
        "066_context_files_message_id",
        include_str!("sql/066_context_files_message_id.sql"),
    ),
    (
        "067_context_files_backfill_legacy",
        include_str!("sql/067_context_files_backfill_legacy.sql"),
    ),
    (
        "068_shared_id_unique",
        include_str!("sql/068_shared_id_unique.sql"),
    ),
    (
        "069_disc_no_agent",
        include_str!("sql/069_disc_no_agent.sql"),
    ),
    (
        "070_agent_model_override",
        include_str!("sql/070_agent_model_override.sql"),
    ),
    (
        "071_message_model",
        include_str!("sql/071_message_model.sql"),
    ),
    (
        "072_message_received_at",
        include_str!("sql/072_message_received_at.sql"),
    ),
    (
        "073_session_activity",
        include_str!("sql/073_session_activity.sql"),
    ),
    (
        "074_disc_awaiting_agent",
        include_str!("sql/074_disc_awaiting_agent.sql"),
    ),
    (
        "075_workflows_pinned",
        include_str!("sql/075_workflows_pinned.sql"),
    ),
    (
        "076_audit_runs_validation_link",
        include_str!("sql/076_audit_runs_validation_link.sql"),
    ),
    (
        "077_discussion_session_resume",
        include_str!("sql/077_discussion_session_resume.sql"),
    ),
    (
        "078_normalize_sqlite_datetimes",
        include_str!("sql/078_normalize_sqlite_datetimes.sql"),
    ),
    (
        "079_discussion_agent_handoff",
        include_str!("sql/079_discussion_agent_handoff.sql"),
    ),
    (
        "080_project_source_exclusions",
        include_str!("sql/080_project_source_exclusions.sql"),
    ),
    (
        "081_planning_tasks",
        include_str!("sql/081_planning_tasks.sql"),
    ),
    (
        "082_message_sequence",
        include_str!("sql/082_message_sequence.sql"),
    ),
    (
        "083_agent_dispatch_jobs",
        include_str!("sql/083_agent_dispatch_jobs.sql"),
    ),
    (
        "084_agent_dispatch_pending_queue",
        include_str!("sql/084_agent_dispatch_pending_queue.sql"),
    ),
    (
        "085_message_revisions",
        include_str!("sql/085_message_revisions.sql"),
    ),
    (
        "086_session_presence_honesty",
        include_str!("sql/086_session_presence_honesty.sql"),
    ),
    (
        "087_planning_proposals",
        include_str!("sql/087_planning_proposals.sql"),
    ),
    (
        "088_proposal_decision_idempotency",
        include_str!("sql/088_proposal_decision_idempotency.sql"),
    ),
    (
        "089_agent_model_provenance",
        include_str!("sql/089_agent_model_provenance.sql"),
    ),
    (
        "090_message_target_agent",
        include_str!("sql/090_message_target_agent.sql"),
    ),
    (
        "091_mcp_preferred_interface",
        include_str!("sql/091_mcp_preferred_interface.sql"),
    ),
    (
        "092_disc_source_binding_contract",
        include_str!("sql/092_disc_source_binding_contract.sql"),
    ),
    (
        "093_discussion_imports",
        include_str!("sql/093_discussion_imports.sql"),
    ),
    (
        "094_plugin_bundle_audit",
        include_str!("sql/094_plugin_bundle_audit.sql"),
    ),
    (
        "095_message_replies",
        include_str!("sql/095_message_replies.sql"),
    ),
    (
        "096_import_provenance",
        include_str!("sql/096_import_provenance.sql"),
    ),
    (
        "097_discussion_session_conversation_id",
        include_str!("sql/097_discussion_session_conversation_id.sql"),
    ),
    (
        "098_message_targets",
        include_str!("sql/098_message_targets.sql"),
    ),
    (
        "099_typed_message_targets",
        include_str!("sql/099_typed_message_targets.sql"),
    ),
    (
        "100_message_cli_authors",
        include_str!("sql/100_message_cli_authors.sql"),
    ),
    (
        "101_discussion_workspaces",
        include_str!("sql/101_discussion_workspaces.sql"),
    ),
    (
        "102_planning_task_idempotency",
        include_str!("sql/102_planning_task_idempotency.sql"),
    ),
    (
        "103_project_dependency_monitoring",
        include_str!("sql/103_project_dependency_monitoring.sql"),
    ),
    (
        "104_message_channels",
        include_str!("sql/104_message_channels.sql"),
    ),
    (
        "105_user_turn_catchup",
        include_str!("sql/105_user_turn_catchup.sql"),
    ),
    (
        "106_awareness_offered_cursor",
        include_str!("sql/106_awareness_offered_cursor.sql"),
    ),
    (
        "107_workflow_step_ids",
        include_str!("sql/107_workflow_step_ids.sql"),
    ),
    (
        "108_lite_llm_model_failures",
        include_str!("sql/108_lite_llm_model_failures.sql"),
    ),
    (
        "109_agent_reply_turn_links",
        include_str!("sql/109_agent_reply_turn_links.sql"),
    ),
    (
        "110_agent_handoff_guard",
        include_str!("sql/110_agent_handoff_guard.sql"),
    ),
    (
        "111_agent_handoff_discussion_unlimited",
        include_str!("sql/111_agent_handoff_discussion_unlimited.sql"),
    ),
    (
        "112_message_target_tiers",
        include_str!("sql/112_message_target_tiers.sql"),
    ),
    (
        "113_session_alias_ordinal",
        include_str!("sql/113_session_alias_ordinal.sql"),
    ),
    (
        "114_awareness_stalled_offers",
        include_str!("sql/114_awareness_stalled_offers.sql"),
    ),
    (
        "115_partial_response_message_id",
        include_str!("sql/115_partial_response_message_id.sql"),
    ),
    (
        "116_cli_session_telemetry",
        include_str!("sql/116_cli_session_telemetry.sql"),
    ),
    (
        "117_message_session_tokens",
        include_str!("sql/117_message_session_tokens.sql"),
    ),
    (
        "118_review_ledger",
        include_str!("sql/118_review_ledger.sql"),
    ),
    (
        "119_quick_exec_runs",
        include_str!("sql/119_quick_exec_runs.sql"),
    ),
    (
        "120_agent_dispatch_started_at",
        include_str!("sql/120_agent_dispatch_started_at.sql"),
    ),
    (
        "121_discussion_workspace_history_leases",
        include_str!("sql/121_discussion_workspace_history_leases.sql"),
    ),
    (
        "122_context_audit_snapshots",
        include_str!("sql/122_context_audit_snapshots.sql"),
    ),
    ("123_live_pages", include_str!("sql/123_live_pages.sql")),
    (
        "124_live_page_library",
        include_str!("sql/124_live_page_library.sql"),
    ),
    (
        "125_live_page_publication_changes",
        include_str!("sql/125_live_page_publication_changes.sql"),
    ),
    ("126_quick_execs", include_str!("sql/126_quick_execs.sql")),
    (
        "127_task_orchestration",
        include_str!("sql/127_task_orchestration.sql"),
    ),
    (
        "128_task_execution_delivery",
        include_str!("sql/128_task_execution_delivery.sql"),
    ),
    (
        "129_orchestration_self_review",
        include_str!("sql/129_orchestration_self_review.sql"),
    ),
    (
        "130_batch_compare_evaluations",
        include_str!("sql/130_batch_compare_evaluations.sql"),
    ),
    (
        "131_batch_compare_prompt_review",
        include_str!("sql/131_batch_compare_prompt_review.sql"),
    ),
    (
        "132_ad_hoc_compare_runs",
        include_str!("sql/132_ad_hoc_compare_runs.sql"),
    ),
    (
        "133_batch_no_response",
        include_str!("sql/133_batch_no_response.sql"),
    ),
    (
        "134_orchestration_campaign_policy",
        include_str!("sql/134_orchestration_campaign_policy.sql"),
    ),
    (
        "135_orchestration_resilience",
        include_str!("sql/135_orchestration_resilience.sql"),
    ),
    (
        "136_planning_actor_session",
        include_str!("sql/136_planning_actor_session.sql"),
    ),
    (
        "137_agent_resume_jobs",
        include_str!("sql/137_agent_resume_jobs.sql"),
    ),
    (
        "138_agent_dispatch_failure_state",
        include_str!("sql/138_agent_dispatch_failure_state.sql"),
    ),
    (
        "139_task_execution_actor_session_repair",
        include_str!("sql/139_task_execution_actor_session_repair.sql"),
    ),
    (
        "140_task_execution_active_blocker_cleanup",
        include_str!("sql/140_task_execution_active_blocker_cleanup.sql"),
    ),
    (
        "141_task_execution_worker_scope",
        include_str!("sql/141_task_execution_worker_scope.sql"),
    ),
    (
        "142_task_execution_worker_dod_snapshot",
        include_str!("sql/142_task_execution_worker_dod_snapshot.sql"),
    ),
    (
        "143_quick_items_pinned",
        include_str!("sql/143_quick_items_pinned.sql"),
    ),
    (
        "144_external_api_connections",
        include_str!("sql/144_external_api_connections.sql"),
    ),
    (
        "145_task_execution_worker_connection",
        include_str!("sql/145_task_execution_worker_connection.sql"),
    ),
    (
        "146_message_target_connection",
        include_str!("sql/146_message_target_connection.sql"),
    ),
    (
        "147_task_execution_commit_leases",
        include_str!("sql/147_task_execution_commit_leases.sql"),
    ),
    (
        "148_task_execution_commit_lease_liveness",
        include_str!("sql/148_task_execution_commit_lease_liveness.sql"),
    ),
    (
        "149_task_execution_progress",
        include_str!("sql/149_task_execution_progress.sql"),
    ),
    (
        "150_project_mcp_sync_report",
        include_str!("sql/150_project_mcp_sync_report.sql"),
    ),
    (
        "151_agent_dispatch_connection",
        include_str!("sql/151_agent_dispatch_connection.sql"),
    ),
    (
        "152_external_api_openrouter_preset",
        include_str!("sql/152_external_api_openrouter_preset.sql"),
    ),
    (
        "153_quick_prompt_external_connection",
        include_str!("sql/153_quick_prompt_external_connection.sql"),
    ),
];

/// Apply one migration inside the caller-owned transaction.
///
/// Migration 136 originally shipped on the 0.11 development branch with only
/// the `planning_task_events` column. Adding the second ALTER to that already
/// receipted file did not repair existing development databases. Migration 139
/// is therefore deliberately implemented as a schema-aware forward repair:
/// fresh databases already have both columns, while historical partial ones
/// receive only the missing column. Keeping the inspection and ALTER inside
/// the migration transaction makes its receipt truthful after a crash.
fn apply_migration(tx: &rusqlite::Transaction<'_>, name: &str, sql: &str) -> Result<()> {
    tx.execute_batch(sql)?;
    if name == "139_task_execution_actor_session_repair" {
        ensure_actor_session_columns(tx)?;
    }
    if name == "152_external_api_openrouter_preset" {
        ensure_openrouter_preset(tx)?;
    }
    Ok(())
}

fn ensure_openrouter_preset(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    const OLD_CHECK: &str = "origin_preset IN ('litellm', 'nvidia', 'other')";
    const NEW_CHECK: &str = "origin_preset IN ('litellm', 'nvidia', 'open_router', 'other')";

    let create_sql: String = tx.query_row(
        "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'external_api_connections'",
        [],
        |row| row.get(0),
    )?;
    if create_sql.contains(NEW_CHECK) {
        return Ok(());
    }
    if !create_sql.contains(OLD_CHECK) {
        return Err(anyhow::anyhow!(
            "external_api_connections has an unknown origin_preset constraint"
        ));
    }

    // SQLite cannot ALTER a CHECK constraint. Rebuilding this parent table is
    // unsafe once message/dispatch rows reference it: DROP TABLE performs an
    // implicit delete and the migration fails at commit. Updating only the
    // stored CREATE statement preserves the table root page, rows, indexes and
    // foreign-key identity. The exact old fragment above is required before we
    // enable writable_schema, so an unexpected schema is never rewritten.
    let updated_sql = create_sql.replacen(OLD_CHECK, NEW_CHECK, 1);
    let schema_version: i64 = tx.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    tx.execute_batch("PRAGMA writable_schema = ON;")?;
    let update_result = tx.execute(
        "UPDATE sqlite_schema SET sql = ?1 WHERE type = 'table' AND name = 'external_api_connections'",
        [&updated_sql],
    );
    let reset_result = tx.execute_batch("PRAGMA writable_schema = OFF;");
    let changed = update_result?;
    reset_result?;
    if changed != 1 {
        return Err(anyhow::anyhow!(
            "external_api_connections schema repair updated {changed} rows"
        ));
    }
    tx.pragma_update(None, "schema_version", schema_version + 1)?;

    let integrity: String = tx.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(anyhow::anyhow!(
            "database integrity check failed after OpenRouter schema repair: {integrity}"
        ));
    }
    Ok(())
}

fn ensure_actor_session_columns(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    // These identifiers are compile-time constants, never caller input.
    for table in ["planning_task_events", "task_execution_events"] {
        let query = format!(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') \
             WHERE name = 'actor_session_id')"
        );
        let exists: bool = tx.query_row(&query, [], |row| row.get(0))?;
        if !exists {
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN actor_session_id TEXT;"
            ))?;
        }
    }
    Ok(())
}

// These migrations shipped on the 0.9.6 development branch before its rebase
// onto 0.9.5. Main had meanwhile claimed 107–112, so the SQL files had to move
// without making databases created by the earlier branch replay the same SQL.
const RENAMED_MIGRATIONS: &[(&str, &str)] = &[
    ("107_session_alias_ordinal", "113_session_alias_ordinal"),
    (
        "108_awareness_stalled_offers",
        "114_awareness_stalled_offers",
    ),
    (
        "109_partial_response_message_id",
        "115_partial_response_message_id",
    ),
    ("110_cli_session_telemetry", "116_cli_session_telemetry"),
    ("111_message_session_tokens", "117_message_session_tokens"),
    ("112_review_ledger", "118_review_ledger"),
    ("113_quick_exec_runs", "119_quick_exec_runs"),
];

fn reconcile_renamed_migrations(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (legacy_name, current_name) in RENAMED_MIGRATIONS {
        tx.execute(
            "INSERT INTO _migrations (name, applied_at)
             SELECT ?2, applied_at
               FROM _migrations
              WHERE name = ?1
                AND NOT EXISTS (
                    SELECT 1 FROM _migrations WHERE name = ?2
                )
              LIMIT 1",
            [legacy_name, current_name],
        )?;
    }
    tx.commit()?;
    Ok(())
}

fn migration_is_applied(conn: &Connection, current_name: &str) -> Result<bool> {
    let current_applied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
        [current_name],
        |row| row.get(0),
    )?;
    if current_applied {
        return Ok(true);
    }

    let Some((legacy_name, _)) = RENAMED_MIGRATIONS
        .iter()
        .find(|(_, renamed)| *renamed == current_name)
    else {
        return Ok(false);
    };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
        [legacy_name],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// Run all migrations, optionally backing up the database file first.
pub fn run_with_backup(conn: &Connection, db_path: Option<&Path>) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    let migrations: &[(&str, &str)] = MIGRATIONS;

    // Check if there are pending migrations before backing up
    if let Some(path) = db_path {
        if path.exists() {
            let has_pending = migrations
                .iter()
                .any(|(name, _)| !migration_is_applied(conn, name).unwrap_or(false));
            if has_pending {
                // Fold the WAL back into the main db file FIRST, so a plain
                // file copy is a consistent snapshot. Without this, recent
                // writes live only in `<db>-wal` and the backup is stale/torn
                // (it would omit everything since the last checkpoint).
                if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
                    tracing::warn!(
                        "WAL checkpoint before backup failed (backup may be stale): {}",
                        e
                    );
                }
                let backup_path = path.with_extension("db.backup");
                if let Err(e) = std::fs::copy(path, &backup_path) {
                    tracing::warn!("Failed to backup database before migration: {}", e);
                } else {
                    tracing::info!("Database backed up to {}", backup_path.display());
                }
                // Also snapshot config.toml (co-located in the data dir) — it
                // holds auth_token + other config a bad migration/crash could
                // strand. Best-effort; absence is fine (Docker/env configs).
                if let Some(dir) = path.parent() {
                    let cfg = dir.join("config.toml");
                    if cfg.exists() {
                        let cfg_backup = dir.join("config.toml.backup");
                        if let Err(e) = std::fs::copy(&cfg, &cfg_backup) {
                            tracing::warn!("Failed to backup config.toml before migration: {}", e);
                        }
                    }
                }
            }
        }
    }

    reconcile_renamed_migrations(conn)?;

    for (name, sql) in migrations {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;

        if !already_applied {
            tracing::info!("Running migration: {}", name);
            // Apply the migration SQL and its bookkeeping row ATOMICALLY. A crash
            // (or an error) mid-migration must not leave the schema changed
            // without the `_migrations` row — that would re-run non-idempotent
            // SQL on the next boot and brick startup. `unchecked_transaction`
            // works on `&Connection`; on any error the tx drops → rollback, so
            // the DB is left exactly as before this migration (restore from
            // `<db>.db.backup` if the failure is a corrupt file, not just SQL).
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| anyhow::anyhow!("begin tx for migration {name}: {e}"))?;
            apply_migration(&tx, name, sql)
                .map_err(|e| anyhow::anyhow!("migration {name} failed and was rolled back: {e}"))?;
            tx.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
            tx.commit()
                .map_err(|e| anyhow::anyhow!("commit migration {name}: {e}"))?;
        }
    }

    // Idempotent schema fixups (safe to run multiple times, handles upgrades from
    // older 025 that didn't include disk_path)
    let _ = conn.execute_batch("ALTER TABLE context_files ADD COLUMN disk_path TEXT;");

    Ok(())
}

/// Test-only: apply migrations up to AND INCLUDING `stop_after` (by name), so a
/// test can build an older schema, seed pre-migration data, then run the
/// remaining migrations through [`run`]. Panics on an unknown migration name.
#[cfg(test)]
pub(crate) fn run_through(conn: &Connection, stop_after: &str) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    let stop_idx = MIGRATIONS
        .iter()
        .position(|(name, _)| *name == stop_after)
        .unwrap_or_else(|| panic!("run_through: unknown migration name `{stop_after}`"));
    for (name, sql) in &MIGRATIONS[..=stop_idx] {
        let already_applied: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;
        if !already_applied {
            let tx = conn.unchecked_transaction()?;
            apply_migration(&tx, name, sql)?;
            tx.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])?;
            tx.commit()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_with_backup_creates_backup_file() {
        // Create a temp directory and a SQLite file with some data
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Create and populate the database, then close the connection
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, val TEXT);")
                .unwrap();
            conn.execute("INSERT INTO t(val) VALUES (?1)", ["hello"])
                .unwrap();
        }

        // Open a new connection and run migrations (which will create a backup)
        let conn = Connection::open(&db_path).unwrap();
        run_with_backup(&conn, Some(&db_path)).expect("run_with_backup should succeed");

        // Verify the backup file was created
        let backup_path = db_path.with_extension("db.backup");
        assert!(
            backup_path.exists(),
            "Backup file should exist at {:?}",
            backup_path
        );

        // Verify the original file still exists
        assert!(
            db_path.exists(),
            "Original database file should still exist"
        );

        // Verify the backup contains valid data by opening it as a SQLite DB
        let backup_conn = Connection::open(&backup_path).unwrap();
        let val: String = backup_conn
            .query_row("SELECT val FROM t WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(val, "hello", "Backup database should contain original data");

        // Verify the original database still has our data (migrations don't destroy it)
        let val: String = conn
            .query_row("SELECT val FROM t WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(val, "hello");
    }

    #[test]
    fn run_with_backup_no_backup_when_no_path() {
        // When db_path is None, no backup should be attempted (in-memory DB)
        let conn = Connection::open_in_memory().unwrap();
        run_with_backup(&conn, None).expect("run_with_backup with None path should succeed");
        // No assertion on files — just ensure it doesn't panic
    }

    #[test]
    fn live_page_publication_changes_backfill_existing_ledger_rows() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "124_live_page_library").unwrap();
        conn.execute(
            "INSERT INTO live_pages
             (id, title, slug, data_revision, created_at, updated_at)
             VALUES ('page-old', 'Old Page', 'old-page', 1, '2026-08-14T08:00:00Z',
                     '2026-08-14T08:01:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO live_page_publications
             (id, page_id, data_revision, datasets_json, published_at)
             VALUES ('publication-old', 'page-old', 1, '[\"summary\",\"traffic\"]',
                     '2026-08-14T08:01:00Z')",
            [],
        )
        .unwrap();

        run(&conn).unwrap();

        let (changed, unchanged): (String, String) = conn
            .query_row(
                "SELECT changed_datasets_json, unchanged_datasets_json
                   FROM live_page_publications WHERE id = 'publication-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(changed, "[\"summary\",\"traffic\"]");
        assert_eq!(unchanged, "[]");
    }

    #[test]
    fn renamed_development_migrations_are_not_replayed() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "112_message_target_tiers").unwrap();

        let historical_migrations = [
            (
                "107_session_alias_ordinal",
                include_str!("sql/113_session_alias_ordinal.sql"),
            ),
            (
                "108_awareness_stalled_offers",
                include_str!("sql/114_awareness_stalled_offers.sql"),
            ),
            (
                "109_partial_response_message_id",
                include_str!("sql/115_partial_response_message_id.sql"),
            ),
            (
                "110_cli_session_telemetry",
                include_str!("sql/116_cli_session_telemetry.sql"),
            ),
            (
                "111_message_session_tokens",
                include_str!("sql/117_message_session_tokens.sql"),
            ),
            (
                "112_review_ledger",
                include_str!("sql/118_review_ledger.sql"),
            ),
            (
                "113_quick_exec_runs",
                include_str!("sql/119_quick_exec_runs.sql"),
            ),
        ];
        for (name, sql) in historical_migrations {
            conn.execute_batch(sql).unwrap();
            conn.execute("INSERT INTO _migrations (name) VALUES (?1)", [name])
                .unwrap();
        }

        run(&conn).expect("renumbered migrations must be recognized as already applied");

        for (_, current_name) in RENAMED_MIGRATIONS {
            let applied: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
                    [current_name],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(applied, "missing migration receipt for {current_name}");
        }
        let alias_column_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('discussion_sessions')
                 WHERE name = 'alias_ordinal'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias_column_count, 1);
    }

    #[test]
    fn preferred_interface_migration_backfills_api_and_mcp_configs() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "089_agent_model_provenance").unwrap();
        conn.execute_batch(
            "INSERT INTO mcp_servers
                (id, name, transport, source, api_spec_json)
             VALUES
                ('api', 'API', 'api_only', 'registry', '{}'),
                ('mcp', 'MCP', 'stdio', 'registry', NULL);
             INSERT INTO mcp_configs (id, server_id, label)
             VALUES ('cfg-api', 'api', 'API'), ('cfg-mcp', 'mcp', 'MCP');",
        )
        .unwrap();

        run(&conn).unwrap();

        let api: String = conn
            .query_row(
                "SELECT preferred_interface FROM mcp_configs WHERE id = 'cfg-api'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mcp: String = conn
            .query_row(
                "SELECT preferred_interface FROM mcp_configs WHERE id = 'cfg-mcp'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(api, "api");
        assert_eq!(mcp, "mcp");
        assert!(conn
            .execute(
                "UPDATE mcp_configs SET preferred_interface = 'invalid' WHERE id = 'cfg-mcp'",
                [],
            )
            .is_err());
    }

    #[test]
    fn sqlite_datetimes_are_normalized_once_to_rfc3339() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p-dt', 'Dates', '/tmp/dates', ?1, ?1)",
            ["2026-07-07 19:11:11"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('d-dt', 'Dates', ?1, ?1)",
            ["2026-07-07 19:11:11"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp)
             VALUES ('m-dt', 'd-dt', 'User', 'hello', ?1)",
            ["2026-07-07 19:11:11"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflows
             (id, name, trigger_json, steps_json, actions_json, safety_json,
              enabled, created_at, updated_at)
             VALUES ('w-dt', 'Dates', '{\"type\":\"Manual\"}', '[]', '[]', '{}',
                     1, ?1, ?1)",
            ["2026-07-07 19:11:11"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflow_runs
             (id, workflow_id, status, step_results_json, tokens_used,
              started_at, finished_at)
             VALUES ('r-dt', 'w-dt', 'Success', '[]', 0, ?1, ?1)",
            ["2026-07-07 19:11:11"],
        )
        .unwrap();

        conn.execute(
            "DELETE FROM _migrations WHERE name = '078_normalize_sqlite_datetimes'",
            [],
        )
        .unwrap();
        run(&conn).unwrap();

        for (table, column, id) in [
            ("projects", "created_at", "p-dt"),
            ("projects", "updated_at", "p-dt"),
            ("discussions", "created_at", "d-dt"),
            ("discussions", "updated_at", "d-dt"),
            ("messages", "timestamp", "m-dt"),
            ("workflows", "created_at", "w-dt"),
            ("workflows", "updated_at", "w-dt"),
            ("workflow_runs", "started_at", "r-dt"),
            ("workflow_runs", "finished_at", "r-dt"),
        ] {
            let sql = format!("SELECT {column} FROM {table} WHERE id = ?1");
            let value: String = conn.query_row(&sql, [id], |row| row.get(0)).unwrap();
            assert_eq!(value, "2026-07-07T19:11:11Z", "{table}.{column}");
        }
    }

    /// A migration that fails mid-way must be ATOMIC: neither its schema change
    /// nor its `_migrations` bookkeeping row may persist. Proves the per-migration
    /// transaction rolls back on error (was: partial schema → boot brick).
    #[test]
    fn failing_migration_rolls_back_atomically() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE _migrations (id INTEGER PRIMARY KEY, name TEXT NOT NULL, \
             applied_at TEXT NOT NULL DEFAULT (datetime('now')));",
        )
        .unwrap();

        // A batch that creates a table THEN runs invalid SQL — the CREATE must be
        // rolled back with the failure, and no _migrations row written.
        let bad = "CREATE TABLE half_applied(x INTEGER); THIS IS NOT SQL;";
        let tx = conn.unchecked_transaction().unwrap();
        let res = tx.execute_batch(bad);
        assert!(res.is_err(), "the invalid batch must error");
        drop(tx); // rollback

        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='half_applied'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !table_exists,
            "the CREATE must have been rolled back, not left half-applied"
        );
    }

    #[test]
    fn actor_session_forward_repair_upgrades_a_receipted_partial_136() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "135_orchestration_resilience").unwrap();

        // Reproduce the exact historical development state: migration 136 had
        // added the planning column and recorded its receipt before the task
        // execution ALTER was appended to the already-shipped SQL file.
        conn.execute_batch(
            "ALTER TABLE planning_task_events ADD COLUMN actor_session_id TEXT;
             INSERT INTO _migrations (name) VALUES ('136_planning_actor_session');",
        )
        .unwrap();

        let column_count = |table: &str| -> i64 {
            conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM pragma_table_info('{table}') \
                     WHERE name = 'actor_session_id'"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(column_count("planning_task_events"), 1);
        assert_eq!(column_count("task_execution_events"), 0);

        run(&conn).unwrap();

        assert_eq!(column_count("planning_task_events"), 1);
        assert_eq!(column_count("task_execution_events"), 1);
        let repair_receipt: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM _migrations \
                 WHERE name = '139_task_execution_actor_session_repair')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            repair_receipt,
            "the forward repair must be durably receipted"
        );

        // A second startup is a no-op, not a duplicate-column boot failure.
        run(&conn).unwrap();
        assert_eq!(column_count("task_execution_events"), 1);
    }

    #[test]
    fn actor_session_forward_repair_is_a_noop_on_a_fresh_database() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();
        run(&conn).unwrap();

        for table in ["planning_task_events", "task_execution_events"] {
            let count: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('{table}') \
                         WHERE name = 'actor_session_id'"
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{table} must contain exactly one repaired column");
        }
    }

    #[test]
    fn active_blocker_cleanup_repairs_only_non_live_holds() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "139_task_execution_actor_session_repair").unwrap();

        // This migration-level test exercises only the state projection repaired
        // by migration 140; parent aggregates are deliberately out of scope.
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute_batch(
            "INSERT INTO task_executions (
                 id, orchestration_run_id, task_id, parent_discussion_id, status,
                 blocked_from_status, interrupted_from_status, blocked_reason,
                 blocked_reason_code, created_at, updated_at
             ) VALUES
             ('working-stale', 'run-w', 'task-w', 'disc-w', 'Working',
              'Provisioning', NULL, 'awaiting_worker_acceptance',
              'awaiting_worker_acceptance', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
             ('done-stale', 'run-d', 'task-d', 'disc-d', 'Done',
              'Provisioning', NULL, 'awaiting_worker_acceptance',
              'awaiting_worker_acceptance', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
             ('blocked-live', 'run-b', 'task-b', 'disc-b', 'Blocked',
              'Provisioning', NULL, 'awaiting_worker_acceptance',
              'awaiting_worker_acceptance', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
             ('interrupted-live', 'run-i', 'task-i', 'disc-i', 'Interrupted',
              'Provisioning', 'Blocked', 'awaiting_worker_acceptance',
              'awaiting_worker_acceptance', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
             ('interrupted-stale', 'run-is', 'task-is', 'disc-is', 'Interrupted',
              'Provisioning', 'Working', 'awaiting_worker_acceptance',
              'awaiting_worker_acceptance', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
             ('interrupted-corrupt', 'run-ic', 'task-ic', 'disc-ic', 'Interrupted',
              'Provisioning', NULL, 'awaiting_worker_acceptance',
              'awaiting_worker_acceptance', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
        )
        .unwrap();

        run(&conn).unwrap();
        let blocker = |id: &str| -> (Option<String>, Option<String>, Option<String>) {
            conn.query_row(
                "SELECT blocked_from_status, blocked_reason, blocked_reason_code \
                 FROM task_executions WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };

        for id in [
            "working-stale",
            "done-stale",
            "interrupted-stale",
            "interrupted-corrupt",
        ] {
            assert_eq!(blocker(id), (None, None, None), "{id} must be repaired");
        }
        let live = (
            Some("Provisioning".to_string()),
            Some("awaiting_worker_acceptance".to_string()),
            Some("awaiting_worker_acceptance".to_string()),
        );
        assert_eq!(blocker("blocked-live"), live);
        assert_eq!(blocker("interrupted-live"), live);
    }

    #[test]
    fn message_sequence_migration_backfills_and_enforces_uniqueness() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE discussions (id TEXT PRIMARY KEY);
             CREATE TABLE messages (
                 id TEXT PRIMARY KEY,
                 discussion_id TEXT NOT NULL,
                 sort_order INTEGER NOT NULL
             );
             INSERT INTO discussions (id) VALUES ('d-old'), ('d-empty');
             INSERT INTO messages (id, discussion_id, sort_order)
             VALUES ('m1', 'd-old', 2), ('m2', 'd-old', 5);",
        )
        .unwrap();

        conn.execute_batch(include_str!("sql/082_message_sequence.sql"))
            .unwrap();

        let old_next: i64 = conn
            .query_row(
                "SELECT next_message_seq FROM discussions WHERE id = 'd-old'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let empty_next: i64 = conn
            .query_row(
                "SELECT next_message_seq FROM discussions WHERE id = 'd-empty'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_next, 6);
        assert_eq!(empty_next, 1);

        let duplicate = conn.execute(
            "INSERT INTO messages (id, discussion_id, sort_order)
             VALUES ('m3', 'd-old', 5)",
            [],
        );
        assert!(
            duplicate.is_err(),
            "sort_order must be unique per discussion"
        );
    }

    #[test]
    fn agent_dispatch_pending_queue_migration_repairs_the_legacy_active_index() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_dispatch_jobs (
                 id TEXT PRIMARY KEY,
                 discussion_id TEXT NOT NULL,
                 status TEXT NOT NULL
             );
             CREATE UNIQUE INDEX idx_agent_dispatch_one_active_discussion
             ON agent_dispatch_jobs(discussion_id)
             WHERE status IN ('Pending', 'Running');
             INSERT INTO agent_dispatch_jobs (id, discussion_id, status)
             VALUES ('pending-1', 'disc-1', 'Pending');",
        )
        .unwrap();

        conn.execute_batch(include_str!("sql/084_agent_dispatch_pending_queue.sql"))
            .unwrap();

        conn.execute(
            "INSERT INTO agent_dispatch_jobs (id, discussion_id, status)
             VALUES ('pending-2', 'disc-1', 'Pending')",
            [],
        )
        .expect("multiple pending turns must queue for the same discussion");
        conn.execute(
            "INSERT INTO agent_dispatch_jobs (id, discussion_id, status)
             VALUES ('running-1', 'disc-1', 'Running')",
            [],
        )
        .unwrap();
        let second_running = conn.execute(
            "INSERT INTO agent_dispatch_jobs (id, discussion_id, status)
             VALUES ('running-2', 'disc-1', 'Running')",
            [],
        );
        assert!(
            second_running.is_err(),
            "only one worker may hold a Running claim per discussion"
        );

        let legacy_index_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index'
                       AND name = 'idx_agent_dispatch_one_active_discussion'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!legacy_index_exists);
    }

    #[test]
    fn message_replies_migration_registers_column_and_lookup_index() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let has_column: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('messages')
                     WHERE name = 'reply_to_message_id'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let has_index: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_messages_reply_to'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(has_column);
        assert!(has_index);
    }

    #[test]
    fn message_channels_migration_registers_column_and_lookup_index() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let has_column: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('messages')
                     WHERE name = 'channel'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let has_index: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index' AND name = 'idx_messages_discussion_channel_sort'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(has_column);
        assert!(has_index);
    }

    #[test]
    fn discussion_session_conversation_id_migration_adds_nullable_column() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let has_nullable_column: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM pragma_table_info('discussion_sessions')
                     WHERE name = 'conversation_id' AND \"notnull\" = 0
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(has_nullable_column);
    }

    #[test]
    fn typed_message_targets_upgrade_an_already_applied_plural_schema() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "098_message_targets").unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO message_targets (message_id, agent_type, position)
             VALUES ('legacy-message', 'Codex', 0);",
        )
        .unwrap();

        run(&conn).unwrap();

        let upgraded: (String, String, Option<i64>, i64) = conn
            .query_row(
                "SELECT target_kind, agent_type, cli_session_id, position
                 FROM message_targets
                 WHERE message_id = 'legacy-message'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            upgraded,
            ("agent".into(), "Codex".into(), None, 0),
            "099 must preserve 098 rows while assigning their legacy punctual-agent identity"
        );
    }

    #[test]
    fn message_target_tiers_upgrade_existing_typed_rows_as_inherited() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "111_agent_handoff_discussion_unlimited").unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO message_targets (
                 message_id, target_kind, agent_type, cli_session_id, position
             ) VALUES ('legacy-target', 'agent', 'Codex', NULL, 0);",
        )
        .unwrap();

        run(&conn).unwrap();

        let inherited: Option<String> = conn
            .query_row(
                "SELECT model_tier FROM message_targets
                 WHERE message_id = 'legacy-target'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(inherited, None);
        assert!(conn
            .execute(
                "UPDATE message_targets SET model_tier = 'ultra'
                 WHERE message_id = 'legacy-target'",
                [],
            )
            .is_err());
    }

    #[test]
    fn message_cli_authors_migration_adds_durable_exact_session_provenance() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let has_table: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'message_cli_authors'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let has_index: bool = conn
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM sqlite_master
                     WHERE type = 'index'
                       AND name = 'idx_message_cli_authors_session'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert!(has_table);
        assert!(has_index);
    }

    #[test]
    fn discussion_workspaces_migration_backfills_attached_and_unlocked_legacy_rows() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "100_message_cli_authors").unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('p-ws', 'Workspace', '/repo', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discussions (
                 id, project_id, title, created_at, updated_at, workspace_mode,
                 workspace_path, worktree_branch
             ) VALUES
                 ('d-attached', 'p-ws', 'Attached', '2026-01-01', '2026-01-02',
                  'Isolated', '/repo/.kronn/worktrees/attached', 'kronn/attached'),
                 ('d-detached', 'p-ws', 'Detached', '2026-01-01', '2026-01-03',
                  'Isolated', NULL, 'kronn/detached')",
            [],
        )
        .unwrap();

        run(&conn).unwrap();

        let attached: (String, String, Option<String>) = conn
            .query_row(
                "SELECT ownership, state, canonical_path
                   FROM discussion_workspaces WHERE disc_id = 'd-attached'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            attached,
            (
                "managed".into(),
                "attached".into(),
                Some("/repo/.kronn/worktrees/attached".into())
            )
        );

        let detached: (String, Option<String>) = conn
            .query_row(
                "SELECT state, workspace_path
                   FROM discussion_workspaces WHERE disc_id = 'd-detached'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(detached, ("detached".into(), None));
    }

    #[test]
    fn workflow_step_ids_migration_backfills_durable_distinct_uuids() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "106_awareness_offered_cursor").unwrap();
        let preserved = "11111111-2222-4333-8444-555555555555";
        conn.execute(
            "INSERT INTO workflows (
                 id, name, trigger_json, steps_json, actions_json,
                 created_at, updated_at, on_failure
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
            rusqlite::params![
                "wf-step-ids",
                "Legacy workflow",
                "\"Manual\"",
                format!(r#"[{{"name":"fetch"}},{{"id":"{preserved}","name":"reshape"}}]"#),
                "[]",
                "2026-08-09T00:00:00Z",
                r#"[{"name":"notify_failure"}]"#,
            ],
        )
        .unwrap();

        run(&conn).unwrap();

        let (steps_json, failure_json): (String, String) = conn
            .query_row(
                "SELECT steps_json, on_failure FROM workflows WHERE id = ?1",
                ["wf-step-ids"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let steps: serde_json::Value = serde_json::from_str(&steps_json).unwrap();
        let failures: serde_json::Value = serde_json::from_str(&failure_json).unwrap();
        let fetch_id = steps[0]["id"].as_str().unwrap();
        let reshape_id = steps[1]["id"].as_str().unwrap();
        let failure_id = failures[0]["id"].as_str().unwrap();

        assert!(uuid::Uuid::parse_str(fetch_id).is_ok());
        assert_eq!(reshape_id, preserved);
        assert!(uuid::Uuid::parse_str(failure_id).is_ok());
        assert_ne!(fetch_id, reshape_id);
        assert_ne!(fetch_id, failure_id);
        assert_eq!(steps[0]["name"], "fetch");
        assert_eq!(steps[1]["name"], "reshape");
    }

    #[test]
    fn agent_reply_turn_links_backfill_the_dispatch_trigger() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "108_lite_llm_model_failures").unwrap();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('d-turns', 'Turns', '2026-08-10T00:00:00Z', '2026-08-10T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO messages (
                 id, discussion_id, role, content, timestamp, sort_order
             ) VALUES
                 ('u-1', 'd-turns', 'User', 'first', '2026-08-10T00:00:00Z', 0),
                 ('u-2', 'd-turns', 'User', 'second', '2026-08-10T00:00:01Z', 1);
             INSERT INTO agent_dispatch_jobs (
                 id, discussion_id, trigger_message_id, trigger_sort_order,
                 dedupe_key, status, available_at, created_at, updated_at
             ) VALUES (
                 'job-1', 'd-turns', 'u-1', 0, 'turn-link-job', 'Completed',
                 '2026-08-10T00:00:00Z', '2026-08-10T00:00:00Z', '2026-08-10T00:00:02Z'
             );
             INSERT INTO messages (
                 id, discussion_id, role, content, agent_type, timestamp,
                 sort_order, agent_dispatch_job_id
             ) VALUES (
                 'a-late', 'd-turns', 'Agent', 'late reply', 'Ollama',
                 '2026-08-10T00:00:02Z', 2, 'job-1'
             );",
        )
        .unwrap();

        run(&conn).unwrap();

        let reply_to: Option<String> = conn
            .query_row(
                "SELECT reply_to_message_id FROM messages WHERE id = 'a-late'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reply_to.as_deref(), Some("u-1"));
    }

    #[test]
    fn task_orchestration_migration_widens_actor_kind_preserving_events() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "126_quick_execs").unwrap();
        conn.execute(
            "INSERT INTO planning_tasks (id, task_number, title, created_at, updated_at)
             VALUES ('t-old', 1, 'Legacy', '2026-08-16T00:00:00Z', '2026-08-16T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO planning_task_events
                 (id, task_id, action, actor_kind, actor_id, changes_json, created_at)
             VALUES ('ev-old', 't-old', 'created', 'agent', 'Codex', '{}', '2026-08-16T00:01:00Z')",
            [],
        )
        .unwrap();

        run(&conn).unwrap();

        // The pre-existing event survived the table rebuild verbatim.
        let (task_id, actor_kind, actor_id): (String, String, String) = conn
            .query_row(
                "SELECT task_id, actor_kind, actor_id FROM planning_task_events WHERE id = 'ev-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (task_id.as_str(), actor_kind.as_str(), actor_id.as_str()),
            ("t-old", "agent", "Codex")
        );

        // The widened CHECK now admits a backend-attributed event…
        conn.execute(
            "INSERT INTO planning_task_events
                 (id, task_id, action, actor_kind, changes_json, created_at)
             VALUES ('ev-backend', 't-old', 'closed', 'backend', '{}', '2026-08-16T00:02:00Z')",
            [],
        )
        .expect("backend actor must be accepted after 127");
        // …but an unknown kind is still rejected.
        assert!(conn
            .execute(
                "INSERT INTO planning_task_events
                     (id, task_id, action, actor_kind, changes_json, created_at)
                 VALUES ('ev-bad', 't-old', 'x', 'martian', '{}', '2026-08-16T00:03:00Z')",
                [],
            )
            .is_err());

        // The new orchestration tables exist and the one-active-per-task index is present.
        let index_present: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index'
                 AND name='idx_task_executions_one_active_per_task')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(index_present);
    }

    #[test]
    fn quick_item_favorites_migration_keeps_existing_rows_unpinned() {
        let conn = Connection::open_in_memory().unwrap();
        run_through(&conn, "142_task_execution_worker_dod_snapshot").unwrap();
        conn.execute_batch(
            "INSERT INTO quick_prompts
                 (id, name, prompt_template, created_at, updated_at)
             VALUES ('qp-old', 'Prompt', 'Body', '2026-08-26T00:00:00Z', '2026-08-26T00:00:00Z');
             INSERT INTO quick_apis
                 (id, name, api_plugin_slug, api_config_id, api_endpoint_path,
                  created_at, updated_at)
             VALUES ('qa-old', 'API', 'demo', 'cfg', '/items',
                     '2026-08-26T00:00:00Z', '2026-08-26T00:00:00Z');
             INSERT INTO quick_execs
                 (id, name, command, created_at, updated_at)
             VALUES ('qe-old', 'Exec', 'echo', '2026-08-26T00:00:00Z', '2026-08-26T00:00:00Z');",
        )
        .unwrap();

        run(&conn).unwrap();

        for (table, id) in [
            ("quick_prompts", "qp-old"),
            ("quick_apis", "qa-old"),
            ("quick_execs", "qe-old"),
        ] {
            let pinned: i64 = conn
                .query_row(
                    &format!("SELECT pinned FROM {table} WHERE id = ?1"),
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(pinned, 0, "{table} legacy row became favorite");
        }
    }

    /// Release gate for 0.11.0: model a persistent 0.10.0 database by applying
    /// every migration through 126, close it, then reopen it through the exact
    /// production backup + migration path. This is intentionally broader than
    /// the migration-127 unit test above: it proves the upgrade receipts and
    /// operator rollback snapshot on a real file.
    #[test]
    fn persistent_0_10_database_upgrades_with_backup_and_preserves_planning_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("kronn.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            run_through(&conn, "126_quick_execs").unwrap();
            conn.execute(
                "INSERT INTO planning_tasks
                     (id, task_number, title, description, status, priority, created_at, updated_at)
                 VALUES
                     ('release-upgrade-task', 11, 'Preserve me', '0.10 data',
                      'in_progress', 'high', '2026-08-14T00:00:00Z', '2026-08-14T00:00:00Z')",
                [],
            )
            .unwrap();
        }

        let conn = Connection::open(&db_path).unwrap();
        run_with_backup(&conn, Some(&db_path)).unwrap();

        let preserved: (String, String, String) = conn
            .query_row(
                "SELECT title, description, status FROM planning_tasks
                 WHERE id = 'release-upgrade-task'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                "Preserve me".to_string(),
                "0.10 data".to_string(),
                "in_progress".to_string(),
            )
        );
        for migration in [
            "127_task_orchestration",
            "128_task_execution_delivery",
            "129_orchestration_self_review",
            "134_orchestration_campaign_policy",
        ] {
            let applied: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
                    [migration],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(applied, "missing post-0.10 migration receipt: {migration}");
        }
        let backup_path = db_path.with_extension("db.backup");
        assert!(
            backup_path.exists(),
            "pre-upgrade rollback snapshot is missing"
        );
        let backup = Connection::open(backup_path).unwrap();
        let orchestration_applied: bool = backup
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = '127_task_orchestration')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !orchestration_applied,
            "backup must remain at the 0.10 schema boundary"
        );
    }

    /// Release gate for 0.12.0: upgrade the last 0.11 schema boundary through
    /// migrations 144-153 using the production backup path, while retaining a
    /// representative project -> discussion -> plan -> execution lineage and a
    /// Quick Prompt created before named external connections existed.
    #[test]
    fn persistent_0_11_database_upgrades_through_153_and_preserves_core_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("kronn.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            run_through(&conn, "143_quick_items_pinned").unwrap();
            conn.execute_batch(
                "INSERT INTO projects (id, name, path, created_at, updated_at)
                 VALUES ('release-project', 'Release project', '/release-project',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                 INSERT INTO discussions (id, project_id, title, created_at, updated_at)
                 VALUES ('release-discussion', 'release-project', 'Release discussion',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                 INSERT INTO planning_tasks
                     (id, task_number, title, description, status, priority, created_at, updated_at)
                 VALUES ('release-task', 1200, 'Release task', 'Preserve 0.11 data',
                         'in_progress', 'high',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                 INSERT INTO orchestration_runs
                     (id, discussion_id, project_id, target_branch, created_at, updated_at)
                 VALUES ('release-run', 'release-discussion', 'release-project', 'feat/0.12.0',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                 INSERT INTO task_executions
                     (id, orchestration_run_id, task_id, parent_discussion_id, status,
                      child_branch, created_at, updated_at)
                 VALUES ('release-execution', 'release-run', 'release-task',
                         'release-discussion', 'Working', 'kronn/release-task',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');
                 INSERT INTO quick_prompts
                     (id, name, prompt_template, created_at, updated_at)
                 VALUES ('release-qp', 'Release prompt', 'Translate {{text}}',
                         '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z');",
            )
            .unwrap();
        }

        let conn = Connection::open(&db_path).unwrap();
        run_with_backup(&conn, Some(&db_path)).unwrap();

        let preserved: (String, String, String, String) = conn
            .query_row(
                "SELECT p.name, d.title, t.description, e.child_branch
                   FROM projects p
                   JOIN discussions d ON d.project_id = p.id
                   JOIN planning_tasks t ON t.id = 'release-task'
                   JOIN task_executions e ON e.task_id = t.id
                  WHERE p.id = 'release-project'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                "Release project".into(),
                "Release discussion".into(),
                "Preserve 0.11 data".into(),
                "kronn/release-task".into(),
            )
        );

        let preserved_qp: (String, String, Option<String>) = conn
            .query_row(
                "SELECT name, prompt_template, connection_id
                   FROM quick_prompts
                  WHERE id = 'release-qp'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            preserved_qp,
            ("Release prompt".into(), "Translate {{text}}".into(), None)
        );

        for migration in [
            "144_external_api_connections",
            "145_task_execution_worker_connection",
            "146_message_target_connection",
            "147_task_execution_commit_leases",
            "148_task_execution_commit_lease_liveness",
            "149_task_execution_progress",
            "150_project_mcp_sync_report",
            "151_agent_dispatch_connection",
            "152_external_api_openrouter_preset",
            "153_quick_prompt_external_connection",
        ] {
            let applied: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM _migrations WHERE name = ?1)",
                    [migration],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(applied, "missing 0.12 migration receipt: {migration}");
        }

        let backup = Connection::open(db_path.with_extension("db.backup")).unwrap();
        let migration_144_applied: bool = backup
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM _migrations
                  WHERE name = '144_external_api_connections')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !migration_144_applied,
            "backup must remain at the 0.11 schema boundary"
        );
    }

    #[test]
    fn migration_152_repairs_receipted_databases_and_accepts_openrouter() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run_through(&conn, "151_agent_dispatch_connection").unwrap();
        // Reproduce a development database that receipted migration 144 before
        // OpenRouter existed. Editing 144 cannot repair that database because
        // the migration runner correctly refuses to replay receipts.
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE external_api_connections_historical (
                 id TEXT PRIMARY KEY,
                 display_name TEXT NOT NULL,
                 mention_alias TEXT NOT NULL UNIQUE,
                 endpoint TEXT,
                 credential_slug TEXT NOT NULL UNIQUE,
                 origin_preset TEXT NOT NULL CHECK (origin_preset IN ('litellm', 'nvidia', 'other')),
                 economy_model TEXT,
                 default_model TEXT,
                 reasoning_model TEXT,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             DROP TABLE external_api_connections;
             ALTER TABLE external_api_connections_historical RENAME TO external_api_connections;
             CREATE INDEX idx_external_api_connections_origin_preset
                 ON external_api_connections(origin_preset);
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_api_connections
                (id, display_name, mention_alias, endpoint, credential_slug, origin_preset)
             VALUES ('lite', 'LiteLLM', 'litellm', 'http://localhost:4000', 'litellm', 'litellm')",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('d-router', 'Router', '2026-08-30T00:00:00Z', '2026-08-30T00:00:00Z');
             INSERT INTO messages (
                 id, discussion_id, role, content, timestamp, sort_order
             ) VALUES (
                 'm-router', 'd-router', 'User', 'hello', '2026-08-30T00:00:00Z', 0
             );
             INSERT INTO message_targets (
                 message_id, target_kind, agent_type, connection_id, position
             ) VALUES ('m-router', 'agent', 'Custom', 'lite', 0);",
        )
        .unwrap();

        let rejected_before_repair = conn.execute(
            "INSERT INTO external_api_connections
                (id, display_name, mention_alias, endpoint, credential_slug, origin_preset)
             VALUES ('before', 'Before', 'before', 'https://openrouter.ai/api', 'before', 'open_router')",
            [],
        );
        assert!(rejected_before_repair.is_err());

        run_with_backup(&conn, None).unwrap();
        let preserved: String = conn
            .query_row(
                "SELECT display_name FROM external_api_connections WHERE id = 'lite'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, "LiteLLM");
        let preserved_target: String = conn
            .query_row(
                "SELECT connection_id FROM message_targets WHERE message_id = 'm-router'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_target, "lite");
        conn.execute(
            "INSERT INTO external_api_connections
                (id, display_name, mention_alias, endpoint, credential_slug, origin_preset)
             VALUES ('router', 'OpenRouter', 'openrouter', 'https://openrouter.ai/api', 'router', 'open_router')",
            [],
        )
        .unwrap();
        let foreign_key_errors: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(foreign_key_errors, 0);
    }

    #[test]
    fn run_with_backup_snapshots_config_toml_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("kronn.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch("CREATE TABLE t(id INTEGER);").unwrap();
        }
        // Co-located config.toml holding a secret-ish value.
        let cfg = dir.path().join("config.toml");
        std::fs::write(&cfg, "auth_token = \"tok-123\"\n").unwrap();

        let conn = Connection::open(&db_path).unwrap();
        run_with_backup(&conn, Some(&db_path)).expect("migrations should succeed");

        let cfg_backup = dir.path().join("config.toml.backup");
        assert!(
            cfg_backup.exists(),
            "config.toml must be snapshotted before migrations"
        );
        assert_eq!(
            std::fs::read_to_string(&cfg_backup).unwrap(),
            "auth_token = \"tok-123\"\n",
            "config backup must be a faithful copy"
        );
    }
}
