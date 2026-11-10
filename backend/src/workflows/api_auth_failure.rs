//! KT-1524 — keep the last authentication outcome of every API connection.
//!
//! A call that the provider refuses for its credentials (401, a 403 that names
//! a credential problem, a failed OAuth / token-exchange refresh) flags the
//! connection (`mcp_auth_failures`); the next successful call unflags it. A
//! failed step says so in plain words, first line of its output, so the run
//! page never needs the raw (possibly megabytes-long) output to tell the user
//! which connection to fix.

use crate::models::{McpAuthFailureKind, McpServer, RunStatus};
use crate::workflows::steps::StepOutcome;

/// Last line of a step output whose failure is a refused credential. The
/// front end keys the run-level notice on it.
pub const AUTH_REFUSED_SIGNAL: &str = "[SIGNAL: auth_refused]";

/// Longest provider message kept on the connection and in the plain sentence.
const PROVIDER_MESSAGE_CHARS: usize = 240;

/// What a failed call says about the connection's credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRefusal {
    pub kind: McpAuthFailureKind,
    pub http_status: Option<u16>,
    /// The provider's answer, one line, scrubbed by the executor already.
    pub provider_message: String,
}

/// 403 bodies that blame the credential. A bare 403 is a missing scope or a
/// rate limit (GitHub answers 403 to both), which a new token would not cure.
const FORBIDDEN_CREDENTIAL_MARKERS: &[&str] = &[
    "bad credentials",
    "invalid token",
    "invalid_token",
    "invalid api key",
    "invalid_api_key",
    "invalid credentials",
    "invalid access token",
    "token expired",
    "token has expired",
    "token is expired",
    "expired token",
    "expired_token",
    "revoked",
    "unauthorized",
    "unauthenticated",
    "not authenticated",
    "authentication failed",
    "authentication required",
    "requires authentication",
];

const RATE_LIMIT_MARKERS: &[&str] = &["rate limit", "rate_limit", "abuse detection", "too many"];

/// Whether `output` (a failed ApiCall step's output) is a credential refusal.
pub fn classify(output: &str) -> Option<AuthRefusal> {
    // The error text, without the `[SIGNAL: …]` lines the executor appends.
    let message = output.split("\n[SIGNAL:").next().unwrap_or(output).trim();
    let first = message.lines().next()?.trim();

    if let Some(rest) = message.strip_prefix("HTTP ") {
        let status: u16 = rest.split_whitespace().next()?.parse().ok()?;
        let detail = message.split_once(" — ").map_or("", |(_, body)| body);
        let kind = match status {
            401 => McpAuthFailureKind::Refused,
            403 if blames_credential(detail) => McpAuthFailureKind::Forbidden,
            _ => return None,
        };
        return Some(AuthRefusal {
            kind,
            http_status: Some(status),
            provider_message: one_line(detail),
        });
    }

    // The call never left: the token endpoint refused the client credentials.
    // A network error or a missing config key is not a refused credential.
    for prefix in [
        "OAuth2 token unavailable: ",
        "TokenExchange token unavailable: ",
    ] {
        if let Some(detail) = first.strip_prefix(prefix) {
            if detail.contains("token exchange failed (4") {
                return Some(AuthRefusal {
                    kind: McpAuthFailureKind::RefreshFailed,
                    http_status: None,
                    provider_message: one_line(detail),
                });
            }
        }
    }
    None
}

fn blames_credential(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    !RATE_LIMIT_MARKERS
        .iter()
        .any(|marker| body.contains(marker))
        && FORBIDDEN_CREDENTIAL_MARKERS
            .iter()
            .any(|marker| body.contains(marker))
}

/// The provider's answer on one line: a JSON error body keeps its `message`
/// when it has one, whitespace collapses, the length is bounded.
fn one_line(raw: &str) -> String {
    let raw = raw.trim();
    let message = serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| {
            ["message", "error_description", "error"]
                .iter()
                .find_map(|key| value.get(key)?.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| raw.to_string());
    let collapsed = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > PROVIDER_MESSAGE_CHARS {
        let head: String = collapsed.chars().take(PROVIDER_MESSAGE_CHARS - 1).collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

/// The sentence a failed step starts with. English like every backend message;
/// the front end shows the same sentence under a translated heading.
pub fn plain_words(connection: &str, plugin: &str, refusal: &AuthRefusal) -> String {
    let what = match (refusal.kind, refusal.http_status) {
        (McpAuthFailureKind::RefreshFailed, _) => "could not renew its token".to_string(),
        (_, Some(status)) => format!("refused the token (HTTP {status})"),
        (_, None) => "refused the token".to_string(),
    };
    let provider = if refusal.provider_message.is_empty() {
        String::new()
    } else {
        format!(": {}", refusal.provider_message)
    };
    format!(
        "Connection \"{connection}\" ({plugin}) {what}{provider} — update the token in Settings → APIs."
    )
}

/// `output` led by the plain sentence and closed by the signal line.
pub(crate) fn annotate(output: &str, sentence: &str) -> String {
    format!("{sentence}\n{}\n{AUTH_REFUSED_SIGNAL}", output.trim_end())
}

/// The plain sentence a refused call's output leads with, when `output` is one
/// (a step that aggregates several calls reuses it for its own output).
pub(crate) fn sentence_of(output: &str) -> Option<&str> {
    if !output.contains(AUTH_REFUSED_SIGNAL) {
        return None;
    }
    output
        .lines()
        .find(|line| line.starts_with("Connection \""))
        .map(str::trim)
}

/// Record the auth outcome of one finished call on its connection: a success
/// clears the flag, a credential refusal sets it (and, with `annotate_step`,
/// leads the step output with the plain sentence). Any other failure leaves the
/// flag as it was: a 500 says nothing about the token.
///
/// Best effort — a DB error is logged, never propagated: bookkeeping must not
/// fail an otherwise finished call.
pub async fn track_outcome(
    state: &crate::AppState,
    plugin: &McpServer,
    config_id: &str,
    outcome: &mut StepOutcome,
    annotate_step: bool,
) {
    if config_id.is_empty() {
        return;
    }
    if outcome.result.status == RunStatus::Success {
        let id = config_id.to_string();
        let cleared = state
            .db
            .with_conn(move |conn| crate::db::mcps::clear_auth_failure(conn, &id))
            .await;
        if let Err(error) = cleared {
            tracing::warn!(target: "kronn::api_auth", "clearing the auth flag failed: {error}");
        }
        return;
    }
    if outcome.result.status != RunStatus::Failed {
        return;
    }
    let Some(refusal) = classify(&outcome.result.output) else {
        return;
    };

    let id = config_id.to_string();
    let kind = refusal.kind;
    let status = refusal.http_status;
    let message = refusal.provider_message.clone();
    let recorded = state
        .db
        .with_conn(move |conn| -> anyhow::Result<String> {
            crate::db::mcps::record_auth_failure(conn, &id, kind, status, &message)?;
            Ok(crate::db::mcps::get_config(conn, &id)?
                .map(|config| config.label)
                .unwrap_or(id))
        })
        .await;
    let label = match recorded {
        Ok(label) => label,
        Err(error) => {
            tracing::warn!(target: "kronn::api_auth", "recording the auth failure failed: {error}");
            config_id.to_string()
        }
    };
    if annotate_step {
        let sentence = plain_words(&label, &plugin.name, &refusal);
        outcome.result.output = annotate(&outcome.result.output, &sentence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16, body: &str) -> String {
        format!(
            "HTTP {status} on GET https://api.github.com/user — {body}\n[SIGNAL: ERROR]\n[SIGNAL: http_{status}]"
        )
    }

    #[test]
    fn a_401_is_a_refused_token_with_the_provider_message() {
        let refusal = classify(&http(
            401,
            r#"{"message":"Bad credentials","status":"401"}"#,
        ))
        .unwrap();
        assert_eq!(refusal.kind, McpAuthFailureKind::Refused);
        assert_eq!(refusal.http_status, Some(401));
        assert_eq!(refusal.provider_message, "Bad credentials");
    }

    #[test]
    fn a_403_counts_only_when_it_blames_the_credential() {
        let bad = classify(&http(403, "Invalid token: revoked by the owner")).unwrap();
        assert_eq!(bad.kind, McpAuthFailureKind::Forbidden);
        // Missing scope and GitHub's rate limit are 403s a new token would not cure.
        assert_eq!(
            classify(&http(403, "Resource not accessible by integration")),
            None
        );
        assert_eq!(
            classify(&http(
                403,
                "API rate limit exceeded; unauthorized requests are limited"
            )),
            None
        );
    }

    #[test]
    fn server_errors_and_other_failures_say_nothing_about_the_token() {
        assert_eq!(classify(&http(500, "boom")), None);
        assert_eq!(classify(&http(404, "Not Found")), None);
        assert_eq!(
            classify("HTTP request failed after 3 attempts: timed out"),
            None
        );
        assert_eq!(classify(""), None);
    }

    #[test]
    fn a_refused_token_exchange_is_a_failed_refresh() {
        let refusal = classify(
            "OAuth2 token unavailable: token exchange failed (401 Unauthorized): invalid_client",
        )
        .unwrap();
        assert_eq!(refusal.kind, McpAuthFailureKind::RefreshFailed);
        assert_eq!(refusal.http_status, None);
        // A network error while minting is not a refused credential.
        assert_eq!(
            classify("OAuth2 token unavailable: error sending request: timed out"),
            None
        );
    }

    #[test]
    fn the_plain_sentence_names_the_connection_the_status_and_the_action() {
        let refusal = classify(&http(401, r#"{"message":"Bad credentials"}"#)).unwrap();
        let sentence = plain_words("GitHub Euronews", "GitHub", &refusal);
        assert_eq!(
            sentence,
            "Connection \"GitHub Euronews\" (GitHub) refused the token (HTTP 401): Bad credentials — update the token in Settings → APIs."
        );
        let refresh = AuthRefusal {
            kind: McpAuthFailureKind::RefreshFailed,
            http_status: None,
            provider_message: "invalid_client".into(),
        };
        assert!(
            plain_words("Adobe", "Adobe Analytics", &refresh).contains("could not renew its token")
        );
    }

    #[test]
    fn the_sentence_leads_the_output_and_the_signal_closes_it() {
        let raw = http(401, "nope");
        let annotated = annotate(
            &raw,
            "Connection \"X\" (Y) refused the token (HTTP 401) — update.",
        );
        assert!(annotated.starts_with("Connection \"X\""));
        assert!(annotated.contains(&raw));
        assert!(annotated.ends_with(AUTH_REFUSED_SIGNAL));
    }

    #[test]
    fn a_long_provider_message_is_bounded_to_one_line() {
        let refusal = classify(&http(401, &format!("a\n{}", "b ".repeat(400)))).unwrap();
        assert!(!refusal.provider_message.contains('\n'));
        assert!(refusal.provider_message.chars().count() <= PROVIDER_MESSAGE_CHARS);
    }

    // ─── Through the executor, against a mock provider ───────────────────

    use crate::models::{
        ApiAuthKind, ApiEndpoint, ApiSpec, HostSyncMode, McpConfig, McpSource, McpTransport,
        StepType, WorkflowStep,
    };
    use crate::workflows::api_call_executor::{execute_api_call_step_with_db, SecurityPolicy};
    use crate::workflows::template::TemplateContext;
    use std::collections::HashMap;
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "ghp_kt1524_secret_never_in_output";

    async fn state_with_connection(base_url: &str) -> crate::AppState {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let secret = crate::core::crypto::generate_secret();
        state.config.write().await.encryption_secret = Some(secret.clone());
        let plugin = McpServer {
            id: "custom-github".into(),
            name: "GitHub".into(),
            description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: Some(ApiSpec {
                base_url: base_url.into(),
                auth: ApiAuthKind::Bearer {
                    env_key: "GITHUB_TOKEN".into(),
                },
                endpoints: vec![ApiEndpoint {
                    path: "/user".into(),
                    method: "GET".into(),
                    description: String::new(),
                }],
                docs_url: None,
                config_keys: vec![],
                default_headers: vec![],
                test_endpoint: None,
            }),
        };
        let env = HashMap::from([("GITHUB_TOKEN".to_string(), TOKEN.to_string())]);
        let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
        state
            .db
            .with_conn(move |conn| -> anyhow::Result<()> {
                crate::db::mcps::upsert_server(conn, &plugin)?;
                crate::db::mcps::insert_config(
                    conn,
                    &McpConfig {
                        id: "cfg-gh".into(),
                        server_id: "custom-github".into(),
                        label: "GitHub Euronews".into(),
                        env_keys: vec!["GITHUB_TOKEN".into()],
                        env_encrypted: encrypted,
                        args_override: None,
                        is_global: true,
                        include_general: true,
                        config_hash: "kt1524".into(),
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

    async fn call(state: &crate::AppState) -> crate::models::StepResult {
        let step = WorkflowStep {
            name: "collect".into(),
            step_type: StepType::ApiCall,
            api_plugin_slug: Some("custom-github".into()),
            api_config_id: Some("cfg-gh".into()),
            api_endpoint_path: Some("/user".into()),
            api_method: Some("GET".into()),
            api_max_retries: Some(0),
            ..WorkflowStep::default()
        };
        execute_api_call_step_with_db(
            &step,
            None,
            state,
            &TemplateContext::new(),
            SecurityPolicy::allow_loopback_for_tests(),
            &crate::core::api_access::ApiCaller::unidentified_agent(),
        )
        .await
        .result
    }

    async fn failure(state: &crate::AppState) -> Option<crate::models::McpAuthFailure> {
        state
            .db
            .with_conn(|conn| crate::db::mcps::list_auth_failures(conn))
            .await
            .unwrap()
            .remove("cfg-gh")
    }

    #[tokio::test]
    async fn a_401_flags_the_connection_and_the_step_names_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(
                    serde_json::json!({"message": "Bad credentials", "status": "401"}),
                ),
            )
            .mount(&server)
            .await;
        let state = state_with_connection(&server.uri()).await;

        let result = call(&state).await;

        assert_eq!(result.status, RunStatus::Failed);
        let first_line = result.output.lines().next().unwrap();
        assert_eq!(
            first_line,
            "Connection \"GitHub Euronews\" (GitHub) refused the token (HTTP 401): Bad credentials — update the token in Settings → APIs."
        );
        assert!(result.output.contains("HTTP 401 on GET"), "raw output kept");
        assert!(result.output.trim_end().ends_with(AUTH_REFUSED_SIGNAL));
        assert!(!result.output.contains(TOKEN));

        let recorded = failure(&state).await.expect("the refusal is recorded");
        assert_eq!(recorded.kind, McpAuthFailureKind::Refused);
        assert_eq!(recorded.http_status, Some(401));
        assert_eq!(recorded.message, "Bad credentials");
        assert!(!recorded.failed_at.is_empty());

        // The card and the notice read it from the connection list.
        let listed = state
            .db
            .with_conn(|conn| crate::db::mcps::list_configs_display(conn, None))
            .await
            .unwrap();
        assert_eq!(listed[0].auth_failure.as_ref(), Some(&recorded));
    }

    #[tokio::test]
    async fn the_next_successful_call_clears_the_flag() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"login": "me"})),
            )
            .mount(&server)
            .await;
        let state = state_with_connection(&server.uri()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::mcps::record_auth_failure(
                    conn,
                    "cfg-gh",
                    McpAuthFailureKind::Refused,
                    Some(401),
                    "Bad credentials",
                )
            })
            .await
            .unwrap();
        assert!(failure(&state).await.is_some());

        let result = call(&state).await;

        assert_eq!(result.status, RunStatus::Success, "{}", result.output);
        assert!(
            failure(&state).await.is_none(),
            "a working token unflags it"
        );
    }

    #[tokio::test]
    async fn a_403_without_a_credential_message_does_not_flag_the_connection() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_json(
                serde_json::json!({"message": "Resource not accessible by personal access token"}),
            ))
            .mount(&server)
            .await;
        let state = state_with_connection(&server.uri()).await;

        let result = call(&state).await;

        assert_eq!(result.status, RunStatus::Failed);
        assert!(
            result.output.starts_with("HTTP 403 on GET"),
            "{}",
            result.output
        );
        assert!(failure(&state).await.is_none());
    }

    #[tokio::test]
    async fn a_server_error_leaves_an_existing_flag_alone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let state = state_with_connection(&server.uri()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::mcps::record_auth_failure(
                    conn,
                    "cfg-gh",
                    McpAuthFailureKind::Refused,
                    Some(401),
                    "Bad credentials",
                )
            })
            .await
            .unwrap();

        let result = call(&state).await;

        assert_eq!(result.status, RunStatus::Failed);
        assert!(
            failure(&state).await.is_some(),
            "a 500 says nothing about the token"
        );
    }

    #[tokio::test]
    async fn deleting_the_connection_drops_its_flag() {
        let server = MockServer::start().await;
        let state = state_with_connection(&server.uri()).await;
        state
            .db
            .with_conn(|conn| -> anyhow::Result<()> {
                crate::db::mcps::record_auth_failure(
                    conn,
                    "cfg-gh",
                    McpAuthFailureKind::Forbidden,
                    Some(403),
                    "revoked",
                )?;
                crate::db::mcps::delete_config(conn, "cfg-gh")?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(failure(&state).await.is_none());
    }
}
