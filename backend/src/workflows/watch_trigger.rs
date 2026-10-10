//! `Watch` trigger (KT-1099): on its interval, the scheduler polls a source
//! through the API broker and creates a run only when the source changed.
//!
//! A poll writes `workflow_watch_state`, never `workflow_runs`. Detection:
//! the stored ETag / Last-Modified go out as a conditional request (a 304 is
//! "unchanged"); a 200 is then compared by validators or by the fingerprint
//! of the body or of a JSONPath result, per the trigger's mode.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use tokio::time::Duration;

use super::api_call_executor::{self, WatchPollRequest, WatchPollResponse};
use super::{project_scope, AdmissionMark, WorkflowEngine};
use crate::db::workflow_watch_state::{self, WatchBaseline, WatchOccurrence, WatchPollRecord};
use crate::models::*;

/// A body past this size fails the poll: a watch should target a small resource.
pub(crate) const WATCH_BODY_READ_BYTES: u64 = 4 * 1024 * 1024;
/// The triggering response handed to the run is cut at this many bytes.
pub(crate) const WATCH_VARIABLE_BYTES: usize = 64 * 1024;
const WATCH_POLL_TIMEOUT: Duration = Duration::from_secs(15);

/// What one poll found, before any run is created.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PollVerdict {
    /// Nothing stored for this source yet: record it, no run.
    Baseline(WatchBaseline),
    /// `None` keeps the stored baseline as it is.
    Unchanged(Option<WatchBaseline>),
    Changed {
        baseline: WatchBaseline,
        /// Identity of the reached state, from the signal the detection compared.
        state_key: String,
        trigger_context: serde_json::Value,
    },
    Failed {
        error: String,
        http_status: Option<u16>,
    },
}

fn sorted(
    map: &Option<std::collections::HashMap<String, String>>,
) -> std::collections::BTreeMap<&String, &String> {
    map.iter().flatten().collect()
}

/// Identity of what is actually polled, read from the source step after its
/// Quick API filled it: a human edit of that Quick API resets the baseline.
pub(crate) fn source_key(watch: &WatchTrigger, effective: &WorkflowStep) -> String {
    let identity = serde_json::json!({
        "quick_api_id": watch.quick_api_id,
        "api_plugin_slug": effective.api_plugin_slug,
        "api_config_id": effective.api_config_id,
        "api_endpoint_path": effective.api_endpoint_path,
        "api_method": effective.api_method,
        "api_query": sorted(&effective.api_query),
        "api_path_params": sorted(&effective.api_path_params),
        "api_headers": sorted(&effective.api_headers),
        "detection": watch.detection,
    });
    sha256_hex(identity.to_string().as_bytes())
}

/// The value a poll compares against the stored baseline, picked per mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Signal {
    Etag,
    LastModified,
    Fingerprint,
}

impl Signal {
    /// Validators count only in Validators mode, and only when both sides carry them.
    fn compared(detection: &WatchDetection, stored: &WatchBaseline, seen: &WatchBaseline) -> Self {
        match detection {
            WatchDetection::Validators if seen.etag.is_some() && stored.etag.is_some() => {
                Self::Etag
            }
            WatchDetection::Validators
                if seen.last_modified.is_some() && stored.last_modified.is_some() =>
            {
                Self::LastModified
            }
            _ => Self::Fingerprint,
        }
    }

    fn of(self, baseline: &WatchBaseline) -> Option<&str> {
        match self {
            Self::Etag => baseline.etag.as_deref(),
            Self::LastModified => baseline.last_modified.as_deref(),
            Self::Fingerprint => baseline.fingerprint.as_deref(),
        }
    }
}

/// Identity of the state a change reaches: only the signal its detection
/// compared, so a header the mode ignores never makes the same change look new.
fn state_key(signal: Signal, seen: &WatchBaseline) -> String {
    let name = match signal {
        Signal::Etag => "etag",
        Signal::LastModified => "last_modified",
        Signal::Fingerprint => "fingerprint",
    };
    let identity = serde_json::json!([seen.source_key, name, signal.of(seen)]);
    sha256_hex(identity.to_string().as_bytes())
}

/// The broker reads a source as an ApiCall step; a Watch never sends a body.
pub(crate) fn source_step(watch: &WatchTrigger) -> WorkflowStep {
    WorkflowStep {
        name: "watch".into(),
        step_type: StepType::ApiCall,
        quick_api_id: watch.quick_api_id.clone(),
        api_plugin_slug: watch.api_plugin_slug.clone(),
        api_config_id: watch.api_config_id.clone(),
        api_endpoint_path: watch.api_endpoint_path.clone(),
        api_query: watch.api_query.clone(),
        ..WorkflowStep::default()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Longest prefix of `text` within `max` bytes, on a character boundary.
fn bounded(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// The JSONPath result as canonical JSON, the input of its fingerprint.
fn extract(body: &str, path: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("Watch JSONPath needs a JSON body: {e}"))?;
    let path = serde_json_path::JsonPath::parse(path)
        .map_err(|e| format!("Invalid JSONPath `{path}`: {e}"))?;
    let nodes: Vec<&serde_json::Value> = path.query(&value).all();
    Ok(serde_json::to_string(&nodes).unwrap_or_default())
}

/// Decides what a poll means against the stored baseline of the same source.
pub(crate) fn evaluate(
    watch: &WatchTrigger,
    stored: Option<&WatchBaseline>,
    source_key: &str,
    response: Result<WatchPollResponse, String>,
    now: DateTime<Utc>,
) -> PollVerdict {
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return PollVerdict::Failed {
                error,
                http_status: None,
            }
        }
    };
    let Some(body) = response.body.as_deref() else {
        // A 304: the server confirmed our validators.
        return PollVerdict::Unchanged(stored.map(|stored| {
            WatchBaseline {
                etag: response.etag.clone().or_else(|| stored.etag.clone()),
                last_modified: response
                    .last_modified
                    .clone()
                    .or_else(|| stored.last_modified.clone()),
                ..stored.clone()
            }
        }));
    };
    let extracted = match &watch.detection {
        WatchDetection::JsonPath { path } => match extract(body, path) {
            Ok(value) => Some(value),
            Err(error) => {
                return PollVerdict::Failed {
                    error,
                    http_status: Some(response.status),
                }
            }
        },
        WatchDetection::Validators | WatchDetection::Body => None,
    };
    let fingerprint = sha256_hex(extracted.as_deref().unwrap_or(body).as_bytes());
    let baseline = WatchBaseline {
        source_key: source_key.to_string(),
        etag: response.etag.clone(),
        last_modified: response.last_modified.clone(),
        fingerprint: Some(fingerprint.clone()),
        seq: stored.map_or(0, |stored| stored.seq),
    };
    let Some(stored) = stored else {
        return PollVerdict::Baseline(baseline);
    };
    let signal = Signal::compared(&watch.detection, stored, &baseline);
    let changed = signal.of(&baseline) != signal.of(stored);
    if !changed {
        return PollVerdict::Unchanged(Some(baseline));
    }

    let (body_text, truncated) = bounded(body, WATCH_VARIABLE_BYTES);
    let mut context = serde_json::json!({
        "type": "watch",
        "triggered_at": now.to_rfc3339(),
        "trigger.status": response.status.to_string(),
        "trigger.body": body_text,
        "trigger.body_truncated": truncated.to_string(),
        "trigger.fingerprint": fingerprint,
    });
    if let Some(object) = context.as_object_mut() {
        if let Some(etag) = &response.etag {
            object.insert("trigger.etag".into(), etag.clone().into());
        }
        if let Some(date) = &response.last_modified {
            object.insert("trigger.last_modified".into(), date.clone().into());
        }
        if let Some(value) = &extracted {
            let (text, _) = bounded(value, WATCH_VARIABLE_BYTES);
            object.insert("trigger.extract".into(), text.into());
        }
    }
    PollVerdict::Changed {
        state_key: state_key(signal, &baseline),
        baseline,
        trigger_context: context,
    }
}

impl WorkflowEngine {
    /// Polls a Watch source once and creates the runs a change calls for.
    pub(super) async fn handle_watch_trigger(
        &self,
        wf: &Workflow,
        watch: &WatchTrigger,
    ) -> anyhow::Result<()> {
        let record = self.poll_and_admit(wf, watch).await?;
        let wf_id = wf.id.clone();
        self.db()
            .with_conn(move |conn| workflow_watch_state::record_poll(conn, &wf_id, &record))
            .await
    }

    /// The poll and the runs it admits; acknowledging it (`record_poll`) is
    /// separate, and a change re-detected before then is not run again.
    async fn poll_and_admit(
        &self,
        wf: &Workflow,
        watch: &WatchTrigger,
    ) -> anyhow::Result<WatchPollRecord> {
        let failed = |error: String| WatchPollRecord {
            at: Utc::now(),
            result: WatchPollResult::Error,
            http_status: None,
            error: Some(error),
            baseline: None,
        };
        let mut source = source_step(watch);
        if let Err(error) =
            super::quick_api_hydrate::hydrate_step_from_quick_api(&mut source, self.db(), None)
                .await
        {
            tracing::warn!(workflow_id = %wf.id, "Watch source unavailable: {error}");
            return Ok(failed(error));
        }
        let key = source_key(watch, &source);
        let wf_id = wf.id.clone();
        let stored = self
            .db()
            .with_read_conn(move |conn| workflow_watch_state::get_baseline(conn, &wf_id))
            .await?;
        let seq = stored.as_ref().map_or(0, |stored| stored.seq);
        let stored = stored.filter(|stored| stored.source_key == key);

        let response = api_call_executor::execute_watch_poll(
            &self.state,
            self.api_policy,
            WatchPollRequest {
                source: &source,
                workflow: wf,
                project_id: wf.project_id.as_deref(),
                if_none_match: stored.as_ref().and_then(|s| s.etag.as_deref()),
                if_modified_since: stored.as_ref().and_then(|s| s.last_modified.as_deref()),
                max_body_bytes: WATCH_BODY_READ_BYTES,
                timeout: WATCH_POLL_TIMEOUT,
            },
        )
        .await;
        let http_status = response.as_ref().ok().map(|r| r.status);
        let now = Utc::now();

        Ok(
            match evaluate(watch, stored.as_ref(), &key, response, now) {
                PollVerdict::Baseline(baseline) => WatchPollRecord {
                    at: now,
                    result: WatchPollResult::Baseline,
                    http_status,
                    error: None,
                    baseline: Some(baseline),
                },
                PollVerdict::Unchanged(baseline) => WatchPollRecord {
                    at: now,
                    result: WatchPollResult::Unchanged,
                    http_status,
                    error: None,
                    baseline,
                },
                PollVerdict::Failed { error, http_status } => {
                    tracing::warn!(workflow_id = %wf.id, "Watch poll failed: {error}");
                    WatchPollRecord {
                        http_status,
                        ..failed(error)
                    }
                }
                PollVerdict::Changed {
                    baseline,
                    state_key,
                    trigger_context,
                } => {
                    let complete = self
                        .spawn_watch_runs(wf, trigger_context, seq, state_key)
                        .await?;
                    // Until every served project has its run, the baseline stays:
                    // the next poll detects the same change for the rest.
                    WatchPollRecord {
                        at: now,
                        result: if complete {
                            WatchPollResult::Changed
                        } else {
                            WatchPollResult::Deferred
                        },
                        http_status,
                        error: None,
                        baseline: complete.then_some(baseline),
                    }
                }
            },
        )
    }

    /// One run per served project, as a cron occurrence, skipping the projects
    /// this change already ran for; returns whether every project has its run.
    async fn spawn_watch_runs(
        &self,
        wf: &Workflow,
        trigger_context: serde_json::Value,
        baseline_seq: i64,
        state_key: String,
    ) -> anyhow::Result<bool> {
        let scoped = wf.clone();
        let projects = self
            .db()
            .with_read_conn(move |conn| project_scope::scheduled_projects(conn, &scoped))
            .await?;
        let mut complete = !projects.is_empty();
        for project_id in projects {
            let occurrence = WatchOccurrence {
                baseline_seq,
                state_key: state_key.clone(),
                project_key: project_id.clone().unwrap_or_default(),
            };
            let (wf_id, check) = (wf.id.clone(), occurrence.clone());
            let already = self
                .db()
                .with_read_conn(move |conn| workflow_watch_state::is_admitted(conn, &wf_id, &check))
                .await?;
            if already {
                continue;
            }
            match self
                .spawn_run(
                    wf,
                    trigger_context.clone(),
                    Some(AdmissionMark::WatchOccurrence(occurrence)),
                    project_id.clone(),
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => complete = false,
                Err(error) => {
                    complete = false;
                    tracing::warn!(
                        workflow_id = %wf.id,
                        project_id = ?project_id,
                        "watch run not admitted: {error:#}"
                    );
                }
            }
        }
        Ok(complete)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::workflows::api_call_executor::SecurityPolicy;

    const TOKEN: &str = "kt1099-watch-token-never-in-output";
    const WF: &str = "wf-watch";

    fn response(status: u16, body: Option<&str>, etag: Option<&str>) -> WatchPollResponse {
        WatchPollResponse {
            status,
            etag: etag.map(String::from),
            last_modified: None,
            body: body.map(String::from),
        }
    }

    fn trigger(detection: WatchDetection) -> WatchTrigger {
        WatchTrigger {
            api_plugin_slug: Some("watched".into()),
            api_config_id: Some("cfg-watched".into()),
            api_endpoint_path: Some("/status".into()),
            interval: "*/5 * * * *".into(),
            detection,
            ..WatchTrigger::default()
        }
    }

    fn baseline_of(verdict: PollVerdict) -> WatchBaseline {
        match verdict {
            PollVerdict::Baseline(b) => b,
            other => panic!("expected a baseline, got {other:?}"),
        }
    }

    #[test]
    fn a_jsonpath_watch_ignores_changes_outside_its_path() {
        let watch = trigger(WatchDetection::JsonPath {
            path: "$.items[*].id".into(),
        });
        let key = source_key(&watch, &source_step(&watch));
        let first = r#"{"items":[{"id":1,"seen":"10:00"}]}"#;
        let stored = baseline_of(evaluate(
            &watch,
            None,
            &key,
            Ok(response(200, Some(first), Some("\"a\""))),
            Utc::now(),
        ));
        let same_ids = r#"{"items":[{"id":1,"seen":"10:05"}]}"#;
        assert!(matches!(
            evaluate(
                &watch,
                Some(&stored),
                &key,
                Ok(response(200, Some(same_ids), Some("\"b\""))),
                Utc::now()
            ),
            PollVerdict::Unchanged(Some(_))
        ));
        let new_id = r#"{"items":[{"id":1},{"id":2}]}"#;
        match evaluate(
            &watch,
            Some(&stored),
            &key,
            Ok(response(200, Some(new_id), None)),
            Utc::now(),
        ) {
            PollVerdict::Changed {
                trigger_context, ..
            } => {
                assert_eq!(trigger_context["trigger.extract"], "[1,2]");
            }
            other => panic!("expected a change, got {other:?}"),
        }
        assert!(matches!(
            evaluate(
                &watch,
                Some(&stored),
                &key,
                Ok(response(200, Some("not json"), None)),
                Utc::now()
            ),
            PollVerdict::Failed {
                http_status: Some(200),
                ..
            }
        ));
    }

    #[test]
    fn validators_decide_when_the_server_sends_them_and_the_body_otherwise() {
        let watch = trigger(WatchDetection::Validators);
        let key = source_key(&watch, &source_step(&watch));
        let stored = baseline_of(evaluate(
            &watch,
            None,
            &key,
            Ok(response(200, Some("v1"), Some("\"e1\""))),
            Utc::now(),
        ));
        // Same ETag on a 200 (the server ignored If-None-Match): unchanged.
        assert!(matches!(
            evaluate(
                &watch,
                Some(&stored),
                &key,
                Ok(response(200, Some("v1-other-bytes"), Some("\"e1\""))),
                Utc::now()
            ),
            PollVerdict::Unchanged(_)
        ));
        assert!(matches!(
            evaluate(
                &watch,
                Some(&stored),
                &key,
                Ok(response(200, Some("v1"), Some("\"e2\""))),
                Utc::now()
            ),
            PollVerdict::Changed { .. }
        ));
        // No validator in the response: the body fingerprint decides.
        assert!(matches!(
            evaluate(
                &watch,
                Some(&stored),
                &key,
                Ok(response(200, Some("v1"), None)),
                Utc::now()
            ),
            PollVerdict::Unchanged(_)
        ));
    }

    #[test]
    fn the_run_variable_is_bounded_on_a_character_boundary() {
        let watch = trigger(WatchDetection::Body);
        let key = source_key(&watch, &source_step(&watch));
        let stored = baseline_of(evaluate(
            &watch,
            None,
            &key,
            Ok(response(200, Some("a"), None)),
            Utc::now(),
        ));
        let big = "é".repeat(WATCH_VARIABLE_BYTES);
        match evaluate(
            &watch,
            Some(&stored),
            &key,
            Ok(response(200, Some(&big), None)),
            Utc::now(),
        ) {
            PollVerdict::Changed {
                trigger_context, ..
            } => {
                let body = trigger_context["trigger.body"].as_str().unwrap();
                assert!(body.len() <= WATCH_VARIABLE_BYTES);
                assert_eq!(trigger_context["trigger.body_truncated"], "true");
            }
            other => panic!("expected a change, got {other:?}"),
        }
    }

    #[test]
    fn a_changed_source_key_does_not_reuse_the_old_state() {
        let mut watch = trigger(WatchDetection::Body);
        let before = source_key(&watch, &source_step(&watch));
        watch.api_endpoint_path = Some("/other".into());
        assert_ne!(before, source_key(&watch, &source_step(&watch)));
        watch.api_endpoint_path = Some("/status".into());
        watch.interval = "0 * * * *".into();
        assert_eq!(
            before,
            source_key(&watch, &source_step(&watch)),
            "the cadence is not the source"
        );
    }

    // ─── R3: the occurrence identity follows the detection mode ──────────

    fn validated(body: &str, etag: &str, last_modified: &str) -> WatchPollResponse {
        WatchPollResponse {
            status: 200,
            etag: Some(etag.into()),
            last_modified: Some(last_modified.into()),
            body: Some(body.into()),
        }
    }

    fn changed_key(
        watch: &WatchTrigger,
        stored: &WatchBaseline,
        response: WatchPollResponse,
    ) -> String {
        let key = source_key(watch, &source_step(watch));
        match evaluate(watch, Some(stored), &key, Ok(response), Utc::now()) {
            PollVerdict::Changed { state_key, .. } => state_key,
            other => panic!("expected a change, got {other:?}"),
        }
    }

    fn first_poll(watch: &WatchTrigger, response: WatchPollResponse) -> WatchBaseline {
        let key = source_key(watch, &source_step(watch));
        baseline_of(evaluate(watch, None, &key, Ok(response), Utc::now()))
    }

    const MON: &str = "Mon, 05 Oct 2026 10:00:00 GMT";
    const TUE: &str = "Tue, 06 Oct 2026 10:00:00 GMT";
    const WED: &str = "Wed, 07 Oct 2026 10:00:00 GMT";

    #[test]
    fn body_and_jsonpath_identify_a_change_by_their_own_fingerprint_only() {
        let body = trigger(WatchDetection::Body);
        let stored = first_poll(&body, validated("v1", "\"e1\"", MON));
        let served = changed_key(&body, &stored, validated("v2", "\"e2\"", TUE));
        assert_eq!(
            served,
            changed_key(&body, &stored, validated("v2", "\"e3\"", WED)),
            "Body ignores validators"
        );
        assert_ne!(
            served,
            changed_key(&body, &stored, validated("v3", "\"e2\"", TUE)),
            "a new body is a new state"
        );

        let path = trigger(WatchDetection::JsonPath { path: "$.v".into() });
        let stored = first_poll(&path, validated(r#"{"v":1,"at":0}"#, "\"e1\"", MON));
        let served = changed_key(
            &path,
            &stored,
            validated(r#"{"v":2,"at":1}"#, "\"e2\"", TUE),
        );
        assert_eq!(
            served,
            changed_key(
                &path,
                &stored,
                validated(r#"{"v":2,"at":2}"#, "\"e3\"", WED)
            ),
            "JsonPath ignores validators and fields outside its path"
        );
        assert_ne!(
            served,
            changed_key(
                &path,
                &stored,
                validated(r#"{"v":3,"at":1}"#, "\"e2\"", TUE)
            ),
            "a new extracted value is a new state"
        );
    }

    #[test]
    fn validators_mode_identifies_a_change_by_the_validator_it_compared() {
        let watch = trigger(WatchDetection::Validators);
        let stored = first_poll(&watch, validated("v1", "\"e1\"", MON));
        let served = changed_key(&watch, &stored, validated("v2", "\"e2\"", TUE));
        assert_eq!(
            served,
            changed_key(&watch, &stored, validated("v2-other-bytes", "\"e2\"", WED)),
            "the ETag decided, not the body"
        );
        assert_ne!(
            served,
            changed_key(&watch, &stored, validated("v2", "\"e3\"", TUE)),
            "a new ETag is a new state"
        );
        let undated = WatchBaseline {
            etag: None,
            ..stored.clone()
        };
        let by_date =
            |etag: &str, date: &str| changed_key(&watch, &undated, validated("v1", etag, date));
        assert_ne!(by_date("\"e2\"", TUE), by_date("\"e2\"", WED));
    }

    #[test]
    fn the_state_key_format_is_pinned() {
        // Stored occurrence rows hold this key: a new format would rerun a
        // change still pending for some project when the backend upgrades.
        let seen = WatchBaseline {
            source_key: "src".into(),
            etag: Some("\"e\"".into()),
            last_modified: Some(MON.into()),
            fingerprint: Some("fp".into()),
            seq: 7,
        };
        let expected = sha256_hex(r#"["src","fingerprint","fp"]"#.as_bytes());
        assert_eq!(state_key(Signal::Fingerprint, &seen), expected);
        let expected = sha256_hex(r#"["src","etag","\"e\""]"#.as_bytes());
        assert_eq!(state_key(Signal::Etag, &seen), expected);
    }

    // ─── Through the scheduler and the broker ────────────────────────────

    /// An engine whose only API credential lives encrypted in the database.
    async fn engine(base_url: &str) -> WorkflowEngine {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        let secret = crate::core::crypto::generate_secret();
        state.config.write().await.encryption_secret = Some(secret.clone());
        let plugin = McpServer {
            id: "watched".into(),
            name: "Watched".into(),
            description: String::new(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: Some(ApiSpec {
                base_url: base_url.into(),
                auth: ApiAuthKind::Bearer {
                    env_key: "WATCH_TOKEN".into(),
                },
                endpoints: vec![],
                docs_url: None,
                config_keys: vec![],
                default_headers: vec![ApiDefaultHeader {
                    name: "X-Api-Version".into(),
                    value: "2026-10-01".into(),
                }],
                test_endpoint: None,
            }),
        };
        let env = HashMap::from([("WATCH_TOKEN".to_string(), TOKEN.to_string())]);
        let encrypted = crate::db::mcps::encrypt_env(&env, &secret).unwrap();
        state
            .db
            .with_conn(move |conn| -> anyhow::Result<()> {
                crate::db::mcps::upsert_server(conn, &plugin)?;
                crate::db::mcps::insert_config(
                    conn,
                    &McpConfig {
                        id: "cfg-watched".into(),
                        server_id: "watched".into(),
                        label: "Watched".into(),
                        env_keys: vec!["WATCH_TOKEN".into()],
                        env_encrypted: encrypted,
                        args_override: None,
                        is_global: true,
                        include_general: true,
                        config_hash: "kt1099".into(),
                        project_ids: Vec::new(),
                        host_sync: HostSyncMode::None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let mut engine = WorkflowEngine::new(state);
        engine.api_policy = SecurityPolicy::allow_loopback_for_tests();
        engine
    }

    async fn watch_workflow(engine: &WorkflowEngine, detection: WatchDetection) -> Workflow {
        let now = Utc::now();
        let wf = Workflow {
            id: WF.into(),
            name: WF.into(),
            project_id: None,
            project_scope: None,
            trigger: WorkflowTrigger::Watch(trigger(detection)),
            steps: vec![],
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
        let stored = wf.clone();
        engine
            .db()
            .with_conn(move |conn| crate::db::workflows::insert_workflow(conn, &stored))
            .await
            .unwrap();
        wf
    }

    async fn runs(engine: &WorkflowEngine) -> Vec<WorkflowRun> {
        engine
            .db()
            .with_conn(|conn| crate::db::workflows::list_runs(conn, WF))
            .await
            .unwrap()
    }

    async fn status(engine: &WorkflowEngine) -> WatchStatus {
        engine
            .db()
            .with_read_conn(|conn| workflow_watch_state::get_status(conn, WF))
            .await
            .unwrap()
            .expect("a poll records its status")
    }

    /// Answers only a request carrying the stored credential and the API's default header.
    fn brokered(etag_in: Option<&str>) -> wiremock::MockBuilder {
        let builder = Mock::given(method("GET"))
            .and(path("/status"))
            .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
            .and(header("X-Api-Version", "2026-10-01"));
        match etag_in {
            Some(etag) => builder.and(header("If-None-Match", etag)),
            None => builder,
        }
    }

    #[tokio::test]
    async fn unchanged_polls_create_no_run_and_a_change_creates_exactly_one() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let wf = watch_workflow(&engine, WatchDetection::Validators).await;

        brokered(None)
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"e1\"")
                    .set_body_string(r#"{"v":1}"#),
            )
            .up_to_n_times(1)
            .with_priority(3)
            .mount(&server)
            .await;
        brokered(Some("\"e1\""))
            .respond_with(ResponseTemplate::new(304).insert_header("ETag", "\"e1\""))
            .up_to_n_times(2)
            .with_priority(2)
            .mount(&server)
            .await;

        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Baseline)
        );
        engine.fire_trigger(&wf).await.unwrap();
        engine.fire_trigger(&wf).await.unwrap();
        let after_304s = status(&engine).await;
        assert_eq!(after_304s.unchanged_count, 2);
        assert_eq!(after_304s.last_http_status, Some(304));
        assert!(
            runs(&engine).await.is_empty(),
            "a poll creates no workflow_runs row"
        );

        // The source changes; its body echoes the credential.
        brokered(Some("\"e1\""))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("ETag", "\"e2\"")
                    .set_body_string(format!(r#"{{"v":2,"echo":"{TOKEN}"}}"#)),
            )
            .with_priority(1)
            .mount(&server)
            .await;
        engine.fire_trigger(&wf).await.unwrap();

        let created = runs(&engine).await;
        assert_eq!(created.len(), 1, "a change creates exactly one run");
        let context = created[0].trigger_context.clone().unwrap();
        assert_eq!(context["type"], "watch");
        assert_eq!(context["trigger.etag"], "\"e2\"");
        let body = context["trigger.body"].as_str().unwrap();
        assert!(body.contains(r#""v":2"#), "{body}");
        assert!(
            !context.to_string().contains(TOKEN),
            "credential leaked: {context}"
        );
        let changed = status(&engine).await;
        assert_eq!(changed.changed_count, 1);
        assert_eq!(changed.last_result, Some(WatchPollResult::Changed));
        assert!(changed.last_change_at.is_some());

        let received = server.received_requests().await.unwrap();
        assert_eq!(received.len(), 4, "every poll went out once, no retry");
        assert!(received.iter().all(|r| r.method.as_str() == "GET"));
    }

    #[tokio::test]
    async fn an_identical_fingerprint_creates_no_run() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let wf = watch_workflow(&engine, WatchDetection::Body).await;
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"open":3}"#))
            .mount(&server)
            .await;

        for _ in 0..3 {
            engine.fire_trigger(&wf).await.unwrap();
        }

        assert!(runs(&engine).await.is_empty());
        let watched = status(&engine).await;
        assert_eq!(watched.unchanged_count, 2, "the first poll is the baseline");
        assert_eq!(watched.changed_count, 0);
    }

    #[tokio::test]
    async fn consecutive_failures_show_as_failing_and_a_success_clears_them() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let wf = watch_workflow(&engine, WatchDetection::Body).await;
        brokered(None)
            .respond_with(ResponseTemplate::new(500).set_body_string(format!("boom {TOKEN}")))
            .up_to_n_times(workflow_watch_state::WATCH_FAILURE_THRESHOLD as u64)
            .with_priority(1)
            .mount(&server)
            .await;
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .with_priority(2)
            .mount(&server)
            .await;

        for poll in 1..=workflow_watch_state::WATCH_FAILURE_THRESHOLD {
            engine.fire_trigger(&wf).await.unwrap();
            let failing = status(&engine).await;
            assert_eq!(failing.consecutive_failures, poll);
            assert_eq!(
                failing.failing,
                poll >= workflow_watch_state::WATCH_FAILURE_THRESHOLD
            );
        }
        let failing = status(&engine).await;
        assert_eq!(failing.error_count, 3);
        assert_eq!(failing.last_result, Some(WatchPollResult::Error));
        let error = failing.last_error.unwrap();
        assert!(error.contains("HTTP 500"), "{error}");
        assert!(!error.contains(TOKEN), "credential leaked: {error}");

        engine.fire_trigger(&wf).await.unwrap();
        let recovered = status(&engine).await;
        assert_eq!(recovered.consecutive_failures, 0);
        assert!(!recovered.failing);
        assert_eq!(recovered.error_count, 3, "the history stays");
        assert!(
            runs(&engine).await.is_empty(),
            "a failed poll creates no run"
        );
    }

    #[tokio::test]
    async fn a_change_no_run_was_admitted_for_is_detected_again() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let mut wf = watch_workflow(&engine, WatchDetection::Body).await;
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string("v1"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(&server)
            .await;
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string("v2"))
            .with_priority(2)
            .mount(&server)
            .await;
        engine.fire_trigger(&wf).await.unwrap();

        // A required variable no scheduler can fill: the run's preflight refuses it.
        wf.variables = vec![PromptVariable {
            name: "ticket".into(),
            label: "ticket".into(),
            placeholder: String::new(),
            description: None,
            required: true,
            pattern: None,
            source: None,
            source_ref: None,
            allow_manual_override: false,
            control: None,
        }];
        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Deferred)
        );
        assert!(runs(&engine).await.is_empty());

        wf.variables.clear();
        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            runs(&engine).await.len(),
            1,
            "the same change runs once admitted"
        );
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Changed)
        );
    }

    // ─── R1: durable occurrences and the effective source ────────────────

    fn watch_of(wf: &Workflow) -> WatchTrigger {
        match &wf.trigger {
            WorkflowTrigger::Watch(watch) => watch.clone(),
            other => panic!("not a Watch: {other:?}"),
        }
    }

    async fn store(engine: &WorkflowEngine, wf: &Workflow) {
        let stored = wf.clone();
        engine
            .db()
            .with_conn(move |conn| crate::db::workflows::update_workflow(conn, &stored))
            .await
            .unwrap();
    }

    async fn serve_v1_then_v2(server: &MockServer) {
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string("v1"))
            .up_to_n_times(1)
            .with_priority(1)
            .mount(server)
            .await;
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string("v2"))
            .with_priority(2)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_change_admitted_but_never_acknowledged_runs_once_after_a_restart() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let wf = watch_workflow(&engine, WatchDetection::Body).await;
        serve_v1_then_v2(&server).await;
        engine.fire_trigger(&wf).await.unwrap();

        // The run is admitted, then the process dies before `record_poll`.
        let unacknowledged = engine.poll_and_admit(&wf, &watch_of(&wf)).await.unwrap();
        assert_eq!(unacknowledged.result, WatchPollResult::Changed);
        drop(unacknowledged);
        assert_eq!(runs(&engine).await.len(), 1);

        let mut restarted = WorkflowEngine::new(engine.state.clone());
        restarted.api_policy = SecurityPolicy::allow_loopback_for_tests();
        restarted.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            runs(&restarted).await.len(),
            1,
            "the same change never runs twice"
        );
        assert_eq!(
            status(&restarted).await.last_result,
            Some(WatchPollResult::Changed)
        );
        restarted.fire_trigger(&wf).await.unwrap();
        assert_eq!(runs(&restarted).await.len(), 1);
        assert_eq!(
            status(&restarted).await.last_result,
            Some(WatchPollResult::Unchanged)
        );
    }

    #[tokio::test]
    async fn a_source_returning_to_an_earlier_state_is_a_new_change_each_time() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let wf = watch_workflow(&engine, WatchDetection::Body).await;
        // A (baseline), B, A, B: three real changes, each its own run.
        for (priority, body) in [(1, "A"), (2, "B"), (3, "A"), (4, "B")] {
            brokered(None)
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .up_to_n_times(1)
                .with_priority(priority)
                .mount(&server)
                .await;
        }
        engine.fire_trigger(&wf).await.unwrap();
        for expected in 1..=3 {
            engine.fire_trigger(&wf).await.unwrap();
            assert_eq!(
                runs(&engine).await.len(),
                expected,
                "change #{expected} fires"
            );
        }
        let bodies: Vec<String> = runs(&engine)
            .await
            .iter()
            .map(|run| run.trigger_context.as_ref().unwrap()["trigger.body"].to_string())
            .collect();
        assert_eq!(bodies.iter().filter(|b| b.contains('A')).count(), 1);
        assert_eq!(bodies.iter().filter(|b| b.contains('B')).count(), 2);
        assert_eq!(status(&engine).await.changed_count, 3);
    }

    async fn insert_project(engine: &WorkflowEngine, id: &str) {
        let project: Project = serde_json::from_value(serde_json::json!({
            "id": id, "name": id, "path": format!("/nonexistent/{id}"),
            "repo_url": null, "token_override": null,
            "ai_config": {"detected": false, "configs": []},
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        engine
            .db()
            .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
            .await
            .unwrap();
    }

    fn running_run(id: &str, project: &str) -> WorkflowRun {
        serde_json::from_value(serde_json::json!({
            "id": id, "workflow_id": WF, "status": "Running", "trigger_context": null,
            "step_results": [], "tokens_used": 0, "workspace_path": null,
            "started_at": Utc::now(), "finished_at": null, "run_type": "linear",
            "batch_total": 0, "batch_completed": 0, "batch_failed": 0,
            "batch_no_response": 0, "batch_name": null, "parent_run_id": null,
            "state": {}, "produced_branches": [], "concurrency_key": null,
            "triggered_by_run_id": null, "project_id": project
        }))
        .unwrap()
    }

    fn per_project(runs: &[WorkflowRun], project: &str) -> usize {
        runs.iter()
            .filter(|run| run.project_id.as_deref() == Some(project))
            .count()
    }

    #[tokio::test]
    async fn a_project_refused_a_change_gets_it_later_without_rerunning_the_others() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        insert_project(&engine, "proj-a").await;
        insert_project(&engine, "proj-b").await;
        let mut wf = watch_workflow(&engine, WatchDetection::Body).await;
        wf.project_scope = Some(WorkflowProjectScope::Projects {
            project_ids: vec!["proj-a".into(), "proj-b".into()],
        });
        wf.concurrency_limit = Some(1);
        store(&engine, &wf).await;
        let mut busy = running_run("run-busy-b", "proj-b");
        let inserted = busy.clone();
        engine
            .db()
            .with_conn(move |conn| crate::db::workflows::insert_run(conn, &inserted))
            .await
            .unwrap();
        serve_v1_then_v2(&server).await;
        engine.fire_trigger(&wf).await.unwrap();

        engine.fire_trigger(&wf).await.unwrap();
        let first = runs(&engine).await;
        assert_eq!(per_project(&first, "proj-a"), 1);
        assert_eq!(per_project(&first, "proj-b"), 1, "only the busy run");
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Deferred)
        );

        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            per_project(&runs(&engine).await, "proj-a"),
            1,
            "B still busy, A not rerun"
        );

        busy.status = RunStatus::Success;
        busy.finished_at = Some(Utc::now());
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(&busy);
        engine
            .db()
            .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await
            .unwrap();
        engine.fire_trigger(&wf).await.unwrap();
        let after = runs(&engine).await;
        assert_eq!(
            per_project(&after, "proj-a"),
            1,
            "A never runs the change twice"
        );
        assert_eq!(
            per_project(&after, "proj-b"),
            2,
            "B gets the change once released"
        );
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Changed)
        );

        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(runs(&engine).await.len(), 3, "acknowledged: nothing more");
    }

    /// Serves each `(body, etag, last_modified)` once, the last one forever.
    async fn serve_in_order(server: &MockServer, responses: &[(&str, &str, &str)]) {
        for (index, (body, etag, date)) in responses.iter().enumerate() {
            let mock = brokered(None)
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("ETag", *etag)
                        .insert_header("Last-Modified", *date)
                        .set_body_string(*body),
                )
                .with_priority(index as u8 + 1);
            let mock = if index + 1 < responses.len() {
                mock.up_to_n_times(1)
            } else {
                mock
            };
            mock.mount(server).await;
        }
    }

    /// proj-a and proj-b; proj-b busy so the first change defers it.
    async fn two_projects_b_busy(
        engine: &WorkflowEngine,
        detection: WatchDetection,
    ) -> (Workflow, WorkflowRun) {
        insert_project(engine, "proj-a").await;
        insert_project(engine, "proj-b").await;
        let mut wf = watch_workflow(engine, detection).await;
        wf.project_scope = Some(WorkflowProjectScope::Projects {
            project_ids: vec!["proj-a".into(), "proj-b".into()],
        });
        wf.concurrency_limit = Some(1);
        store(engine, &wf).await;
        let busy = running_run("run-busy-b", "proj-b");
        let inserted = busy.clone();
        engine
            .db()
            .with_conn(move |conn| crate::db::workflows::insert_run(conn, &inserted))
            .await
            .unwrap();
        (wf, busy)
    }

    async fn release(engine: &WorkflowEngine, mut busy: WorkflowRun) {
        busy.status = RunStatus::Success;
        busy.finished_at = Some(Utc::now());
        let snap = crate::db::workflows::RunProgressSnapshot::from_run(&busy);
        engine
            .db()
            .with_conn(move |conn| crate::db::workflows::update_run_progress(conn, snap))
            .await
            .unwrap();
    }

    /// Waits until no run of `project` is still active (the limit is per workflow).
    async fn settle(engine: &WorkflowEngine, project: &str) {
        for _ in 0..200 {
            let all = runs(engine).await;
            if all
                .iter()
                .filter(|run| run.project_id.as_deref() == Some(project))
                .all(|run| run.status.is_terminal())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("{project} runs never settled");
    }

    /// Baseline, change (A admitted, B deferred), the same change under new
    /// validators, then B released: returns A's run count after each poll and
    /// B's final count.
    async fn deferred_change_under_moving_validators(
        detection: WatchDetection,
        bodies: [&str; 4],
    ) -> (Vec<usize>, usize) {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let (wf, busy) = two_projects_b_busy(&engine, detection).await;
        serve_in_order(
            &server,
            &[
                (bodies[0], "\"e1\"", MON),
                (bodies[1], "\"e2\"", TUE),
                (bodies[2], "\"e3\"", WED),
                (bodies[3], "\"e4\"", "Thu, 08 Oct 2026 10:00:00 GMT"),
            ],
        )
        .await;
        let mut a_counts = Vec::new();
        for poll in 0..4 {
            settle(&engine, "proj-a").await;
            if poll == 3 {
                release(&engine, busy.clone()).await;
            }
            engine.fire_trigger(&wf).await.unwrap();
            a_counts.push(per_project(&runs(&engine).await, "proj-a"));
        }
        (a_counts, per_project(&runs(&engine).await, "proj-b"))
    }

    #[tokio::test]
    async fn a_body_watch_serves_a_deferred_change_once_whatever_the_validators() {
        let counts =
            deferred_change_under_moving_validators(WatchDetection::Body, ["v1", "v2", "v2", "v2"])
                .await;
        assert_eq!(
            counts,
            (vec![0, 1, 1, 1], 2),
            "A and B each run the change once"
        );
    }

    #[tokio::test]
    async fn a_jsonpath_watch_serves_a_deferred_change_once_whatever_the_validators() {
        let counts = deferred_change_under_moving_validators(
            WatchDetection::JsonPath { path: "$.v".into() },
            [
                r#"{"v":1,"at":0}"#,
                r#"{"v":2,"at":1}"#,
                r#"{"v":2,"at":2}"#,
                r#"{"v":2,"at":3}"#,
            ],
        )
        .await;
        assert_eq!(
            counts,
            (vec![0, 1, 1, 1], 2),
            "A and B each run the change once"
        );
    }

    #[tokio::test]
    async fn a_jsonpath_value_moving_during_a_deferral_is_a_new_change() {
        let counts = deferred_change_under_moving_validators(
            WatchDetection::JsonPath { path: "$.v".into() },
            [r#"{"v":1}"#, r#"{"v":2}"#, r#"{"v":3}"#, r#"{"v":3}"#],
        )
        .await;
        assert_eq!(counts.0, vec![0, 1, 2, 2], "A runs 2, then 3, once each");
    }

    #[tokio::test]
    async fn a_validators_watch_takes_a_new_etag_as_a_new_change() {
        let counts = deferred_change_under_moving_validators(
            WatchDetection::Validators,
            ["v1", "v2", "v2", "v2"],
        )
        .await;
        assert_eq!(counts.0, vec![0, 1, 2, 3], "each new ETag is a new state");
    }

    #[tokio::test]
    async fn an_unacknowledged_jsonpath_change_is_not_rerun_under_a_new_etag() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let wf = watch_workflow(&engine, WatchDetection::JsonPath { path: "$.v".into() }).await;
        serve_in_order(
            &server,
            &[
                (r#"{"v":1,"at":0}"#, "\"e1\"", MON),
                (r#"{"v":2,"at":1}"#, "\"e2\"", TUE),
                (r#"{"v":2,"at":2}"#, "\"e3\"", WED),
            ],
        )
        .await;
        engine.fire_trigger(&wf).await.unwrap();
        let unacknowledged = engine.poll_and_admit(&wf, &watch_of(&wf)).await.unwrap();
        assert_eq!(unacknowledged.result, WatchPollResult::Changed);
        assert_eq!(runs(&engine).await.len(), 1);

        let mut restarted = WorkflowEngine::new(engine.state.clone());
        restarted.api_policy = SecurityPolicy::allow_loopback_for_tests();
        restarted.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            runs(&restarted).await.len(),
            1,
            "the same change never runs twice"
        );
        assert_eq!(
            status(&restarted).await.last_result,
            Some(WatchPollResult::Changed)
        );
    }

    async fn insert_quick_api(engine: &WorkflowEngine, endpoint: &str) -> QuickApi {
        let now = Utc::now();
        let api: QuickApi = serde_json::from_value(serde_json::json!({
            "id": "qa-watched", "name": "Watched status", "icon": "", "project_id": null,
            "api_plugin_slug": "watched", "api_config_id": "cfg-watched",
            "api_endpoint_path": endpoint, "variables": [],
            "created_at": now, "updated_at": now
        }))
        .unwrap();
        let stored = api.clone();
        engine
            .db()
            .with_conn(move |conn| crate::db::quick_apis::insert_quick_api(conn, &stored))
            .await
            .unwrap();
        api
    }

    async fn quick_api_watch(engine: &WorkflowEngine) -> Workflow {
        let mut wf = watch_workflow(engine, WatchDetection::Body).await;
        wf.trigger = WorkflowTrigger::Watch(WatchTrigger {
            quick_api_id: Some("qa-watched".into()),
            interval: "*/5 * * * *".into(),
            detection: WatchDetection::Body,
            ..WatchTrigger::default()
        });
        store(engine, &wf).await;
        wf
    }

    #[tokio::test]
    async fn an_agent_edit_of_a_quick_api_only_the_trigger_uses_disables_the_watch() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let mut api = insert_quick_api(&engine, "/status").await;
        let wf = quick_api_watch(&engine).await;

        api.api_endpoint_path = "/elsewhere".into();
        let edited = api.clone();
        let disabled = engine
            .db()
            .with_conn(move |conn| {
                crate::db::quick_apis::update_quick_api_invalidating(conn, &edited, Some("Claude"))
            })
            .await
            .unwrap();
        assert_eq!(disabled, 1);
        let stored = engine
            .db()
            .with_read_conn(|conn| crate::db::workflows::get_workflow(conn, WF))
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.enabled, "durably disabled");

        // The scheduler polls nothing until a human turns it back on.
        engine
            .check_triggers_since(Utc::now() - chrono::Duration::minutes(10))
            .await
            .unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());

        // Deleting the Quick API is refused while the trigger names it.
        let refused = engine
            .db()
            .with_conn(|conn| crate::db::quick_apis::delete_quick_api(conn, "qa-watched"))
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("Watch trigger"), "{refused}");
        let used = engine
            .db()
            .with_read_conn(|conn| {
                crate::db::quick_apis::count_workflow_step_usage(conn, "qa-watched")
            })
            .await
            .unwrap();
        assert_eq!(used, 1);
        drop(wf);
    }

    #[tokio::test]
    async fn a_human_edit_of_the_watched_quick_api_resets_the_baseline() {
        let server = MockServer::start().await;
        let engine = engine(&server.uri()).await;
        let mut api = insert_quick_api(&engine, "/status").await;
        let wf = quick_api_watch(&engine).await;
        brokered(None)
            .respond_with(ResponseTemplate::new(200).set_body_string("status v1"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/other"))
            .and(header("Authorization", format!("Bearer {TOKEN}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_string("other v1"))
            .mount(&server)
            .await;
        engine.fire_trigger(&wf).await.unwrap();
        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Unchanged)
        );

        api.api_endpoint_path = "/other".into();
        let edited = api.clone();
        engine
            .db()
            .with_conn(move |conn| {
                crate::db::quick_apis::update_quick_api_invalidating(conn, &edited, None)
            })
            .await
            .unwrap();
        engine.fire_trigger(&wf).await.unwrap();
        assert_eq!(
            status(&engine).await.last_result,
            Some(WatchPollResult::Baseline),
            "a new effective source starts from a new baseline, not a change"
        );
        assert!(runs(&engine).await.is_empty());
        let last = server.received_requests().await.unwrap().pop().unwrap();
        assert_eq!(last.url.path(), "/other");
    }
}
