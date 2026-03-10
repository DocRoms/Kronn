use axum::{extract::State, Json};

use crate::models::*;
use crate::AppState;

/// GET /api/stats/tokens
pub async fn token_usage(
    State(state): State<AppState>,
) -> Json<ApiResponse<TokenUsageSummary>> {
    // Get real token usage from messages table
    match state.db.with_conn(|conn| {
        // Per-project usage (join discussions -> messages)
        let mut by_project_stmt = conn.prepare(
            "SELECT d.project_id, p.name, m.agent_type, SUM(m.tokens_used) as total_tokens, COUNT(DISTINCT d.id) as disc_count
             FROM messages m
             JOIN discussions d ON m.discussion_id = d.id
             LEFT JOIN projects p ON d.project_id = p.id
             WHERE m.tokens_used > 0
             GROUP BY d.project_id, m.agent_type
             ORDER BY total_tokens DESC"
        )?;

        let rows: Vec<(Option<String>, Option<String>, Option<String>, u64, u32)> = by_project_stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3).unwrap_or(0) as u64,
                row.get::<_, i64>(4).unwrap_or(0) as u32,
            ))
        })?.filter_map(|r| r.ok()).collect();

        // Aggregate by project
        let mut project_map: std::collections::HashMap<String, ProjectUsage> = std::collections::HashMap::new();
        for (pid, pname, _agent, tokens, disc_count) in &rows {
            let key = pid.clone().unwrap_or_else(|| "global".into());
            let entry = project_map.entry(key).or_insert_with(|| ProjectUsage {
                project_id: pid.clone().unwrap_or_else(|| "global".into()),
                project_name: pname.clone().unwrap_or_else(|| "Global".into()),
                tokens_used: 0,
                task_count: *disc_count,
            });
            entry.tokens_used += tokens;
        }
        let by_project: Vec<ProjectUsage> = project_map.into_values().collect();

        // Aggregate by provider (agent_type -> provider)
        let mut provider_map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for (_, _, agent_type, tokens, _) in &rows {
            let provider = match agent_type.as_deref() {
                Some("ClaudeCode") => "Anthropic",
                Some("Codex") => "OpenAI",
                Some("GeminiCli") => "Google",
                Some("Vibe") => "Mistral",
                _ => "Other",
            };
            *provider_map.entry(provider.into()).or_insert(0) += tokens;
        }

        let by_provider: Vec<ProviderUsage> = provider_map.into_iter().map(|(provider, tokens)| {
            ProviderUsage {
                provider,
                tokens_used: tokens,
                tokens_limit: None,
                cost_usd: None,
            }
        }).collect();

        let total: u64 = by_project.iter().map(|p| p.tokens_used).sum();

        Ok(TokenUsageSummary {
            total_tokens: total,
            by_provider,
            by_project,
            daily_history: vec![],
        })
    }).await {
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
            "SELECT m.agent_type, d.project_id, p.name, SUM(m.tokens_used) as total, COUNT(*) as msg_count
             FROM messages m
             JOIN discussions d ON m.discussion_id = d.id
             LEFT JOIN projects p ON d.project_id = p.id
             WHERE m.tokens_used > 0 AND m.agent_type IS NOT NULL
             GROUP BY m.agent_type, d.project_id
             ORDER BY m.agent_type, total DESC"
        )?;

        let rows: Vec<(String, Option<String>, Option<String>, u64, u32)> = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3).unwrap_or(0) as u64,
                row.get::<_, i64>(4).unwrap_or(0) as u32,
            ))
        })?.filter_map(|r| r.ok()).collect();

        let mut agent_map: std::collections::HashMap<String, AgentUsageSummary> = std::collections::HashMap::new();
        let mut agent_order: Vec<String> = Vec::new();
        for (agent, pid, pname, tokens, msgs) in rows {
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

        // Preserve insertion order
        let result: Vec<AgentUsageSummary> = agent_order.into_iter()
            .filter_map(|k| agent_map.remove(&k))
            .collect();

        Ok(result)
    }).await {
        Ok(data) => Json(ApiResponse::ok(data)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}
