//! What an audit run consumed, per phase and in total (KT-1084).
//!
//! Aggregation only: every figure is read from what the phases already stored,
//! none is re-measured. Absence stays absence — a phase whose agents reported
//! nothing has no figure, never 0 (the rule of `core::pricing` and the step
//! rows).
//!
//! - briefing: the project's briefing discussion(s) opened since the previous
//!   Full run (the briefing is linked to the project, not to a run);
//! - audit: the run's own steps plus the ones it carried over, each counted once;
//! - validation: the agent replies of the run's validation discussion.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{AuditPhaseCost, AuditRun, AuditRunCost, AuditRunStep};

/// Titles `start_briefing` gives the briefing discussion, one per language.
const BRIEFING_TITLES: [&str; 3] = [
    "Project Briefing",
    "Briefing projet",
    "Briefing del proyecto",
];

/// One step or one agent reply, reduced to what the phases add up.
#[derive(Default)]
struct Item {
    total: Option<u64>,
    /// Uncached input; only set when the item's cache split is known.
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
    cost_micros: Option<u64>,
    cost_estimated: bool,
    carried: bool,
}

/// `None` only when neither side is known: a partial sum is a floor, not unknown.
fn add_opt(sum: Option<u64>, part: Option<u64>) -> Option<u64> {
    match (sum, part) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
    }
}

fn accumulate(items: impl IntoIterator<Item = Item>) -> AuditPhaseCost {
    let mut phase = AuditPhaseCost::default();
    for item in items {
        phase.items += 1;
        if item.total.is_some() {
            phase.measured_items += 1;
            if item.input.is_none() {
                phase.split_unknown_items += 1;
            }
        }
        phase.total_tokens = add_opt(phase.total_tokens, item.total);
        phase.input_tokens = add_opt(phase.input_tokens, item.input);
        phase.output_tokens = add_opt(phase.output_tokens, item.output);
        phase.cache_read_tokens = add_opt(phase.cache_read_tokens, item.cache_read);
        phase.cache_write_tokens = add_opt(phase.cache_write_tokens, item.cache_write);
        match item.cost_micros {
            Some(micros) => {
                phase.cost_usd_micros = add_opt(phase.cost_usd_micros, Some(micros));
                phase.cost_estimated |= item.cost_estimated;
            }
            None => phase.cost_unknown_items += 1,
        }
        phase.carried_items += u32::from(item.carried);
    }
    phase
}

/// The sum of the phases, under the same rules as each of them.
fn total_of(phases: [&AuditPhaseCost; 3]) -> AuditPhaseCost {
    let mut total = AuditPhaseCost::default();
    for phase in phases {
        total.items += phase.items;
        total.measured_items += phase.measured_items;
        total.total_tokens = add_opt(total.total_tokens, phase.total_tokens);
        total.input_tokens = add_opt(total.input_tokens, phase.input_tokens);
        total.output_tokens = add_opt(total.output_tokens, phase.output_tokens);
        total.cache_read_tokens = add_opt(total.cache_read_tokens, phase.cache_read_tokens);
        total.cache_write_tokens = add_opt(total.cache_write_tokens, phase.cache_write_tokens);
        total.split_unknown_items += phase.split_unknown_items;
        total.cost_usd_micros = add_opt(total.cost_usd_micros, phase.cost_usd_micros);
        total.cost_estimated |= phase.cost_estimated;
        total.cost_unknown_items += phase.cost_unknown_items;
        total.carried_items += phase.carried_items;
    }
    total
}

/// A finished step, priced (`price_steps`). A step still running is not counted.
fn step_item(step: &AuditRunStep) -> Option<Item> {
    step.ended_at?;
    let (cost_micros, cost_estimated) = match (step.cost_usd_micros, step.estimated_cost_usd_micros)
    {
        (Some(reported), _) => (Some(reported), false),
        (None, Some(estimated)) => (Some(estimated), true),
        (None, None) => (None, false),
    };
    let breakdown = step.breakdown.unwrap_or_default();
    Some(Item {
        total: step.step_tokens,
        input: breakdown.uncached_input,
        output: breakdown.output.or(step.output_tokens),
        cache_read: breakdown.cache_read.or(step.cache_read_tokens),
        cache_write: breakdown.cache_write.or(step.cache_write_tokens),
        cost_micros,
        cost_estimated,
        carried: step.carried_from_run_id.is_some(),
    })
}

/// The audit phase: every row of the run, a carried one standing for the run
/// that really ran it, so the lineage's cost is counted once.
pub fn audit_phase(conn: &Connection, run_id: &str) -> Result<AuditPhaseCost> {
    let mut steps = crate::db::audit_runs::list_audit_steps(conn, run_id)?;
    crate::db::audit_runs::price_steps(conn, &mut steps)?;
    Ok(accumulate(steps.iter().filter_map(step_item)))
}

/// The agent replies of one discussion. Its stored per-reply cost is either the
/// agent's own report or Kronn's pricing and the row does not say which, so it
/// is always flagged estimated.
fn discussion_items(conn: &Connection, discussion_id: &str) -> Result<Vec<Item>> {
    let mut statement = conn.prepare(
        "SELECT m.tokens_used, m.cost_usd,
                u.input_tokens, u.output_tokens, u.cache_read_tokens, u.cache_write_tokens
           FROM messages m
           LEFT JOIN message_usage u ON u.message_id = m.id
          WHERE m.discussion_id = ?1 AND m.role = 'Agent' AND m.agent_type IS NOT NULL
          ORDER BY m.sort_order",
    )?;
    let rows = statement
        .query_map(params![discussion_id], |row| {
            let tokens_used = row.get::<_, i64>(0)?.max(0) as u64;
            let cost_usd: Option<f64> = row.get(1)?;
            let read = |index| -> rusqlite::Result<Option<u64>> {
                Ok(row.get::<_, Option<i64>>(index)?.map(|v| v.max(0) as u64))
            };
            let (input, output, cache_read, cache_write) = (read(2)?, read(3)?, read(4)?, read(5)?);
            // The counters are stored only with a known cache split: the headline
            // is then fresh traffic, as a step's is. Without them it is the total
            // the reply reported, and 0 there means nothing was reported.
            let split_known = input.is_some() && output.is_some() && cache_read.is_some();
            let total = if split_known {
                add_opt(input, output)
            } else {
                (tokens_used > 0).then_some(tokens_used)
            };
            let cost_micros = cost_usd.and_then(crate::agents::chat_codec::usd_to_micros);
            Ok(Item {
                total,
                input: if split_known { input } else { None },
                output: if split_known { output } else { None },
                cache_read: if split_known { cache_read } else { None },
                cache_write: if split_known { cache_write } else { None },
                cost_micros,
                cost_estimated: cost_micros.is_some(),
                carried: false,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The briefing discussions of the run's project opened after the previous Full
/// run began and not after this one did. A briefing is not linked to a run, and
/// only a Full audit is preceded by one.
fn briefing_discussion_ids(conn: &Connection, run: &AuditRun) -> Result<Vec<String>> {
    if run.kind != "Full" {
        return Ok(Vec::new());
    }
    let started = run.started_at.to_rfc3339();
    let previous: Option<String> = conn
        .query_row(
            "SELECT started_at FROM audit_runs
              WHERE project_id = ?1 AND kind = 'Full' AND id <> ?2
                AND julianday(started_at) < julianday(?3)
              ORDER BY julianday(started_at) DESC LIMIT 1",
            params![run.project_id, run.id, started],
            |row| row.get(0),
        )
        .optional()?;
    let mut statement = conn.prepare(
        "SELECT id FROM discussions
          WHERE project_id = ?1 AND title IN (?2, ?3, ?4)
            AND julianday(created_at) <= julianday(?5)
            AND (?6 IS NULL OR julianday(created_at) > julianday(?6))
          ORDER BY julianday(created_at)",
    )?;
    let ids = statement
        .query_map(
            params![
                run.project_id,
                BRIEFING_TITLES[0],
                BRIEFING_TITLES[1],
                BRIEFING_TITLES[2],
                started,
                previous
            ],
            |row| row.get(0),
        )?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(ids)
}

/// The run's cost per phase and in total; `None` for an unknown run.
pub fn for_run(conn: &Connection, run_id: &str) -> Result<Option<AuditRunCost>> {
    let Some(run) = crate::db::audit_runs::get_by_id(conn, run_id)? else {
        return Ok(None);
    };
    let mut briefing_items = Vec::new();
    for id in briefing_discussion_ids(conn, &run)? {
        briefing_items.extend(discussion_items(conn, &id)?);
    }
    let validation_items = match &run.validation_discussion_id {
        Some(id) => discussion_items(conn, id)?,
        None => Vec::new(),
    };
    let briefing = accumulate(briefing_items);
    let audit = audit_phase(conn, run_id)?;
    let validation = accumulate(validation_items);
    Ok(Some(AuditRunCost {
        run_id: run.id,
        total: total_of([&briefing, &audit, &validation]),
        briefing,
        audit,
        validation,
    }))
}

#[cfg(test)]
#[path = "audit_run_cost_test.rs"]
mod audit_run_cost_test;
