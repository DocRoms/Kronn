//! Integration tests for the 0.9.0 Continual Learning API — the validation
//! pipeline (spec §6) end-to-end through the real router + handlers + in-memory DB.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};

// Serialize tests that mutate the process-global KRONN_USER_CONTEXT_DIR so
// concurrent set_var/remove_var can't cross-contaminate.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

fn app_with(enabled: bool) -> Router {
    let db = Arc::new(kronn::db::Database::open_in_memory().expect("in-memory DB"));
    let mut cfg = kronn::core::config::default_config();
    cfg.server.auth_token = None;
    cfg.server.continual_learning_enabled = enabled;
    let state = AppState::new_defaults(Arc::new(RwLock::new(cfg)), db, DEFAULT_MAX_CONCURRENT_AGENTS);
    build_router_with_auth(state, false)
}

/// Default test app has the feature ON (the pipeline tests need capture enabled;
/// the master toggle itself is covered by `propose_blocked_when_feature_off`).
fn test_app() -> Router {
    app_with(true)
}

async fn post(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn get(app: Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn propose_blocked_when_feature_off() {
    // Master toggle OFF (the ship default) — capture is refused cleanly.
    let app = app_with(false);
    let (st, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "uses pnpm",
            "evidence": [{"kind": "user", "ref": "user:2026-06-01"}],
            "kind": "preference"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(j["data"]["accepted"], false);
    assert!(j["data"]["reason"].as_str().unwrap().to_lowercase().contains("désactivé"));
    // nothing captured
    let (_st, jc) = get(app, "/api/learnings/pending").await;
    assert_eq!(jc["data"]["count"], 0);
}

#[tokio::test]
async fn propose_rejects_empty_evidence() {
    let app = test_app();
    let (st, j) = post(
        app,
        "/api/learnings/propose",
        json!({"claim": "uses pnpm", "evidence": [], "kind": "preference"}),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(j["success"], true);
    assert_eq!(j["data"]["accepted"], false);
    assert!(j["data"]["reason"].as_str().unwrap().contains("evidence"));
}

#[tokio::test]
async fn propose_rejects_secret_in_claim() {
    let app = test_app();
    let (_st, j) = post(
        app,
        "/api/learnings/propose",
        json!({
            // synthetic connection string (matches core::redact, no real vendor
            // prefix → safe for commit / GitHub push protection)
            "claim": "the db is at postgres://admin:hunter2primarysecret@db.internal:5432/app",
            "evidence": [{"kind": "user", "ref": "user:2026-05-31"}],
            "kind": "preference"
        }),
    )
    .await;
    assert_eq!(j["data"]["accepted"], false);
    assert!(j["data"]["reason"].as_str().unwrap().to_lowercase().contains("secret"));
}

#[tokio::test]
async fn propose_accepts_preference_with_user_evidence_then_lists_and_counts() {
    let app = test_app();
    // accept (user evidence → Unchecked, not fabricated; preference kind)
    let (_st, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "prefers tabs over spaces",
            "evidence": [{"kind": "user", "ref": "user:2026-05-31", "quote": "I prefer tabs"}],
            "kind": "preference"
        }),
    )
    .await;
    assert_eq!(j["data"]["accepted"], true, "resp: {j}");
    assert_eq!(j["data"]["learning"]["status"], "pending");

    // pending count = 1
    let (_st, jc) = get(app.clone(), "/api/learnings/pending").await;
    assert_eq!(jc["data"]["count"], 1);

    // list filtered by status
    let (_st, jl) = get(app, "/api/learnings?status=pending").await;
    assert_eq!(jl["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn propose_warns_on_overgeneralization_but_still_accepts() {
    let app = test_app();
    let (_st, j) = post(
        app,
        "/api/learnings/propose",
        json!({
            "claim": "Toujours utiliser pnpm",
            "evidence": [{"kind": "user", "ref": "user:2026-05-31"}],
            "kind": "preference"
        }),
    )
    .await;
    assert_eq!(j["data"]["accepted"], true);
    let warnings = j["data"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w.as_str().unwrap().contains("généralisation")),
        "expected over-generalization warning, got {warnings:?}"
    );
}

#[tokio::test]
async fn dedup_blocks_second_identical_proposal() {
    let app = test_app();
    let body = json!({
        "claim": "same exact claim",
        "evidence": [{"kind": "user", "ref": "user:2026-05-31"}],
        "kind": "preference"
    });
    let (_s1, j1) = post(app.clone(), "/api/learnings/propose", body.clone()).await;
    assert_eq!(j1["data"]["accepted"], true);
    let (_s2, j2) = post(app, "/api/learnings/propose", body).await;
    assert_eq!(j2["data"]["accepted"], false);
    assert!(j2["data"]["reason"].as_str().unwrap().contains("dédup"));
}

#[tokio::test]
async fn negative_learning_auto_refuses_after_three_rejects() {
    // P1-2 + partial index: a claim rejected 3× is auto-refused on the 4th
    // proposal by negative-learning (NOT by dedup). Uses preference/no-project
    // so propose + reject route to the same scope (User) → consistent hash.
    let app = test_app();
    let body = json!({
        "claim": "flaky claim to reject thrice",
        "kind": "preference",
        "evidence": [{"kind": "user", "ref": "user:2026-06-01"}]
    });
    for _ in 0..3 {
        let (_s, j) = post(app.clone(), "/api/learnings/propose", body.clone()).await;
        assert_eq!(j["data"]["accepted"], true, "re-proposal after reject must be allowed: {j}");
        let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();
        let (_r, _jr) = post(app.clone(), &format!("/api/learnings/{id}/reject"), json!({})).await;
    }
    let (_s, j4) = post(app, "/api/learnings/propose", body).await;
    assert_eq!(j4["data"]["accepted"], false, "4th must be auto-refused: {j4}");
    let reason = j4["data"]["reason"].as_str().unwrap().to_lowercase();
    assert!(
        reason.contains("rejeté") || reason.contains("auto-refus"),
        "expected negative-learning refusal, got: {reason}"
    );
}

#[tokio::test]
async fn validate_refuses_preference_without_dated_user_evidence() {
    // §5 binding — a preference promoted to User scope needs a dated user
    // evidence ([src: user:YYYY-MM-DD]). A bare/undated user ref is refused.
    let app = test_app();
    let (_s, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "likes terse output",
            "kind": "preference",
            "evidence": [{"kind": "user", "ref": "user (no date)"}]
        }),
    )
    .await;
    assert_eq!(j["data"]["accepted"], true, "propose accepts (no date check at propose): {j}");
    let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();
    let (_s2, jv) = post(app, &format!("/api/learnings/{id}/validate"), json!({})).await;
    assert_eq!(jv["success"], false, "undated preference must be refused at validate: {jv}");
    assert!(jv["error"].as_str().unwrap().to_lowercase().contains("user"));
}

#[tokio::test]
async fn validate_accepts_inference_with_single_human_validation() {
    let _env = ENV_LOCK.lock().await;
    // 0.9.0 policy (documented, not double-validation): an inference is promoted
    // on a single human validation. Pins the intended behaviour so a future
    // change is deliberate.
    let tmp = std::env::temp_dir().join(format!("kronn_uc_inf_{}", uuid::Uuid::new_v4()));
    std::env::set_var("KRONN_USER_CONTEXT_DIR", &tmp);
    let app = test_app();
    let (_s, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "the team seems to prefer small PRs",
            "kind": "inference",
            "evidence": [{"kind": "disc", "ref": "disc-123"}]
        }),
    )
    .await;
    assert_eq!(j["data"]["accepted"], true);
    let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();
    let (_s2, jv) = post(app, &format!("/api/learnings/{id}/validate"), json!({})).await;
    assert_eq!(jv["success"], true, "inference promotes on single validation in 0.9.0: {jv}");
    assert_eq!(jv["data"]["status"], "promoted");
    std::env::remove_var("KRONN_USER_CONTEXT_DIR");
    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn validate_refuses_fact_without_verified_evidence() {
    // P0 — a `fact` backed only by user/url evidence (Unchecked, not Verified)
    // is accepted at propose (warn) but MUST be refused at validate.
    let app = test_app();
    let (_s, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "the build uses cargo workspaces",
            "kind": "fact",
            "evidence": [{"kind": "user", "ref": "user:2026-06-01"}]
        }),
    )
    .await;
    assert_eq!(j["data"]["accepted"], true, "propose only warns: {j}");
    let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();
    let (_s2, jv) = post(app, &format!("/api/learnings/{id}/validate"), json!({})).await;
    assert_eq!(jv["success"], false, "fact w/o verified evidence must be refused: {jv}");
    assert!(jv["error"].as_str().unwrap().to_lowercase().contains("fact"));
}

#[tokio::test]
async fn reject_after_promote_is_refused() {
    // Race-safety: once promoted, a direct /reject must NOT flip the DB to
    // rejected (which would desync DB vs the already-written learnings.md).
    let _env = ENV_LOCK.lock().await;
    let tmp = std::env::temp_dir().join(format!("kronn_uc_rap_{}", uuid::Uuid::new_v4()));
    std::env::set_var("KRONN_USER_CONTEXT_DIR", &tmp);
    let app = test_app();
    let (_s, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "prefers spaces",
            "kind": "preference",
            "evidence": [{"kind": "user", "ref": "user:2026-06-01"}]
        }),
    )
    .await;
    let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();
    let (_s2, jv) = post(app.clone(), &format!("/api/learnings/{id}/validate"), json!({})).await;
    assert_eq!(jv["data"]["status"], "promoted", "promoted first: {jv}");
    // now reject the promoted row → must be refused
    let (_s3, jr) = post(app, &format!("/api/learnings/{id}/reject"), json!({})).await;
    assert_eq!(jr["success"], false, "reject-after-promote must be refused: {jr}");
    assert!(jr["error"].as_str().unwrap().to_lowercase().contains("pending"));
    std::env::remove_var("KRONN_USER_CONTEXT_DIR");
    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn validate_refuses_non_pending() {
    // P2 — a rejected (non-pending) row must not be promotable via direct API.
    let app = test_app();
    let (_s, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "prefers dark mode",
            "kind": "preference",
            "evidence": [{"kind": "user", "ref": "user:2026-06-01"}]
        }),
    )
    .await;
    let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();
    let (_s2, _jr) = post(app.clone(), &format!("/api/learnings/{id}/reject"), json!({})).await;
    let (_s3, jv) = post(app, &format!("/api/learnings/{id}/validate"), json!({})).await;
    assert_eq!(jv["success"], false, "validating a rejected row must fail: {jv}");
    assert!(jv["error"].as_str().unwrap().to_lowercase().contains("pending"));
}

#[tokio::test]
async fn validate_promotes_user_scope_to_file() {
    let _env = ENV_LOCK.lock().await;
    // Isolate the user-context dir so the promotion write is sandboxed.
    let tmp = std::env::temp_dir().join(format!("kronn_uc_{}", uuid::Uuid::new_v4()));
    std::env::set_var("KRONN_USER_CONTEXT_DIR", &tmp);

    let app = test_app();
    let (_st, j) = post(
        app.clone(),
        "/api/learnings/propose",
        json!({
            "claim": "validate and promote me",
            "evidence": [{"kind": "user", "ref": "user:2026-05-31"}],
            "kind": "preference"
        }),
    )
    .await;
    let id = j["data"]["learning"]["id"].as_str().unwrap().to_string();

    let (_st, jv) = post(app, &format!("/api/learnings/{id}/validate"), json!({})).await;
    assert_eq!(jv["success"], true, "resp: {jv}");
    assert_eq!(jv["data"]["status"], "promoted");
    assert_eq!(jv["data"]["scope"], "user");

    let target = tmp.join("learnings.md");
    let content = std::fs::read_to_string(&target).expect("learnings.md written");
    assert!(content.contains("validate and promote me"));
    assert!(content.contains(&format!("(lc_id:{id})")));

    std::env::remove_var("KRONN_USER_CONTEXT_DIR");
    std::fs::remove_dir_all(&tmp).ok();
}
