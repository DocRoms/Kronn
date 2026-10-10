//! `/api/assistant-conversations` — the kept conversations of the
//! configuration assistants (KT-1111). Deletion goes through the ordinary
//! `DELETE /api/discussions/{id}`: the link cascades with the discussion.

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use ts_rs::TS;

use crate::core::secret_scrub::SecretSet;
use crate::db::assistant_conversations::{
    self as store, AssistantContext, AssistantConversation, AssistantKind, LinkPatch, ListFilter,
    NewLink, TransientSecrets,
};
use crate::models::ApiResponse;
use crate::AppState;

const MAX_ID_CHARS: usize = 256;
const MAX_LABEL_CHARS: usize = 200;
const MAX_SIGNATURE_CHARS: usize = 128;
const MAX_TRANSIENT_SECRETS: usize = 64;

#[derive(Debug, Default, Deserialize, TS)]
#[ts(export)]
pub struct UpdateAssistantConversationRequest {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub target_id: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub target_step: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub target_label: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub last_proposal_signature: Option<String>,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub last_applied_signature: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListAssistantConversationsQuery {
    #[serde(default)]
    pub kind: Option<AssistantKind>,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub unattached: bool,
    #[serde(default)]
    pub include_unattached: bool,
    /// Comma-separated discussion ids still owed to the target.
    #[serde(default)]
    pub pending_ids: Option<String>,
    #[serde(default)]
    pub unattached_step: Option<String>,
    #[serde(default)]
    pub target_step: Option<String>,
    #[serde(default)]
    pub plugin_id: Option<String>,
}

fn bounded(field: &str, value: Option<&str>, max: usize) -> Result<(), String> {
    match value {
        Some(text) if text.chars().count() > max => {
            Err(format!("{field} is longer than {max} characters"))
        }
        Some(text) if text.chars().any(char::is_control) => {
            Err(format!("{field} contains control characters"))
        }
        _ => Ok(()),
    }
}

fn blank_to_none(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

pub fn validate_context(context: &AssistantContext) -> Result<(), String> {
    bounded("target_id", context.target_id.as_deref(), MAX_ID_CHARS)?;
    bounded("target_step", context.target_step.as_deref(), MAX_ID_CHARS)?;
    bounded("plugin_id", context.plugin_id.as_deref(), MAX_ID_CHARS)?;
    bounded(
        "target_label",
        context.target_label.as_deref(),
        MAX_LABEL_CHARS,
    )?;
    if context.secrets.values().len() > MAX_TRANSIENT_SECRETS {
        return Err(format!(
            "at most {MAX_TRANSIENT_SECRETS} secret values can be masked"
        ));
    }
    Ok(())
}

pub fn validate_update(request: &UpdateAssistantConversationRequest) -> Result<(), String> {
    bounded("target_id", request.target_id.as_deref(), MAX_ID_CHARS)?;
    bounded("target_step", request.target_step.as_deref(), MAX_ID_CHARS)?;
    bounded(
        "target_label",
        request.target_label.as_deref(),
        MAX_LABEL_CHARS,
    )?;
    bounded(
        "last_proposal_signature",
        request.last_proposal_signature.as_deref(),
        MAX_SIGNATURE_CHARS,
    )?;
    bounded(
        "last_applied_signature",
        request.last_applied_signature.as_deref(),
        MAX_SIGNATURE_CHARS,
    )
}

/// `GET /api/assistant-conversations`
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListAssistantConversationsQuery>,
) -> Json<ApiResponse<Vec<AssistantConversation>>> {
    let filter = ListFilter {
        kind: query.kind,
        target_id: blank_to_none(query.target_id),
        unattached: query.unattached,
        include_unattached: query.include_unattached,
        pending_ids: query
            .pending_ids
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= MAX_ID_CHARS)
            .take(MAX_TRANSIENT_SECRETS)
            .map(str::to_string)
            .collect(),
        unattached_step: blank_to_none(query.unattached_step),
        target_step: blank_to_none(query.target_step),
        plugin_id: blank_to_none(query.plugin_id),
    };
    match state
        .db
        .with_read_conn(move |conn| store::list(conn, &filter))
        .await
    {
        Ok(rows) => Json(ApiResponse::ok(rows)),
        Err(error) => Json(ApiResponse::err(format!(
            "Failed to list assistant conversations: {error}"
        ))),
    }
}

/// `PATCH /api/assistant-conversations/{discussion_id}`
pub async fn update(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
    Json(request): Json<UpdateAssistantConversationRequest>,
) -> Json<ApiResponse<AssistantConversation>> {
    if let Err(message) = validate_update(&request) {
        return Json(ApiResponse::err(message));
    }
    let patch = LinkPatch {
        target_id: blank_to_none(request.target_id),
        target_step: blank_to_none(request.target_step),
        target_label: request.target_label.map(|label| label.trim().to_string()),
        last_proposal_signature: blank_to_none(request.last_proposal_signature),
        last_applied_signature: blank_to_none(request.last_applied_signature),
    };
    let result = state
        .db
        .with_conn(move |conn| {
            if !store::update(conn, &discussion_id, &patch)? {
                return Ok(None);
            }
            store::get(conn, &discussion_id)
        })
        .await;
    match result {
        Ok(Some(row)) => Json(ApiResponse::ok(row)),
        Ok(None) => Json(ApiResponse::err("Assistant conversation not found")),
        Err(error) => Json(ApiResponse::err(format!(
            "Failed to update the assistant conversation: {error}"
        ))),
    }
}

/// Masks the client's transient values and the plugin's stored credentials.
async fn secret_set(
    state: &AppState,
    plugin: Option<String>,
    transient: &TransientSecrets,
) -> anyhow::Result<SecretSet> {
    let mut set = SecretSet::new();
    for value in transient.values() {
        set.add(value);
    }
    let key = state.config.read().await.encryption_secret.clone();
    // The writers mask with the current key, including after a rekey.
    state.db.assistant_guard().set_key(key.clone());
    if let (Some(server_id), Some(key)) = (plugin, key) {
        let stored = state
            .db
            .with_read_conn(move |conn| store::stored_plugin_secrets(conn, &server_id, &key))
            .await?;
        set.extend(&stored);
    }
    Ok(set)
}

/// What an assistant conversation may store or send to a model: known values
/// first, then the common token shapes.
pub(crate) fn protect(set: &SecretSet, text: &str) -> String {
    crate::core::redact::redact_secrets(&set.scrub(text))
}

/// Checks a creation's assistant context and masks the title and the first
/// message with it. Returns the link to insert in the creation transaction.
pub(crate) async fn prepare_creation(
    state: &AppState,
    context: AssistantContext,
    title: &mut String,
    initial_prompt: &mut String,
) -> Result<NewLink, String> {
    validate_context(&context)?;
    let target_id = blank_to_none(context.target_id);
    let plugin_id = blank_to_none(context.plugin_id);
    let set = secret_set(
        state,
        store::plugin_of(context.kind, target_id.as_deref(), plugin_id.as_deref()),
        &context.secrets,
    )
    .await
    .map_err(|error| format!("Assistant secret check failed: {error}"))?;
    *title = protect(&set, title);
    *initial_prompt = protect(&set, initial_prompt);
    Ok(NewLink {
        discussion_id: String::new(),
        kind: Some(context.kind),
        target_id,
        target_step: blank_to_none(context.target_step).map(|step| protect(&set, &step)),
        plugin_id,
        target_label: protect(&set, context.target_label.as_deref().unwrap_or("").trim()),
    })
}

/// Masks a message before it is stored or dispatched: in an assistant
/// conversation always, elsewhere only when the client named secrets.
pub(crate) async fn protect_message(
    state: &AppState,
    discussion_id: &str,
    content: &mut String,
    transient: &TransientSecrets,
) -> anyhow::Result<()> {
    let lookup = discussion_id.to_string();
    let link = state
        .db
        .with_read_conn(move |conn| store::get(conn, &lookup))
        .await?;
    if link.is_none() && transient.is_empty() {
        return Ok(());
    }
    let plugin = link.as_ref().and_then(|link| {
        store::plugin_of(
            link.kind,
            link.target_id.as_deref(),
            link.plugin_id.as_deref(),
        )
    });
    let set = secret_set(state, plugin, transient).await?;
    *content = if link.is_some() {
        protect(&set, content)
    } else {
        set.scrub(content)
    };
    Ok(())
}

/// The turns a message starts, one per dispatch, holding its transient
/// values in memory until each dispatch ends. Dropped without `keep`, it
/// forgets them at once (the message was refused or deduplicated).
pub(crate) struct TurnRegistration {
    guard: std::sync::Arc<store::AssistantGuard>,
    ids: Vec<String>,
    kept: bool,
}

impl TurnRegistration {
    pub(crate) fn begin(
        state: &AppState,
        discussion_id: &str,
        dispatch_ids: Vec<String>,
        transient: &TransientSecrets,
    ) -> Self {
        let guard = state.db.assistant_guard().clone();
        for id in &dispatch_ids {
            guard.begin_turn(id, discussion_id, transient);
        }
        Self {
            guard,
            ids: dispatch_ids,
            kept: false,
        }
    }

    /// The dispatches exist: each turn now ends with its dispatch.
    pub(crate) fn keep(mut self) {
        self.kept = true;
    }
}

impl Drop for TurnRegistration {
    fn drop(&mut self) {
        if !self.kept {
            for id in &self.ids {
                self.guard.end_turn(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> AssistantContext {
        AssistantContext {
            kind: AssistantKind::CustomApi,
            target_id: None,
            target_step: None,
            plugin_id: None,
            target_label: Some("Insider".into()),
            secrets: TransientSecrets::default(),
        }
    }

    #[test]
    fn a_context_bounds_its_fields() {
        assert!(validate_context(&context()).is_ok());
        assert!(validate_context(&AssistantContext {
            target_label: Some("é".repeat(MAX_LABEL_CHARS + 1)),
            ..context()
        })
        .is_err());
        assert!(validate_context(&AssistantContext {
            target_step: Some("step\nname".into()),
            ..context()
        })
        .is_err());
        assert!(validate_context(&AssistantContext {
            secrets: TransientSecrets::from(vec!["x".to_string(); MAX_TRANSIENT_SECRETS + 1]),
            ..context()
        })
        .is_err());
    }

    #[test]
    fn transient_secrets_never_print() {
        let secrets = TransientSecrets::from(vec!["tok_live_abcdef123".to_string()]);
        assert!(!format!("{secrets:?}").contains("tok_live"));
        assert!(!format!(
            "{:?}",
            AssistantContext {
                secrets,
                ..context()
            }
        )
        .contains("tok_live"));
    }

    #[test]
    fn a_bridge_token_cannot_reach_these_routes() {
        for (method, pattern) in [
            ("GET", "/api/assistant-conversations"),
            ("PATCH", "/api/assistant-conversations/{discussion_id}"),
        ] {
            assert!(crate::core::bridge_token::route_for(method, pattern).is_none());
        }
    }

    #[test]
    fn update_bounds_signatures() {
        assert!(validate_update(&UpdateAssistantConversationRequest::default()).is_ok());
        assert!(validate_update(&UpdateAssistantConversationRequest {
            last_applied_signature: Some("x".repeat(MAX_SIGNATURE_CHARS + 1)),
            ..UpdateAssistantConversationRequest::default()
        })
        .is_err());
    }
}
