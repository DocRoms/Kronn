use anyhow::{bail, Result};
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

fn compact_text(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
        (!compact.is_empty()).then_some(compact)
    })
}

/// Resolve one opaque Kronn UUID across the user-facing object families.
///
/// The lookup is intentionally one indexed SQL statement: an agent should not
/// need to probe seven MCP tools just to discover what kind of ID the user
/// pasted. UUID collisions across families are treated as an error rather than
/// returning an arbitrary object.
pub fn resolve_id(conn: &Connection, id: &str) -> Result<Option<ResolvedId>> {
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
                'task', t.id, 'KT-' || t.task_number, t.title,
                t.status || ' · ' || t.priority,
                CASE WHEN parent.id IS NULL THEN NULL ELSE 'task' END,
                parent.id, parent.title, 'task_get'
            FROM planning_tasks t
            LEFT JOIN planning_tasks parent ON parent.id = t.parent_id
            WHERE t.id = :id

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
        )
        SELECT
            kind, id, reference, title, summary,
            parent_kind, parent_id, parent_title, suggested_tool
        FROM resolved
        LIMIT 2
        "#,
    )?;

    let rows = statement.query_map(named_params! {":id": id}, |row| {
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
    })?;
    let matches = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    if matches.len() > 1 {
        bail!("Ambiguous Kronn id: it exists in more than one object family");
    }
    Ok(matches.into_iter().next())
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
