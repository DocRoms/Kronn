//! KT-1030 — « Ma Todo », the board Kronn installs by default: one Live Page
//! and six agentless workflows built on `TaskBoard` + `PublishPageData`.
//!
//! It is installed once, through the Artifact import machinery (fresh copies,
//! ids remapped), and recorded in `default_contents`. A deleted board is never
//! reinstalled on its own, and an instance that already has its own todo page
//! keeps it untouched.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use rusqlite::Connection;
use serde::Serialize;
use ts_rs::TS;

use crate::db::default_contents::{self, DefaultContentStatus};
use crate::models::{
    ArtifactBundle, ArtifactBundleDataset, ArtifactBundlePage, LivePageDatasetKind,
    LivePageWriteOperation, PromptVariable, PromptVariableControl, PromptVariableSource,
    PublishPageDataConfig, PublishPageDataWrite, StepType, TaskBoardConfig, TaskBoardOperation,
    Workflow, WorkflowSafety, WorkflowStep, WorkflowTrigger,
};

pub const KEY: &str = "todo_board";
/// The planning tag of the board's tasks.
pub const TAG: &str = "todo";
pub const DATASET: &str = "todo";
const BOARD_HTML: &str = include_str!("default_todo/board.html");
/// Titles and slugs a todo page of one's own is recognised by.
const KNOWN_TITLES: [&str; 4] = ["Ma Todo", "My Todo", "Mi Todo", "我的待办"];
const KNOWN_SLUGS: [&str; 4] = ["ma-todo", "my-todo", "mi-todo", "todo"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DefaultTodoState {
    /// Kronn's board exists.
    Installed,
    /// Kronn installed it and the user deleted its page.
    Removed,
    /// The user had their own todo page: Kronn did not install its board.
    KeptExisting,
    /// Never handled (startup has not run, or failed).
    NotInstalled,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DefaultTodoStatus {
    pub state: DefaultTodoState,
    /// Kronn's board page, while it exists.
    pub page_id: Option<String>,
    /// Kronn's board workflows that still exist.
    pub workflow_ids: Vec<String>,
    /// A todo page of the user's own, if one is recognised.
    pub own_page_id: Option<String>,
}

struct Texts {
    title: &'static str,
    slug: &'static str,
    names: [&'static str; 6],
    task: &'static str,
    title_label: &'static str,
    description_label: &'static str,
    description_placeholder: &'static str,
    tags_label: &'static str,
    tags_placeholder: &'static str,
}

fn texts(language: &str) -> Texts {
    match language {
        "fr" => Texts {
            title: "Ma Todo",
            slug: "ma-todo",
            names: [
                "ajouter",
                "faite",
                "déplacer",
                "modifier",
                "discussion",
                "rafraîchir",
            ],
            task: "Tâche (remplie par la page)",
            title_label: "Titre",
            description_label: "Description (facultatif, Markdown)",
            description_placeholder:
                "Le contexte en une ou deux phrases : pourquoi, pour qui, le lien utile…",
            tags_label: "Tags (facultatif)",
            tags_placeholder: "front, urgent",
        },
        "es" => Texts {
            title: "Mi Todo",
            slug: "mi-todo",
            names: [
                "añadir",
                "hecha",
                "mover",
                "editar",
                "discusión",
                "actualizar",
            ],
            task: "Tarea (la rellena la página)",
            title_label: "Título",
            description_label: "Descripción (opcional, Markdown)",
            description_placeholder:
                "El contexto en una o dos frases: por qué, para quién, el enlace útil…",
            tags_label: "Etiquetas (opcional)",
            tags_placeholder: "front, urgente",
        },
        "zh" => Texts {
            title: "我的待办",
            slug: "my-todo",
            names: ["添加", "完成", "移动", "编辑", "讨论", "刷新"],
            task: "任务（由页面填写）",
            title_label: "标题",
            description_label: "描述（可选，支持 Markdown）",
            description_placeholder: "用一两句话说明背景：原因、对象、相关链接……",
            tags_label: "标签（可选）",
            tags_placeholder: "front, urgent",
        },
        _ => Texts {
            title: "My Todo",
            slug: "my-todo",
            names: ["add", "done", "move", "edit", "discussion", "refresh"],
            task: "Task (filled by the page)",
            title_label: "Title",
            description_label: "Description (optional, Markdown)",
            description_placeholder:
                "The context in a sentence or two: why, for whom, the useful link…",
            tags_label: "Tags (optional)",
            tags_placeholder: "front, urgent",
        },
    }
}

fn language_code(language: &str) -> &'static str {
    match language {
        "fr" => "fr",
        "es" => "es",
        "zh" => "zh",
        _ => "en",
    }
}

fn variable(name: &str, label: &str, required: bool) -> PromptVariable {
    PromptVariable {
        name: name.into(),
        label: label.into(),
        placeholder: String::new(),
        description: None,
        required,
        pattern: None,
        source: Some(PromptVariableSource::UserInput),
        source_ref: None,
        allow_manual_override: false,
        control: None,
    }
}

fn workflow(
    id: &str,
    name: String,
    board: TaskBoardConfig,
    variables: Vec<PromptVariable>,
) -> Workflow {
    let now = Utc::now();
    Workflow {
        project_scope: None,
        id: id.into(),
        name,
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps: vec![
            WorkflowStep {
                name: "board".into(),
                step_type: StepType::TaskBoard,
                task_board: Some(board),
                ..Default::default()
            },
            WorkflowStep {
                name: "publish".into(),
                step_type: StepType::PublishPageData,
                page_publish: Some(PublishPageDataConfig {
                    page_id: "kronn-todo".into(),
                    writes: vec![PublishPageDataWrite {
                        dataset: DATASET.into(),
                        operation: LivePageWriteOperation::Replace,
                        value_from: "steps.board.data.rows".into(),
                        observed_at: None,
                        dedupe_key: None,
                        key_field: None,
                    }],
                }),
                ..Default::default()
            },
        ],
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        concurrency_key: None,
        guards: None,
        artifacts: Default::default(),
        on_failure: vec![],
        exec_allowlist: vec![],
        variables,
        enabled: true,
        retention: None,
        pinned: false,
        created_at: now,
        updated_at: now,
    }
}

/// The board's six workflows, in the language Kronn speaks.
pub fn workflows(language: &str) -> Vec<Workflow> {
    let t = texts(language);
    let name = |index: usize| format!("{} — {}", t.title, t.names[index]);
    let board = |operation| TaskBoardConfig {
        tag: TAG.into(),
        operation,
        ..Default::default()
    };
    let task = || variable("task", t.task, true);
    let mut description = variable("description", t.description_label, false);
    description.placeholder = t.description_placeholder.into();
    description.control = Some(PromptVariableControl::Textarea);
    let mut tags = variable("tags", t.tags_label, false);
    tags.placeholder = t.tags_placeholder.into();
    vec![
        workflow(
            "kronn-todo-add",
            name(0),
            TaskBoardConfig {
                title: "{{title}}".into(),
                description: "{{description ?? \"\"}}".into(),
                tags: "{{tags ?? \"\"}}".into(),
                ..board(TaskBoardOperation::Add)
            },
            vec![
                variable("title", t.title_label, true),
                description.clone(),
                tags,
            ],
        ),
        workflow(
            "kronn-todo-toggle",
            name(1),
            TaskBoardConfig {
                task: "{{task}}".into(),
                ..board(TaskBoardOperation::Toggle)
            },
            vec![task()],
        ),
        workflow(
            "kronn-todo-move",
            name(2),
            TaskBoardConfig {
                task: "{{task}}".into(),
                before: "{{before}}".into(),
                column: "{{column}}".into(),
                ..board(TaskBoardOperation::Move)
            },
            vec![
                task(),
                variable("before", "before", true),
                variable("column", "column", true),
            ],
        ),
        workflow(
            "kronn-todo-edit",
            name(3),
            TaskBoardConfig {
                task: "{{task}}".into(),
                title: "{{title ?? \"\"}}".into(),
                description: "{{description ?? \"\"}}".into(),
                ..board(TaskBoardOperation::Edit)
            },
            vec![task(), variable("title", t.title_label, false), description],
        ),
        workflow(
            "kronn-todo-discuss",
            name(4),
            TaskBoardConfig {
                task: "{{task}}".into(),
                ..board(TaskBoardOperation::Discuss)
            },
            vec![task()],
        ),
        workflow(
            "kronn-todo-refresh",
            name(5),
            board(TaskBoardOperation::Read),
            vec![],
        ),
    ]
}

/// The shipped board as a `kronn.artifact` bundle, with the board's current
/// rows as its first snapshot.
pub fn bundle(conn: &Connection, language: &str) -> Result<String> {
    let t = texts(language);
    let rows = crate::workflows::task_board_step::board_rows(conn, TAG, 15)?;
    let now = Utc::now();
    let bundle = ArtifactBundle {
        kind: crate::api::artifact_portability::BUNDLE_KIND.into(),
        version: crate::api::artifact_portability::BUNDLE_VERSION,
        exported_at: now,
        artifact: ArtifactBundlePage {
            id: "kronn-todo".into(),
            title: t.title.into(),
            slug: t.slug.into(),
            html: BOARD_HTML.replace("__KRONN_LANG__", language_code(language)),
            created_by_agent: None,
            datasets: vec![ArtifactBundleDataset {
                name: DATASET.into(),
                kind: LivePageDatasetKind::Snapshot,
                has_current: true,
                current: serde_json::Value::Array(rows),
                schema: None,
                max_points: 1,
                max_age_days: None,
                updated_at: now,
                points: vec![],
            }],
            embed_origins: vec![],
        },
        referenced_artifacts: vec![],
        referenced_workflows: workflows(language),
        referenced_quick_prompts: vec![],
        referenced_quick_apis: vec![],
        referenced_quick_execs: vec![],
        redacted_fields: vec![],
    };
    Ok(serde_json::to_string(&bundle)?)
}

/// A todo page of the user's own: one titled or slugged like Kronn's, that
/// Kronn did not install.
fn own_page(conn: &Connection, installed: Option<&str>) -> Result<Option<String>> {
    let mut statement =
        conn.prepare("SELECT id, title, slug FROM live_pages ORDER BY created_at")?;
    let pages = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pages
        .into_iter()
        .find(|(id, title, slug)| {
            Some(id.as_str()) != installed
                && (KNOWN_TITLES
                    .iter()
                    .any(|known| known.to_lowercase() == title.trim().to_lowercase())
                    || KNOWN_SLUGS.contains(&slug.as_str()))
        })
        .map(|(id, _, _)| id))
}

fn page_exists(conn: &Connection, page_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_pages WHERE id = ?1)",
        [page_id],
        |row| row.get(0),
    )?)
}

fn existing_workflows(conn: &Connection, ids: &[String]) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for id in ids {
        if crate::db::workflows::get_workflow(conn, id)?.is_some() {
            found.push(id.clone());
        }
    }
    Ok(found)
}

pub fn status(conn: &Connection) -> Result<DefaultTodoStatus> {
    let record = default_contents::get(conn, KEY)?;
    let page_id = match record.as_ref().and_then(|r| r.page_id.clone()) {
        Some(id) if page_exists(conn, &id)? => Some(id),
        _ => None,
    };
    let workflow_ids = match &record {
        Some(record) => existing_workflows(conn, &record.workflow_ids)?,
        None => vec![],
    };
    let state = match record.map(|r| r.status) {
        None => DefaultTodoState::NotInstalled,
        Some(DefaultContentStatus::KeptExisting) if page_id.is_none() => {
            DefaultTodoState::KeptExisting
        }
        Some(_) if page_id.is_some() => DefaultTodoState::Installed,
        Some(_) => DefaultTodoState::Removed,
    };
    let own_page_id = own_page(conn, page_id.as_deref())?;
    Ok(DefaultTodoStatus {
        state,
        page_id,
        workflow_ids,
        own_page_id,
    })
}

/// Create the board, enable its workflows and record it, in one transaction.
fn install_in(conn: &Connection, language: &str) -> Result<(String, Vec<String>)> {
    let tx = conn.unchecked_transaction()?;
    let content = bundle(&tx, language)?;
    let imported = crate::api::artifact_portability::import_shipped_in_transaction(&tx, content)?;
    let page = imported.artifact;
    let html: String = tx.query_row(
        "SELECT html FROM live_page_revisions WHERE id = ?1",
        [&page.current_revision_id],
        |row| row.get(0),
    )?;
    let mut workflow_ids: Vec<String> = Vec::new();
    for (_, raw) in crate::db::live_page_actions::extract_page_action_blocks(&html) {
        let action: serde_json::Value = serde_json::from_str(&raw)?;
        let id = action["target_id"]
            .as_str()
            .context("Shipped action without a target")?
            .to_string();
        if !workflow_ids.contains(&id) {
            workflow_ids.push(id);
        }
    }
    if workflow_ids.len() != 6 {
        bail!("The shipped board must wire six workflows");
    }
    // First-party, agentless definitions shipped in the binary: Kronn itself
    // enables them, or the board would not work without configuration.
    for id in &workflow_ids {
        tx.execute("UPDATE workflows SET enabled = 1 WHERE id = ?1", [id])?;
        crate::db::workflows::clear_auto_disabled(&tx, id)?;
    }
    default_contents::put(
        &tx,
        KEY,
        DefaultContentStatus::Installed,
        Some(&page.id),
        &workflow_ids,
    )?;
    tx.commit()?;
    Ok((page.id, workflow_ids))
}

/// First launch (or first launch of a version that ships the board): install
/// it once. Never again after that, whatever the user did with it.
pub fn ensure_installed(conn: &Connection, language: &str) -> Result<DefaultTodoState> {
    if default_contents::get(conn, KEY)?.is_some() {
        return Ok(status(conn)?.state);
    }
    if own_page(conn, None)?.is_some() {
        default_contents::put(conn, KEY, DefaultContentStatus::KeptExisting, None, &[])?;
        return Ok(DefaultTodoState::KeptExisting);
    }
    install_in(conn, language)?;
    Ok(DefaultTodoState::Installed)
}

/// An explicit (re)install asked by a human: a fresh copy, refused while
/// Kronn's board page still exists. An own todo page is left as it is.
pub fn reinstall(conn: &Connection, language: &str) -> Result<DefaultTodoStatus> {
    if status(conn)?.state == DefaultTodoState::Installed {
        bail!("Kronn's todo board is already installed");
    }
    install_in(conn, language)?;
    status(conn)
}

/// Boot hook shared by the standalone and desktop mains; never fatal.
pub async fn install_on_boot(database: &crate::db::Database, language: &str) {
    let language = language.to_string();
    match database
        .with_conn(move |conn| ensure_installed(conn, &language))
        .await
    {
        Ok(DefaultTodoState::Installed) => tracing::info!("Default todo board present"),
        Ok(state) => tracing::info!("Default todo board not installed: {state:?}"),
        Err(error) => tracing::warn!("Default todo board could not be installed: {error}"),
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

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn pages(conn: &Connection) -> i64 {
        count(conn, "SELECT COUNT(*) FROM live_pages")
    }

    fn workflows_count(conn: &Connection) -> i64 {
        count(conn, "SELECT COUNT(*) FROM workflows")
    }

    fn own_todo(conn: &Connection) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO live_pages (id, project_id, title, slug, current_revision_id, data_revision, created_at, updated_at)
             VALUES ('own-page', NULL, 'Ma Todo', 'ma-todo', 'own-rev', 0, ?1, ?1)",
            [&now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO live_page_revisions (id, page_id, revision, html, created_at)
             VALUES ('own-rev', 'own-page', 1, '<h1>mine</h1>', ?1)",
            [&now],
        )
        .unwrap();
    }

    #[test]
    fn first_launch_installs_the_board_once_and_a_restart_changes_nothing() {
        let conn = conn();
        assert_eq!(
            ensure_installed(&conn, "fr").unwrap(),
            DefaultTodoState::Installed
        );
        let status = status(&conn).unwrap();
        assert_eq!(status.state, DefaultTodoState::Installed);
        assert_eq!(status.workflow_ids.len(), 6);
        assert_eq!((pages(&conn), workflows_count(&conn)), (1, 6));
        let enabled = count(
            &conn,
            "SELECT COUNT(*) FROM workflows WHERE enabled = 1 AND disabled_reason IS NULL",
        );
        assert_eq!(enabled, 6, "Kronn's own agentless workflows arrive usable");
        let page = crate::db::live_pages::get_live_page(&conn, status.page_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(page.page.title, "Ma Todo");
        assert!(page.revision.html.contains("var LANG = 'fr'"));

        for _ in 0..2 {
            assert_eq!(
                ensure_installed(&conn, "fr").unwrap(),
                DefaultTodoState::Installed
            );
        }
        assert_eq!(
            (pages(&conn), workflows_count(&conn)),
            (1, 6),
            "never duplicated"
        );
    }

    #[test]
    fn a_deleted_board_is_never_reinstalled_on_its_own_but_can_be_on_request() {
        let conn = conn();
        ensure_installed(&conn, "en").unwrap();
        let installed = status(&conn).unwrap();
        crate::db::live_pages::delete_live_page(&conn, installed.page_id.as_deref().unwrap())
            .unwrap();
        for id in &installed.workflow_ids {
            crate::db::workflows::delete_workflow(&conn, id).unwrap();
        }
        assert_eq!(
            ensure_installed(&conn, "en").unwrap(),
            DefaultTodoState::Removed
        );
        assert_eq!((pages(&conn), workflows_count(&conn)), (0, 0));

        let again = reinstall(&conn, "en").unwrap();
        assert_eq!(again.state, DefaultTodoState::Installed);
        assert_ne!(again.page_id, installed.page_id);
        assert_eq!((pages(&conn), workflows_count(&conn)), (1, 6));
        assert!(reinstall(&conn, "en")
            .unwrap_err()
            .to_string()
            .contains("already installed"));
        assert_eq!(
            ensure_installed(&conn, "en").unwrap(),
            DefaultTodoState::Installed
        );
        assert_eq!((pages(&conn), workflows_count(&conn)), (1, 6));
    }

    #[test]
    fn an_own_todo_page_is_kept_and_never_overwritten() {
        let conn = conn();
        own_todo(&conn);
        assert_eq!(
            ensure_installed(&conn, "fr").unwrap(),
            DefaultTodoState::KeptExisting
        );
        assert_eq!((pages(&conn), workflows_count(&conn)), (1, 0));
        let status = status(&conn).unwrap();
        assert_eq!(status.state, DefaultTodoState::KeptExisting);
        assert_eq!(status.own_page_id.as_deref(), Some("own-page"));
        assert_eq!(
            ensure_installed(&conn, "fr").unwrap(),
            DefaultTodoState::KeptExisting
        );

        // Switching is the human's call: Kronn's board joins, theirs stays as it was.
        let switched = reinstall(&conn, "fr").unwrap();
        assert_eq!(switched.state, DefaultTodoState::Installed);
        assert_eq!(switched.own_page_id.as_deref(), Some("own-page"));
        let own = crate::db::live_pages::get_live_page(&conn, "own-page")
            .unwrap()
            .unwrap();
        assert_eq!(own.revision.html, "<h1>mine</h1>");
        assert_eq!(own.page.slug, "ma-todo");
        assert_ne!(switched.page_id.as_deref(), Some("own-page"));
    }

    #[test]
    fn the_shipped_board_depends_on_nothing_outside_kronn() {
        let conn = conn();
        let content = bundle(&conn, "en").unwrap();
        for forbidden in [
            "python",
            "127.0.0.1",
            "localhost",
            "http://",
            "https://",
            ":3140",
            ":5173",
        ] {
            assert!(
                !content.contains(forbidden),
                "shipped board mentions {forbidden}"
            );
        }
        for workflow in workflows("en") {
            assert!(workflow.exec_allowlist.is_empty());
            assert!(workflow.project_id.is_none());
            for step in &workflow.steps {
                assert!(
                    matches!(
                        step.step_type,
                        StepType::TaskBoard | StepType::PublishPageData
                    ),
                    "{}: {:?}",
                    workflow.name,
                    step.step_type
                );
            }
            crate::api::workflows::validate_workflow_for_import(&workflow).unwrap();
        }
    }

    #[test]
    fn moves_and_toggles_are_trust_eligible_and_nothing_is_pre_approved() {
        let conn = conn();
        ensure_installed(&conn, "en").unwrap();
        let page_id = status(&conn).unwrap().page_id.unwrap();
        let states = crate::db::live_page_action_trusts::list_for_page(&conn, &page_id).unwrap();
        assert_eq!(states.len(), 6);
        for state in &states {
            let typed = matches!(state.action_ref.as_str(), "todo-add" | "todo-edit");
            assert_eq!(
                state.fingerprint.is_some(),
                !typed,
                "{}: {:?}",
                state.action_ref,
                state.refusal
            );
            assert!(
                state.trust.is_none() && !state.active,
                "{} pre-approved",
                state.action_ref
            );
        }
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM live_page_action_trusts"),
            0
        );
    }
}
