//! Durable ceiling questions and human budget decisions. Grants change counters
//! only; loop-detection guards and global timeouts remain enforced.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::agents::tools::CeilingAllowance;

/// Every question Kronn asks about a ceiling carries a key with this prefix.
pub const QUESTION_KEY_PREFIX: &str = "kronn-ceiling-";
/// The options of that question, in the order they are shown.
pub const OPTION_GRANT: &str = "grant";
pub const OPTION_UNLIMITED: &str = "no-limit-discussion";
pub const OPTION_STOP: &str = "stop";
/// Rounds granted at once: a third of the default run, enough to finish an
/// enumeration, not enough to lose a loop in.
pub const ROUND_STEP: usize = 50;

/// Calls granted at once for a tool: its own ceiling, at most fifty. The
/// twelve-call guard on paid APIs stays twelve at a time.
pub fn tool_step(limit: usize) -> usize {
    limit.clamp(1, 50)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ceiling {
    Tool {
        tool: String,
        limit: usize,
        step: usize,
    },
    Rounds {
        limit: usize,
        step: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// One more step, for the next run of the agent that hit the ceiling.
    Grant,
    /// The tool counters are lifted for the rest of the discussion.
    Unlimited,
    /// The partial answer stands; nobody is woken up.
    Stop,
}

impl Decision {
    fn from_options(selected: &[String]) -> Option<Self> {
        selected.iter().find_map(|id| match id.as_str() {
            OPTION_GRANT => Some(Self::Grant),
            OPTION_UNLIMITED => Some(Self::Unlimited),
            OPTION_STOP => Some(Self::Stop),
            _ => None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Grant => "grant",
            Self::Unlimited => "unlimited",
            Self::Stop => "stop",
        }
    }
}

/// What an answer to a discussion question meant for the ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    /// Not a question Kronn asked about a ceiling.
    NotACeiling,
    Decided(Decision),
    /// A ceiling question answered with free text only: the agent reads it,
    /// no budget moves.
    Undecided,
}

pub fn record(
    conn: &Connection,
    discussion_id: &str,
    question_key: &str,
    agent_type: &str,
    ceilings: &[Ceiling],
) -> Result<()> {
    conn.execute(
        "INSERT INTO discussion_ceiling_requests
             (discussion_id, question_key, agent_type, ceilings_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(discussion_id, question_key) DO NOTHING",
        params![
            discussion_id,
            question_key,
            agent_type,
            serde_json::to_string(ceilings)?,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// Record the human's answer to `question_key`. Only a question Kronn itself
/// recorded counts: a look-alike written by an agent is `NotACeiling`.
pub fn decide(
    conn: &Connection,
    discussion_id: &str,
    question_key: &str,
    selected_option_ids: &[String],
) -> Result<Answered> {
    let known: Option<String> = conn
        .query_row(
            "SELECT question_key FROM discussion_ceiling_requests
             WHERE discussion_id = ?1 AND question_key = ?2",
            params![discussion_id, question_key],
            |row| row.get(0),
        )
        .optional()?;
    if known.is_none() {
        return Ok(Answered::NotACeiling);
    }
    let Some(decision) = Decision::from_options(selected_option_ids) else {
        return Ok(Answered::Undecided);
    };
    conn.execute(
        "UPDATE discussion_ceiling_requests SET decision = ?3, decided_at = ?4
         WHERE discussion_id = ?1 AND question_key = ?2 AND decision IS NULL",
        params![
            discussion_id,
            question_key,
            decision.as_str(),
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(Answered::Decided(decision))
}

/// What the next run of `agent_type` in this discussion may spend on top of
/// the ceilings. A one-run grant is consumed here, by the run that reads it.
pub fn take_allowance(
    conn: &Connection,
    discussion_id: &str,
    agent_type: &str,
) -> Result<CeilingAllowance> {
    let mut allowance = CeilingAllowance {
        ask_on_ceiling: true,
        ..CeilingAllowance::default()
    };
    // Lifted counters hold for the whole discussion, whoever runs next.
    let mut lifted = conn.prepare(
        "SELECT ceilings_json FROM discussion_ceiling_requests
         WHERE discussion_id = ?1 AND decision = 'unlimited'",
    )?;
    for ceilings in lifted.query_map(params![discussion_id], |row| row.get::<_, String>(0))? {
        for ceiling in serde_json::from_str::<Vec<Ceiling>>(&ceilings?)? {
            if let Ceiling::Tool { tool, .. } = ceiling {
                allowance.unlimited_tools.insert(tool);
            }
        }
    }
    // One step each, for the agent that asked, spent by its next run. An
    // unlimited answer still grants its rounds this way: no option lifts the
    // round ceiling for good.
    let mut pending = conn.prepare(
        "SELECT question_key, decision, ceilings_json FROM discussion_ceiling_requests
         WHERE discussion_id = ?1 AND agent_type = ?2 AND consumed_at IS NULL
           AND decision IN ('grant', 'unlimited')",
    )?;
    let rows = pending
        .query_map(params![discussion_id, agent_type], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let now = Utc::now().to_rfc3339();
    for (key, decision, ceilings) in rows {
        for ceiling in serde_json::from_str::<Vec<Ceiling>>(&ceilings)? {
            match ceiling {
                Ceiling::Tool { tool, step, .. } if decision == "grant" => {
                    *allowance.extra_calls.entry(tool).or_insert(0) += step;
                }
                Ceiling::Tool { .. } => {}
                Ceiling::Rounds { step, .. } => allowance.extra_rounds += step,
            }
        }
        conn.execute(
            "UPDATE discussion_ceiling_requests SET consumed_at = ?3
             WHERE discussion_id = ?1 AND question_key = ?2",
            params![discussion_id, key, now],
        )?;
    }
    Ok(allowance)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at) \
             VALUES ('disc', 'Ceilings', '2026-09-19T00:00:00Z', '2026-09-19T00:00:00Z')",
            [],
        )
        .unwrap();
        conn
    }

    fn web_fetch() -> Ceiling {
        Ceiling::Tool {
            tool: "web_fetch".into(),
            limit: 120,
            step: tool_step(120),
        }
    }

    fn rounds() -> Ceiling {
        Ceiling::Rounds {
            limit: 150,
            step: ROUND_STEP,
        }
    }

    fn select(option: &str) -> Vec<String> {
        vec![option.to_string()]
    }

    #[test]
    fn a_grant_is_spent_by_the_next_run_of_the_agent_that_asked() {
        let conn = connection();
        record(
            &conn,
            "disc",
            "kronn-ceiling-a",
            "Ollama",
            &[web_fetch(), rounds()],
        )
        .unwrap();
        assert_eq!(
            decide(&conn, "disc", "kronn-ceiling-a", &select(OPTION_GRANT)).unwrap(),
            Answered::Decided(Decision::Grant)
        );

        // Another agent in the room does not spend it.
        assert!(take_allowance(&conn, "disc", "Custom")
            .unwrap()
            .extra_calls
            .is_empty());

        let allowance = take_allowance(&conn, "disc", "Ollama").unwrap();
        assert_eq!(allowance.extra_calls.get("web_fetch"), Some(&50));
        assert_eq!(allowance.extra_rounds, ROUND_STEP);
        assert!(allowance.ask_on_ceiling);

        let spent = take_allowance(&conn, "disc", "Ollama").unwrap();
        assert!(spent.extra_calls.is_empty(), "one run only");
        assert_eq!(spent.extra_rounds, 0);
    }

    #[test]
    fn no_limit_lifts_the_tool_counter_for_the_whole_discussion_but_never_the_rounds() {
        let conn = connection();
        record(
            &conn,
            "disc",
            "kronn-ceiling-b",
            "Ollama",
            &[web_fetch(), rounds()],
        )
        .unwrap();
        decide(&conn, "disc", "kronn-ceiling-b", &select(OPTION_UNLIMITED)).unwrap();

        let first = take_allowance(&conn, "disc", "Ollama").unwrap();
        assert!(first.unlimited_tools.contains("web_fetch"));
        assert_eq!(first.extra_rounds, ROUND_STEP, "the rounds get one step");

        for agent in ["Ollama", "Custom"] {
            let later = take_allowance(&conn, "disc", agent).unwrap();
            assert!(later.unlimited_tools.contains("web_fetch"), "{agent}");
            assert_eq!(later.extra_rounds, 0, "{agent}");
        }
    }

    #[test]
    fn stopping_or_answering_in_prose_grants_nothing() {
        let conn = connection();
        record(&conn, "disc", "kronn-ceiling-c", "Ollama", &[web_fetch()]).unwrap();
        assert_eq!(
            decide(&conn, "disc", "kronn-ceiling-c", &select(OPTION_STOP)).unwrap(),
            Answered::Decided(Decision::Stop)
        );
        record(&conn, "disc", "kronn-ceiling-d", "Ollama", &[web_fetch()]).unwrap();
        assert_eq!(
            decide(&conn, "disc", "kronn-ceiling-d", &[]).unwrap(),
            Answered::Undecided
        );

        let allowance = take_allowance(&conn, "disc", "Ollama").unwrap();
        assert!(allowance.extra_calls.is_empty());
        assert!(allowance.unlimited_tools.is_empty());
    }

    #[test]
    fn a_question_kronn_did_not_record_cannot_grant_a_budget() {
        let conn = connection();
        // An agent can write a question with the same key and options in its
        // own message; only Kronn's record makes the answer count.
        assert_eq!(
            decide(
                &conn,
                "disc",
                "kronn-ceiling-forged",
                &select(OPTION_UNLIMITED)
            )
            .unwrap(),
            Answered::NotACeiling
        );
        assert!(take_allowance(&conn, "disc", "Ollama")
            .unwrap()
            .unlimited_tools
            .is_empty());
    }

    #[test]
    fn a_grant_follows_the_tool_s_own_ceiling_up_to_fifty() {
        assert_eq!(tool_step(12), 12);
        assert_eq!(tool_step(48), 48);
        assert_eq!(tool_step(120), 50);
    }
}
