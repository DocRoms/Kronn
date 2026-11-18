use std::collections::HashMap;

use base64::{engine::general_purpose::STANDARD, Engine as _};

use serde_json::{json, Value};

use wiremock::matchers::{any, header, method, path};

use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::models::*;

use crate::workflows::api_call_executor::{
    execute_api_call_step_with_db, ApiCallLogContext, SecurityPolicy,
};

use crate::workflows::batch_apicall_step::execute_batch_apicall_step_with_policy;

use crate::workflows::publish_page_step::execute_publish_page_data_step;

use crate::workflows::step_output_format::{format_step_output_simple, parse_envelope_for_test};

use crate::workflows::template::TemplateContext;

const USER: &str = "bot@example.com";

const TOKEN: &str = "kt816-jira-token-never-in-output";

const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const THUMB: &str = "/rest/api/2/attachment/thumbnail";

fn png() -> Vec<u8> {
    STANDARD.decode(PNG_1X1).unwrap()
}

fn basic_credential() -> String {
    format!("Basic {}", STANDARD.encode(format!("{USER}:{TOKEN}")))
}

/// Serves `body` for `/thumbnail/<id>` only with the Jira credential;
/// every other request gets a 401.
async fn mount_thumbnail(server: &MockServer, id: &str, body: Vec<u8>, content_type: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{THUMB}/{id}")))
        .and(header("Authorization", basic_credential().as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, content_type))
        .mount(server)
        .await;
}

async fn mount_auth_wall(server: &MockServer) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(401))
        .with_priority(10)
        .mount(server)
        .await;
}

/// App state whose only Jira credential lives encrypted in the database.
async fn jira_state(base_url: &str) -> crate::AppState {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
    let config = Arc::new(RwLock::new(crate::core::config::default_config()));
    let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
    let secret = crate::core::crypto::generate_secret();
    state.config.write().await.encryption_secret = Some(secret.clone());
    let plugin = McpServer {
        id: "mcp-atlassian".into(),
        name: "Jira".into(),
        description: String::new(),
        transport: McpTransport::ApiOnly,
        source: McpSource::Manual,
        api_spec: Some(ApiSpec {
            base_url: base_url.into(),
            auth: ApiAuthKind::Basic {
                user_env: "JIRA_USERNAME".into(),
                password_env: "JIRA_API_TOKEN".into(),
            },
            endpoints: vec![ApiEndpoint {
                method: "GET".into(),
                path: format!("{THUMB}/{{id}}"),
                description: "attachment thumbnail".into(),
            }],
            docs_url: None,
            config_keys: vec![],
            default_headers: vec![],
            test_endpoint: None,
        }),
    };
    let env = HashMap::from([
        ("JIRA_USERNAME".to_string(), USER.to_string()),
        ("JIRA_API_TOKEN".to_string(), TOKEN.to_string()),
    ]);
    let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
    state
        .db
        .with_conn(move |conn| -> anyhow::Result<()> {
            crate::db::mcps::upsert_server(conn, &plugin)?;
            crate::db::mcps::insert_config(
                conn,
                &McpConfig {
                    id: "cfg-jira".into(),
                    server_id: "mcp-atlassian".into(),
                    label: "Jira".into(),
                    env_keys: vec!["JIRA_USERNAME".into(), "JIRA_API_TOKEN".into()],
                    env_encrypted: encrypted,
                    args_override: None,
                    is_global: true,
                    include_general: true,
                    config_hash: "kt816-jira".into(),
                    project_ids: Vec::new(),
                    host_sync: HostSyncMode::None,
                },
            )?;
            Ok(())
        })
        .await
        .unwrap();
    state
}

fn thumbnail_step(endpoint: &str, response: Option<ApiResponseMode>) -> WorkflowStep {
    WorkflowStep {
        name: "thumb".into(),
        step_type: StepType::ApiCall,
        api_plugin_slug: Some("mcp-atlassian".into()),
        api_config_id: Some("cfg-jira".into()),
        api_endpoint_path: Some(endpoint.into()),
        api_max_retries: Some(0),
        api_response: response,
        ..WorkflowStep::default()
    }
}

fn images() -> Option<ApiResponseMode> {
    Some(ApiResponseMode::Binary {
        accept: vec![],
        max_bytes: None,
    })
}

fn assert_no_credential(text: &str) {
    assert!(!text.contains(TOKEN), "token leaked: {text}");
    assert!(
        !text.contains(&basic_credential()),
        "credential leaked: {text}"
    );
    assert!(
        !text.contains(&STANDARD.encode(format!("{USER}:{TOKEN}"))),
        "encoded credential leaked"
    );
}

async fn run_single(state: &crate::AppState, step: &WorkflowStep) -> StepResult {
    execute_api_call_step_with_db(
        step,
        None,
        state,
        &TemplateContext::new(),
        SecurityPolicy::allow_loopback_for_tests(),
        &crate::core::api_access::ApiCaller::Human,
    )
    .await
    .result
}

#[tokio::test]
async fn broker_fetches_a_jira_thumbnail_as_binary_without_exposing_the_credential() {
    let server = MockServer::start().await;
    mount_thumbnail(&server, "10001", png(), "image/png").await;
    mount_auth_wall(&server).await;
    let state = jira_state(&server.uri()).await;

    // The endpoint really demands the credential: without it, 401.
    let anonymous = reqwest::get(format!("{}{THUMB}/10001", server.uri()))
        .await
        .unwrap();
    assert_eq!(anonymous.status().as_u16(), 401);

    let result = run_single(&state, &thumbnail_step(&format!("{THUMB}/10001"), images())).await;
    assert_eq!(result.status, RunStatus::Success, "{}", result.output);
    let envelope = parse_envelope_for_test(&result.output);
    let data = &envelope["data"];
    assert_eq!(data["content_type"], "image/png");
    assert_eq!(data["size"], png().len());
    assert_eq!(
        STANDARD.decode(data["base64"].as_str().unwrap()).unwrap(),
        png()
    );
    assert_eq!(data["data_uri"], format!("data:image/png;base64,{PNG_1X1}"));
    assert!(
        envelope["summary"]
            .as_str()
            .unwrap()
            .ends_with(&format!("image/png, {} bytes", png().len())),
        "{}",
        envelope["summary"]
    );
    assert!(result.output.contains("[SIGNAL: OK]"));
    assert_no_credential(&result.output);

    let received = server.received_requests().await.unwrap();
    let brokered = received
        .iter()
        .filter(|request| request.headers.contains_key("authorization"))
        .count();
    assert_eq!(brokered, 1, "exactly one authenticated call");

    let logs = state
        .db
        .with_conn(|conn| {
            crate::db::api_call_logs::list(conn, Default::default())
                .map_err(|error| anyhow::anyhow!(error))
        })
        .await
        .unwrap();
    assert_eq!(logs.len(), 1);
    assert_no_credential(&format!("{:?}", logs[0]));
}

#[tokio::test]
async fn json_steps_keep_their_behaviour_when_binary_is_not_requested() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{THUMB}/json")))
        .and(header("Authorization", basic_credential().as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "10001"})))
        .mount(&server)
        .await;
    mount_thumbnail(&server, "10001", png(), "image/png").await;
    mount_auth_wall(&server).await;
    let state = jira_state(&server.uri()).await;

    for response in [None, Some(ApiResponseMode::Json)] {
        let json = run_single(
            &state,
            &thumbnail_step(&format!("{THUMB}/json"), response.clone()),
        )
        .await;
        assert_eq!(json.status, RunStatus::Success, "{}", json.output);
        assert_eq!(
            parse_envelope_for_test(&json.output)["data"],
            json!({"id": "10001"})
        );

        // Without the option an image still fails exactly as before.
        let image = run_single(&state, &thumbnail_step(&format!("{THUMB}/10001"), response)).await;
        assert_eq!(image.status, RunStatus::Failed);
        assert!(
            image.output.contains("Response JSON parse failed (200)"),
            "{}",
            image.output
        );
    }
}

#[tokio::test]
async fn binary_mode_refuses_media_types_the_step_did_not_declare() {
    let server = MockServer::start().await;
    mount_thumbnail(&server, "page", b"<html>login</html>".to_vec(), "text/html").await;
    mount_thumbnail(&server, "10001", png(), "image/png").await;
    mount_auth_wall(&server).await;
    let state = jira_state(&server.uri()).await;

    let html = run_single(&state, &thumbnail_step(&format!("{THUMB}/page"), images())).await;
    assert_eq!(html.status, RunStatus::Failed);
    assert!(
        html.output
            .contains("Content-Type `text/html` is not accepted"),
        "{}",
        html.output
    );
    assert!(!html.output.contains("login"));

    let pdf_only = Some(ApiResponseMode::Binary {
        accept: vec!["application/pdf".into()],
        max_bytes: None,
    });
    let png = run_single(&state, &thumbnail_step(&format!("{THUMB}/10001"), pdf_only)).await;
    assert_eq!(png.status, RunStatus::Failed);
    assert!(
        png.output.contains("`image/png` is not accepted"),
        "{}",
        png.output
    );
}

#[tokio::test]
async fn binary_body_over_the_cap_is_refused_rather_than_truncated() {
    let server = MockServer::start().await;
    mount_thumbnail(&server, "10001", png(), "image/png").await;
    mount_auth_wall(&server).await;
    let state = jira_state(&server.uri()).await;

    let capped = Some(ApiResponseMode::Binary {
        accept: vec!["image/png".into()],
        max_bytes: Some(16),
    });
    let result = run_single(&state, &thumbnail_step(&format!("{THUMB}/10001"), capped)).await;
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result.output.contains(&format!(
            "body exceeds max_bytes ({} > 16 bytes)",
            png().len()
        )),
        "{}",
        result.output
    );
    assert!(!result.output.contains("base64"), "no partial payload");
}

/// A chunked body announces no length: the cap must hold while streaming.
#[tokio::test]
async fn chunked_body_over_the_cap_is_refused_while_streaming() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await;
        let chunk = vec![b'x'; 40];
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nTransfer-Encoding: chunked\r\n\r\n"
                .to_vec();
        for _ in 0..3 {
            response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            response.extend_from_slice(&chunk);
            response.extend_from_slice(b"\r\n");
        }
        response.extend_from_slice(b"0\r\n\r\n");
        let _ = socket.write_all(&response).await;
    });
    let policy = BinaryPolicyFixture::png(100);
    let response = reqwest::get(format!("http://{address}/stream"))
        .await
        .unwrap();
    assert_eq!(response.content_length(), None);
    let error = super::read_binary_body(response, &policy.0)
        .await
        .unwrap_err();
    assert!(
        error.contains("body exceeds max_bytes (120 > 100 bytes)"),
        "{error}"
    );
}

struct BinaryPolicyFixture(super::BinaryPolicy);

impl BinaryPolicyFixture {
    fn png(max_bytes: u64) -> Self {
        Self(super::BinaryPolicy {
            accept: vec!["image/png".into()],
            max_bytes,
        })
    }
}

/// DoD path: one BatchApiCall fetches every thumbnail, then PublishPageData
/// stores one data URI per attachment id in the Page dataset.
#[tokio::test]
async fn batch_thumbnails_reach_a_page_dataset_as_data_uris_indexed_by_attachment() {
    let server = MockServer::start().await;
    let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    mount_thumbnail(&server, "10001", png(), "image/png").await;
    mount_thumbnail(&server, "10002", jpeg.clone(), "image/jpeg").await;
    mount_auth_wall(&server).await;
    let state = jira_state(&server.uri()).await;
    create_page(&state).await;

    let mut context = TemplateContext::new();
    context.set_step_output(
        "attachments",
        &format_step_output_simple(json!([{"id": "10001"}, {"id": "10002"}]), "OK", "2"),
    );
    let batch = WorkflowStep {
        name: "thumbs".into(),
        step_type: StepType::BatchApiCall,
        batch_items_from: Some("{{steps.attachments.data}}".into()),
        api_extract: Some(ExtractSpec {
            path: "$.data_uri".into(),
            fallback: None,
            fail_on_empty: true,
        }),
        ..thumbnail_step(&format!("{THUMB}/{{{{batch.item.id}}}}"), images())
    };
    let fetched = execute_batch_apicall_step_with_policy(
        &batch,
        None,
        &state,
        &context,
        ApiCallLogContext::workflow(),
        SecurityPolicy::allow_loopback_for_tests(),
        &crate::core::api_access::ApiCaller::Human,
    )
    .await
    .result;
    assert_eq!(fetched.status, RunStatus::Success, "{}", fetched.output);
    assert_no_credential(&fetched.output);
    context.set_step_output("thumbs", &fetched.output);

    let publish = WorkflowStep {
        name: "publish".into(),
        step_type: StepType::PublishPageData,
        page_publish: Some(PublishPageDataConfig {
            page_id: "team-board".into(),
            writes: vec![PublishPageDataWrite {
                dataset: "ticket_images".into(),
                operation: LivePageWriteOperation::Replace,
                value_from: "steps.thumbs.data.items".into(),
                observed_at: None,
                dedupe_key: None,
                key_field: None,
            }],
        }),
        ..WorkflowStep::default()
    };
    insert_workflow_run(&state, vec![batch.clone(), publish.clone()]).await;
    let published =
        execute_publish_page_data_step(&publish, "wf-team", "run-1", None, &state, &context)
            .await
            .result;
    assert_eq!(published.status, RunStatus::Success, "{}", published.output);

    let page = state
        .db
        .with_conn(|conn| crate::db::live_pages::get_live_page(conn, "team-board"))
        .await
        .unwrap()
        .expect("page");
    let dataset = page
        .datasets
        .iter()
        .find(|view| view.dataset.name == "ticket_images")
        .unwrap();
    let by_id: HashMap<String, String> = dataset
        .dataset
        .current
        .as_ref()
        .and_then(Value::as_array)
        .expect("published items")
        .iter()
        .map(|item| {
            assert_eq!(item["status"], "OK");
            (
                item["input"]["id"].as_str().unwrap().to_string(),
                item["response"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(by_id["10001"], format!("data:image/png;base64,{PNG_1X1}"));
    assert_eq!(
        by_id["10002"],
        format!("data:image/jpeg;base64,{}", STANDARD.encode(&jpeg))
    );
    assert_no_credential(&serde_json::to_string(&page).unwrap());
}

/// The publication ledger references the workflow and its run.
async fn insert_workflow_run(state: &crate::AppState, steps: Vec<WorkflowStep>) {
    let now = chrono::Utc::now();
    let workflow = Workflow {
        refs: Default::default(),
        retention: None,
        project_scope: None,
        id: "wf-team".into(),
        name: "Team board thumbnails".into(),
        project_id: None,
        trigger: WorkflowTrigger::Manual,
        steps,
        actions: vec![],
        safety: WorkflowSafety {
            sandbox: false,
            max_files: None,
            max_lines: None,
            require_approval: false,
        },
        workspace_config: None,
        concurrency_limit: None,
        concurrency_key: None,
        guards: None,
        artifacts: HashMap::new(),
        on_failure: vec![],
        exec_allowlist: vec![],
        variables: vec![],
        enabled: true,
        pinned: false,
        created_at: now,
        updated_at: now,
    };
    let run = WorkflowRun {
        outcome: None,
        id: "run-1".into(),
        workflow_id: "wf-team".into(),
        status: RunStatus::Running,
        trigger_context: None,
        step_results: vec![],
        tokens_used: 0,
        workspace_path: None,
        started_at: now,
        finished_at: None,
        run_type: "linear".into(),
        batch_total: 0,
        batch_completed: 0,
        batch_failed: 0,
        batch_no_response: 0,
        batch_name: None,
        parent_run_id: None,
        state: HashMap::new(),
        produced_branches: vec![],
        concurrency_key: None,
        triggered_by_run_id: None,
        project_id: None,
        parent_workflow_id: None,
        parent_workflow_name: None,
        parent_run_started_at: None,
    };
    state
        .db
        .with_conn(move |conn| {
            crate::db::workflows::insert_workflow(conn, &workflow)?;
            crate::db::workflows::insert_run(conn, &run)
        })
        .await
        .unwrap();
}

async fn create_page(state: &crate::AppState) {
    let now = chrono::Utc::now();
    state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::create_live_page(
                conn,
                &LivePage {
                    id: "page-team".into(),
                    project_id: None,
                    title: "Team board".into(),
                    slug: "team-board".into(),
                    current_revision_id: "rev-team".into(),
                    data_revision: 0,
                    created_at: now,
                    updated_at: now,
                    last_published_at: None,
                    pinned: false,
                    archived: false,
                    html_size_bytes: 0,
                },
                &LivePageRevision {
                    id: "rev-team".into(),
                    page_id: "page-team".into(),
                    revision: 1,
                    html: "<img id=thumb>".into(),
                    created_by_agent: None,
                    created_at: now,
                },
                &[CreateLivePageDataset {
                    name: "ticket_images".into(),
                    kind: LivePageDatasetKind::Snapshot,
                    initial: None,
                    schema: None,
                    max_points: None,
                    max_age_days: None,
                }],
                None,
            )
        })
        .await
        .unwrap();
}
