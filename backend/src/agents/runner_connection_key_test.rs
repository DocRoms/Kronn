//! KT-2038 — the key an HTTP run sends belongs to the endpoint it goes to.
//! A named connection sends its own credential or nothing, never a provider
//! slot; without a connection, only the native LiteLLM / NVIDIA agents read
//! their own slot. Pure: no socket (the gate runs in a sandbox, KT-2036).
//! Every key value below is a fake.

use super::{resolve_http_connection_key, ExternalHttpRuntime};
use crate::db::external_api_connections::{
    LEGACY_LITELLM_ID, LEGACY_NVIDIA_ID, LITELLM_CREDENTIAL_SLUG, NVIDIA_CREDENTIAL_SLUG,
};
use crate::http_transport::external_http_runtime;
use crate::models::{
    AgentType, ApiKey, ExternalApiConnection, ExternalApiConnectionPreset, TokensConfig,
};

const FAKE_LITELLM: &str = "fake-litellm-slot-value";
const FAKE_NVIDIA: &str = "fake-nvidia-slot-value";
const FAKE_OWN: &str = "fake-connection-own-value";

fn key(provider: &str, value: &str) -> ApiKey {
    ApiKey {
        id: format!("id-{provider}"),
        name: format!("name-{provider}"),
        provider: provider.to_string(),
        value: value.to_string(),
        active: true,
    }
}

fn tokens(keys: Vec<ApiKey>) -> TokensConfig {
    TokensConfig {
        anthropic: None,
        openai: None,
        google: None,
        keys,
        disabled_overrides: Vec::new(),
    }
}

fn provider_slots() -> TokensConfig {
    tokens(vec![
        key(LITELLM_CREDENTIAL_SLUG, FAKE_LITELLM),
        key(NVIDIA_CREDENTIAL_SLUG, FAKE_NVIDIA),
    ])
}

fn connection(
    id: &str,
    credential_slug: &str,
    preset: ExternalApiConnectionPreset,
) -> ExternalApiConnection {
    let now = chrono::Utc::now();
    ExternalApiConnection {
        id: id.to_string(),
        display_name: format!("Connection {id}"),
        mention_alias: id.to_string(),
        endpoint: Some("https://connection.invalid/v1".to_string()),
        credential_slug: credential_slug.to_string(),
        origin_preset: preset,
        economy_model: None,
        default_model: Some("model".to_string()),
        reasoning_model: None,
        created_at: now,
        updated_at: now,
        image_model: None,
        video_model: None,
        media_endpoint: None,
        decision_model: None,
        tools_enabled: None,
    }
}

fn runtime(connection: &ExternalApiConnection, tokens: &TokensConfig) -> ExternalHttpRuntime {
    external_http_runtime(connection, tokens).expect("the connection has an endpoint")
}

fn is(resolved: Option<&str>, expected: Option<&str>) -> bool {
    // Compared, never printed: a failing assert must not echo a key value.
    resolved == expected
}

#[test]
fn connection_key_custom_on_keyless_connection_never_falls_back_to_the_litellm_slot() {
    let tokens = provider_slots();
    let keyless = connection(
        "other-keyless",
        "other-keyless",
        ExternalApiConnectionPreset::Other,
    );
    let runtime = runtime(&keyless, &tokens);
    assert!(
        is(
            resolve_http_connection_key(&AgentType::Custom, Some(&runtime), &tokens),
            None
        ),
        "a Custom agent on a keyless connection must send no key"
    );
}

#[test]
fn connection_key_keyless_connection_never_falls_back_for_any_preset_or_agent() {
    let tokens = provider_slots();
    for (preset, agent) in [
        (ExternalApiConnectionPreset::Other, AgentType::Custom),
        (ExternalApiConnectionPreset::OpenRouter, AgentType::Custom),
        // A second LiteLLM proxy / NVIDIA account the operator added, whose
        // own key was not pasted yet: the provider slot is someone else's.
        (ExternalApiConnectionPreset::LiteLlm, AgentType::LiteLlm),
        (ExternalApiConnectionPreset::Nvidia, AgentType::Nvidia),
    ] {
        let keyless = connection("named-keyless", "named-keyless-slot", preset);
        let runtime = runtime(&keyless, &tokens);
        assert!(
            is(
                resolve_http_connection_key(&agent, Some(&runtime), &tokens),
                None
            ),
            "a keyless {preset:?} connection on {agent:?} must send no key"
        );
    }
}

#[test]
fn connection_key_blank_connection_key_sends_nothing() {
    let tokens = tokens(vec![
        key(LITELLM_CREDENTIAL_SLUG, FAKE_LITELLM),
        key("blank-slot", "   "),
    ]);
    let blank = connection("blank", "blank-slot", ExternalApiConnectionPreset::Other);
    let runtime = runtime(&blank, &tokens);
    assert!(is(
        resolve_http_connection_key(&AgentType::Custom, Some(&runtime), &tokens),
        None
    ));
}

#[test]
fn connection_key_connection_with_its_own_key_sends_that_key() {
    let tokens = tokens(vec![
        key(LITELLM_CREDENTIAL_SLUG, FAKE_LITELLM),
        key("own-slot", FAKE_OWN),
    ]);
    for (preset, agent) in [
        (ExternalApiConnectionPreset::Other, AgentType::Custom),
        (ExternalApiConnectionPreset::OpenRouter, AgentType::Custom),
        (ExternalApiConnectionPreset::LiteLlm, AgentType::LiteLlm),
    ] {
        let own = connection("own", "own-slot", preset);
        let runtime = runtime(&own, &tokens);
        assert!(
            is(
                resolve_http_connection_key(&agent, Some(&runtime), &tokens),
                Some(FAKE_OWN)
            ),
            "a {preset:?} connection on {agent:?} must send its own key"
        );
    }
}

#[test]
fn connection_key_native_agents_without_connection_keep_their_own_slot() {
    let tokens = provider_slots();
    assert!(is(
        resolve_http_connection_key(&AgentType::LiteLlm, None, &tokens),
        Some(FAKE_LITELLM)
    ));
    assert!(is(
        resolve_http_connection_key(&AgentType::Nvidia, None, &tokens),
        Some(FAKE_NVIDIA)
    ));
}

#[test]
fn connection_key_other_agents_without_connection_get_no_slot() {
    let tokens = provider_slots();
    for agent in [AgentType::Custom, AgentType::Ollama] {
        assert!(
            is(resolve_http_connection_key(&agent, None, &tokens), None),
            "{agent:?} without a connection must not borrow a provider slot"
        );
    }
}

#[test]
fn connection_key_legacy_connections_keep_their_slot_key() {
    let tokens = provider_slots();
    let litellm = connection(
        LEGACY_LITELLM_ID,
        LITELLM_CREDENTIAL_SLUG,
        ExternalApiConnectionPreset::LiteLlm,
    );
    let nvidia = connection(
        LEGACY_NVIDIA_ID,
        NVIDIA_CREDENTIAL_SLUG,
        ExternalApiConnectionPreset::Nvidia,
    );
    let litellm_runtime = runtime(&litellm, &tokens);
    let nvidia_runtime = runtime(&nvidia, &tokens);
    assert!(is(
        resolve_http_connection_key(&AgentType::LiteLlm, Some(&litellm_runtime), &tokens),
        Some(FAKE_LITELLM)
    ));
    assert!(is(
        resolve_http_connection_key(&AgentType::Nvidia, Some(&nvidia_runtime), &tokens),
        Some(FAKE_NVIDIA)
    ));
}
