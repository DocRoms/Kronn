//! Calls a lower crate makes into a higher one. The lower crate declares the
//! call here; the crate that holds the implementation submits it, and the
//! linker gathers the submissions of every crate in the binary.

use crate::core::anti_halluc::InvalidCard;
use crate::core::secret_scrub::SecretSet;
use crate::models::{DiscussionMessage, McpServer, PlanningActor, PlanningDodItem, TaskExecution};
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;

/// The implementation linked into this binary. `None` only in the test binary
/// of a crate below the one that provides it.
pub fn linked<T: inventory::Collect>() -> Option<&'static T> {
    inventory::iter::<T>.into_iter().next()
}

/// As `linked`, for a call that has no meaning without its implementation.
pub fn required<T: inventory::Collect>() -> Result<&'static T> {
    linked::<T>().ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not linked into this binary",
            std::any::type_name::<T>()
        )
    })
}

/// What an agent message in the main channel carries besides its text
/// (actions, question cards, an audit validation), stored with the message.
pub struct AgentMessageIngest(pub fn(&Connection, &str, &DiscussionMessage, &str) -> Result<()>);
inventory::collect!(AgentMessageIngest);

/// The question cards of one message: (connection, discussion, message, content).
pub struct MessageQuestionIngest(pub fn(&Connection, &str, &str, &str) -> Result<Vec<InvalidCard>>);
inventory::collect!(MessageQuestionIngest);

/// The actions of a Page revision: (connection, page, revision, html).
pub struct PageActionIngest(pub fn(&Connection, &str, &str, &str) -> Result<()>);
inventory::collect!(PageActionIngest);

/// Which execution a lookup means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionOf {
    /// The task's execution that has not ended.
    ActiveForTask,
    /// The task's most recent execution.
    LatestForTask,
    /// An execution id, or a task reference standing for its execution.
    Reference,
}

pub struct TaskExecutionLookup(
    pub fn(&Connection, &str, ExecutionOf) -> Result<Option<TaskExecution>>,
);
inventory::collect!(TaskExecutionLookup);

/// A task's edited DoD adopted by its live executions, in the caller's transaction.
pub struct TaskDodAdoption(pub DodAdoptionFn);
/// (connection, task id, edited DoD, actor) -> the executions that adopted it.
pub type DodAdoptionFn =
    fn(&Connection, &str, &[PlanningDodItem], &PlanningActor) -> Result<Vec<String>>;
inventory::collect!(TaskDodAdoption);

/// Approvals taken back once what they covered is deleted: (connection, by).
pub struct TrustWithdrawal(pub fn(&Connection, &str) -> Result<()>);
inventory::collect!(TrustWithdrawal);

/// The secret values an API plugin's calls carry, for scrubbing.
pub struct ApiCallSecrets(pub fn(&McpServer, &HashMap<String, String>) -> SecretSet);
inventory::collect!(ApiCallSecrets);

/// The declarations of every tool family `tools_load` can bring in.
pub struct LoadableToolDeclarations(pub fn() -> Vec<serde_json::Value>);
inventory::collect!(LoadableToolDeclarations);

/// The Quick Prompts and skills behind a Page: (connection, page id or slug).
pub struct PageResources(
    pub fn(&Connection, &str) -> Result<Option<Vec<crate::models::LivePageResource>>>,
);
inventory::collect!(PageResources);

/// A setting the running process reads outside the config, republished at
/// every save by the crate that holds it.
pub struct ConfigApplied(pub fn(&crate::models::AppConfig));
inventory::collect!(ConfigApplied);

/// A migration of the loaded config; `true` when it changed something to save.
pub struct ConfigMigration(pub fn(&mut crate::models::AppConfig) -> bool);
inventory::collect!(ConfigMigration);

/// Runs `work`, then the withdrawal, in one transaction: both stand or fall
/// together. Inside a transaction the caller already holds, both join it.
pub fn delete_and_withdraw<T>(
    conn: &Connection,
    by: &str,
    work: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let withdraw = |conn: &Connection| match linked::<TrustWithdrawal>() {
        Some(hook) => (hook.0)(conn, by),
        None => Ok(()),
    };
    if !conn.is_autocommit() {
        let out = work(conn)?;
        withdraw(conn)?;
        return Ok(out);
    }
    let tx = conn.unchecked_transaction()?;
    let out = work(&tx)?;
    withdraw(&tx)?;
    tx.commit()?;
    Ok(out)
}
