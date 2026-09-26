//! Binary response mode for `ApiCall` / `BatchApiCall` (`api_response: Binary`).
//!
//! A 2xx body of a declared media type is returned as
//! `{content_type, size, base64, data_uri}` instead of being parsed as JSON.
//! The credentials still never leave the server: only the body comes back.
//! Two guards keep the broker from becoming a file proxy: the media type must
//! match the step's `accept` list, and a body over `max_bytes` is refused.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::header::CONTENT_TYPE;
use serde_json::{json, Value};

use crate::models::{ApiResponseMode, PaginationSpec, WorkflowStep};

/// Default body cap: Jira thumbnails weigh a few tens of KiB.
pub const DEFAULT_BINARY_MAX_BYTES: u64 = 256 * 1024;
/// Largest cap a step may declare; each byte is stored ~1.33x as base64.
pub const BINARY_MAX_BYTES_CEILING: u64 = 2 * 1024 * 1024;
const DEFAULT_ACCEPT: &str = "image/*";
const MAX_ACCEPT_ENTRIES: usize = 20;

/// Validated binary contract of one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryPolicy {
    accept: Vec<String>,
    max_bytes: u64,
}

impl BinaryPolicy {
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    fn accepts(&self, media_type: &str) -> bool {
        let family = media_type.split('/').next().unwrap_or_default();
        self.accept
            .iter()
            .any(|entry| match entry.strip_suffix("/*") {
                Some(accepted_family) => accepted_family == family,
                None => entry == media_type,
            })
    }

    fn accept_label(&self) -> String {
        self.accept.join(", ")
    }
}

/// `Ok(None)` = JSON mode (field absent or `Json`). Used both when the
/// workflow is saved and before any request is sent.
pub fn resolve_binary_policy(step: &WorkflowStep) -> Result<Option<BinaryPolicy>, String> {
    let Some(ApiResponseMode::Binary { accept, max_bytes }) = step.api_response.as_ref() else {
        return Ok(None);
    };
    if matches!(
        step.api_pagination,
        Some(
            PaginationSpec::Offset { .. }
                | PaginationSpec::Cursor { .. }
                | PaginationSpec::Page { .. }
                | PaginationSpec::LinkHeader { .. }
        )
    ) {
        return Err("`api_response: Binary` returns one file; remove `api_pagination`.".into());
    }
    let max_bytes = max_bytes.unwrap_or(DEFAULT_BINARY_MAX_BYTES);
    if max_bytes == 0 || max_bytes > BINARY_MAX_BYTES_CEILING {
        return Err(format!(
            "`api_response.max_bytes` must be between 1 and {BINARY_MAX_BYTES_CEILING} bytes (got {max_bytes})."
        ));
    }
    if accept.len() > MAX_ACCEPT_ENTRIES {
        return Err(format!(
            "`api_response.accept` lists at most {MAX_ACCEPT_ENTRIES} media types."
        ));
    }
    let mut normalized = Vec::with_capacity(accept.len().max(1));
    for entry in accept {
        let entry = entry.trim().to_ascii_lowercase();
        if !is_accept_entry(&entry) {
            return Err(format!(
                "`api_response.accept` entry `{entry}` is not `type/subtype` or `type/*` \
                 (`*/*` is refused: declare the media types the step expects)."
            ));
        }
        if !normalized.contains(&entry) {
            normalized.push(entry);
        }
    }
    if normalized.is_empty() {
        normalized.push(DEFAULT_ACCEPT.to_string());
    }
    Ok(Some(BinaryPolicy {
        accept: normalized,
        max_bytes,
    }))
}

/// Read a 2xx body under `policy`. `Ok(None)` = empty body, handled by the
/// caller like an empty JSON success. The type is checked before download.
pub(super) async fn read_binary_body(
    mut response: reqwest::Response,
    policy: &BinaryPolicy,
) -> Result<Option<Value>, String> {
    if response.content_length() == Some(0) {
        return Ok(None);
    }
    let declared = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let media_type = match declared.as_deref().and_then(media_type_of) {
        Some(media_type) if policy.accepts(&media_type) => media_type,
        Some(media_type) => {
            return Err(format!(
                "Binary response refused: Content-Type `{media_type}` is not accepted by this step \
                 (accept: {}).",
                policy.accept_label()
            ))
        }
        None => {
            return Err(format!(
                "Binary response refused: missing or invalid Content-Type (accept: {}).",
                policy.accept_label()
            ))
        }
    };
    if let Some(length) = response.content_length() {
        if length > policy.max_bytes {
            return Err(too_large(length, policy.max_bytes));
        }
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("Binary response body read failed: {error}"))?
    {
        let total = (body.len() + chunk.len()) as u64;
        if total > policy.max_bytes {
            return Err(too_large(total, policy.max_bytes));
        }
        body.extend_from_slice(&chunk);
    }
    if body.is_empty() {
        return Ok(None);
    }
    Ok(Some(binary_envelope(&media_type, &body)))
}

/// One-line summary that never embeds the payload itself.
pub fn binary_summary(value: &Value) -> Option<String> {
    let content_type = value.get("content_type")?.as_str()?;
    let size = value.get("size")?.as_u64()?;
    Some(format!("{content_type}, {size} bytes"))
}

fn binary_envelope(media_type: &str, body: &[u8]) -> Value {
    let base64 = STANDARD.encode(body);
    let data_uri = format!("data:{media_type};base64,{base64}");
    json!({
        "content_type": media_type,
        "size": body.len(),
        "base64": base64,
        "data_uri": data_uri,
    })
}

fn too_large(size: u64, max_bytes: u64) -> String {
    format!(
        "Binary response refused: body exceeds max_bytes ({size} > {max_bytes} bytes). \
         Raise `api_response.max_bytes` (at most {BINARY_MAX_BYTES_CEILING}) or request a smaller \
         rendition such as a thumbnail."
    )
}

/// `image/PNG; charset=x` → `image/png`, or `None` when it is not a concrete
/// `type/subtype` pair, so nothing unexpected reaches the data URI.
fn media_type_of(header: &str) -> Option<String> {
    let media_type = header.split(';').next()?.trim().to_ascii_lowercase();
    let (family, subtype) = media_type.split_once('/')?;
    (is_token(family) && is_token(subtype)).then_some(media_type)
}

fn is_accept_entry(entry: &str) -> bool {
    let Some((family, subtype)) = entry.split_once('/') else {
        return false;
    };
    is_token(family) && (subtype == "*" || is_token(subtype))
}

/// RFC 6838 restricted name.
fn is_token(part: &str) -> bool {
    let mut chars = part.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && part.len() <= 127
        && chars.all(|c| c.is_ascii_alphanumeric() || "!#$&^_.+-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary_step(accept: &[&str], max_bytes: Option<u64>) -> WorkflowStep {
        WorkflowStep {
            name: "thumb".into(),
            api_response: Some(ApiResponseMode::Binary {
                accept: accept.iter().map(|value| value.to_string()).collect(),
                max_bytes,
            }),
            ..WorkflowStep::default()
        }
    }

    #[test]
    fn absent_or_json_mode_keeps_the_json_path() {
        assert_eq!(resolve_binary_policy(&WorkflowStep::default()), Ok(None));
        let json = WorkflowStep {
            api_response: Some(ApiResponseMode::Json),
            ..WorkflowStep::default()
        };
        assert_eq!(resolve_binary_policy(&json), Ok(None));
    }

    #[test]
    fn binary_defaults_to_images_under_256_kib() {
        let policy = resolve_binary_policy(&binary_step(&[], None))
            .unwrap()
            .unwrap();
        assert_eq!(policy.max_bytes(), 256 * 1024);
        assert!(policy.accepts("image/png"));
        assert!(policy.accepts("image/jpeg"));
        assert!(!policy.accepts("application/pdf"));
        assert!(!policy.accepts("text/html"));
    }

    #[test]
    fn explicit_list_accepts_only_what_it_declares() {
        let policy = resolve_binary_policy(&binary_step(&[" Image/PNG ", "application/pdf"], None))
            .unwrap()
            .unwrap();
        assert!(policy.accepts("image/png"));
        assert!(policy.accepts("application/pdf"));
        assert!(!policy.accepts("image/jpeg"));
    }

    #[test]
    fn wildcard_and_malformed_types_are_refused() {
        for bad in ["*/*", "*", "image", "image/png; q=1", "/png", "image/", ""] {
            let error = resolve_binary_policy(&binary_step(&[bad], None)).unwrap_err();
            assert!(error.contains("accept"), "{bad}: {error}");
        }
    }

    #[test]
    fn cap_must_stay_between_one_byte_and_the_ceiling() {
        assert!(resolve_binary_policy(&binary_step(&[], Some(0))).is_err());
        let error = resolve_binary_policy(&binary_step(&[], Some(BINARY_MAX_BYTES_CEILING + 1)))
            .unwrap_err();
        assert!(error.contains("max_bytes"), "{error}");
        let policy = resolve_binary_policy(&binary_step(&[], Some(BINARY_MAX_BYTES_CEILING)))
            .unwrap()
            .unwrap();
        assert_eq!(policy.max_bytes(), BINARY_MAX_BYTES_CEILING);
    }

    #[test]
    fn multi_page_pagination_is_refused_for_a_single_file() {
        let mut step = binary_step(&[], None);
        step.api_pagination = Some(PaginationSpec::LinkHeader {
            page_size_param: None,
            page_size: None,
            max_pages: None,
        });
        assert!(resolve_binary_policy(&step)
            .unwrap_err()
            .contains("api_pagination"));
        step.api_pagination = Some(PaginationSpec::None);
        assert!(resolve_binary_policy(&step).unwrap().is_some());
    }

    #[test]
    fn response_media_type_is_normalized_before_reaching_the_data_uri() {
        assert_eq!(
            media_type_of("image/PNG; charset=binary").as_deref(),
            Some("image/png")
        );
        assert_eq!(media_type_of("image/png,text/html"), None);
        assert_eq!(media_type_of("image/*"), None);
        assert_eq!(media_type_of("garbage"), None);
    }

    #[test]
    fn envelope_carries_a_ready_data_uri_and_a_payload_free_summary() {
        let envelope = binary_envelope("image/png", &[0x89, b'P', b'N', b'G']);
        assert_eq!(envelope["size"], 4);
        assert_eq!(envelope["base64"], "iVBORw==");
        assert_eq!(envelope["data_uri"], "data:image/png;base64,iVBORw==");
        assert_eq!(
            binary_summary(&envelope).as_deref(),
            Some("image/png, 4 bytes")
        );
    }
}

/// Broker-level proof: credentials come from the encrypted plugin config, the
/// mock server demands them, and only the file comes back.
#[cfg(test)]
mod broker_tests {
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
    use crate::workflows::step_output_format::{
        format_step_output_simple, parse_envelope_for_test,
    };
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
            let image =
                run_single(&state, &thumbnail_step(&format!("{THUMB}/10001"), response)).await;
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
            execute_publish_page_data_step(&publish, "wf-team", "run-1", &state, &context)
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
}
