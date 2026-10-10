//! Live model + reasoning-level discovery for Codex (KT-531 DoD #1).
//!
//! Codex has no verified ACP adapter (`docs/design/adr-003-acp-control-plane.md`),
//! but it ships its own official machine-readable interface: `codex app-server`
//! runs a JSON-RPC 2.0 control protocol over stdio, and its `model/list`
//! method returns each model's id plus `supportedReasoningEfforts` /
//! `defaultReasoningEffort` — the "real reasoning levels" KT-531 requires.
//! This is the "interface machine-readable officielle du runtime" the
//! contract falls back to when a runtime is not ACP-native.
//!
//! The handshake and `model/list` request/response shapes are minimal by
//! design (only the fields this adapter actually reads), so an unexpected
//! but well-formed superset response never fails discovery.

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use crate::db::model_catalog::DiscoveredModel;

use super::{DiscoveryOutcome, ServedListing};

#[derive(Debug, Deserialize)]
struct ModelListEntry {
    id: String,
    /// Present since Codex started sending a human name of its own
    /// ("GPT-5.6-Sol" for `gpt-5.6-sol`). Absent on older builds, where the
    /// id stays the only thing to show.
    #[serde(default)]
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(default)]
    #[serde(rename = "supportedReasoningEfforts")]
    supported_reasoning_efforts: Vec<ReasoningEffort>,
    #[serde(default)]
    #[serde(rename = "defaultReasoningEffort")]
    default_reasoning_effort: Option<String>,
    #[serde(default)]
    hidden: bool,
}

/// Codex used to list its reasoning efforts as bare strings and now sends
/// objects carrying a description alongside the name. Reading only the new
/// shape would break every older Codex; reading only the old one is what
/// turned the whole catalogue into `invalid_catalog`, and a catalogue in
/// error blocks every turn for that agent.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ReasoningEffort {
    Named(String),
    Described {
        #[serde(rename = "reasoningEffort")]
        reasoning_effort: String,
    },
}

impl ReasoningEffort {
    fn name(self) -> String {
        match self {
            ReasoningEffort::Named(name) => name,
            ReasoningEffort::Described { reasoning_effort } => reasoning_effort,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ModelListResult {
    data: Vec<ModelListEntry>,
    /// Opaque continuation token; absent or null on the last page.
    #[serde(default)]
    #[serde(rename = "nextCursor")]
    next_cursor: Option<Value>,
}

/// Bound on `model/list` pages; a longer listing is reported partial.
const MAX_MODEL_LIST_PAGES: u64 = 20;

/// `codex app-server`, run outside any repository (design note §9).
fn discovery_command() -> tokio::process::Command {
    let mut command =
        crate::core::cmd::discovery_cmd("codex", crate::core::child_env::AgentFamily::Codex);
    command
        .arg("app-server")
        .current_dir(std::env::temp_dir())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    command
}

pub async fn discover() -> DiscoveryOutcome {
    let mut command = discovery_command();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return if error.kind() == std::io::ErrorKind::NotFound {
                DiscoveryOutcome::CliMissing(error.to_string())
            } else {
                DiscoveryOutcome::ProviderError(format!("spawn codex app-server: {error}"))
            };
        }
    };
    let Some(stdin) = child.stdin.take() else {
        let _ = child.start_kill();
        return DiscoveryOutcome::ProviderError("codex app-server stdin unavailable".into());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.start_kill();
        return DiscoveryOutcome::ProviderError("codex app-server stdout unavailable".into());
    };
    let mut stdin = stdin;
    let mut reader = BufReader::new(stdout);

    let outcome = run_handshake_and_list(&mut stdin, &mut reader).await;
    let _ = child.start_kill();
    outcome
}

async fn run_handshake_and_list<W, R>(stdin: &mut W, reader: &mut R) -> DiscoveryOutcome
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "clientInfo": {"name": "kronn", "title": "Kronn", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {},
        }
    });
    if let Err(error) = write_frame(stdin, &initialize).await {
        return DiscoveryOutcome::ProviderError(error);
    }
    if let Err(outcome) = read_response(reader, 0).await {
        return outcome;
    }

    let initialized = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
    if let Err(error) = write_frame(stdin, &initialized).await {
        return DiscoveryOutcome::ProviderError(error);
    }

    // Hidden models are still served: only a listing with them, read to its
    // last page, can prove that a model is not served.
    let mut entries = Vec::new();
    let mut cursor: Option<Value> = None;
    let mut complete = false;
    for page in 1..=MAX_MODEL_LIST_PAGES {
        let mut params = json!({"includeHidden": true});
        if let Some(cursor) = &cursor {
            params["cursor"] = cursor.clone();
        }
        let list = json!({"jsonrpc": "2.0", "id": page, "method": "model/list", "params": params});
        if let Err(error) = write_frame(stdin, &list).await {
            return DiscoveryOutcome::ProviderError(error);
        }
        let result = match read_response(reader, page).await {
            Ok(result) => result,
            Err(outcome) => return outcome,
        };
        let parsed: ModelListResult = match serde_json::from_value(result) {
            Ok(parsed) => parsed,
            Err(error) => {
                return DiscoveryOutcome::InvalidCatalog(format!(
                    "codex app-server model/list response did not match the expected shape: {error}"
                ))
            }
        };
        entries.extend(parsed.data);
        match parsed.next_cursor.filter(|next| !next.is_null()) {
            None => {
                complete = true;
                break;
            }
            Some(next) if cursor.as_ref() == Some(&next) => break,
            Some(next) => cursor = Some(next),
        }
    }
    let served = if complete {
        ServedListing::Complete(entries.iter().map(|entry| entry.id.clone()).collect())
    } else {
        ServedListing::Partial
    };
    let models = entries
        .into_iter()
        .filter(|entry| !entry.hidden)
        .map(|entry| DiscoveredModel {
            display_name: entry.display_name.unwrap_or_else(|| entry.id.clone()),
            model_id: entry.id,
            resolved_model: None,
            description: None,
            capabilities: Vec::new(),
            reasoning_modes: entry
                .supported_reasoning_efforts
                .into_iter()
                .map(ReasoningEffort::name)
                .collect(),
            default_reasoning_mode: entry.default_reasoning_effort,
        })
        .collect();
    DiscoveryOutcome::Listing { models, served }
}

async fn write_frame<W: AsyncWrite + Unpin>(stdin: &mut W, frame: &Value) -> Result<(), String> {
    let encoded = serde_json::to_string(frame)
        .map_err(|error| format!("encode codex app-server request: {error}"))?;
    stdin
        .write_all(encoded.as_bytes())
        .await
        .map_err(|error| format!("write codex app-server request: {error}"))?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|error| format!("terminate codex app-server request: {error}"))?;
    stdin
        .flush()
        .await
        .map_err(|error| format!("flush codex app-server request: {error}"))
}

/// Read lines until one carries the response matching `expected_id` (skipping
/// unrelated notifications), then return its `result` object.
async fn read_response<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    expected_id: u64,
) -> Result<Value, DiscoveryOutcome> {
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await.map_err(|error| {
            DiscoveryOutcome::ProviderError(format!("read codex app-server response: {error}"))
        })?;
        if read == 0 {
            return Err(DiscoveryOutcome::ProviderError(
                "codex app-server closed stdout before responding".into(),
            ));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(trimmed) {
            Ok(message) => message,
            Err(_) => continue, // malformed/unexpected frame; keep waiting for the real response
        };
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            continue; // notification, not the response we're waiting for
        };
        if id != expected_id {
            continue;
        }
        if let Some(error) = message.get("error") {
            let detail = error.to_string();
            let lower = detail.to_lowercase();
            return Err(
                if lower.contains("auth")
                    || lower.contains("login")
                    || lower.contains("unauthorized")
                {
                    DiscoveryOutcome::AuthRequired(detail)
                } else {
                    DiscoveryOutcome::ProviderError(detail)
                },
            );
        }
        return Ok(message.get("result").cloned().unwrap_or(Value::Null));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_runs_outside_any_repository() {
        let command = discovery_command();
        assert_eq!(
            command.as_std().get_current_dir(),
            Some(std::env::temp_dir().as_path())
        );
    }

    #[test]
    fn model_list_result_parses_documented_shape() {
        let raw = json!({
            "data": [
                {
                    "id": "gpt-5.1-codex",
                    "supportedReasoningEfforts": ["low", "medium", "high"],
                    "defaultReasoningEffort": "medium",
                    "inputModalities": ["text", "image"],
                    "hidden": false
                },
                {
                    "id": "hidden-model",
                    "hidden": true
                }
            ]
        });
        let parsed: ModelListResult = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(
            efforts(&parsed.data[0]),
            vec!["low", "medium", "high"],
            "the older bare-string shape must keep parsing"
        );
        assert_eq!(
            parsed.data[0].default_reasoning_effort.as_deref(),
            Some("medium")
        );
        assert!(parsed.data[1].hidden);
    }

    fn efforts(entry: &ModelListEntry) -> Vec<String> {
        entry
            .supported_reasoning_efforts
            .iter()
            .map(|effort| match effort {
                ReasoningEffort::Named(name) => name.clone(),
                ReasoningEffort::Described { reasoning_effort } => reasoning_effort.clone(),
            })
            .collect()
    }

    /// Captured verbatim from `codex app-server` on 2026-09-03, after the
    /// catalogue on this machine had been sitting in `invalid_catalog` — which
    /// blocks EVERY turn for that agent, not just discovery.
    #[test]
    fn model_list_result_parses_the_described_effort_shape_codex_sends_now() {
        let raw = json!({
            "data": [
                {
                    "id": "gpt-5.6-sol",
                    "model": "gpt-5.6-sol",
                    "displayName": "GPT-5.6-Sol",
                    "description": "Reliable agentic workhorse for everyday tasks.",
                    "hidden": false,
                    "supportedReasoningEfforts": [
                        {"reasoningEffort": "low", "description": "Fast responses"},
                        {"reasoningEffort": "high", "description": "Greater depth"}
                    ],
                    "defaultReasoningEffort": "low"
                }
            ]
        });
        let parsed: ModelListResult = serde_json::from_value(raw).unwrap();
        assert_eq!(efforts(&parsed.data[0]), vec!["low", "high"]);
        // The name Codex gives itself beats the id, when it sends one.
        assert_eq!(parsed.data[0].display_name.as_deref(), Some("GPT-5.6-Sol"));
    }

    #[test]
    fn an_entry_without_a_display_name_still_shows_its_id() {
        let raw = json!({"data": [{"id": "gpt-5.1-codex"}]});
        let parsed: ModelListResult = serde_json::from_value(raw).unwrap();
        assert!(parsed.data[0].display_name.is_none());
        assert!(efforts(&parsed.data[0]).is_empty());
    }

    /// An in-process `codex app-server`: answers `initialize`, then serves
    /// `model/list` from `pages`, each `(entries, nextCursor)`.
    async fn list_against(pages: Vec<(Value, Value)>) -> (DiscoveryOutcome, Vec<Value>) {
        let (client, server) = tokio::io::duplex(1 << 16);
        let (client_read, mut client_write) = tokio::io::split(client);
        let (server_read, mut server_write) = tokio::io::split(server);
        let fake = tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            let mut requests = Vec::new();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = request.get("id").cloned() else {
                    continue;
                };
                let result = if request["method"] == "model/list" {
                    requests.push(request["params"].clone());
                    let (data, next) = pages
                        .get(requests.len() - 1)
                        .cloned()
                        .unwrap_or((json!([]), Value::Null));
                    json!({"data": data, "nextCursor": next})
                } else {
                    json!({})
                };
                let frame = json!({"jsonrpc": "2.0", "id": id, "result": result});
                server_write
                    .write_all(format!("{frame}\n").as_bytes())
                    .await
                    .unwrap();
            }
            requests
        });
        let mut reader = BufReader::new(client_read);
        let outcome = run_handshake_and_list(&mut client_write, &mut reader).await;
        drop(client_write);
        drop(reader);
        (outcome, fake.await.unwrap())
    }

    fn served(outcome: &DiscoveryOutcome) -> (Vec<String>, &ServedListing) {
        let DiscoveryOutcome::Listing { models, served } = outcome else {
            panic!("a listing is expected: {outcome:?}");
        };
        (models.iter().map(|m| m.model_id.clone()).collect(), served)
    }

    #[tokio::test]
    async fn a_model_on_page_two_is_listed() {
        let (outcome, requests) = list_against(vec![
            (json!([{"id": "gpt-6-astra"}]), json!("2")),
            (json!([{"id": "gpt-5.6-sol"}]), Value::Null),
        ])
        .await;
        let (visible, served) = served(&outcome);
        assert_eq!(visible, vec!["gpt-6-astra", "gpt-5.6-sol"]);
        assert_eq!(
            served,
            &ServedListing::Complete(vec!["gpt-6-astra".into(), "gpt-5.6-sol".into()])
        );
        assert_eq!(requests[0], json!({"includeHidden": true}));
        assert_eq!(requests[1], json!({"includeHidden": true, "cursor": "2"}));
    }

    #[tokio::test]
    async fn a_hidden_model_is_served_but_not_offered() {
        let (outcome, _) = list_against(vec![(
            json!([{"id": "gpt-6-astra"}, {"id": "gpt-5.5", "hidden": true}]),
            Value::Null,
        )])
        .await;
        let (visible, served) = served(&outcome);
        assert_eq!(visible, vec!["gpt-6-astra"]);
        assert_eq!(
            served,
            &ServedListing::Complete(vec!["gpt-6-astra".into(), "gpt-5.5".into()])
        );
    }

    #[tokio::test]
    async fn a_listing_cut_short_is_partial() {
        // Past the page cap.
        let pages = (0..MAX_MODEL_LIST_PAGES + 1)
            .map(|page| (json!([{"id": format!("m{page}")}]), json!(page + 1)))
            .collect();
        let (outcome, requests) = list_against(pages).await;
        assert_eq!(served(&outcome).1, &ServedListing::Partial);
        assert_eq!(requests.len() as u64, MAX_MODEL_LIST_PAGES);

        // A cursor that does not advance.
        let (outcome, _) = list_against(vec![
            (json!([{"id": "a"}]), json!("x")),
            (json!([{"id": "b"}]), json!("x")),
        ])
        .await;
        assert_eq!(served(&outcome).1, &ServedListing::Partial);
    }

    #[tokio::test]
    async fn a_failed_page_is_a_failure_not_a_listing() {
        let (outcome, _) = list_against(vec![
            (json!([{"id": "a"}]), json!("2")),
            (json!("not a list"), Value::Null),
        ])
        .await;
        assert!(
            matches!(outcome, DiscoveryOutcome::InvalidCatalog(_)),
            "{outcome:?}"
        );
    }
}
