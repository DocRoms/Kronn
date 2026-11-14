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

/// KT-1525 — the heavy-output threshold defaults to 256 KB, survives an older
/// config without it, and the Settings form cannot persist 0 or an absurd one.
#[test]
fn the_heavy_output_threshold_defaults_and_is_bounded() {
    assert_eq!(WorkersConfig::default().heavy_output_kb, 256);
    let partial: WorkersConfig = toml::from_str("review_rounds_unlimited = true").unwrap();
    assert_eq!(partial.heavy_output_kb, 256);
    for (kb, valid) in [
        (0, false),
        (1, true),
        (256, true),
        (102_400, true),
        (102_401, false),
    ] {
        let candidate = WorkersConfig {
            heavy_output_kb: kb,
            ..Default::default()
        };
        assert_eq!(candidate.validate().is_ok(), valid, "{kb} KB");
    }
}

/// KT-1718 — a Claude task worker's shell limits: 15 minutes by default and 60 at
/// most (Claude Code's own cap is 10), an older `[workers]` table keeps them, the
/// Settings form cannot persist a default above the maximum or a maximum above
/// the largest generation limit, and a hand-edited value is clamped at spawn.
#[test]
fn the_bash_timeouts_default_are_bounded_and_clamped_at_spawn() {
    let defaults = WorkersConfig::default();
    assert_eq!(defaults.bash_default_timeout_min, 15);
    assert_eq!(defaults.bash_max_timeout_min, 60);
    assert_eq!(defaults.effective_bash_timeouts_ms(), (900_000, 3_600_000));
    let partial: WorkersConfig = toml::from_str("review_rounds_unlimited = true").unwrap();
    assert_eq!(partial.bash_default_timeout_min, 15);
    assert_eq!(partial.bash_max_timeout_min, 60);
    for (default, max, valid) in [
        (15, 60, true),
        (1, 1, true),
        (240, 240, true),
        (0, 60, false),
        (61, 60, false),
        (15, 0, false),
        (15, 241, false),
    ] {
        let candidate = WorkersConfig {
            bash_default_timeout_min: default,
            bash_max_timeout_min: max,
            ..Default::default()
        };
        assert_eq!(candidate.validate().is_ok(), valid, "{default}/{max} min");
    }
    let hand_edited = WorkersConfig {
        bash_default_timeout_min: 500,
        bash_max_timeout_min: 999,
        ..Default::default()
    };
    assert_eq!(
        hand_edited.effective_bash_timeouts_ms(),
        (240 * 60_000, 240 * 60_000)
    );
    let inverted = WorkersConfig {
        bash_default_timeout_min: 90,
        bash_max_timeout_min: 30,
        ..Default::default()
    };
    assert_eq!(
        inverted.effective_bash_timeouts_ms(),
        (30 * 60_000, 30 * 60_000)
    );
}

/// KT-1742 — the lane warm-up entry of Settings > Workers: validated like the
/// rest, stored normalized, published to the lane code at once, and what was
/// saved is what a reload reads back.
#[tokio::test]
#[serial_test::serial]
async fn the_lane_warm_settings_are_validated_stored_and_published() {
    use crate::models::LaneWarmConfig;
    let _restore =
        crate::core::mcp_scanner::host_sync_tests::EnvRestore::capture(&["KRONN_DATA_DIR"]);
    let data = tempfile::tempdir().unwrap();
    crate::core::child_env::set_var("KRONN_DATA_DIR", data.path());
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(config::default_config()));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);

    let with = |lane_warm: LaneWarmConfig| WorkersConfig {
        lane_warm,
        ..Default::default()
    };
    let (status, body) = workers_reply(
        save_workers_config(
            State(state.clone()),
            Json(with(LaneWarmConfig {
                seed_alignment: Some(false),
                first_build_command: Some("  composer install  ".into()),
                ..LaneWarmConfig::default()
            })),
        )
        .await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"]["lane_warm"]["first_build_command"], "composer install",
        "a stored command has no stray blanks"
    );
    let reloaded = config::load().await.unwrap().expect("a saved config");
    assert_eq!(reloaded.workers.lane_warm.seed_alignment, Some(false));
    assert_eq!(
        reloaded.workers.lane_warm.first_build_command.as_deref(),
        Some("composer install")
    );

    for (label, bad) in [
        (
            "a zero bound",
            LaneWarmConfig {
                first_build_timeout_min: Some(0),
                ..LaneWarmConfig::default()
            },
        ),
        (
            "a variable the lane owns",
            LaneWarmConfig {
                compiler_env: Some(vec!["KRONN_PORT=1".into()]),
                ..LaneWarmConfig::default()
            },
        ),
    ] {
        let (status, body) =
            workers_reply(save_workers_config(State(state.clone()), Json(with(bad))).await).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{label}: {body}");
        assert_eq!(
            state.config.read().await.workers.lane_warm.seed_alignment,
            Some(false),
            "{label}: a refused value leaves the saved one"
        );
    }
    // The snapshot is process-wide: leave it as other tests expect it.
    crate::core::task_worker_lane_settings::set_global(&LaneWarmConfig::default());
}

/// KT-1748 — the managed worktree roots are a validated Settings > Workers list:
/// empty by default, absolute paths only, never `..` nor the filesystem root.
#[test]
fn managed_worktree_roots_are_validated_absolute_paths() {
    assert!(WorkersConfig::default().managed_worktree_roots.is_empty());
    let partial: WorkersConfig = toml::from_str("review_rounds_unlimited = true").unwrap();
    assert!(partial.managed_worktree_roots.is_empty());

    let with = |roots: &[&str]| WorkersConfig {
        managed_worktree_roots: roots.iter().map(|root| root.to_string()).collect(),
        ..Default::default()
    };
    assert!(with(&["/srv/lanes", "/Users/me/.kronn/autocode-worktrees"])
        .validate()
        .is_ok());
    for bad in ["", "relative/lane", "/srv/../etc", "/", "/srv/\0lane"] {
        assert!(with(&[bad]).validate().is_err(), "{bad:?} must be refused");
    }
    let too_many: Vec<String> = (0..=crate::models::MAX_MANAGED_WORKTREE_ROOTS)
        .map(|n| format!("/srv/lane-{n}"))
        .collect();
    assert!(WorkersConfig {
        managed_worktree_roots: too_many,
        ..Default::default()
    }
    .validate()
    .is_err());

    // A hand-edited config.toml never widens the managed set past valid paths.
    let hand_edited = with(&["/srv/ok", "relative", "/", "/a/../b"]);
    assert_eq!(
        crate::models::effective_managed_worktree_roots(&hand_edited.managed_worktree_roots),
        vec![std::path::PathBuf::from("/srv/ok")]
    );
    let round_trip: WorkersConfig =
        toml::from_str(&toml::to_string(&with(&["/srv/ok"])).unwrap()).unwrap();
    assert_eq!(
        round_trip.managed_worktree_roots,
        vec!["/srv/ok".to_string()]
    );
}
