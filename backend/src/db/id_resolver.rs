use anyhow::Result;
use rusqlite::{named_params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedIdParent {
    pub kind: String,
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedId {
    pub kind: String,
    pub id: String,
    pub reference: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub parent: Option<ResolvedIdParent>,
    pub suggested_tool: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("Ambiguous Kronn id: it exists in more than one object family ({kinds})")]
pub struct AmbiguousKronnId {
    kinds: String,
}

/// Select the only routing candidate for an opaque id.
///
/// Kept public within the crate so the HTTP layer can merge the indexed DB
/// candidates with the in-process Skill/Profile/Directive registries while
/// preserving the same fail-closed collision rule.
pub(crate) fn select_unique(mut matches: Vec<ResolvedId>) -> Result<Option<ResolvedId>> {
    if matches.len() > 1 {
        let mut kinds = matches
            .iter()
            .map(|resolved| resolved.kind.as_str())
            .collect::<Vec<_>>();
        kinds.sort_unstable();
        kinds.dedup();
        return Err(AmbiguousKronnId {
            kinds: kinds.join(", "),
        }
        .into());
    }
    Ok(matches.pop())
}

fn compact_text(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!compact.is_empty()).then_some(compact)
    })
}

/// Resolve one opaque Kronn UUID across the user-facing object families.
///
/// The lookup is intentionally one indexed SQL statement: an agent should not
/// need to probe many MCP tools just to discover what kind of ID the user
/// pasted. ID collisions across families are treated as an error rather than
/// returning an arbitrary object.
pub fn resolve_id(conn: &Connection, id: &str) -> Result<Option<ResolvedId>> {
    let task_number = id
        .get(..3)
        .filter(|prefix| prefix.eq_ignore_ascii_case("KT-"))
        .and_then(|_| id.get(3..))
        .and_then(|number| number.parse::<i64>().ok());
    let mut statement = conn.prepare(
        r#"
        WITH resolved(
            kind, id, reference, title, summary,
            parent_kind, parent_id, parent_title, suggested_tool
        ) AS (
            SELECT
                'message', m.id, 'MSG-' || substr(m.id, 1, 8),
                m.role || ' message', substr(m.content, 1, 240),
                'discussion', d.id, d.title, 'disc_get_message'
            FROM messages m
            JOIN discussions d ON d.id = m.discussion_id
            WHERE m.id = :id

            UNION ALL
            SELECT
                'discussion', d.id, NULL, d.title, d.agent,
                CASE WHEN p.id IS NULL THEN NULL ELSE 'project' END,
                p.id, p.name, 'disc_load_other'
            FROM discussions d
            LEFT JOIN projects p ON p.id = d.project_id
            WHERE d.id = :id

            UNION ALL
            SELECT
                'project', p.id, NULL, p.name, p.path,
                NULL, NULL, NULL, 'task_list'
            FROM projects p
            WHERE p.id = :id

            UNION ALL
            SELECT
                'workflow', w.id, NULL, w.name,
                CASE WHEN w.enabled = 1 THEN 'enabled' ELSE 'disabled' END,
                CASE WHEN p.id IS NULL THEN NULL ELSE 'project' END,
                p.id, p.name, 'workflow_get'
            FROM workflows w
            LEFT JOIN projects p ON p.id = w.project_id
            WHERE w.id = :id

            UNION ALL
            SELECT
                'workflow_run', r.id, 'RUN-' || substr(r.id, 1, 8),
                w.name || ' run', r.status,
                'workflow', w.id, w.name, 'workflow_run_get'
            FROM workflow_runs r
            JOIN workflows w ON w.id = r.workflow_id
            WHERE r.id = :id

            UNION ALL
            SELECT
                'task', t.id, 'KT-' || t.task_number, t.title,
                t.status || ' · ' || t.priority,
                CASE WHEN parent.id IS NULL THEN NULL ELSE 'task' END,
                parent.id, parent.title, 'task_get'
            FROM planning_tasks t
            LEFT JOIN planning_tasks parent ON parent.id = t.parent_id
            WHERE t.id = :id OR t.task_number = :task_number

            UNION ALL
            SELECT
                'planning_proposal', proposal.id, NULL, 'Planning proposal',
                proposal.aggregate_state,
                'discussion', d.id, d.title, 'proposal_get'
            FROM planning_proposals proposal
            JOIN discussions d ON d.id = proposal.discussion_id
            WHERE proposal.id = :id

            UNION ALL
            SELECT
                'quick_prompt', q.id, NULL, q.name, q.agent,
                CASE WHEN p.id IS NULL THEN NULL ELSE 'project' END,
                p.id, p.name, 'qp_get'
            FROM quick_prompts q
            LEFT JOIN projects p ON p.id = q.project_id
            WHERE q.id = :id

            UNION ALL
            SELECT
                'quick_api', q.id, NULL, q.name,
                coalesce(q.api_method, 'GET') || ' ' || q.api_endpoint_path,
                CASE WHEN p.id IS NULL THEN NULL ELSE 'project' END,
                p.id, p.name, 'qa_list'
            FROM quick_apis q
            LEFT JOIN projects p ON p.id = q.project_id
            WHERE q.id = :id

            UNION ALL
            SELECT
                'quick_exec', q.id, NULL, q.name,
                q.command || ' · ' || q.output_format,
                CASE WHEN p.id IS NULL THEN NULL ELSE 'project' END,
                p.id, p.name, 'qe_list'
            FROM quick_execs q
            LEFT JOIN projects p ON p.id = q.project_id
            WHERE q.id = :id

            UNION ALL
            SELECT
                'page', lp.id, NULL, lp.title,
                CASE
                    WHEN lp.last_published_at IS NULL THEN 'draft'
                    ELSE 'published ' || lp.last_published_at
                END,
                CASE WHEN p.id IS NULL THEN NULL ELSE 'project' END,
                p.id, p.name, 'page_get'
            FROM live_pages lp
            LEFT JOIN projects p ON p.id = lp.project_id
            WHERE lp.id = :id OR lp.slug = :id

            UNION ALL
            SELECT
                'task_execution', execution.id,
                'EXEC-' || substr(execution.id, 1, 8),
                task.title || ' execution',
                execution.status ||
                    CASE
                        WHEN execution.worker_agent_type IS NULL THEN ''
                        ELSE ' · ' || execution.worker_agent_type ||
                            coalesce(' / ' || execution.worker_model, '')
                    END,
                'task', task.id, task.title, 'task_exec_status'
            FROM task_executions execution
            JOIN planning_tasks task ON task.id = execution.task_id
            WHERE execution.id = :id

            UNION ALL
            SELECT
                'mcp_server', server.id, NULL, server.name,
                server.transport || ' · ' || server.source,
                NULL, NULL, NULL, 'mcp_list'
            FROM mcp_servers server
            WHERE server.id = :id

            UNION ALL
            SELECT
                'mcp_config', config.id, NULL, config.label,
                server.name ||
                    CASE WHEN config.is_global = 1 THEN ' · global' ELSE ' · scoped' END,
                'mcp_server', server.id, server.name, 'mcp_list'
            FROM mcp_configs config
            JOIN mcp_servers server ON server.id = config.server_id
            WHERE config.id = :id
        )
        SELECT
            kind, id, reference, title, summary,
            parent_kind, parent_id, parent_title, suggested_tool
        FROM resolved
        "#,
    )?;

    let rows = statement.query_map(
        named_params! {":id": id, ":task_number": task_number},
        |row| {
            let parent_kind: Option<String> = row.get(5)?;
            let parent_id: Option<String> = row.get(6)?;
            let parent_title: Option<String> = row.get(7)?;
            let parent = match (parent_kind, parent_id, parent_title) {
                (Some(kind), Some(id), Some(title)) => Some(ResolvedIdParent { kind, id, title }),
                _ => None,
            };
            Ok(ResolvedId {
                kind: row.get(0)?,
                id: row.get(1)?,
                reference: row.get(2)?,
                title: row.get(3)?,
                summary: compact_text(row.get(4)?),
                parent,
                suggested_tool: row.get(8)?,
            })
        },
    )?;
    let matches = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    select_unique(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::migrations;

    fn connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run(&connection).unwrap();
        connection
    }

    fn seed_project_and_discussion(connection: &Connection) {
        connection
            .execute(
                "INSERT INTO projects
                 (id, name, path, created_at, updated_at)
                 VALUES ('project-1', 'Kronn', '/tmp/kronn', 'now', 'now')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO discussions
                 (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES ('discussion-1', 'project-1', 'Resolver room', 'Codex', 'fr', 'now', 'now')",
                [],
            )
            .unwrap();
    }

    #[test]
    fn resolves_message_with_compact_parent_context() {
        let connection = connection();
        seed_project_and_discussion(&connection);
        connection
            .execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, sort_order)
                 VALUES ('12345678-abcd', 'discussion-1', 'User',
                         '  resolve   this\nmessage  ', 'now', 1)",
                [],
            )
            .unwrap();

        let resolved = resolve_id(&connection, "12345678-abcd").unwrap().unwrap();
        assert_eq!(resolved.kind, "message");
        assert_eq!(resolved.reference.as_deref(), Some("MSG-12345678"));
        assert_eq!(resolved.summary.as_deref(), Some("resolve this message"));
        assert_eq!(
            resolved.parent,
            Some(ResolvedIdParent {
                kind: "discussion".into(),
                id: "discussion-1".into(),
                title: "Resolver room".into(),
            })
        );
        assert_eq!(resolved.suggested_tool.as_deref(), Some("disc_get_message"));
    }

    #[test]
    fn resolves_task_reference_and_parent() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO planning_tasks
                 (id, task_number, title, status, priority, created_at, updated_at)
                 VALUES ('parent-task', 1, 'Parent', 'todo', 'normal', 'now', 'now')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO planning_tasks
                 (id, task_number, parent_id, title, status, priority, created_at, updated_at)
                 VALUES ('child-task', 2, 'parent-task', 'Child', 'in_progress', 'high', 'now', 'now')",
                [],
            )
            .unwrap();

        let resolved = resolve_id(&connection, "child-task").unwrap().unwrap();
        assert_eq!(resolved.kind, "task");
        assert_eq!(resolved.reference.as_deref(), Some("KT-2"));
        assert_eq!(resolved.summary.as_deref(), Some("in_progress · high"));
        assert_eq!(resolved.parent.unwrap().id, "parent-task");
        assert_eq!(resolved.suggested_tool.as_deref(), Some("task_get"));
    }

    #[test]
    fn resolves_every_database_backed_public_mcp_object_family() {
        let connection = connection();
        seed_project_and_discussion(&connection);
        connection
            .execute_batch(
                r#"
                INSERT INTO messages
                    (id, discussion_id, role, content, timestamp, sort_order)
                VALUES
                    ('matrix-message', 'discussion-1', 'User', 'Matrix source', 'now', 1);

                INSERT INTO workflows
                    (id, name, project_id, trigger_json, steps_json, created_at, updated_at)
                VALUES
                    ('workflow-1', 'Matrix workflow', 'project-1', '{}', '[]', 'now', 'now');
                INSERT INTO workflow_runs
                    (id, workflow_id, status, started_at)
                VALUES
                    ('workflow-run-1', 'workflow-1', 'Success', 'now');

                INSERT INTO planning_tasks
                    (id, task_number, title, status, priority, created_at, updated_at)
                VALUES
                    ('task-1', 17, 'Matrix task', 'in_progress', 'critical', 'now', 'now');
                INSERT INTO planning_proposals
                    (id, discussion_id, source_message_id, fence_index,
                     aggregate_state, created_at, updated_at)
                VALUES
                    ('proposal:matrix-message:0', 'discussion-1', 'matrix-message', 0,
                     'pending', 'now', 'now');

                INSERT INTO quick_prompts
                    (id, name, prompt_template, variables_json, agent, project_id,
                     created_at, updated_at)
                VALUES
                    ('qp-1', 'Matrix prompt', 'Do it', '[]', 'Codex', 'project-1',
                     'now', 'now');

                INSERT INTO mcp_servers
                    (id, name, description, transport, args_json, source)
                VALUES
                    ('mcp-matrix', 'Matrix plugin', 'Test plugin', 'stdio', '[]', 'manual');
                INSERT INTO mcp_configs
                    (id, server_id, label, env_encrypted, env_keys_json, is_global, config_hash)
                VALUES
                    ('mcp-config-1', 'mcp-matrix', 'Matrix account', '', '[]', 0, 'matrix');
                INSERT INTO quick_apis
                    (id, name, project_id, api_plugin_slug, api_config_id,
                     api_endpoint_path, variables_json, created_at, updated_at)
                VALUES
                    ('qa-1', 'Matrix API', 'project-1', 'mcp-matrix', 'mcp-config-1',
                     '/v1/matrix', '[]', 'now', 'now');
                INSERT INTO quick_execs
                    (id, name, command, args_json, timeout_secs, output_format,
                     project_id, variables_json, created_at, updated_at)
                VALUES
                    ('qe-1', 'Matrix exec', 'git', '[]', 60, 'json',
                     'project-1', '[]', 'now', 'now');

                INSERT INTO live_pages
                    (id, project_id, title, slug, data_revision, created_at, updated_at)
                VALUES
                    ('page-1', 'project-1', 'Matrix page', 'matrix-page', 0, 'now', 'now');

                INSERT INTO orchestration_runs
                    (id, kind, discussion_id, project_id, max_review_rounds,
                     max_concurrent_executions, integration_strategy,
                     validation_json, status, created_at, updated_at)
                VALUES
                    ('orchestration-1', 'single_task', 'discussion-1', 'project-1', 3,
                     1, 'two_phase_ff_only', '[]', 'active', 'now', 'now');
                INSERT INTO task_executions
                    (id, orchestration_run_id, task_id, parent_discussion_id,
                     status, review_rounds, max_review_rounds, attempt_no,
                     created_at, updated_at)
                VALUES
                    ('execution-1', 'orchestration-1', 'task-1', 'discussion-1',
                     'Working', 0, 3, 0, 'now', 'now');
                "#,
            )
            .unwrap();

        let expected = [
            (
                "matrix-message",
                "message",
                "disc_get_message",
                Some("discussion"),
            ),
            (
                "discussion-1",
                "discussion",
                "disc_load_other",
                Some("project"),
            ),
            ("project-1", "project", "task_list", None),
            ("workflow-1", "workflow", "workflow_get", Some("project")),
            (
                "workflow-run-1",
                "workflow_run",
                "workflow_run_get",
                Some("workflow"),
            ),
            ("task-1", "task", "task_get", None),
            (
                "proposal:matrix-message:0",
                "planning_proposal",
                "proposal_get",
                Some("discussion"),
            ),
            ("qp-1", "quick_prompt", "qp_get", Some("project")),
            ("qa-1", "quick_api", "qa_list", Some("project")),
            ("qe-1", "quick_exec", "qe_list", Some("project")),
            ("page-1", "page", "page_get", Some("project")),
            (
                "execution-1",
                "task_execution",
                "task_exec_status",
                Some("task"),
            ),
            ("mcp-matrix", "mcp_server", "mcp_list", None),
            ("mcp-config-1", "mcp_config", "mcp_list", Some("mcp_server")),
        ];

        for (id, kind, suggested_tool, parent_kind) in expected {
            let resolved = resolve_id(&connection, id).unwrap().unwrap();
            assert_eq!(resolved.kind, kind, "wrong kind for {id}");
            assert_eq!(
                resolved.suggested_tool.as_deref(),
                Some(suggested_tool),
                "wrong tool for {id}"
            );
            assert_eq!(
                resolved.parent.as_ref().map(|parent| parent.kind.as_str()),
                parent_kind,
                "wrong parent for {id}"
            );
        }

        assert_eq!(
            resolve_id(&connection, "kt-17").unwrap().unwrap().id,
            "task-1"
        );
        assert_eq!(
            resolve_id(&connection, "matrix-page").unwrap().unwrap().id,
            "page-1"
        );
    }

    #[test]
    fn returns_none_for_unknown_id() {
        assert_eq!(resolve_id(&connection(), "missing").unwrap(), None);
    }

    #[test]
    fn resolves_saved_quick_exec() {
        let connection = connection();
        connection
            .execute(
                "INSERT INTO quick_execs
                 (id, name, command, args_json, timeout_secs, output_format,
                  variables_json, created_at, updated_at)
                 VALUES ('qe-1', 'AWS errors', 'aws', '[]', 60, 'json', '[]', 'now', 'now')",
                [],
            )
            .unwrap();
        let resolved = resolve_id(&connection, "qe-1").unwrap().unwrap();
        assert_eq!(resolved.kind, "quick_exec");
        assert_eq!(resolved.summary.as_deref(), Some("aws · json"));
        assert_eq!(resolved.suggested_tool.as_deref(), Some("qe_list"));
    }

    #[test]
    fn resolves_live_page_with_publication_state() {
        let connection = connection();
        seed_project_and_discussion(&connection);
        connection
            .execute(
                "INSERT INTO live_pages
                 (id, project_id, title, slug, data_revision, created_at, updated_at, last_published_at)
                 VALUES ('page-1', 'project-1', 'Latency dashboard', 'latency-dashboard', 0,
                         'now', 'now', NULL)",
                [],
            )
            .unwrap();

        let resolved = resolve_id(&connection, "page-1").unwrap().unwrap();
        assert_eq!(resolved.kind, "page");
        assert_eq!(resolved.title, "Latency dashboard");
        assert_eq!(resolved.summary.as_deref(), Some("draft"));
        assert_eq!(resolved.parent.as_ref().unwrap().id, "project-1");
        assert_eq!(resolved.suggested_tool.as_deref(), Some("page_get"));

        connection
            .execute(
                "UPDATE live_pages SET last_published_at = '2026-08-25T00:00:00Z' WHERE id = 'page-1'",
                [],
            )
            .unwrap();
        let published = resolve_id(&connection, "page-1").unwrap().unwrap();
        assert_eq!(
            published.summary.as_deref(),
            Some("published 2026-08-25T00:00:00Z")
        );
    }

    #[test]
    fn rejects_cross_family_collision() {
        let connection = connection();
        seed_project_and_discussion(&connection);
        connection
            .execute(
                "INSERT INTO workflows
                 (id, name, trigger_json, steps_json, created_at, updated_at)
                 VALUES ('project-1', 'Collision', '{}', '[]', 'now', 'now')",
                [],
            )
            .unwrap();

        let error = resolve_id(&connection, "project-1").unwrap_err();
        assert!(error.to_string().contains("Ambiguous Kronn id"));
    }
}
