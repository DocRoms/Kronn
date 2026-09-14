//! Claude's SDK initialization catalogue, without an inference turn.

use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

use super::DiscoveryOutcome;
use crate::db::model_catalog::DiscoveredModel;

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelInfo {
    value: String,
    display_name: String,
    #[serde(default)]
    supported_effort_levels: Vec<String>,
}

fn invalid() -> DiscoveryOutcome {
    DiscoveryOutcome::InvalidCatalog("Claude returned an invalid model catalogue".into())
}

fn printable(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn parse_models(value: Value) -> DiscoveryOutcome {
    if !value.is_object() {
        return invalid();
    }
    let Some(models) = value.get("models") else {
        return DiscoveryOutcome::Unsupported;
    };
    let Ok(models) = serde_json::from_value::<Vec<ModelInfo>>(models.clone()) else {
        return invalid();
    };
    let mut identities = HashSet::new();
    if models.len() > 512
        || models.iter().any(|model| {
            !printable(&model.value, 256)
                || !printable(&model.display_name, 256)
                || !identities.insert(model.value.as_str())
                || model.supported_effort_levels.len() > 16
                || model
                    .supported_effort_levels
                    .iter()
                    .any(|level| !printable(level, 64))
        })
    {
        return invalid();
    }
    DiscoveryOutcome::Live(
        models
            .into_iter()
            .map(|model| DiscoveredModel {
                model_id: model.value,
                display_name: model.display_name,
                capabilities: vec!["chat".into()],
                reasoning_modes: model.supported_effort_levels,
                default_reasoning_mode: None,
            })
            .collect(),
    )
}

fn discovery_args() -> Vec<&'static str> {
    vec![
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--safe-mode",
        "--no-session-persistence",
        "--strict-mcp-config",
        "--mcp-config",
        r#"{"mcpServers":{}}"#,
        "--tools",
        "",
        "--settings",
        r#"{"disableAllHooks":true}"#,
    ]
}

fn provider_error() -> DiscoveryOutcome {
    DiscoveryOutcome::ProviderError(
        "Claude model discovery failed; check CLI authentication and support for safe-mode".into(),
    )
}

fn classify_error(value: &str) -> DiscoveryOutcome {
    let lower = value.to_ascii_lowercase();
    if [
        "unauthorized",
        "authentication",
        "not logged in",
        "login",
        "401",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        DiscoveryOutcome::AuthRequired("Authenticate the Claude CLI to discover its models".into())
    } else {
        provider_error()
    }
}

pub async fn discover() -> DiscoveryOutcome {
    let Some(location) = crate::agents::find_binary("claude") else {
        return DiscoveryOutcome::CliMissing("Claude CLI is not installed".into());
    };
    let mut command = crate::core::cmd::async_cmd(&location.path);
    #[cfg(target_os = "windows")]
    if location.via_wsl {
        command = crate::core::cmd::async_cmd("wsl.exe");
        command.args([
            "-e",
            "bash",
            "-lc",
            "exec \"$@\"",
            "kronn-claude-discovery",
            &location.path,
        ]);
    }
    command
        .args(discovery_args())
        .current_dir(std::env::temp_dir())
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DiscoveryOutcome::CliMissing("Claude CLI executable is unavailable".into());
        }
        Err(_) => return provider_error(),
    };
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        let _ = child.kill().await;
        return provider_error();
    };
    let mut reader = BufReader::new(stdout);
    let request_id = uuid::Uuid::new_v4().to_string();
    let result = tokio::time::timeout(
        Duration::from_secs(18),
        initialize_with_diagnostics(&mut stdin, &mut reader, stderr, &request_id),
    )
    .await
    .unwrap_or(DiscoveryOutcome::Timeout);
    drop(stdin);
    let _ = child.kill().await;
    result
}

async fn initialize_with_diagnostics<
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
    E: AsyncRead + Unpin,
>(
    writer: &mut W,
    reader: &mut R,
    stderr: E,
    request_id: &str,
) -> DiscoveryOutcome {
    let mut diagnostic = Vec::new();
    let outcome = tokio::select! {
        outcome = initialize(writer, reader, request_id) => outcome,
        _ = async {
            let _ = stderr.take(65536).read_to_end(&mut diagnostic).await;
            // stderr EOF is not a protocol result; stdout may still contain the catalogue.
            std::future::pending::<()>().await;
        } => unreachable!(),
    };
    if matches!(outcome, DiscoveryOutcome::ProviderError(_)) && !diagnostic.is_empty() {
        classify_error(&String::from_utf8_lossy(&diagnostic))
    } else {
        outcome
    }
}

async fn initialize<W: AsyncWrite + Unpin, R: AsyncBufRead + Unpin>(
    writer: &mut W,
    reader: &mut R,
    request_id: &str,
) -> DiscoveryOutcome {
    let frame = json!({"type":"control_request", "request_id":request_id,
        "request":{"subtype":"initialize", "hooks":null}});
    let mut bytes = serde_json::to_vec(&frame).expect("JSON control request");
    bytes.push(b'\n');
    if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
        return provider_error();
    }
    let mut bounded = reader.take(MAX_RESPONSE_BYTES);
    for _ in 0..256 {
        let mut line = Vec::new();
        match bounded.read_until(b'\n', &mut line).await {
            Ok(0) => return provider_error(),
            Err(_) => return provider_error(),
            _ => {}
        }
        let Ok(frame) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if frame.get("type").and_then(Value::as_str) == Some("control_request") {
            // No hook, permission or tool request is authorized by discovery.
            return provider_error();
        }
        if frame.get("type").and_then(Value::as_str) != Some("control_response")
            || frame
                .pointer("/response/request_id")
                .and_then(Value::as_str)
                != Some(request_id)
        {
            continue;
        }
        match frame.pointer("/response/subtype").and_then(Value::as_str) {
            Some("success") => {
                return parse_models(
                    frame
                        .pointer("/response/response")
                        .cloned()
                        .unwrap_or(Value::Null),
                )
            }
            Some("error") => {
                return classify_error(
                    frame
                        .pointer("/response/error")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                )
            }
            _ => return invalid(),
        }
    }
    invalid()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_catalog_keeps_exact_fable_identifier_and_reported_efforts() {
        let DiscoveryOutcome::Live(models) = parse_models(json!({"models":[
            {"value":"claude-fable-5-1[1m]","resolvedModel":"claude-fable-5-1","displayName":"Fable","supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"haiku","displayName":"Haiku"}
        ]})) else {
            panic!("the official initialization models must be discovered");
        };
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model_id, "claude-fable-5-1[1m]");
        assert_eq!(models[0].display_name, "Fable");
        assert_eq!(
            models[0].reasoning_modes,
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(models[0].default_reasoning_mode, None);
        assert!(models[1].reasoning_modes.is_empty());
    }

    #[test]
    fn empty_catalog_is_distinct_from_a_runtime_without_discovery() {
        assert_eq!(
            parse_models(json!({"models":[]})),
            DiscoveryOutcome::Live(vec![])
        );
        assert_eq!(
            parse_models(json!({"commands":[]})),
            DiscoveryOutcome::Unsupported
        );
    }

    #[test]
    fn malformed_catalog_fails_as_a_whole_not_as_a_partial_live_snapshot() {
        for value in [
            json!({"models":null}),
            json!({"models":{}}),
            json!({"models":[{"value":"","displayName":"bad"}]}),
            json!({"models":[{"value":"x\u{0}","displayName":"bad"}]}),
            json!({"models":[{"value":"sonnet","displayName":"Sonnet"},{"value":"sonnet","displayName":"duplicate"}]}),
            json!({"models":[{"value":"sonnet","displayName":"Sonnet","supportedEffortLevels":[1]}]}),
        ] {
            assert!(matches!(
                parse_models(value),
                DiscoveryOutcome::InvalidCatalog(_)
            ));
        }
    }

    #[tokio::test]
    async fn initialization_only_sends_one_control_frame_and_ignores_foreign_responses() {
        let responses = concat!(
            "{\"type\":\"system\",\"message\":\"ignore\"}\n",
            "{\"type\":\"control_response\",\"response\":{\"request_id\":\"other\",\"subtype\":\"success\",\"response\":{\"models\":[]}}}\n",
            "{\"type\":\"control_response\",\"response\":{\"request_id\":\"ours\",\"subtype\":\"success\",\"response\":{\"models\":[{\"value\":\"fable\",\"displayName\":\"Fable\"}]}}}\n"
        );
        let mut writer = Vec::new();
        let mut reader = responses.as_bytes();
        let DiscoveryOutcome::Live(models) = initialize(&mut writer, &mut reader, "ours").await
        else {
            panic!("catalogue");
        };
        assert_eq!(models[0].model_id, "fable");
        let frames: Vec<Value> = String::from_utf8(writer)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            frames,
            vec![
                json!({"type":"control_request","request_id":"ours","request":{"subtype":"initialize","hooks":null}})
            ]
        );
    }

    #[tokio::test]
    async fn discovery_refuses_tools_and_scrubs_auth_errors() {
        for (response, auth) in [
            (
                json!({"type":"control_request","request_id":"permission","request":{"subtype":"can_use_tool"}}),
                false,
            ),
            (
                json!({"type":"control_response","response":{"request_id":"ours","subtype":"error","error":"401 secret=do-not-expose"}}),
                true,
            ),
        ] {
            let data = format!("{response}\n");
            let mut reader = data.as_bytes();
            let result = initialize(&mut Vec::new(), &mut reader, "ours").await;
            assert_eq!(matches!(result, DiscoveryOutcome::AuthRequired(_)), auth);
            assert!(!format!("{result:?}").contains("do-not-expose"));
            assert!(!matches!(result, DiscoveryOutcome::Live(_)));
        }
    }

    #[test]
    fn discovery_flags_disable_customizations_tools_and_session_persistence() {
        let args = discovery_args();
        for arg in [
            "--safe-mode",
            "--no-session-persistence",
            "--strict-mcp-config",
        ] {
            assert!(args.contains(&arg));
        }
        assert!(args.windows(2).any(|pair| pair == ["--tools", ""]));
        assert!(!args.contains(&"--model"));
        assert!(
            !args.contains(&"--bare"),
            "bare mode would disable the user's OAuth authentication"
        );
    }

    #[tokio::test]
    async fn stderr_eof_does_not_outrace_a_valid_catalogue() {
        let (mut producer, consumer) = tokio::io::duplex(1024);
        let mut reader = BufReader::new(consumer);
        let mut writer = Vec::new();
        let read =
            initialize_with_diagnostics(&mut writer, &mut reader, tokio::io::empty(), "ours");
        let send = async {
            tokio::task::yield_now().await;
            producer.write_all(b"{\"type\":\"control_response\",\"response\":{\"request_id\":\"ours\",\"subtype\":\"success\",\"response\":{\"models\":[]}}}\n").await.unwrap();
        };
        let (outcome, _) = tokio::join!(read, send);
        assert_eq!(outcome, DiscoveryOutcome::Live(vec![]));
    }

    #[tokio::test]
    async fn oversized_or_unmatched_output_never_becomes_a_live_catalogue() {
        for data in [
            vec![b'x'; MAX_RESPONSE_BYTES as usize + 1],
            b"{\"type\":\"system\"}\n".repeat(257),
        ] {
            let mut reader = data.as_slice();
            let result = initialize(&mut Vec::new(), &mut reader, "ours").await;
            assert!(!matches!(result, DiscoveryOutcome::Live(_)));
        }
    }
}
