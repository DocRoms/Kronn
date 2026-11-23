//! KT-1692 — check the token of every API connection ahead of the call that
//! would need it.
//!
//! KT-1524 flags a connection only after a real call was refused. A connection
//! nobody called since its token expired would stay silent until a workflow
//! failed on it. This module probes each connection that has a test endpoint
//! (the readiness probe the plugin card's "Test" already uses): at start, once
//! a day ([`spawn_scheduler`]) and on demand ([`check_connection`],
//! [`check_all`]). The probe runs through the same ApiCall path as any call, so
//! a refused probe flags the connection and a good one clears it exactly as a
//! real call does (`workflows::api_auth_failure::track_outcome`); this module
//! adds what the flag does not keep: when the connection was probed, what the
//! probe said, and when its token last worked.
//!
//! Bounds: a connection is never probed twice within [`MIN_RECHECK`] (a click
//! storm, a restart loop, a second request while one is in flight all get the
//! stored answer), a batch runs [`CHECK_CONCURRENCY`] probes at a time, and each
//! probe has the 15 s timeout of the readiness probe. The token only ever goes
//! to the connection's own provider; what is stored of an answer is redacted.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Semaphore;

use crate::models::{McpServer, McpTokenCheck, McpTokenCheckOutcome, McpTokenCheckResult};
use crate::workflows::api_call_executor::SecurityPolicy;
use crate::AppState;

/// A connection probed less than this long ago is not probed again.
pub const MIN_RECHECK: Duration = Duration::from_secs(5 * 60);

/// How often the scheduler probes every connection once Kronn is up.
pub const DAILY: Duration = Duration::from_secs(24 * 60 * 60);

/// Wait after boot before the first probe, so the start-up work is not slowed.
const START_DELAY: Duration = Duration::from_secs(20);

/// Probes in flight at once in a batch.
pub const CHECK_CONCURRENCY: usize = 2;

/// Longest probe answer kept on the connection.
const DETAIL_CHARS: usize = 240;

/// Why a connection could not be checked at all (as opposed to a check that
/// found its token refused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    NotFound,
    /// The connection has no test endpoint to probe.
    NoProbe,
    Internal(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::NotFound => f.write_str("Config not found"),
            CheckError::NoProbe => f.write_str(
                "This connection has no test endpoint: declare a GET endpoint without path parameters and select it as the test endpoint",
            ),
            CheckError::Internal(message) => f.write_str(message),
        }
    }
}

pub use crate::core::registry::has_token_probe;

/// Connections being probed right now: a second request for one of them gets
/// the stored answer instead of a second call to the provider.
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(Default::default)
}

struct InFlight(String);

impl InFlight {
    /// Keyed by database too: one process may hold several instances (tests).
    fn enter(state: &AppState, config_id: &str) -> Option<Self> {
        let key = format!("{:p}:{config_id}", Arc::as_ptr(&state.db));
        let mut set = in_flight().lock().unwrap_or_else(|e| e.into_inner());
        // Not `then_some(Self(key))`: that builds the guard eagerly, and the
        // dropped one would lock the set again (deadlock) and free the key of
        // the check that really holds it.
        if set.insert(key.clone()) {
            Some(Self(key))
        } else {
            None
        }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        in_flight()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// Whether `check` is recent enough, at `now`, to answer in place of a probe.
pub fn is_fresh(check: &McpTokenCheck, now: DateTime<Utc>) -> bool {
    let Some(at) = check
        .checked_at
        .as_deref()
        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
    else {
        return false;
    };
    let age = now.signed_duration_since(at.with_timezone(&Utc));
    // A clock that went back counts as fresh: no probe storm on a bad clock.
    age < chrono::Duration::from_std(MIN_RECHECK).unwrap_or_default()
}

/// What may be stored and shown of a probe's answer: the connection's own
/// secret values out, the heuristic redaction of stored text, then bounded.
fn clean_detail(detail: &str, secrets: &crate::core::secret_scrub::SecretSet) -> String {
    let masked = crate::core::redact::redact_stored_text(&secrets.scrub(detail));
    let collapsed = masked.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > DETAIL_CHARS {
        let head: String = collapsed.chars().take(DETAIL_CHARS - 1).collect();
        format!("{head}…")
    } else {
        collapsed
    }
}

/// Probe one connection now, unless it was probed within [`MIN_RECHECK`].
pub async fn check_connection(
    state: &AppState,
    config_id: &str,
) -> Result<McpTokenCheckResult, CheckError> {
    check_connection_with(state, config_id, Utc::now(), SecurityPolicy::production()).await
}

/// [`check_connection`] at a given time and network policy (tests probe a
/// loopback provider).
pub async fn check_connection_with(
    state: &AppState,
    config_id: &str,
    now: DateTime<Utc>,
    policy: SecurityPolicy,
) -> Result<McpTokenCheckResult, CheckError> {
    // The guard comes before the stored row is read: a check that finishes
    // between our read and our entry would leave us judging a stale row as not
    // fresh and sending a second probe. Held, it makes the row we read the
    // one the previous check left.
    let guard = InFlight::enter(state, config_id);
    let secret = state
        .config
        .read()
        .await
        .encryption_secret
        .clone()
        .ok_or_else(|| CheckError::Internal("No encryption secret configured".into()))?;
    let id = config_id.to_string();
    let loaded = state
        .db
        .with_conn(move |conn| {
            let Some(config) = crate::db::mcps::get_config(conn, &id)? else {
                return Ok(None);
            };
            let server = crate::db::mcps::list_servers(conn)?
                .into_iter()
                .find(|server| server.id == config.server_id);
            let stored = crate::db::mcps::get_token_check(conn, &id)?;
            let env = crate::db::mcps::decrypt_env(&config.env_encrypted, &secret);
            Ok::<_, anyhow::Error>(Some((config, server, stored, env)))
        })
        .await
        .map_err(|error| CheckError::Internal(format!("DB error: {error}")))?;
    let (config, server, stored, env) = loaded.ok_or(CheckError::NotFound)?;
    let server = server.filter(has_token_probe).ok_or(CheckError::NoProbe)?;
    let stored = stored.unwrap_or(McpTokenCheck {
        checked_at: None,
        outcome: None,
        detail: String::new(),
        last_good_at: None,
    });

    if guard.is_none() || is_fresh(&stored, now) {
        return Ok(McpTokenCheckResult {
            config_id: config_id.to_string(),
            check: stored,
            rate_limited: true,
        });
    }

    let (outcome, detail, secrets) = match env {
        Err(error) => (
            McpTokenCheckOutcome::Inconclusive,
            format!("Cannot decrypt the connection's configuration: {error}"),
            Default::default(),
        ),
        Ok(env) => {
            // The stored detail only describes the probe: the diagnostic set.
            let secrets = crate::workflows::api_call_executor::diagnostic_secrets(&server, &env);
            let started = Utc::now();
            let probe = crate::api::mcps::probe_api_with_policy(
                state, &server, &config, &env, true, policy,
            )
            .await;
            let (outcome, detail) = judge(state, config_id, &probe, started).await;
            (outcome, detail, secrets)
        }
    };
    let detail = clean_detail(&detail, &secrets);

    let id = config_id.to_string();
    let stored_detail = detail.clone();
    state
        .db
        .with_conn(move |conn| {
            crate::db::mcps::record_token_check(conn, &id, outcome, &stored_detail, now)?;
            crate::db::mcps::get_token_check(conn, &id)
        })
        .await
        .map_err(|error| CheckError::Internal(format!("DB error: {error}")))?
        .map(|check| McpTokenCheckResult {
            config_id: config_id.to_string(),
            check,
            rate_limited: false,
        })
        .ok_or(CheckError::NotFound)
}

/// What a finished probe means for the token. The probe already went through
/// `track_outcome`, which flagged or cleared the connection; the outcome is
/// read from what it left: refused when a refusal was recorded by this probe,
/// inconclusive for any other failure (the flag is then as it was).
async fn judge(
    state: &AppState,
    config_id: &str,
    probe: &crate::models::McpProbeCheck,
    started: DateTime<Utc>,
) -> (McpTokenCheckOutcome, String) {
    if probe.ok {
        return (McpTokenCheckOutcome::Ok, probe.detail.clone());
    }
    let id = config_id.to_string();
    let flag = state
        .db
        .with_conn(move |conn| crate::db::mcps::get_auth_failure(conn, &id))
        .await
        .ok()
        .flatten()
        .filter(|failure| {
            DateTime::parse_from_rfc3339(&failure.failed_at)
                .map(|at| at.with_timezone(&Utc) >= started - chrono::Duration::seconds(1))
                .unwrap_or(false)
        });
    match flag {
        Some(failure) => {
            let status = failure
                .http_status
                .map(|status| format!(" (HTTP {status})"))
                .unwrap_or_default();
            (
                McpTokenCheckOutcome::Refused,
                format!("Token refused{status}: {}", failure.message),
            )
        }
        None => (McpTokenCheckOutcome::Inconclusive, probe.detail.clone()),
    }
}

/// Probe every connection that has a test endpoint, [`CHECK_CONCURRENCY`] at a
/// time. A connection probed within [`MIN_RECHECK`] answers from its stored check.
pub async fn check_all(state: &AppState) -> Result<Vec<McpTokenCheckResult>, CheckError> {
    let ids = state
        .db
        .with_conn(|conn| {
            let servers: HashMap<String, McpServer> = crate::db::mcps::list_servers(conn)?
                .into_iter()
                .map(|server| (server.id.clone(), server))
                .collect();
            Ok::<_, anyhow::Error>(
                crate::db::mcps::list_configs(conn)?
                    .into_iter()
                    .filter(|config| servers.get(&config.server_id).is_some_and(has_token_probe))
                    .map(|config| config.id)
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .map_err(|error| CheckError::Internal(format!("DB error: {error}")))?;

    let semaphore = Arc::new(Semaphore::new(CHECK_CONCURRENCY));
    let mut handles = Vec::with_capacity(ids.len());
    for id in ids {
        let state = state.clone();
        let semaphore = semaphore.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok()?;
            match check_connection(&state, &id).await {
                Ok(result) => Some(result),
                Err(error) => {
                    tracing::warn!(target: "kronn::token_checks", "checking {id} failed: {error}");
                    None
                }
            }
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(Some(result)) = handle.await {
            results.push(result);
        }
    }
    Ok(results)
}

/// Check every connection shortly after start, then once a day, for as long as
/// the process lives. Its own task: it never delays the boot nor a request.
/// Mirrored in `desktop/src-tauri/src/main.rs` (feature in the lib, spawn per-binary).
pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(START_DELAY).await;
        let mut interval = tokio::time::interval(DAILY);
        loop {
            interval.tick().await;
            match check_all(&state).await {
                Ok(results) => tracing::info!(
                    target: "kronn::token_checks",
                    "checked {} connection token(s)",
                    results.iter().filter(|result| !result.rate_limited).count()
                ),
                Err(error) => {
                    tracing::warn!(target: "kronn::token_checks", "daily check failed: {error}")
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::activity::{collect_activity, default_sources, ActivityContext};
    use crate::models::{
        ApiAuthKind, ApiEndpoint, ApiSpec, HostSyncMode, McpAuthFailureKind, McpConfig, McpSource,
        McpTransport,
    };
    use wiremock::matchers::{header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "ghp_kt1692_secret_never_stored";

    fn spec(base_url: &str) -> ApiSpec {
        ApiSpec {
            base_url: base_url.into(),
            auth: ApiAuthKind::Bearer {
                env_key: "SLIDES_TOKEN".into(),
            },
            endpoints: vec![ApiEndpoint {
                path: "/me".into(),
                method: "GET".into(),
                description: String::new(),
            }],
            docs_url: None,
            config_keys: vec![],
            default_headers: vec![],
            test_endpoint: None,
        }
    }

    /// Two connections: `cfg-slides` has a probe (custom API with a GET
    /// endpoint), `cfg-plain` has none (an API-less plugin).
    async fn state_with_connections(base_url: &str) -> AppState {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        let state = AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let secret = crate::core::crypto::generate_secret();
        state.config.write().await.encryption_secret = Some(secret.clone());
        let env = HashMap::from([("SLIDES_TOKEN".to_string(), TOKEN.to_string())]);
        let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
        let api = McpServer {
            id: "custom-slides".into(),
            name: "Slides".into(),
            description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: Some(spec(base_url)),
        };
        let plain = McpServer {
            id: "custom-plain".into(),
            name: "Plain".into(),
            description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: None,
        };
        let config_of = |id: &str, server: &str, label: &str| McpConfig {
            id: id.into(),
            server_id: server.into(),
            label: label.into(),
            env_keys: vec!["SLIDES_TOKEN".into()],
            env_encrypted: encrypted.clone(),
            args_override: None,
            is_global: true,
            include_general: true,
            config_hash: id.into(),
            project_ids: Vec::new(),
            host_sync: HostSyncMode::None,
        };
        let slides = config_of("cfg-slides", "custom-slides", "Slides.com");
        let plain_config = config_of("cfg-plain", "custom-plain", "Plain");
        state
            .db
            .with_conn(move |conn| -> anyhow::Result<()> {
                crate::db::mcps::upsert_server(conn, &api)?;
                crate::db::mcps::upsert_server(conn, &plain)?;
                crate::db::mcps::insert_config(conn, &slides)?;
                crate::db::mcps::insert_config(conn, &plain_config)?;
                Ok(())
            })
            .await
            .unwrap();
        state
    }

    async fn check(
        state: &AppState,
        now: DateTime<Utc>,
    ) -> Result<McpTokenCheckResult, CheckError> {
        check_connection_with(
            state,
            "cfg-slides",
            now,
            SecurityPolicy::allow_loopback_for_tests(),
        )
        .await
    }

    async fn flag(state: &AppState) -> Option<crate::models::McpAuthFailure> {
        state
            .db
            .with_conn(crate::db::mcps::list_auth_failures)
            .await
            .unwrap()
            .remove("cfg-slides")
    }

    async fn activity_rows(state: &AppState) -> Vec<crate::models::ActivityRow> {
        state
            .db
            .with_conn(|conn| {
                let ctx = ActivityContext {
                    conn,
                    audits: &[],
                    now: Utc::now(),
                };
                Ok(collect_activity(&ctx, &default_sources()).rows)
            })
            .await
            .unwrap()
    }

    async fn refuse(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).set_body_json(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_refused_probe_flags_the_connection_and_stays_out_of_the_activity_overview() {
        let server = MockServer::start().await;
        refuse(&server, serde_json::json!({"message": "Bad credentials"})).await;
        let state = state_with_connections(&server.uri()).await;
        let now = Utc::now();

        let result = check(&state, now).await.unwrap();

        assert!(!result.rate_limited);
        assert_eq!(result.check.outcome, Some(McpTokenCheckOutcome::Refused));
        assert_eq!(result.check.checked_at, Some(now.to_rfc3339()));
        assert_eq!(result.check.last_good_at, None);
        assert!(
            result.check.detail.contains("Bad credentials"),
            "{}",
            result.check.detail
        );
        // Flagged exactly like a refused call.
        let flagged = flag(&state).await.expect("the connection is flagged");
        assert_eq!(flagged.http_status, Some(401));
        // The Notifications tab lists it from the flag; the activity overview
        // carries no row for it, so it is shown once and counts for nothing.
        let rows = activity_rows(&state).await;
        assert!(rows.is_empty(), "{rows:#?}");
    }

    #[tokio::test]
    async fn a_good_probe_clears_the_flag() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 1})))
            .mount(&server)
            .await;
        let state = state_with_connections(&server.uri()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::mcps::record_auth_failure(
                    conn,
                    "cfg-slides",
                    McpAuthFailureKind::Refused,
                    Some(401),
                    "Bad credentials",
                )
            })
            .await
            .unwrap();
        assert!(flag(&state).await.is_some());
        let now = Utc::now();

        let result = check(&state, now).await.unwrap();

        assert_eq!(result.check.outcome, Some(McpTokenCheckOutcome::Ok));
        assert_eq!(result.check.last_good_at, Some(now.to_rfc3339()));
        assert!(flag(&state).await.is_none(), "a good probe unflags it");
    }

    #[tokio::test]
    async fn a_server_error_is_inconclusive_and_leaves_the_flag_as_it_was() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let state = state_with_connections(&server.uri()).await;

        let result = check(&state, Utc::now()).await.unwrap();

        assert_eq!(
            result.check.outcome,
            Some(McpTokenCheckOutcome::Inconclusive)
        );
        assert!(
            flag(&state).await.is_none(),
            "a 500 says nothing about the token"
        );
    }

    #[tokio::test]
    async fn a_connection_is_not_probed_twice_within_the_minimum_spacing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(2)
            .mount(&server)
            .await;
        let state = state_with_connections(&server.uri()).await;
        let now = Utc::now();

        let first = check(&state, now).await.unwrap();
        let second = check(&state, now + chrono::Duration::seconds(30))
            .await
            .unwrap();
        let later = check(&state, now + chrono::Duration::minutes(6))
            .await
            .unwrap();

        assert!(!first.rate_limited);
        assert!(second.rate_limited, "a click storm gets the stored answer");
        assert_eq!(second.check, first.check);
        assert!(!later.rate_limited, "past the spacing it probes again");
        // `expect(2)` is verified when the server drops: three checks, two
        // requests, so the spacing saved the one in the middle.
        drop(server);
    }

    #[tokio::test]
    async fn two_concurrent_checks_of_one_connection_send_one_provider_call() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({}))
                    .set_delay(std::time::Duration::from_millis(150)),
            )
            .mount(&server)
            .await;
        let state = state_with_connections(&server.uri()).await;
        let now = Utc::now();

        let (a, b, c) = tokio::join!(check(&state, now), check(&state, now), check(&state, now));
        // A check arriving once the first is done reads what it stored.
        let after = check(&state, now).await.unwrap();

        let results = [a.unwrap(), b.unwrap(), c.unwrap()];
        assert_eq!(
            results.iter().filter(|result| !result.rate_limited).count(),
            1,
            "{results:#?}"
        );
        assert!(after.rate_limited);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "one probe reaches the provider"
        );
    }

    #[tokio::test]
    async fn the_stored_answer_never_holds_the_token_a_provider_echoes() {
        let server = MockServer::start().await;
        refuse(
            &server,
            serde_json::json!({"message": format!("invalid token {TOKEN}")}),
        )
        .await;
        let state = state_with_connections(&server.uri()).await;

        let result = check(&state, Utc::now()).await.unwrap();

        assert!(
            !result.check.detail.contains(TOKEN),
            "{}",
            result.check.detail
        );
        let stored = state
            .db
            .with_conn(|conn| crate::db::mcps::get_token_check(conn, "cfg-slides"))
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.detail.contains(TOKEN), "{}", stored.detail);
        assert!(stored.detail.chars().count() <= DETAIL_CHARS);
    }

    #[tokio::test]
    async fn a_connection_without_a_probe_cannot_be_checked() {
        let server = MockServer::start().await;
        let state = state_with_connections(&server.uri()).await;

        let error = check_connection_with(
            &state,
            "cfg-plain",
            Utc::now(),
            SecurityPolicy::allow_loopback_for_tests(),
        )
        .await
        .unwrap_err();
        let missing = check_connection_with(
            &state,
            "nope",
            Utc::now(),
            SecurityPolicy::allow_loopback_for_tests(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, CheckError::NoProbe);
        assert_eq!(missing, CheckError::NotFound);
    }

    #[tokio::test]
    async fn a_good_real_call_dates_the_last_good_call_of_a_connection_without_a_probe() {
        let server = MockServer::start().await;
        let state = state_with_connections(&server.uri()).await;
        let at = Utc::now();
        state
            .db
            .with_conn(move |conn| crate::db::mcps::mark_good_call(conn, "cfg-plain", at))
            .await
            .unwrap();

        let listed = state
            .db
            .with_conn(|conn| crate::db::mcps::list_configs_display(conn, None))
            .await
            .unwrap();
        let plain = listed.iter().find(|c| c.id == "cfg-plain").unwrap();
        let slides = listed.iter().find(|c| c.id == "cfg-slides").unwrap();

        assert!(!plain.token_probe);
        let shown = plain
            .token_check
            .as_ref()
            .expect("a last good call is shown");
        assert_eq!(shown.last_good_at, Some(at.to_rfc3339()));
        assert_eq!(shown.checked_at, None);
        assert_eq!(shown.outcome, None);
        assert!(
            slides.token_probe,
            "a custom API with a GET endpoint has a probe"
        );
        assert!(slides.token_check.is_none(), "never checked, never called");
    }

    #[tokio::test]
    async fn a_good_call_clears_the_flag_even_when_the_last_good_date_cannot_be_written() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id": 1})))
            .mount(&server)
            .await;
        let state = state_with_connections(&server.uri()).await;
        state
            .db
            .with_conn(|conn| {
                crate::db::mcps::record_auth_failure(
                    conn,
                    "cfg-slides",
                    McpAuthFailureKind::Refused,
                    Some(401),
                    "Bad credentials",
                )?;
                // The date write now fails; the refusal flag lives elsewhere.
                conn.execute_batch("DROP TABLE mcp_token_checks")?;
                Ok(())
            })
            .await
            .unwrap();
        assert!(flag(&state).await.is_some());
        let (config, server_def, env) = {
            let secret = state.config.read().await.encryption_secret.clone().unwrap();
            state
                .db
                .with_conn(move |conn| {
                    let config = crate::db::mcps::get_config(conn, "cfg-slides")?.unwrap();
                    let server = crate::db::mcps::list_servers(conn)?
                        .into_iter()
                        .find(|server| server.id == config.server_id)
                        .unwrap();
                    let env = crate::db::mcps::decrypt_env(&config.env_encrypted, &secret).unwrap();
                    Ok::<_, anyhow::Error>((config, server, env))
                })
                .await
                .unwrap()
        };

        let probe = crate::api::mcps::probe_api_with_policy(
            &state,
            &server_def,
            &config,
            &env,
            true,
            SecurityPolicy::allow_loopback_for_tests(),
        )
        .await;

        assert!(probe.ok, "{}", probe.detail);
        assert!(
            flag(&state).await.is_none(),
            "a failed date write must not keep the refusal up"
        );
    }

    #[tokio::test]
    async fn check_all_probes_only_the_connections_that_have_a_probe() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .expect(0)
            .mount(&server)
            .await;
        // Production policy: the loopback mock is refused, so no request leaves
        // and the probe is only counted, never answered.
        let state = state_with_connections(&server.uri()).await;

        let results = check_all(&state).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].config_id, "cfg-slides");
        drop(server);
    }

    #[test]
    fn freshness_is_the_minimum_spacing() {
        let now = Utc::now();
        let at = |secs: i64| McpTokenCheck {
            checked_at: Some((now - chrono::Duration::seconds(secs)).to_rfc3339()),
            outcome: Some(McpTokenCheckOutcome::Ok),
            detail: String::new(),
            last_good_at: None,
        };
        assert!(is_fresh(&at(10), now));
        assert!(!is_fresh(&at(MIN_RECHECK.as_secs() as i64 + 1), now));
        assert!(!is_fresh(
            &McpTokenCheck {
                checked_at: None,
                ..at(0)
            },
            now
        ));
    }
}
