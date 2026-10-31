//! KT-1345 — Settings > Workers: the default review-round limit.

use super::*;

async fn workers_reply(response: Response) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

fn workers(rounds: u32, unlimited: bool) -> WorkersConfig {
    WorkersConfig {
        default_max_review_rounds: rounds,
        review_rounds_unlimited: unlimited,
    }
}

/// The review-round default round-trips as an integer and as « unlimited »
/// (the sentinel, never 0), and an invalid value is a 400 that leaves the
/// saved one alone.
#[tokio::test]
#[serial_test::serial]
async fn workers_config_round_trips_an_integer_unlimited_and_refuses_invalid() {
    // Saving persists config.toml: never the developer's real one.
    let _restore =
        crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&["KRONN_DATA_DIR"]);
    let data = tempfile::tempdir().unwrap();
    crate::core::child_env::set_var("KRONN_DATA_DIR", data.path());
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config()));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

    // Out of the box: 5 rounds, capped.
    let Json(initial) = get_workers_config(State(state.clone())).await;
    let initial = initial.data.unwrap();
    assert_eq!(initial.default_max_review_rounds, 5);
    assert!(!initial.review_rounds_unlimited);
    assert_eq!(
        state
            .config
            .read()
            .await
            .workers
            .effective_max_review_rounds(),
        5
    );

    let (status, body) =
        workers_reply(save_workers_config(State(state.clone()), Json(workers(8, false))).await)
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["default_max_review_rounds"], 8);
    assert_eq!(
        state
            .config
            .read()
            .await
            .workers
            .effective_max_review_rounds(),
        8
    );

    // Unlimited keeps the integer for when the box is unticked again.
    let (status, body) =
        workers_reply(save_workers_config(State(state.clone()), Json(workers(8, true))).await)
            .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stored = state.config.read().await.workers.clone();
    assert_eq!(
        stored.effective_max_review_rounds(),
        UNLIMITED_REVIEW_ROUNDS
    );
    assert_ne!(stored.effective_max_review_rounds(), 0);
    assert_eq!(stored.default_max_review_rounds, 8);

    // What was saved is what a reload reads back.
    let reloaded = config::load().await.unwrap().expect("a saved config");
    assert!(reloaded.workers.review_rounds_unlimited);
    assert_eq!(reloaded.workers.default_max_review_rounds, 8);

    for (rounds, label) in [(0, "zero"), (1_001, "above the cap")] {
        let (status, body) = workers_reply(
            save_workers_config(State(state.clone()), Json(workers(rounds, false))).await,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert_eq!(body["error_code"], "validation");
        assert_eq!(
            state.config.read().await.workers.default_max_review_rounds,
            8,
            "{label}: a refused value leaves the saved one"
        );
    }
}

/// A config.toml written before KT-1345 has no `[workers]`: it loads with the
/// default of 5, not with a missing-field error.
#[test]
fn an_older_config_without_workers_loads_with_the_default() {
    let mut table = toml::Table::try_from(config::default_config()).unwrap();
    table.remove("workers");
    let config: AppConfig = table.try_into().unwrap();
    assert_eq!(config.workers, WorkersConfig::default());
    assert_eq!(config.workers.default_max_review_rounds, 5);

    let partial: WorkersConfig = toml::from_str("review_rounds_unlimited = true").unwrap();
    assert_eq!(partial.default_max_review_rounds, 5);
    assert_eq!(
        partial.effective_max_review_rounds(),
        UNLIMITED_REVIEW_ROUNDS
    );
}

/// The `max_review_rounds` argument of an MCP launch: an integer in range, or
/// the string `unlimited`; everything else is refused.
#[test]
fn the_review_rounds_argument_is_an_integer_or_unlimited_and_nothing_else() {
    use serde_json::json;
    assert_eq!(parse_review_rounds_arg(&json!(1)), Ok(1));
    assert_eq!(parse_review_rounds_arg(&json!(12)), Ok(12));
    assert_eq!(parse_review_rounds_arg(&json!(1_000)), Ok(1_000));
    assert_eq!(
        parse_review_rounds_arg(&json!("unlimited")),
        Ok(UNLIMITED_REVIEW_ROUNDS)
    );
    assert_eq!(
        parse_review_rounds_arg(&json!(" Unlimited ")),
        Ok(UNLIMITED_REVIEW_ROUNDS)
    );
    for bad in [
        json!(0),
        json!(-3),
        json!(1_001),
        json!(2.5),
        json!("5"),
        json!("none"),
        json!(null),
        json!(true),
        json!([3]),
        json!({ "rounds": 3 }),
    ] {
        assert!(
            parse_review_rounds_arg(&bad).is_err(),
            "{bad} must be refused"
        );
    }
}
