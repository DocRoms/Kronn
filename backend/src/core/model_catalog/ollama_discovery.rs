//! Strict, bounded discovery of the locally configured Ollama catalogue.

use std::collections::HashSet;
use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize;

use crate::db::model_catalog::DiscoveredModel;

use super::DiscoveryOutcome;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TAGS_RESPONSE_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq)]
pub struct OllamaTag {
    pub name: String,
    pub size: u64,
    pub modified_at: String,
    /// What the server holds for this tag, as `/api/tags` reports it (empty
    /// when an older server does not). The one fact KT-930's "update
    /// available" is compared against.
    pub digest: String,
}

#[derive(Deserialize)]
struct TagsEnvelope {
    models: Vec<TagWire>,
}

#[derive(Deserialize)]
struct TagWire {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_at: String,
    #[serde(default)]
    digest: String,
}

fn valid_model_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value == value.trim()
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

pub async fn discover(base_url: &str) -> Result<Vec<OllamaTag>, DiscoveryOutcome> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| DiscoveryOutcome::ProviderError("failed to create HTTP client".into()))?;
    let response = client
        .get(format!("{}/api/tags", base_url.trim_end_matches('/')))
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                DiscoveryOutcome::Timeout
            } else {
                DiscoveryOutcome::ProviderError("Ollama catalogue request failed".into())
            }
        })?;
    if !response.status().is_success() {
        return Err(DiscoveryOutcome::ProviderError(format!(
            "Ollama catalogue returned HTTP {}",
            response.status().as_u16()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TAGS_RESPONSE_BYTES as u64)
    {
        return Err(DiscoveryOutcome::InvalidCatalog(
            "Ollama catalogue response exceeded the size limit".into(),
        ));
    }

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            if error.is_timeout() {
                DiscoveryOutcome::Timeout
            } else {
                DiscoveryOutcome::ProviderError(
                    "Ollama catalogue response could not be read".into(),
                )
            }
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_TAGS_RESPONSE_BYTES {
            return Err(DiscoveryOutcome::InvalidCatalog(
                "Ollama catalogue response exceeded the size limit".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }

    parse_tags(&bytes)
}

/// The `/api/tags` body as a list of distinct, well-named tags. Separate from
/// the request so what is read off the wire (the digest included) is testable
/// without a server.
fn parse_tags(bytes: &[u8]) -> Result<Vec<OllamaTag>, DiscoveryOutcome> {
    let envelope: TagsEnvelope = serde_json::from_slice(bytes).map_err(|_| {
        DiscoveryOutcome::InvalidCatalog("Ollama catalogue schema is invalid".into())
    })?;
    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    for tag in envelope.models {
        if !valid_model_id(&tag.name) {
            return Err(DiscoveryOutcome::InvalidCatalog(
                "Ollama catalogue contains an invalid model identifier".into(),
            ));
        }
        if seen.insert(tag.name.clone()) {
            tags.push(OllamaTag {
                name: tag.name,
                size: tag.size,
                modified_at: tag.modified_at,
                digest: tag.digest,
            });
        }
    }
    Ok(tags)
}

pub fn discovered_models(tags: &[OllamaTag]) -> Vec<DiscoveredModel> {
    tags.iter()
        .map(|tag| DiscoveredModel {
            model_id: tag.name.clone(),
            display_name: tag.name.clone(),
            resolved_model: None,
            description: None,
            capabilities: vec!["chat".into()],
            reasoning_modes: Vec::new(),
            default_reasoning_mode: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KT-930 — the local half of "is there an update" is the digest
    /// `/api/tags` reports for each tag; it must come through as written, and
    /// an older server that omits it must still list the model.
    #[test]
    fn each_tag_carries_the_digest_the_server_reports() {
        let body = br#"{"models":[
            {"name":"qwen3:8b","size":5200000000,"modified_at":"2026-09-01T10:00:00Z",
             "digest":"500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41"},
            {"name":"old-server:latest","size":1,"modified_at":"2026-01-01T00:00:00Z"}
        ]}"#;
        let tags = parse_tags(body).unwrap_or_else(|_| panic!("a valid catalogue"));
        assert_eq!(tags.len(), 2);
        assert_eq!(
            tags[0].digest,
            "500a1f067a9f782620b40bee6f7b0c89e17ae61f686b92c24933e4ca4b2b8b41"
        );
        assert_eq!(tags[1].digest, "", "absent, not invented");
        assert_eq!(tags[1].name, "old-server:latest");
    }

    #[test]
    fn a_malformed_or_badly_named_catalogue_is_refused() {
        assert!(parse_tags(b"not json").is_err());
        assert!(parse_tags(br#"{"models":[{"name":"has space:1"}]}"#).is_err());
        let twice = parse_tags(br#"{"models":[{"name":"a:1"},{"name":"a:1"}]}"#)
            .unwrap_or_else(|_| panic!("duplicates collapse"));
        assert_eq!(twice.len(), 1);
    }
}
