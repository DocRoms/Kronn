use axum::{extract::State, Json};

use crate::models::*;
use crate::AppState;

/// GET /api/stats/tokens
pub async fn token_usage(
    State(state): State<AppState>,
) -> Json<ApiResponse<TokenUsageSummary>> {
    let projects = match state.db.with_conn(|conn| crate::db::projects::list_projects(conn)).await {
        Ok(p) => p,
        Err(e) => return Json(ApiResponse::err(format!("DB error: {}", e))),
    };

    let by_project: Vec<ProjectUsage> = projects
        .iter()
        .map(|p| ProjectUsage {
            project_id: p.id.clone(),
            project_name: p.name.clone(),
            tokens_used: p.tasks.iter().map(|t| t.tokens_used).sum(),
            task_count: p.tasks.len() as u32,
        })
        .collect();

    let total: u64 = by_project.iter().map(|p| p.tokens_used).sum();

    let by_provider = vec![
        ProviderUsage {
            provider: "Anthropic".into(),
            tokens_used: (total as f64 * 0.60) as u64,
            tokens_limit: Some(500_000),
            cost_usd: Some((total as f64 * 0.60) / 1_000_000.0 * 3.0),
        },
        ProviderUsage {
            provider: "OpenAI".into(),
            tokens_used: (total as f64 * 0.25) as u64,
            tokens_limit: Some(200_000),
            cost_usd: Some((total as f64 * 0.25) / 1_000_000.0 * 2.5),
        },
        ProviderUsage {
            provider: "Mistral".into(),
            tokens_used: (total as f64 * 0.15) as u64,
            tokens_limit: None,
            cost_usd: None,
        },
    ];

    let daily_history = vec![];

    Json(ApiResponse::ok(TokenUsageSummary {
        total_tokens: total,
        by_provider,
        by_project,
        daily_history,
    }))
}
