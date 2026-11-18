use super::*;

fn conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    crate::db::migrations::run(&conn).unwrap();
    conn
}

fn run(
    conn: &Connection,
    operation: TaskBoardOperation,
    set: impl FnOnce(&mut BoardInput),
) -> Result<Value> {
    let mut input = BoardInput {
        tag: "todo".into(),
        operation,
        done_limit: 15,
        ..Default::default()
    };
    set(&mut input);
    apply(conn, &input, "wf-test")
}

/// `(title, column)` of the board's cards, markers left out.
fn board(conn: &Connection) -> Vec<(String, String)> {
    board_rows(conn, "todo", 15)
        .unwrap()
        .into_iter()
        .filter(|row| row.get("sentinel").is_none())
        .map(|row| {
            (
                row["title"].as_str().unwrap().into(),
                row["column"].as_str().unwrap().into(),
            )
        })
        .collect()
}

#[tokio::test]
async fn discuss_creates_and_links_a_discussion_without_starting_an_agent() {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let task_id = state
        .db
        .with_conn(|conn| {
            let input = BoardInput {
                tag: "todo".into(),
                operation: TaskBoardOperation::Add,
                title: "Write the notes".into(),
                description: "From the meeting".into(),
                done_limit: 15,
                ..Default::default()
            };
            apply(conn, &input, "wf-test")?["id"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow!("no task id"))
        })
        .await
        .expect("task");
    let step = WorkflowStep {
        name: "board".into(),
        step_type: crate::models::StepType::TaskBoard,
        task_board: Some(crate::models::TaskBoardConfig {
            tag: "todo".into(),
            operation: TaskBoardOperation::Discuss,
            task: task_id.clone(),
            ..Default::default()
        }),
        ..WorkflowStep::default()
    };

    let outcome = execute_task_board_step(&step, "wf-test", &state, &TemplateContext::new()).await;
    assert_eq!(
        outcome.result.status,
        RunStatus::Success,
        "{}",
        outcome.result.output
    );

    let (discussion, linked, awaiting, running, jobs) = state
        .db
        .with_conn(move |conn| {
            let linked: Option<String> = conn.query_row(
                "SELECT discussion_id FROM planning_task_discussions WHERE task_id = ?1",
                [&task_id],
                |row| row.get(0),
            )?;
            let id = linked.clone().ok_or_else(|| anyhow!("task not linked"))?;
            let discussion = crate::db::discussions::get_discussion(conn, &id)?
                .ok_or_else(|| anyhow!("discussion missing"))?;
            let awaiting: bool = conn.query_row(
                "SELECT awaiting_agent FROM discussions WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )?;
            let running: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_dispatch_jobs \
                     WHERE discussion_id = ?1 AND status = 'Running')",
                [&id],
                |row| row.get(0),
            )?;
            let jobs: i64 = conn.query_row(
                "SELECT (SELECT COUNT(*) FROM agent_dispatch_jobs WHERE discussion_id = ?1) \
                          + (SELECT COUNT(*) FROM agent_resume_jobs WHERE discussion_id = ?1)",
                [&id],
                |row| row.get(0),
            )?;
            Ok((discussion, linked, awaiting, running, jobs))
        })
        .await
        .expect("read back");
    assert_eq!(linked.as_deref(), Some(discussion.id.as_str()));
    assert!(
        !discussion.messages.is_empty(),
        "the first message is stored"
    );
    assert!(discussion.messages[0].content.contains("Write the notes"));
    assert!(!awaiting, "no agent is owed a reply");
    assert!(!running, "no agent runs");
    assert_eq!(jobs, 0, "no agent job is created or queued");
}
