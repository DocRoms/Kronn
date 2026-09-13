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

    let envelope: TagsEnvelope = serde_json::from_slice(&bytes).map_err(|_| {
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
            capabilities: vec!["chat".into()],
            reasoning_modes: Vec::new(),
            default_reasoning_mode: None,
        })
        .collect()
}
