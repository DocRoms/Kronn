//! The agent / named-connection pairs a workflow's steps run on (KT-2032).
//!
//! A step's `agent_settings.connection_id`, a shadow's agent and its merge's
//! agent each pair an agent with a connection. The run refuses a pair whose
//! connection is gone or goes with another agent
//! (`steps::resolve_step_connection`); saving refuses it first with the same
//! rule (`external_api_connections::agent_mismatch`), and the readiness check
//! reports it on a workflow saved before.

use crate::models::{AgentType, ExternalApiConnection, ShadowAgent, StepType, WorkflowStep};

/// One pair a run would check.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionPair {
    pub step: String,
    /// `Some("shadow `x`")` or `Some("merge")`; `None` for the step itself.
    pub part: Option<String>,
    pub agent: AgentType,
    pub connection_id: String,
}

/// A pair the run would refuse, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct PairRefusal {
    pub pair: ConnectionPair,
    /// The agent the connection goes with; `None` when it does not exist.
    pub expected: Option<AgentType>,
}

impl PairRefusal {
    pub fn message(&self) -> String {
        let pair = &self.pair;
        let place = match &pair.part {
            Some(part) => format!("Step `{}`, {part}", pair.step),
            None => format!("Step `{}`", pair.step),
        };
        let connection = &pair.connection_id;
        match &self.expected {
            Some(expected) => format!(
                "{place}: connection {connection} goes with agent {expected:?}, not {:?}: use agent {expected:?} for connection {connection}",
                pair.agent
            ),
            None => format!(
                "{place}: connection {connection} does not exist: pick an existing connection"
            ),
        }
    }
}

fn shadow_pair(step: &str, part: String, choice: &ShadowAgent) -> Option<ConnectionPair> {
    // As `shadow::apply_shadow_agent` applies it: a blank id is no connection.
    let connection_id = choice
        .connection_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())?;
    Some(ConnectionPair {
        step: step.to_string(),
        part: Some(part),
        agent: choice.agent.clone(),
        connection_id: connection_id.to_string(),
    })
}

/// Every pair `steps` name: each Agent step's own connection, each shadow's
/// agent and the shadow merge's agent.
pub fn pairs(steps: &[WorkflowStep]) -> Vec<ConnectionPair> {
    let mut found = Vec::new();
    for step in steps {
        let name = &step.name;
        if step.step_type == StepType::Agent {
            if let Some(connection_id) = step
                .agent_settings
                .as_ref()
                .and_then(|settings| settings.connection_id.as_deref())
                .filter(|id| !id.trim().is_empty())
            {
                found.push(ConnectionPair {
                    step: name.clone(),
                    part: None,
                    agent: step.agent.clone(),
                    connection_id: connection_id.to_string(),
                });
            }
        }
        for shadow in &step.shadows {
            if let Some(choice) = &shadow.agent {
                found.extend(shadow_pair(name, format!("shadow `{}`", shadow.id), choice));
            }
        }
        if let Some(choice) = step.shadow_merge.as_ref().and_then(|m| m.agent.as_ref()) {
            found.extend(shadow_pair(name, "merge".into(), choice));
        }
    }
    found
}

/// The pairs of `steps` the run would refuse, each connection read through
/// `lookup` (`Ok(None)` = it does not exist).
pub fn refusals(
    steps: &[WorkflowStep],
    mut lookup: impl FnMut(&str) -> anyhow::Result<Option<ExternalApiConnection>>,
) -> anyhow::Result<Vec<PairRefusal>> {
    let mut refused = Vec::new();
    for pair in pairs(steps) {
        let expected = match lookup(&pair.connection_id)? {
            Some(connection) => {
                match crate::db::external_api_connections::agent_mismatch(&connection, &pair.agent)
                {
                    Some(expected) => Some(expected),
                    None => continue,
                }
            }
            None => None,
        };
        refused.push(PairRefusal { pair, expected });
    }
    Ok(refused)
}

/// [`refusals`] against the connections stored in `conn`.
pub fn refusals_in_db(
    conn: &rusqlite::Connection,
    steps: &[WorkflowStep],
) -> anyhow::Result<Vec<PairRefusal>> {
    refusals(steps, |id| {
        crate::db::external_api_connections::get(conn, id)
    })
}

/// The save's check: `Err` with every refused pair of both chains, one per line.
pub fn validate_in_db(
    conn: &rusqlite::Connection,
    steps: &[WorkflowStep],
    on_failure: &[WorkflowStep],
) -> Result<(), String> {
    let mut messages = Vec::new();
    for chain in [steps, on_failure] {
        let refused = refusals_in_db(conn, chain)
            .map_err(|error| format!("The step connections could not be checked: {error}"))?;
        messages.extend(refused.iter().map(PairRefusal::message));
    }
    if messages.is_empty() {
        Ok(())
    } else {
        Err(messages.join("\n"))
    }
}
