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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;

use crate::db::model_catalog::DiscoveredModel;

use super::DiscoveryOutcome;

#[derive(Debug, Deserialize)]
struct ModelListEntry {
    id: String,
    #[serde(default)]
    #[serde(rename = "supportedReasoningEfforts")]
    supported_reasoning_efforts: Vec<String>,
    #[serde(default)]
    #[serde(rename = "defaultReasoningEffort")]
    default_reasoning_effort: Option<String>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Debug, Deserialize)]
struct ModelListResult {
    data: Vec<ModelListEntry>,
}

pub async fn discover() -> DiscoveryOutcome {
    let mut command = crate::core::cmd::async_cmd("codex");
    command
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
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

async fn run_handshake_and_list(
    stdin: &mut ChildStdin,
    reader: &mut BufReader<tokio::process::ChildStdout>,
) -> DiscoveryOutcome {
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

    let list = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "model/list",
        "params": {"includeHidden": false},
    });
    if let Err(error) = write_frame(stdin, &list).await {
        return DiscoveryOutcome::ProviderError(error);
    }
    let result = match read_response(reader, 1).await {
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
    DiscoveryOutcome::Live(
        parsed
            .data
            .into_iter()
            .filter(|entry| !entry.hidden)
            .map(|entry| DiscoveredModel {
                display_name: entry.id.clone(),
                model_id: entry.id,
                capabilities: Vec::new(),
                reasoning_modes: entry.supported_reasoning_efforts,
                default_reasoning_mode: entry.default_reasoning_effort,
            })
            .collect(),
    )
}

async fn write_frame(stdin: &mut ChildStdin, frame: &Value) -> Result<(), String> {
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
async fn read_response(
    reader: &mut BufReader<tokio::process::ChildStdout>,
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
            parsed.data[0].supported_reasoning_efforts,
            vec!["low", "medium", "high"]
        );
        assert_eq!(
            parsed.data[0].default_reasoning_effort.as_deref(),
            Some("medium")
        );
        assert!(parsed.data[1].hidden);
    }
}
