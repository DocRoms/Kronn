//! Executor for `StepType::TaskBoard` (KT-1030).
//!
//! A board is the set of planning tasks sharing one tag, in three columns
//! (`todo`, `in_progress`, `done`). Every operation runs against Kronn's own
//! database and returns the whole board as rows a Page can publish, so the
//! board needs nothing installed beyond Kronn: no shell, no HTTP, no token.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::db::task_boards::{self, BoardTask};
use crate::models::{
    CreatePlanningTaskRequest, LinkPlanningDiscussionRequest, PlanningActor, PlanningActorKind,
    PlanningTaskStatus, RunStatus, StepResult, TaskBoardConfig, TaskBoardOperation,
    UpdatePlanningTaskRequest, WorkflowStep,
};
use crate::AppState;

use super::steps::StepOutcome;
use super::template::TemplateContext;

pub const COLUMNS: [&str; 3] = ["todo", "in_progress", "done"];
const DEFAULT_DONE_LIMIT: u32 = 15;
const MAX_DONE_LIMIT: u32 = 100;
const TITLE_MAX: usize = 240;
/// Refused beyond, never cut: a description is the user's text.
const DESCRIPTION_MAX: usize = 20_000;
const MAX_EXTRA_TAGS: usize = 20;

/// The end-of-column marker: a row a Page binds to for "move to the end of".
pub fn column_marker(column: &str) -> String {
    format!("__col_{column}__")
}

/// A config with its templates rendered for this run.
#[derive(Debug, Clone, Default)]
pub struct BoardInput {
    pub tag: String,
    pub operation: TaskBoardOperation,
    pub task: String,
    pub before: String,
    pub column: String,
    pub title: String,
    pub description: String,
    pub tags: String,
    pub done_limit: u32,
}

fn render(context: &TemplateContext, template: &str) -> Result<String> {
    if template.trim().is_empty() {
        return Ok(String::new());
    }
    Ok(context.render_strict(template)?.trim().to_string())
}

fn render_input(config: &TaskBoardConfig, context: &TemplateContext) -> Result<BoardInput> {
    let tag = render(context, &config.tag)?;
    validate_tag(&tag)?;
    Ok(BoardInput {
        tag,
        operation: config.operation,
        task: render(context, &config.task)?,
        before: render(context, &config.before)?,
        column: render(context, &config.column)?,
        title: render(context, &config.title)?,
        // Line breaks of a description are its content: only trimmed at the ends.
        description: render(context, &config.description)?,
        tags: render(context, &config.tags)?,
        done_limit: config
            .done_limit
            .unwrap_or(DEFAULT_DONE_LIMIT)
            .clamp(1, MAX_DONE_LIMIT),
    })
}

pub fn validate_tag(tag: &str) -> Result<()> {
    if tag.is_empty() || tag.chars().count() > 80 || tag.contains(',') {
        bail!("A task board needs one tag of 1-80 characters, without a comma");
    }
    Ok(())
}

fn column_of(status: &str) -> &'static str {
    match status {
        "in_progress" => "in_progress",
        "done" => "done",
        _ => "todo",
    }
}

fn status_for(column: &str) -> PlanningTaskStatus {
    match column {
        "in_progress" => PlanningTaskStatus::InProgress,
        "done" => PlanningTaskStatus::Done,
        _ => PlanningTaskStatus::Todo,
    }
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn checked_description(description: &str) -> Result<String> {
    if description.chars().count() > DESCRIPTION_MAX {
        bail!("A description holds at most {DESCRIPTION_MAX} characters");
    }
    Ok(description.to_string())
}

/// A tag as the board stores it: lower case, dashes for spaces.
fn normalize_tag(raw: &str) -> String {
    raw.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// Open tasks in board order: tasks the stored order does not know yet (added
/// elsewhere) first, newest first, then the stored order.
fn open_in_order<'a>(tasks: &'a [BoardTask], order: &[String]) -> Vec<&'a BoardTask> {
    let position: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect();
    let open = tasks.iter().filter(|task| task.status != "done");
    let (mut known, mut fresh): (Vec<_>, Vec<_>) =
        open.partition(|task| position.contains_key(task.id.as_str()));
    fresh.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    known.sort_by_key(|task| position[task.id.as_str()]);
    fresh.extend(known);
    fresh
}

fn open_ids(conn: &Connection, tag: &str) -> Result<Vec<String>> {
    let tasks = task_boards::load_tasks(conn, tag)?;
    let order = task_boards::get_order(conn, tag)?;
    Ok(open_in_order(&tasks, &order)
        .into_iter()
        .map(|task| task.id.clone())
        .collect())
}

fn row(task: &BoardTask, tag: &str) -> Value {
    let mut row = json!({
        "id": task.id,
        "ref": task.reference,
        "title": task.title,
        "column": column_of(&task.status),
        "priority": task.priority,
        "tags": task.tags.iter().filter(|t| !t.eq_ignore_ascii_case(tag)).collect::<Vec<_>>(),
        // Whole: the edit card is pre-filled from it, so a cut would be written back.
        "description": task.description,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
    });
    if let Some(discussion) = &task.discussion_id {
        row["discussion_id"] = json!(discussion);
    }
    row
}

/// The board as Page rows: open cards in order, the latest done ones, then one
/// end marker per column.
pub fn board_rows(conn: &Connection, tag: &str, done_limit: u32) -> Result<Vec<Value>> {
    let tasks = task_boards::load_tasks(conn, tag)?;
    let order = task_boards::get_order(conn, tag)?;
    let mut done: Vec<&BoardTask> = tasks.iter().filter(|t| t.status == "done").collect();
    done.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    done.truncate(done_limit as usize);
    let mut rows: Vec<Value> = open_in_order(&tasks, &order)
        .into_iter()
        .chain(done)
        .map(|task| row(task, tag))
        .collect();
    rows.extend(
        COLUMNS
            .iter()
            .map(|column| json!({"id": column_marker(column), "column": column, "sentinel": true})),
    );
    Ok(rows)
}

fn actor(workflow_id: &str) -> PlanningActor {
    PlanningActor {
        kind: PlanningActorKind::Backend,
        id: Some(format!("workflow:{workflow_id}")),
        session_id: None,
        source_message_id: None,
    }
}

/// The board task `reference` names; refuses any task without the board tag.
fn board_task(conn: &Connection, tag: &str, reference: &str) -> Result<BoardTask> {
    if reference.is_empty() {
        bail!("No task given");
    }
    let id = crate::db::planning::lookup_task_id(conn, reference)?
        .ok_or_else(|| anyhow!("Task {reference} not found"))?;
    task_boards::load_tasks(conn, tag)?
        .into_iter()
        .find(|task| task.id == id)
        .ok_or_else(|| anyhow!("Task {reference} is not on the board (tag `{tag}` missing)"))
}

fn update(
    conn: &Connection,
    task_id: &str,
    actor: &PlanningActor,
    change: impl FnOnce(&mut UpdatePlanningTaskRequest),
) -> Result<()> {
    let mut request = UpdatePlanningTaskRequest {
        title: None,
        description: None,
        status: None,
        priority: None,
        parent_id: None,
        blocked_reason: None,
        rank: None,
        project_ids: None,
        tags: None,
        definition_of_done: None,
        links: None,
        actor: actor.clone(),
    };
    change(&mut request);
    crate::db::planning::update_task(conn, task_id, &request)?;
    Ok(())
}

fn put_first(conn: &Connection, tag: &str, task_id: &str) -> Result<()> {
    let mut ids = vec![task_id.to_string()];
    ids.extend(open_ids(conn, tag)?.into_iter().filter(|id| id != task_id));
    task_boards::set_order(conn, tag, &ids)
}

/// Apply every operation but `discuss`, in one transaction. Returns what the
/// operation touched, for the step's output.
pub fn apply(conn: &Connection, input: &BoardInput, workflow_id: &str) -> Result<Value> {
    let tx = conn.unchecked_transaction()?;
    let actor = actor(workflow_id);
    let tag = input.tag.as_str();
    let touched = match input.operation {
        TaskBoardOperation::Read => json!(null),
        TaskBoardOperation::Add => {
            let title = truncate(&input.title, TITLE_MAX);
            if title.trim().is_empty() {
                bail!("A task needs a title");
            }
            let mut tags = vec![tag.to_string()];
            for extra in input.tags.split(',').map(normalize_tag) {
                if !extra.is_empty() && !tags.iter().any(|t| t.eq_ignore_ascii_case(&extra)) {
                    tags.push(truncate(&extra, 80));
                }
            }
            tags.truncate(MAX_EXTRA_TAGS + 1);
            let created = crate::db::planning::create_task(
                &tx,
                &CreatePlanningTaskRequest {
                    title,
                    discussion_id: None,
                    idempotency_key: None,
                    description: checked_description(&input.description)?,
                    status: PlanningTaskStatus::Todo,
                    priority: Default::default(),
                    parent_id: None,
                    project_ids: vec![],
                    tags,
                    definition_of_done: vec![],
                    links: vec![],
                    actor: actor.clone(),
                },
            )?;
            put_first(&tx, tag, &created.summary.id)?;
            json!({"id": created.summary.id, "ref": created.summary.reference, "column": "todo"})
        }
        TaskBoardOperation::Toggle => {
            let task = board_task(&tx, tag, &input.task)?;
            let reopen = task.status == "done";
            let status = if reopen {
                PlanningTaskStatus::Todo
            } else {
                PlanningTaskStatus::Done
            };
            update(&tx, &task.id, &actor, |r| r.status = Some(status))?;
            if reopen {
                // A reopened task comes back on top, where one looks for it.
                put_first(&tx, tag, &task.id)?;
            }
            json!({"id": task.id, "ref": task.reference, "column": if reopen { "todo" } else { "done" }})
        }
        TaskBoardOperation::Move => {
            let task = board_task(&tx, tag, &input.task)?;
            let current = column_of(&task.status);
            let target = COLUMNS
                .iter()
                .copied()
                .find(|column| *column == input.column || column_marker(column) == input.column)
                .or_else(|| {
                    COLUMNS
                        .iter()
                        .copied()
                        .find(|column| column_marker(column) == input.before)
                })
                .unwrap_or(current);
            if target != current {
                update(&tx, &task.id, &actor, |r| {
                    r.status = Some(status_for(target))
                })?;
            }
            if target != "done" {
                let tasks = task_boards::load_tasks(&tx, tag)?;
                let order = task_boards::get_order(&tx, tag)?;
                let mut list: Vec<&BoardTask> = open_in_order(&tasks, &order)
                    .into_iter()
                    .filter(|t| t.id != task.id)
                    .collect();
                let moved = tasks
                    .iter()
                    .find(|t| t.id == task.id)
                    .context("Moved task disappeared")?;
                let position = list
                    .iter()
                    .position(|t| t.id == input.before && column_of(&t.status) == target)
                    .unwrap_or_else(|| {
                        // The end of the target column: right after its last card.
                        list.iter()
                            .rposition(|t| column_of(&t.status) == target)
                            .map_or(list.len(), |last| last + 1)
                    });
                list.insert(position, moved);
                let ids: Vec<String> = list.iter().map(|t| t.id.clone()).collect();
                task_boards::set_order(&tx, tag, &ids)?;
            }
            json!({"id": task.id, "ref": task.reference, "column": target})
        }
        TaskBoardOperation::Edit => {
            let task = board_task(&tx, tag, &input.task)?;
            // Only what the reader changed is written: an untouched field keeps
            // its stored bytes, whatever the card or the template did to it.
            let title = input.title.trim();
            let title = (!title.is_empty() && title != task.title.trim())
                .then(|| truncate(title, TITLE_MAX));
            let description = (input.description.trim() != task.description.trim())
                .then(|| checked_description(&input.description))
                .transpose()?;
            if title.is_some() || description.is_some() {
                update(&tx, &task.id, &actor, |r| {
                    r.title = title;
                    r.description = description;
                })?;
            }
            json!({"id": task.id, "ref": task.reference})
        }
        TaskBoardOperation::Discuss => bail!("`discuss` creates a discussion: use execute"),
    };
    tx.commit()?;
    Ok(touched)
}

/// The first message of a task's discussion, in Kronn's language.
fn discussion_prompt(language: &str, task: &BoardTask) -> String {
    let intro = match language {
        "fr" => "Tâche de ma todo",
        "es" => "Tarea de mi lista",
        "zh" => "我的待办任务",
        _ => "Task from my todo list",
    };
    let mut prompt = format!("{intro} : **{}** ({}).", task.title, task.reference);
    if language != "fr" {
        prompt = prompt.replacen(" :", ":", 1);
    }
    if !task.description.trim().is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(task.description.trim());
    }
    prompt
}

/// Create a discussion about the task and link it; a task keeps one.
async fn discuss(
    state: &AppState,
    step: &WorkflowStep,
    input: &BoardInput,
    workflow_id: &str,
) -> Result<Value> {
    let (tag, reference) = (input.tag.clone(), input.task.clone());
    let task = state
        .db
        .with_conn(move |conn| board_task(conn, &tag, &reference))
        .await?;
    if let Some(existing) = &task.discussion_id {
        return Ok(
            json!({"id": task.id, "ref": task.reference, "discussion_id": existing, "existing": true}),
        );
    }
    let language = state.config.read().await.language.clone();
    let request = crate::models::CreateDiscussionRequest {
        project_id: None,
        title: truncate(&task.title, 120),
        agent: step.agent.clone(),
        connection_id: None,
        language: language.clone(),
        initial_prompt: discussion_prompt(&language, &task),
        initial_targets: vec![],
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        workspace_mode: None,
        base_branch: None,
        tier: Default::default(),
        originating_qp_id: None,
        launch_variables: HashMap::new(),
        no_agent: false,
        assistant: None,
    };
    let created =
        crate::api::discussions::create(axum::extract::State(state.clone()), axum::Json(request))
            .await
            .0;
    let discussion = match (created.success, created.data) {
        (true, Some(discussion)) => discussion,
        _ => bail!(
            "Discussion refused: {}",
            created.error.unwrap_or_else(|| "unknown error".into())
        ),
    };
    let (task_id, discussion_id, actor) =
        (task.id.clone(), discussion.id.clone(), actor(workflow_id));
    state
        .db
        .with_conn(move |conn| {
            crate::db::planning::link_discussion(
                conn,
                &task_id,
                &LinkPlanningDiscussionRequest {
                    discussion_id,
                    placement: Default::default(),
                    is_primary: false,
                    position: None,
                    actor,
                },
            )
        })
        .await
        .with_context(|| format!("Discussion {} created but not linked", discussion.id))?;
    Ok(
        json!({"id": task.id, "ref": task.reference, "discussion_id": discussion.id, "existing": false}),
    )
}

pub async fn execute_task_board_step(
    step: &WorkflowStep,
    workflow_id: &str,
    state: &AppState,
    context: &TemplateContext,
) -> StepOutcome {
    let started = Instant::now();
    let Some(config) = step.task_board.as_ref() else {
        return fail(step, started, "TaskBoard step missing `task_board`");
    };
    let input = match render_input(config, context) {
        Ok(input) => input,
        Err(error) => return fail(step, started, error),
    };
    let touched = if input.operation == TaskBoardOperation::Discuss {
        discuss(state, step, &input, workflow_id).await
    } else {
        let (input, workflow_id) = (input.clone(), workflow_id.to_string());
        state
            .db
            .with_conn(move |conn| apply(conn, &input, &workflow_id))
            .await
    };
    let touched = match touched {
        Ok(touched) => touched,
        Err(error) => return fail(step, started, error),
    };
    let (tag, limit) = (input.tag.clone(), input.done_limit);
    let rows = match state
        .db
        .with_read_conn(move |conn| board_rows(conn, &tag, limit))
        .await
    {
        Ok(rows) => rows,
        Err(error) => return fail(step, started, error),
    };
    let count = |column: &str| {
        rows.iter()
            .filter(|row| row["column"] == column && row.get("sentinel").is_none())
            .count()
    };
    let counts =
        json!({"todo": count("todo"), "in_progress": count("in_progress"), "done": count("done")});
    let summary = format!(
        "Board `{}`: {} to do, {} in progress, {} done shown",
        input.tag, counts["todo"], counts["in_progress"], counts["done"]
    );
    succeed(
        step,
        started,
        json!({"tag": input.tag, "task": touched, "counts": counts, "rows": rows}),
        summary,
    )
}

fn succeed(step: &WorkflowStep, started: Instant, payload: Value, summary: String) -> StepOutcome {
    let output = super::step_output_format::format_step_output_simple(payload, "OK", &summary);
    let condition_action = super::steps::evaluate_conditions(&step.on_result, &output);
    let condition_result = condition_action.as_ref().map(|action| match action {
        crate::models::ConditionAction::Stop => "Stop".to_string(),
        crate::models::ConditionAction::Skip => "Skip".to_string(),
        crate::models::ConditionAction::Goto { step_name, .. } => format!("Goto:{step_name}"),
    });
    StepOutcome {
        result: result(step, started, RunStatus::Success, output, condition_result),
        condition_action,
    }
}

fn fail(step: &WorkflowStep, started: Instant, error: impl std::fmt::Display) -> StepOutcome {
    StepOutcome {
        result: result(step, started, RunStatus::Failed, error.to_string(), None),
        condition_action: None,
    }
}

fn result(
    step: &WorkflowStep,
    started: Instant,
    status: RunStatus,
    output: String,
    condition_result: Option<String>,
) -> StepResult {
    StepResult {
        step_name: step.name.clone(),
        status,
        output,
        tokens_used: Some(0),
        duration_ms: started.elapsed().as_millis() as u64,
        started_at: None,
        condition_result,
        envelope_detected: None,
        step_kind: None,
        step_agent: None,
        step_model: None,
        step_api_plugin_slug: None,
        step_api_endpoint_path: None,
        is_rollback: false,
        child_run_id: None,
        agent_provenance: None,
        native_tool_calls: Box::default(),
        cached_prompt_tokens: None,
        cache_write_prompt_tokens: None,
        last_activity: None,
        quota_wait: None,
        terminal_stop: None,
    }
}

#[cfg(test)]
mod tests {
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

    fn add(conn: &Connection, title: &str) -> String {
        run(conn, TaskBoardOperation::Add, |i| i.title = title.into()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
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

    fn titles(conn: &Connection, column: &str) -> Vec<String> {
        board(conn)
            .into_iter()
            .filter(|(_, c)| c == column)
            .map(|(t, _)| t)
            .collect()
    }

    #[test]
    fn add_puts_the_task_on_top_with_normalised_tags_and_markers_close_the_board() {
        let conn = conn();
        add(&conn, "first");
        let id = run(&conn, TaskBoardOperation::Add, |i| {
            i.title = "second".into();
            i.description = "**why**".into();
            i.tags = " Front End , todo,front end,,".into();
        })
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(titles(&conn, "todo"), vec!["second", "first"]);
        let rows = board_rows(&conn, "todo", 15).unwrap();
        let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
        assert_eq!(row["tags"], json!(["front-end"]));
        assert_eq!(row["description"], "**why**");
        let markers: Vec<_> = rows
            .iter()
            .filter(|r| r["sentinel"] == true)
            .map(|r| r["id"].clone())
            .collect();
        assert_eq!(
            markers,
            vec![
                json!("__col_todo__"),
                json!("__col_in_progress__"),
                json!("__col_done__")
            ]
        );
        assert!(run(&conn, TaskBoardOperation::Add, |i| i.title = "  ".into()).is_err());
    }

    #[test]
    fn move_places_a_card_before_another_or_at_the_end_of_a_column() {
        let conn = conn();
        let a = add(&conn, "a");
        let b = add(&conn, "b");
        let c = add(&conn, "c");
        assert_eq!(titles(&conn, "todo"), vec!["c", "b", "a"]);
        run(&conn, TaskBoardOperation::Move, |i| {
            i.task = a.clone();
            i.before = c.clone();
            i.column = column_marker("todo");
        })
        .unwrap();
        assert_eq!(titles(&conn, "todo"), vec!["a", "c", "b"]);
        run(&conn, TaskBoardOperation::Move, |i| {
            i.task = c.clone();
            i.before = column_marker("in_progress");
            i.column = column_marker("in_progress");
        })
        .unwrap();
        assert_eq!(titles(&conn, "in_progress"), vec!["c"]);
        assert_eq!(titles(&conn, "todo"), vec!["a", "b"]);
        run(&conn, TaskBoardOperation::Move, |i| {
            i.task = b.clone();
            i.before = c.clone();
            i.column = column_marker("in_progress");
        })
        .unwrap();
        assert_eq!(titles(&conn, "in_progress"), vec!["b", "c"]);
        run(&conn, TaskBoardOperation::Move, |i| {
            i.task = b.clone();
            i.before = column_marker("done");
            i.column = column_marker("done");
        })
        .unwrap();
        assert_eq!(titles(&conn, "done"), vec!["b"]);
        let status: String = conn
            .query_row(
                "SELECT status FROM planning_tasks WHERE id = ?1",
                [&b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "done");
    }

    #[test]
    fn toggle_finishes_then_reopens_on_top() {
        let conn = conn();
        let a = add(&conn, "a");
        add(&conn, "b");
        run(&conn, TaskBoardOperation::Toggle, |i| i.task = a.clone()).unwrap();
        assert_eq!(titles(&conn, "done"), vec!["a"]);
        run(&conn, TaskBoardOperation::Move, |i| {
            i.task = a.clone();
            i.column = "todo".into();
        })
        .unwrap();
        run(&conn, TaskBoardOperation::Toggle, |i| i.task = a.clone()).unwrap();
        run(&conn, TaskBoardOperation::Toggle, |i| i.task = a.clone()).unwrap();
        assert_eq!(titles(&conn, "todo"), vec!["a", "b"]);
    }

    #[test]
    fn a_row_carries_what_a_card_shows() {
        let conn = conn();
        let id = run(&conn, TaskBoardOperation::Add, |i| {
            i.title = "card".into();
            i.description = "## Why".into();
            i.tags = "ux, Front".into();
        })
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let rows = board_rows(&conn, "todo", 15).unwrap();
        let row = rows.iter().find(|r| r["id"] == id.as_str()).unwrap();
        assert_eq!(row["priority"], "normal");
        assert_eq!(row["tags"], json!(["front", "ux"]));
        assert_eq!(row["description"], "## Why");
        assert!(row["ref"].as_str().unwrap().starts_with("KT-"));
        assert!(row["created_at"].as_str().is_some() && row["updated_at"].as_str().is_some());
        assert!(row.get("discussion_id").is_none());
    }

    #[test]
    fn an_edit_writes_only_what_changed_and_refuses_an_oversized_description() {
        let conn = conn();
        let long = format!("  {}\n", "🦀é".repeat(1500));
        let id = run(&conn, TaskBoardOperation::Add, |i| i.title = "a".into()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        conn.execute(
            "UPDATE planning_tasks SET description = ?1 WHERE id = ?2",
            rusqlite::params![long, id],
        )
        .unwrap();
        run(&conn, TaskBoardOperation::Edit, |i| {
            i.task = id.clone();
            i.title = "b".into();
            i.description = long.trim().into();
        })
        .unwrap();
        let (title, stored): (String, String) = conn
            .query_row(
                "SELECT title, description FROM planning_tasks WHERE id = ?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((title.as_str(), stored.as_bytes()), ("b", long.as_bytes()));
        let rows = board_rows(&conn, "todo", 15).unwrap();
        assert_eq!(
            rows[0]["description"].as_str().unwrap().as_bytes(),
            long.as_bytes()
        );
        let error = run(&conn, TaskBoardOperation::Edit, |i| {
            i.task = id.clone();
            i.description = "x".repeat(DESCRIPTION_MAX + 1);
        })
        .unwrap_err();
        assert!(error.to_string().contains("at most"), "{error}");
    }

    #[test]
    fn edit_keeps_the_title_when_left_empty() {
        let conn = conn();
        let a = add(&conn, "a");
        run(&conn, TaskBoardOperation::Edit, |i| {
            i.task = a.clone();
            i.description = "new".into();
        })
        .unwrap();
        let rows = board_rows(&conn, "todo", 15).unwrap();
        assert_eq!(
            (rows[0]["title"].as_str(), rows[0]["description"].as_str()),
            (Some("a"), Some("new"))
        );
    }

    #[test]
    fn a_task_without_the_board_tag_is_never_touched() {
        let conn = conn();
        let other = crate::db::planning::create_task(
            &conn,
            &CreatePlanningTaskRequest {
                title: "elsewhere".into(),
                discussion_id: None,
                idempotency_key: None,
                description: String::new(),
                status: PlanningTaskStatus::Todo,
                priority: Default::default(),
                parent_id: None,
                project_ids: vec![],
                tags: vec!["backlog".into()],
                definition_of_done: vec![],
                links: vec![],
                actor: Default::default(),
            },
        )
        .unwrap();
        for operation in [
            TaskBoardOperation::Toggle,
            TaskBoardOperation::Edit,
            TaskBoardOperation::Move,
        ] {
            let error = run(&conn, operation, |i| {
                i.task = other.summary.reference.clone();
                i.title = "hijacked".into();
                i.column = "done".into();
            })
            .unwrap_err();
            assert!(error.to_string().contains("not on the board"), "{error}");
        }
        let (title, status): (String, String) = conn
            .query_row(
                "SELECT title, status FROM planning_tasks WHERE id = ?1",
                [&other.summary.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((title.as_str(), status.as_str()), ("elsewhere", "todo"));
    }

    #[test]
    fn a_task_added_elsewhere_shows_first_and_done_ones_are_bounded() {
        let conn = conn();
        add(&conn, "a");
        crate::db::planning::create_task(
            &conn,
            &CreatePlanningTaskRequest {
                title: "from an agent".into(),
                discussion_id: None,
                idempotency_key: None,
                description: String::new(),
                status: PlanningTaskStatus::Idea,
                priority: Default::default(),
                parent_id: None,
                project_ids: vec![],
                tags: vec!["TODO".into()],
                definition_of_done: vec![],
                links: vec![],
                actor: Default::default(),
            },
        )
        .unwrap();
        assert_eq!(titles(&conn, "todo"), vec!["from an agent", "a"]);
        for n in 0..3 {
            let id = add(&conn, &format!("d{n}"));
            run(&conn, TaskBoardOperation::Toggle, |i| i.task = id.clone()).unwrap();
        }
        let done = board_rows(&conn, "todo", 2)
            .unwrap()
            .into_iter()
            .filter(|r| r["column"] == "done" && r.get("sentinel").is_none())
            .count();
        assert_eq!(done, 2);
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

        let outcome =
            execute_task_board_step(&step, "wf-test", &state, &TemplateContext::new()).await;
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
}
