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
        ..Default::default()
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

/// KT-1632 — the cap on local task workers defaults to 2, an older config
/// loads with it, and the form can never persist a cap that would queue every
/// worker for ever.
#[test]
fn the_task_worker_cap_defaults_to_two_and_refuses_zero_and_excess() {
    assert_eq!(WorkersConfig::default().max_concurrent_task_workers, 2);
    assert_eq!(
        WorkersConfig::default().effective_max_concurrent_task_workers(),
        2
    );

    let mut table = toml::Table::try_from(config::default_config()).unwrap();
    table.remove("workers");
    let older: AppConfig = table.try_into().unwrap();
    assert_eq!(older.workers.max_concurrent_task_workers, 2);
    let partial: WorkersConfig = toml::from_str("review_rounds_unlimited = true").unwrap();
    assert_eq!(partial.max_concurrent_task_workers, 2);

    for (cap, valid) in [(0, false), (1, true), (4, true), (64, true), (65, false)] {
        let candidate = WorkersConfig {
            max_concurrent_task_workers: cap,
            ..Default::default()
        };
        assert_eq!(candidate.validate().is_ok(), valid, "cap {cap}");
    }
    // A hand-edited 0 still bounds, rather than parking every worker.
    let edited = WorkersConfig {
        max_concurrent_task_workers: 0,
        ..Default::default()
    };
    assert_eq!(edited.effective_max_concurrent_task_workers(), 1);
}

async fn dispatch_limits(state: &crate::AppState) -> serde_json::Value {
    let config = state.config.read().await;
    serde_json::from_str(&crate::agents::runner::dispatch_concurrency_limits(&config)).unwrap()
}

/// KT-1632 — Settings > Workers reaches the claim: the dispatcher's limits
/// carry the cap next to the per-agent ones, and a saved value changes it.
#[tokio::test]
#[serial_test::serial]
async fn the_saved_cap_is_the_one_the_dispatcher_admits_under() {
    let _restore =
        crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&["KRONN_DATA_DIR"]);
    let data = tempfile::tempdir().unwrap();
    crate::core::child_env::set_var("KRONN_DATA_DIR", data.path());
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config()));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let key = crate::db::agent_dispatch::TASK_WORKER_LIMIT_KEY;

    let initial = dispatch_limits(&state).await;
    assert_eq!(initial[key], 2);
    // The per-agent and local-pool caps are still there.
    assert!(initial["__local_global"].is_number());
    assert!(initial["ClaudeCode"].is_number());

    let (status, body) = workers_reply(
        save_workers_config(
            State(state.clone()),
            Json(WorkersConfig {
                max_concurrent_task_workers: 4,
                ..Default::default()
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["max_concurrent_task_workers"], 4);
    assert_eq!(dispatch_limits(&state).await[key], 4);

    let (status, body) = workers_reply(
        save_workers_config(
            State(state.clone()),
            Json(WorkersConfig {
                max_concurrent_task_workers: 0,
                ..Default::default()
            }),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        state
            .config
            .read()
            .await
            .workers
            .max_concurrent_task_workers,
        4,
        "a refused value leaves the saved one"
    );
}

/// KT-1633 — Settings > Workers: the background-priority switch is on out of
/// the box, an older `[workers]` table without it loads as on, it round-trips
/// through `config.toml`, and saving it reaches the process (`config::save`
/// publishes it) so the next spawn reads it.
#[tokio::test]
#[serial_test::serial]
async fn the_background_priority_switch_defaults_on_round_trips_and_reaches_the_process() {
    use crate::core::background_priority;
    let _restore =
        crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&["KRONN_DATA_DIR"]);
    let data = tempfile::tempdir().unwrap();
    crate::core::child_env::set_var("KRONN_DATA_DIR", data.path());
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config()));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

    let Json(initial) = get_workers_config(State(state.clone())).await;
    assert!(initial.data.unwrap().background_priority);
    let partial: WorkersConfig = toml::from_str("review_rounds_unlimited = true").unwrap();
    assert!(partial.background_priority, "an older table keeps it on");

    let off = WorkersConfig {
        background_priority: false,
        ..workers(5, false)
    };
    let (status, body) =
        workers_reply(save_workers_config(State(state.clone()), Json(off)).await).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["background_priority"], false);
    assert!(
        !background_priority::enabled(),
        "the save reached the process"
    );
    assert_eq!(
        background_priority::plan_here(background_priority::Priority::Background),
        background_priority::Plan::default()
    );
    let reloaded = config::load().await.unwrap().expect("a saved config");
    assert!(!reloaded.workers.background_priority);

    let (status, _) =
        workers_reply(save_workers_config(State(state.clone()), Json(workers(5, false))).await)
            .await;
    assert_eq!(status, StatusCode::OK);
    assert!(state.config.read().await.workers.background_priority);
    // Restored for the tests that follow: the setting is process-wide.
    background_priority::set_enabled(true);
}
