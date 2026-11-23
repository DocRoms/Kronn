//! KT-1990 — the run view makes an Artifact from the HTML an execution of a
//! step wrote (the main one or a shadow), opens the renders of a step in a
//! mosaic, and the run delete routes take those Artifacts with the runs.
//! Through the production router, as the UI calls it.

use std::{net::SocketAddr, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request, Router};
use http_body_util::BodyExt;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

const PAGE_MAIN: &str =
    "<!DOCTYPE html>\n<html><head><title>Snake</title></head><body><canvas id=\"main\"></canvas></body></html>";
const PAGE_QWEN: &str = "<html><body><canvas id=\"qwen\"></canvas>é</body></html>";
const PAGE_ARGON: &str = "<!doctype html><html><body><canvas id=\"argon\"></canvas></body></html>";

fn fenced(page: &str) -> String {
    format!("Here is the game.\n\n```html\n{page}\n```\n\nHave fun!")
}

async fn fixture() -> (Router, Arc<kronn::db::Database>) {
    let db = Arc::new(kronn::db::Database::open_in_memory().unwrap());
    let main_output = fenced(PAGE_MAIN);
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES ('wf', 'Mini jeux', '\"Manual\"', '[]', '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z'),
                    ('wf-other', 'Other', '\"Manual\"', '[]', '2026-11-20T00:00:00Z', '2026-11-20T00:00:00Z')",
            [],
        )?;
        let steps = json!([
            {"step_name": "setup", "status": "Success", "output": "ok", "duration_ms": 1},
            {"step_name": "jeu", "status": "Success", "output": main_output, "duration_ms": 10}
        ]);
        for (id, workflow, started) in [
            ("run-0", "wf", "2026-11-20T00:00:00Z"),
            ("run-1", "wf", "2026-11-20T01:00:00Z"),
            ("run-other", "wf-other", "2026-11-20T01:00:00Z"),
        ] {
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, started_at,
                                            finished_at, run_type)
                 VALUES (?1, ?2, 'Success', ?3, ?4, ?4, 'linear')",
                rusqlite::params![id, workflow, steps.to_string(), started],
            )?;
        }
        // The main row first; the shadows start in another order than they are
        // inserted: qwen first, then argon.
        for (run, shadow, label, output, started) in [
            ("run-1", "", "", String::new(), "2026-11-20T01:00:01Z"),
            ("run-1", "argon", "Argon", fenced(PAGE_ARGON), "2026-11-20T01:00:09Z"),
            ("run-1", "qwen", "Qwen local", format!("Voici : {PAGE_QWEN}"), "2026-11-20T01:00:03Z"),
            ("run-1", "plain", "Plain", "Three findings, no page.".to_string(), "2026-11-20T01:00:04Z"),
            ("run-1", "cut", "Cut", format!("```html\n{}", &PAGE_ARGON[..30]), "2026-11-20T01:00:05Z"),
            (
                "run-1",
                "stored-cut",
                "Stored cut",
                format!("{}\n[output cut at 1000000 bytes]", fenced(PAGE_ARGON)),
                "2026-11-20T01:00:06Z",
            ),
            ("run-other", "", "", String::new(), "2026-11-20T01:00:01Z"),
            ("run-other", "argon", "Argon", fenced(PAGE_ARGON), "2026-11-20T01:00:02Z"),
        ] {
            let finished = if shadow.is_empty() { None } else { Some(started) };
            conn.execute(
                "INSERT INTO step_shadow_runs
                    (id, run_id, result_index, step_name, shadow_id, label, status, identity_json,
                     output, model, started_at, finished_at)
                 VALUES (?1, ?2, 1, 'jeu', ?3, ?4, 'success', '{\"agent\":\"Ollama\"}', ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    format!("{run}-{shadow}"),
                    run,
                    shadow,
                    label,
                    output,
                    (!shadow.is_empty()).then(|| format!("model-{shadow}")),
                    started,
                    finished
                ],
            )?;
        }
        Ok(())
    })
    .await
    .unwrap();
    // Authenticated as in production: the operator token, and the marker an
    // agent joined through the bridge adds to every request.
    let mut config = kronn::core::config::default_config();
    config.server.auth_enabled = true;
    config.server.auth_strict_localhost = false;
    config.server.auth_token = Some(OPERATOR.into());
    let state = AppState::new_defaults(
        Arc::new(RwLock::new(config)),
        db.clone(),
        DEFAULT_MAX_CONCURRENT_AGENTS,
    );
    (build_router_with_auth(state, true), db)
}

const OPERATOR: &str = "operator-token-of-the-step-artifacts-test";

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>, agent: bool) -> Value {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {OPERATOR}"));
    if agent {
        builder = builder.header("x-kronn-agent", "1");
    }
    let body = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&body).unwrap())
        }
        None => Body::empty(),
    };
    let mut request = builder.body(body).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40405))));
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn make(app: &Router, shadow: Option<&str>) -> Value {
    call(
        app,
        "POST",
        "/api/workflows/wf/runs/run-1/step-artifacts",
        Some(json!({"result_index": 1, "shadow_id": shadow})),
        false,
    )
    .await
}

async fn page_count(db: &kronn::db::Database) -> i64 {
    db.with_read_conn(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM live_pages", [], |row| row.get(0))?)
    })
    .await
    .unwrap()
}

fn execution<'a>(comparison: &'a Value, shadow: &str) -> &'a Value {
    if shadow.is_empty() {
        return &comparison[0]["main"];
    }
    comparison[0]["shadows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["shadow_id"] == shadow)
        .unwrap()
}

/// Behaviours 1, 2 and 4: one Artifact per execution, from the last complete
/// HTML block; a reload opens the same one; no HTML or a cut one makes nothing.
#[tokio::test]
async fn an_execution_s_html_becomes_one_artifact_opened_again_after_a_reload() {
    let (app, db) = fixture().await;
    let shadows = call(
        &app,
        "GET",
        "/api/workflows/wf/runs/run-1/step-shadows",
        None,
        false,
    )
    .await;
    assert_eq!(shadows["success"], true, "{shadows}");
    let html = |shadow: &str| execution(&shadows["data"], shadow)["html"].clone();
    assert_eq!(html(""), "complete");
    assert_eq!(html("qwen"), "complete");
    assert_eq!(html("plain"), "none");
    assert_eq!(html("cut"), "cut_unclosed_fence");
    assert_eq!(html("stored-cut"), "cut_output");
    assert!(execution(&shadows["data"], "")
        .get("artifact_id")
        .is_none_or(Value::is_null));

    let made = make(&app, None).await;
    assert_eq!(made["success"], true, "{made}");
    assert_eq!(made["data"]["created"], true);
    let main_page = made["data"]["page_id"].as_str().unwrap().to_string();

    let page = call(&app, "GET", &format!("/api/pages/{main_page}"), None, false).await;
    assert_eq!(page["data"]["title"], "Mini jeux · jeu · main", "{page}");
    assert_eq!(page["data"]["revision"]["html"], PAGE_MAIN);

    let qwen = make(&app, Some("qwen")).await;
    assert_eq!(qwen["data"]["created"], true, "{qwen}");
    let qwen_page = qwen["data"]["page_id"].as_str().unwrap().to_string();
    let page = call(&app, "GET", &format!("/api/pages/{qwen_page}"), None, false).await;
    assert_eq!(page["data"]["title"], "Mini jeux · jeu · Qwen local");
    assert_eq!(
        page["data"]["revision"]["html"], PAGE_QWEN,
        "a bare document is found"
    );

    // After a reload the run view reads the link and the button opens it.
    let again = make(&app, None).await;
    assert_eq!(again["data"]["created"], false, "{again}");
    assert_eq!(again["data"]["page_id"], main_page.as_str());
    let shadows = call(
        &app,
        "GET",
        "/api/workflows/wf/runs/run-1/step-shadows",
        None,
        false,
    )
    .await;
    assert_eq!(
        execution(&shadows["data"], "")["artifact_id"],
        main_page.as_str()
    );
    assert_eq!(
        execution(&shadows["data"], "qwen")["artifact_id"],
        qwen_page.as_str()
    );
    assert_eq!(page_count(&db).await, 2);

    // No HTML says so; a cut document is refused with the reason; nothing made.
    for (shadow, reason) in [
        ("plain", "holds no HTML document"),
        ("cut", "never closes"),
        ("stored-cut", "stored cut"),
    ] {
        let refused = make(&app, Some(shadow)).await;
        assert_eq!(refused["success"], false, "{shadow}: {refused}");
        assert_eq!(refused["error_code"], "validation", "{refused}");
        assert!(
            refused["error"].as_str().unwrap().contains(reason),
            "{shadow}: {refused}"
        );
    }
    assert_eq!(page_count(&db).await, 2, "a refusal makes nothing");

    // An unknown shadow or step, or a run of another workflow, is not found.
    let unknown = make(&app, Some("nobody")).await;
    assert_eq!(unknown["error_code"], "not_found", "{unknown}");
    let other = call(
        &app,
        "POST",
        "/api/workflows/wf/runs/run-other/step-artifacts",
        Some(json!({"result_index": 1})),
        false,
    )
    .await;
    assert_eq!(other["error_code"], "not_found", "{other}");

    // Deleting the Artifact removes its link: the run view offers the button again.
    let deleted = call(
        &app,
        "DELETE",
        &format!("/api/pages/{qwen_page}"),
        None,
        false,
    )
    .await;
    assert_eq!(deleted["success"], true, "{deleted}");
    let shadows = call(
        &app,
        "GET",
        "/api/workflows/wf/runs/run-1/step-shadows",
        None,
        false,
    )
    .await;
    assert!(execution(&shadows["data"], "qwen")["artifact_id"].is_null());
    let remade = make(&app, Some("qwen")).await;
    assert_eq!(remade["data"]["created"], true, "{remade}");
}

/// One more shadow of `run-1`'s step `jeu`, started after the others.
async fn add_shadow(db: &kronn::db::Database, shadow: &'static str, output: String) {
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO step_shadow_runs
                (id, run_id, result_index, step_name, shadow_id, label, status, identity_json,
                 output, model, started_at, finished_at)
             VALUES (?1, 'run-1', 1, 'jeu', ?2, ?2, 'success', '{\"agent\":\"Ollama\"}', ?3,
                     'model', '2026-11-20T01:00:30Z', '2026-11-20T01:00:31Z')",
            rusqlite::params![format!("run-1-{shadow}"), shadow, output],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

async fn artifact_html(app: &Router, shadow: &str) -> String {
    let made = make(app, Some(shadow)).await;
    assert_eq!(made["success"], true, "{shadow}: {made}");
    let page_id = made["data"]["page_id"].as_str().unwrap();
    let page = call(app, "GET", &format!("/api/pages/{page_id}"), None, false).await;
    page["data"]["revision"]["html"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The copied page gets the pass an agent's message gets in a discussion, not
/// the audit pass: game code naming a secret word and a link's query survive
/// byte for byte, a real credential is still masked.
#[tokio::test]
async fn an_artifact_holds_the_page_as_written_with_only_real_credentials_masked() {
    let (app, db) = fixture().await;
    let game = "<!DOCTYPE html>\n<html><body><script>\n\
        ball.spin = 0.4;\n\
        const secret = false;\n\
        const piece = { pin: 3, token: next() };\n\
        password = input.value;\n\
        </script>\n\
        <a href=\"https://example.org/play?level=2&mode=hard\">Hard</a>\n\
        </body></html>";
    add_shadow(&db, "game", fenced(game)).await;
    assert_eq!(artifact_html(&app, "game").await, game);

    const CREDENTIAL: &str =
        "github_pat_11ABCDEFG0abcdefghijkl_0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijk";
    let leaky = format!("<html><body><script>const key = \"{CREDENTIAL}\";</script></body></html>");
    add_shadow(&db, "leaky", fenced(&leaky)).await;
    let html = artifact_html(&app, "leaky").await;
    assert!(!html.contains(CREDENTIAL), "{html}");
    assert!(html.contains("***REDACTED***"), "{html}");
}

/// Only a person sets the link: a caller marked as an agent is refused.
#[tokio::test]
async fn an_agent_cannot_make_or_delete_these_artifacts() {
    let (app, db) = fixture().await;
    for (method, path, body) in [
        (
            "POST",
            "/api/workflows/wf/runs/run-1/step-artifacts",
            Some(json!({"result_index": 1})),
        ),
        (
            "POST",
            "/api/workflows/wf/runs/run-1/step-artifacts/mosaic",
            Some(json!({"result_index": 1})),
        ),
        ("DELETE", "/api/workflow-runs/run-1/step-artifacts", None),
    ] {
        let refused = call(&app, method, path, body, true).await;
        assert_eq!(refused["success"], false, "{path}: {refused}");
        assert!(
            refused["error"].as_str().unwrap().contains("Only a person"),
            "{refused}"
        );
    }
    assert_eq!(page_count(&db).await, 0);
}

/// Behaviour 3: the mosaic makes the missing Artifacts and lists the renders,
/// main first, then the shadows in start order.
#[tokio::test]
async fn the_mosaic_lists_main_first_then_shadows_in_start_order() {
    let (app, db) = fixture().await;
    let argon = make(&app, Some("argon")).await;
    let argon_page = argon["data"]["page_id"].as_str().unwrap().to_string();

    let mosaic = call(
        &app,
        "POST",
        "/api/workflows/wf/runs/run-1/step-artifacts/mosaic",
        Some(json!({"result_index": 1})),
        false,
    )
    .await;
    assert_eq!(mosaic["success"], true, "{mosaic}");
    assert_eq!(mosaic["data"]["created"], 2, "main and qwen were missing");
    let ids: Vec<String> = mosaic["data"]["page_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 3, "only the complete renders: {mosaic}");
    assert_eq!(ids[2], argon_page, "argon started after qwen");
    let mut titles = Vec::new();
    for id in &ids {
        let page = call(&app, "GET", &format!("/api/pages/{id}"), None, false).await;
        titles.push(page["data"]["title"].as_str().unwrap().to_string());
    }
    assert_eq!(
        titles,
        [
            "Mini jeux · jeu · main",
            "Mini jeux · jeu · Qwen local",
            "Mini jeux · jeu · Argon"
        ]
    );
    // Again: the same tiles, nothing new.
    let again = call(
        &app,
        "POST",
        "/api/workflows/wf/runs/run-1/step-artifacts/mosaic",
        Some(json!({"result_index": 1})),
        false,
    )
    .await;
    assert_eq!(again["data"]["created"], 0);
    assert_eq!(again["data"]["page_ids"], mosaic["data"]["page_ids"]);
    assert_eq!(page_count(&db).await, 3);

    // The library folder lists the run with its Artifacts in the same order.
    let folders = call(&app, "GET", "/api/step-artifacts", None, false).await;
    let folder = &folders["data"][0];
    assert_eq!(folder["run_id"], "run-1", "{folders}");
    assert_eq!(folder["workflow_name"], "Mini jeux");
    assert_eq!(folder["run_number"], 2, "run-0 is #1");
    let labels: Vec<&str> = folder["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["main", "Qwen local", "Argon"]);
    assert_eq!(folder["artifacts"][2]["model"], "model-argon");

    // « Delete the folder » takes the Artifacts, the run stays.
    let gone = call(
        &app,
        "DELETE",
        "/api/workflow-runs/run-1/step-artifacts",
        None,
        false,
    )
    .await;
    assert_eq!(gone["data"]["count"], 3, "{gone}");
    assert_eq!(page_count(&db).await, 0);
    let run = call(&app, "GET", "/api/workflows/wf/runs/run-1", None, false).await;
    assert_eq!(run["success"], true);
}

/// A step with fewer than two renders has no mosaic.
#[tokio::test]
async fn a_mosaic_needs_two_renders() {
    let (app, db) = fixture().await;
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE step_shadow_runs SET output = 'no page' WHERE run_id = 'run-1' AND shadow_id <> ''",
            [],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let refused = call(
        &app,
        "POST",
        "/api/workflows/wf/runs/run-1/step-artifacts/mosaic",
        Some(json!({"result_index": 1})),
        false,
    )
    .await;
    assert_eq!(refused["success"], false, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("at least two"));
    assert_eq!(page_count(&db).await, 0);
}

async fn make_in(app: &Router, workflow: &str, run: &str, shadow: Option<&str>) {
    let made = call(
        app,
        "POST",
        &format!("/api/workflows/{workflow}/runs/{run}/step-artifacts"),
        Some(json!({"result_index": 1, "shadow_id": shadow})),
        false,
    )
    .await;
    assert_eq!(made["success"], true, "{made}");
}

async fn count(app: &Router, workflow: &str, run: Option<&str>) -> Value {
    let query = run.map(|run| format!("?run_id={run}")).unwrap_or_default();
    let counted = call(
        app,
        "GET",
        &format!("/api/workflows/{workflow}/step-artifacts/count{query}"),
        None,
        false,
    )
    .await;
    counted["data"]["count"].clone()
}

/// Behaviour 6: the three delete paths take the Artifacts of their runs, and
/// the count the confirmation shows is the number they take.
#[tokio::test]
async fn deleting_a_run_all_runs_or_the_workflow_deletes_their_artifacts() {
    // One run.
    let (app, db) = fixture().await;
    make_in(&app, "wf", "run-1", None).await;
    make_in(&app, "wf", "run-1", Some("argon")).await;
    make_in(&app, "wf-other", "run-other", None).await;
    assert_eq!(count(&app, "wf", Some("run-1")).await, 2);
    assert_eq!(count(&app, "wf", None).await, 2);
    let deleted = call(&app, "DELETE", "/api/workflows/wf/runs/run-1", None, false).await;
    assert_eq!(deleted["success"], true, "{deleted}");
    assert_eq!(
        page_count(&db).await,
        1,
        "another workflow's Artifact stays"
    );
    assert_eq!(count(&app, "wf", None).await, 0);

    // Every run of a workflow.
    let (app, db) = fixture().await;
    make_in(&app, "wf", "run-1", Some("qwen")).await;
    make_in(&app, "wf-other", "run-other", None).await;
    let deleted = call(&app, "DELETE", "/api/workflows/wf/runs", None, false).await;
    assert_eq!(deleted["success"], true, "{deleted}");
    assert_eq!(page_count(&db).await, 1);

    // The workflow.
    let (app, db) = fixture().await;
    make_in(&app, "wf", "run-1", None).await;
    make_in(&app, "wf", "run-1", Some("argon")).await;
    make_in(&app, "wf-other", "run-other", None).await;
    let deleted = call(&app, "DELETE", "/api/workflows/wf", None, false).await;
    assert_eq!(deleted["success"], true, "{deleted}");
    assert_eq!(page_count(&db).await, 1);
    let folders = call(&app, "GET", "/api/step-artifacts", None, false).await;
    assert_eq!(folders["data"].as_array().unwrap().len(), 1);
    assert_eq!(folders["data"][0]["run_id"], "run-other");
}

/// KT-1999 — a page whose script holds an `<html>` or `</html>` string becomes
/// an Artifact holding the whole page, fenced or bare, not the string's
/// fragment.
#[tokio::test]
async fn a_tag_in_a_script_string_keeps_the_whole_page() {
    let (app, db) = fixture().await;
    let template = "<!DOCTYPE html>\n<html><head><title>Game</title></head><body>\n\
        <canvas id=\"c\"></canvas>\n<script>\nconst tpl = \"<html><body>hi</body></html>\";\n\
        </script>\n</body></html>";
    let closing = "<!doctype html>\n<html><body><script>\nconst end = \"</html>\";\n</script>\n\
        <p>still the page</p>\n</body></html>";
    add_shadow(&db, "fenced-template", fenced(template)).await;
    add_shadow(&db, "fenced-closing", fenced(closing)).await;
    add_shadow(
        &db,
        "bare-template",
        format!("Here it is:\n{template}\nEnjoy."),
    )
    .await;
    add_shadow(
        &db,
        "bare-closing",
        format!("Here it is:\n{closing}\nEnjoy."),
    )
    .await;
    for (shadow, page) in [
        ("fenced-template", template),
        ("fenced-closing", closing),
        ("bare-template", template),
        ("bare-closing", closing),
    ] {
        assert_eq!(artifact_html(&app, shadow).await, page, "{shadow}");
    }
}

/// KT-1999 — the count answers only for a run of the workflow the path names,
/// as « Make an Artifact » does.
#[tokio::test]
async fn the_count_of_a_run_of_another_workflow_is_not_found() {
    let (app, _db) = fixture().await;
    make_in(&app, "wf-other", "run-other", None).await;
    let refused = call(
        &app,
        "POST",
        "/api/workflows/wf/runs/run-other/step-artifacts",
        Some(json!({"result_index": 1})),
        false,
    )
    .await;
    for run in ["run-other", "no-such-run"] {
        let counted = call(
            &app,
            "GET",
            &format!("/api/workflows/wf/step-artifacts/count?run_id={run}"),
            None,
            false,
        )
        .await;
        assert_eq!(counted["success"], false, "{run}: {counted}");
        assert_eq!(counted["error"], refused["error"], "{counted} vs {refused}");
        assert_eq!(counted["error"], "Run not found", "{counted}");
    }
    assert_eq!(count(&app, "wf-other", Some("run-other")).await, 1);
}

async fn run_exists(db: &kronn::db::Database, run: &'static str) -> bool {
    db.with_read_conn(move |conn| {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM workflow_runs WHERE id = ?1",
            [run],
            |row| row.get(0),
        )?;
        Ok(n == 1)
    })
    .await
    .unwrap()
}

/// KT-1999 — deleting a run answers only for a run of the workflow the path
/// names: a run of another workflow, or an unknown run, is not found and
/// nothing is deleted; the owning workflow still deletes it.
#[tokio::test]
async fn deleting_a_run_of_another_workflow_is_not_found() {
    let (app, db) = fixture().await;
    make_in(&app, "wf-other", "run-other", None).await;
    for run in ["run-other", "no-such-run"] {
        let refused = call(
            &app,
            "DELETE",
            &format!("/api/workflows/wf/runs/{run}"),
            None,
            false,
        )
        .await;
        assert_eq!(refused["success"], false, "{run}: {refused}");
        assert_eq!(refused["error"], "Run not found", "{refused}");
    }
    assert!(run_exists(&db, "run-other").await);
    assert_eq!(
        page_count(&db).await,
        1,
        "the other workflow's Artifact stays"
    );
    assert_eq!(count(&app, "wf-other", Some("run-other")).await, 1);

    let deleted = call(
        &app,
        "DELETE",
        "/api/workflows/wf-other/runs/run-other",
        None,
        false,
    )
    .await;
    assert_eq!(deleted["success"], true, "{deleted}");
    assert!(!run_exists(&db, "run-other").await);
    assert_eq!(page_count(&db).await, 0);
}
