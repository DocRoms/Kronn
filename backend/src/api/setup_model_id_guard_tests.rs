//! KT-1998 — the tier slots and the fallback links a settings save stores go
//! through the shared model rule. A plain `#[cfg(test)]` module, so the
//! `model_id_guard` filter runs it with or without the `full-tests` feature.

use super::*;

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config())),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// A dash-led id, line breaks or a tab around an id, and a long run of
/// spaces before one.
fn bad_models() -> Vec<String> {
    vec![
        "--help".into(),
        " -c".into(),
        "\ngpt-5\n".into(),
        "\tgpt-5".into(),
        format!("{}gpt-5", " ".repeat(100_000)),
    ]
}

fn assert_model_refusal(error: &str, bad: &str) {
    assert!(
        error.contains("must not start with '-'") || error.contains("one line"),
        "{bad:?}: {error}"
    );
}

async fn agents_snapshot(state: &AppState) -> serde_json::Value {
    serde_json::to_value(&state.config.read().await.agents).unwrap()
}

/// A tier slot holding a bad model is refused and nothing is stored; a slot
/// of plain spaces still means "no model".
#[tokio::test]
async fn model_id_guard_tier_settings_refuse_a_bad_model() {
    let state = state();
    let before = agents_snapshot(&state).await;
    for bad in bad_models() {
        for slot in 0..4 {
            let mut tiers = ModelTiersConfig::default();
            let value = Some(bad.clone());
            match slot {
                0 => tiers.open_code.economy = value,
                1 => tiers.gemini_cli.default = value,
                2 => tiers.claude_code.reasoning = value,
                _ => tiers.ollama.decision = value,
            }
            let Json(response) = set_model_tiers(State(state.clone()), Json(tiers)).await;
            assert_model_refusal(&response.error.expect("refused"), &bad);
        }
    }
    assert_eq!(before, agents_snapshot(&state).await, "nothing is written");
    let mut blank = ModelTiersConfig::default();
    blank.codex.economy = Some("  ".into());
    crate::models::setup::check_model_tiers(&blank).expect("blank is no model");
}

/// An agent fallback link with a bad model is refused, judged as sent, and
/// the row is not stored; the error reads once ("Fallback refused: model …").
#[tokio::test]
async fn model_id_guard_agent_fallback_link_refuses_a_bad_model() {
    let state = state();
    let before = agents_snapshot(&state).await;
    for bad in bad_models() {
        let Json(response) = set_agent_fallback(
            State(state.clone()),
            Json(SetAgentFallbackRequest {
                agent: AgentType::ClaudeCode,
                mode: FallbackMode::Standard,
                fallback: Some(AgentFallback {
                    agent: AgentType::Codex,
                    model: Some(bad.clone()),
                    mode: None,
                }),
            }),
        )
        .await;
        let error = response.error.expect("refused");
        assert!(error.starts_with("Fallback refused: model "), "{error}");
        assert_model_refusal(&error, &bad);
    }
    assert_eq!(before, agents_snapshot(&state).await, "nothing is written");
}

/// A connection fallback row with a bad model is refused by its save
/// handler, before any lookup, and the row is not stored.
#[tokio::test]
async fn model_id_guard_connection_fallback_row_refuses_a_bad_model() {
    let state = state();
    let before = agents_snapshot(&state).await;
    for bad in bad_models() {
        let Json(response) = set_connection_fallback(
            State(state.clone()),
            Json(SetConnectionFallbackRequest {
                owner: crate::core::connection_fallback::COMFYUI_OWNER.into(),
                mode: FallbackMode::Image,
                fallback: Some(ConnectionFallbackTarget {
                    connection: "conn-a".into(),
                    model: Some(bad.clone()),
                }),
            }),
        )
        .await;
        let error = response.error.expect("refused");
        assert!(error.starts_with("Fallback refused: model "), "{error}");
        assert_model_refusal(&error, &bad);
    }
    assert_eq!(before, agents_snapshot(&state).await, "nothing is written");
}
