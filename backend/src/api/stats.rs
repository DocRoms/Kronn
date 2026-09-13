use axum::{extract::State, Json};

use crate::models::*;
use crate::AppState;

/// (agent_type, project_id, project_name, total_tokens, recorded_tokens, recorded_cost_sum)
///
/// `recorded_tokens` / `recorded_cost_sum` cover only the group's rows that
/// have a non-null `cost_usd` — `SUM(cost_usd)` alone silently drops NULL
/// rows, so `total_tokens - recorded_tokens` is what must be priced as an
/// estimate or reported unknown, never folded into the recorded sum
/// (KT-637 review).
type DiscRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    u64,
    u64,
    Option<f64>,
);
/// (discussion_id, title, agent_type, total_tokens, recorded_tokens, recorded_cost_sum)
type DiscAgentRow = (
    String,
    Option<String>,
    Option<String>,
    u64,
    u64,
    Option<f64>,
);
/// (agent_type, project_id, project_name, tokens, cost_db, message_count)
type AgentRow = (
    String,
    Option<String>,
    Option<String>,
    u64,
    Option<f64>,
    u32,
);

/// GET /api/stats/tokens
pub async fn token_usage(State(state): State<AppState>) -> Json<ApiResponse<TokenUsageSummary>> {
    match state
        .db
        .with_conn(|conn| {
            // ── 1. Discussion tokens (from messages table) ──
            let mut disc_stmt = conn.prepare(
                "SELECT m.agent_type, d.project_id, p.name,
                    SUM(m.tokens_used),
                    SUM(CASE WHEN m.cost_usd IS NOT NULL THEN m.tokens_used ELSE 0 END),
                    SUM(m.cost_usd)
             FROM messages m
             JOIN discussions d ON m.discussion_id = d.id
             LEFT JOIN projects p ON d.project_id = p.id
             WHERE m.tokens_used > 0
             GROUP BY d.project_id, m.agent_type
             ORDER BY SUM(m.tokens_used) DESC",
            )?;

            let disc_rows: Vec<DiscRow> = disc_stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, i64>(3).unwrap_or(0) as u64,
                        row.get::<_, i64>(4).unwrap_or(0) as u64,
                        row.get::<_, Option<f64>>(5).unwrap_or(None),
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();

            let discussion_tokens: u64 = disc_rows.iter().map(|(_, _, _, t, _, _)| t).sum();

            // ── 2. Workflow tokens (from workflow_runs table) ──
            let workflow_tokens: u64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(tokens_used), 0) FROM workflow_runs WHERE tokens_used > 0",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0) as u64;

            // ── 3. By provider (recorded cost — provenance unguaranteed, justified estimate, or honest unknown) ──
            let mut provider_map: std::collections::HashMap<String, (u64, CostAggregate)> =
                std::collections::HashMap::new();
            for (agent_type, _, _, tokens, recorded_tokens, cost_sum) in &disc_rows {
                let provider = match agent_type.as_deref() {
                    Some("ClaudeCode") => "Anthropic",
                    Some("Codex") => "OpenAI",
                    Some("GeminiCli") => "Google",
                    Some("Vibe") => "Mistral",
                    Some("Kiro") => "Amazon",
                    Some("CopilotCli") => "GitHub",
                    _ => "Other",
                };
                let entry = provider_map
                    .entry(provider.into())
                    .or_insert_with(|| (0, CostAggregate::default()));
                entry.0 += tokens;
                entry
                    .1
                    .add(*cost_sum, tokens - recorded_tokens, agent_type.as_deref());
            }

            let by_provider: Vec<ProviderUsage> = provider_map
                .into_iter()
                .map(|(provider, (tokens, cost))| ProviderUsage {
                    provider,
                    tokens_used: tokens,
                    tokens_limit: None,
                    cost,
                })
                .collect();

            // ── 4. By project (recorded cost — provenance unguaranteed, justified estimate, or honest unknown) ──
            let mut project_map: std::collections::HashMap<String, (String, u64, CostAggregate)> =
                std::collections::HashMap::new();
            for (agent_type, pid, pname, tokens, recorded_tokens, cost_sum) in &disc_rows {
                let key = pid.clone().unwrap_or_else(|| "global".into());
                let entry = project_map.entry(key).or_insert_with(|| {
                    (
                        pname.clone().unwrap_or_else(|| "Global".into()),
                        0,
                        CostAggregate::default(),
                    )
                });
                entry.1 += tokens;
                entry
                    .2
                    .add(*cost_sum, tokens - recorded_tokens, agent_type.as_deref());
            }
            let mut by_project: Vec<ProjectUsage> = project_map
                .into_iter()
                .map(|(id, (name, tokens, cost))| ProjectUsage {
                    project_id: id,
                    project_name: name,
                    tokens_used: tokens,
                    cost,
                })
                .collect();
            by_project.sort_by_key(|p| std::cmp::Reverse(p.tokens_used));

            // ── 5. Top discussions ──
            // Grouped by (discussion, agent_type): a discussion may mix agents,
            // and a group's real agent must drive its own cost — never the
            // arbitrary "ClaudeCode" the old query assumed for every discussion.
            let mut top_disc_stmt = conn.prepare(
                "SELECT d.id, d.title, m.agent_type,
                    SUM(m.tokens_used),
                    SUM(CASE WHEN m.cost_usd IS NOT NULL THEN m.tokens_used ELSE 0 END),
                    SUM(m.cost_usd)
             FROM messages m
             JOIN discussions d ON m.discussion_id = d.id
             WHERE m.tokens_used > 0
             GROUP BY d.id, m.agent_type",
            )?;
            let disc_agent_rows: Vec<DiscAgentRow> = top_disc_stmt
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, i64>(3).unwrap_or(0) as u64,
                        row.get::<_, i64>(4).unwrap_or(0) as u64,
                        row.get::<_, Option<f64>>(5).unwrap_or(None),
                    ))
                })?
                .filter_map(|r| r.ok())
                .collect();
            let mut disc_agg: std::collections::HashMap<String, (String, u64, CostAggregate)> =
                std::collections::HashMap::new();
            for (id, title, agent_type, tokens, recorded_tokens, cost_sum) in &disc_agent_rows {
                let entry = disc_agg.entry(id.clone()).or_insert_with(|| {
                    (
                        title.clone().unwrap_or_else(|| "Sans titre".into()),
                        0,
                        CostAggregate::default(),
                    )
                });
                entry.1 += tokens;
                entry
                    .2
                    .add(*cost_sum, tokens - recorded_tokens, agent_type.as_deref());
            }
            let mut top_discussions: Vec<UsageEntry> = disc_agg
                .into_iter()
                .map(|(id, (name, tokens_used, cost))| UsageEntry {
                    id,
                    name,
                    tokens_used,
                    cost,
                })
                .collect();
            top_discussions.sort_by_key(|d| std::cmp::Reverse(d.tokens_used));
            top_discussions.truncate(5);

            // ── 6. Top workflows ──
            // workflow_runs carries no per-run agent attribution, so its cost
            // is categorically unknown — never priced against Claude's table.
            let mut top_wf_stmt = conn.prepare(
                "SELECT w.id, w.name, SUM(r.tokens_used)
             FROM workflow_runs r
             JOIN workflows w ON r.workflow_id = w.id
             WHERE r.tokens_used > 0
             GROUP BY w.id
             ORDER BY SUM(r.tokens_used) DESC
             LIMIT 5",
            )?;
            let top_workflows: Vec<UsageEntry> = top_wf_stmt
                .query_map([], |row| {
                    let tokens = row.get::<_, i64>(2).unwrap_or(0) as u64;
                    let mut cost = CostAggregate::default();
                    cost.add(None, tokens, None);
                    Ok(UsageEntry {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        tokens_used: tokens,
                        cost,
                    })
                })?
                .filter_map(|r| r.ok())
                .collect();

            // ── 7. Daily history (last 30 days) ──
            let mut daily_stmt = conn.prepare(
                "SELECT DATE(m.timestamp) as day, m.agent_type,
                    SUM(m.tokens_used),
                    SUM(CASE WHEN m.cost_usd IS NOT NULL THEN m.tokens_used ELSE 0 END),
                    SUM(m.cost_usd)
             FROM messages m
             WHERE m.tokens_used > 0
               AND m.timestamp >= DATE('now', '-30 days')
             GROUP BY day, m.agent_type
             ORDER BY day",
            )?;
            let mut daily_map: std::collections::BTreeMap<String, DailyUsage> =
                std::collections::BTreeMap::new();
            daily_stmt
                .query_map([], |row| {
                    let day: String = row.get(0)?;
                    let agent_type: Option<String> = row.get(1)?;
                    let tokens = row.get::<_, i64>(2).unwrap_or(0) as u64;
                    let recorded_tokens = row.get::<_, i64>(3).unwrap_or(0) as u64;
                    let cost_sum: Option<f64> = row.get(4).unwrap_or(None);
                    Ok((day, agent_type, tokens, recorded_tokens, cost_sum))
                })?
                .filter_map(|r| r.ok())
                .for_each(|(day, agent_type, tokens, recorded_tokens, cost_sum)| {
                    let entry = daily_map.entry(day.clone()).or_insert_with(|| DailyUsage {
                        date: day,
                        tokens: 0,
                        cost: CostAggregate::default(),
                        anthropic: 0,
                        openai: 0,
                        google: 0,
                        mistral: 0,
                        amazon: 0,
                        github: 0,
                    });
                    entry.tokens += tokens;
                    entry
                        .cost
                        .add(cost_sum, tokens - recorded_tokens, agent_type.as_deref());
                    match agent_type.as_deref() {
                        Some("ClaudeCode") => entry.anthropic += tokens,
                        Some("Codex") => entry.openai += tokens,
                        Some("GeminiCli") => entry.google += tokens,
                        Some("Vibe") => entry.mistral += tokens,
                        Some("Kiro") => entry.amazon += tokens,
                        Some("CopilotCli") => entry.github += tokens,
                        _ => {}
                    }
                });
            let daily_history: Vec<DailyUsage> = daily_map.into_values().collect();

            // ── 8. Total cost ──
            // Merge every provider's known/estimated cost, then fold in
            // workflow tokens as unknown: workflow_runs has no per-run agent
            // attribution, so its share of the total is never "0", it's
            // "not accounted for" — a non-zero unknown_cost_tokens means
            // total_cost is partial relative to total_tokens.
            let mut total_cost = by_provider
                .iter()
                .fold(CostAggregate::default(), |acc, p| acc.merge(&p.cost));
            total_cost.unknown_cost_tokens += workflow_tokens;
            let total_tokens = discussion_tokens + workflow_tokens;

            Ok(TokenUsageSummary {
                total_tokens,
                total_cost,
                discussion_tokens,
                workflow_tokens,
                by_provider,
                by_project,
                top_discussions,
                top_workflows,
                daily_history,
            })
        })
        .await
    {
        Ok(summary) => Json(ApiResponse::ok(summary)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/stats/agent-usage
/// Returns token usage grouped by agent type, with per-project breakdown
pub async fn agent_usage(
    State(state): State<AppState>,
) -> Json<ApiResponse<Vec<AgentUsageSummary>>> {
    match state.db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT m.agent_type, d.project_id, p.name, SUM(m.tokens_used) as total, SUM(m.cost_usd), COUNT(*) as msg_count
             FROM messages m
             JOIN discussions d ON m.discussion_id = d.id
             LEFT JOIN projects p ON d.project_id = p.id
             WHERE m.tokens_used > 0 AND m.agent_type IS NOT NULL
             GROUP BY m.agent_type, d.project_id
             ORDER BY m.agent_type, total DESC"
        )?;

        let rows: Vec<AgentRow> = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3).unwrap_or(0) as u64,
                row.get::<_, Option<f64>>(4).unwrap_or(None),
                row.get::<_, i64>(5).unwrap_or(0) as u32,
            ))
        })?.filter_map(|r| r.ok()).collect();

        let mut agent_map: std::collections::HashMap<String, AgentUsageSummary> = std::collections::HashMap::new();
        let mut agent_order: Vec<String> = Vec::new();
        for (agent, pid, pname, tokens, _cost, msgs) in rows {
            if !agent_map.contains_key(&agent) {
                agent_order.push(agent.clone());
            }
            let entry = agent_map.entry(agent.clone()).or_insert_with(|| AgentUsageSummary {
                agent_type: agent,
                total_tokens: 0,
                message_count: 0,
                by_project: vec![],
            });
            entry.total_tokens += tokens;
            entry.message_count += msgs;
            entry.by_project.push(AgentProjectUsage {
                project_id: pid.unwrap_or_else(|| "global".into()),
                project_name: pname.unwrap_or_else(|| "Global".into()),
                tokens_used: tokens,
                message_count: msgs,
            });
        }

        let result: Vec<AgentUsageSummary> = agent_order.into_iter()
            .filter_map(|k| agent_map.remove(&k))
            .collect();

        Ok(result)
    }).await {
        Ok(data) => Json(ApiResponse::ok(data)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
