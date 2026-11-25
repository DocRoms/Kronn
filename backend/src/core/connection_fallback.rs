//! Mode-scoped fallbacks for named HTTP connections (every mode) and local ComfyUI
//! (image and video only).
//! Connection identity is retained independently of the shared Custom agent type.

use crate::core::agent_fallback::MAX_CHAIN_DEPTH;
use crate::core::comfyui::CONNECTION_ID as COMFYUI;
use crate::models::{
    AgentsConfig, ConnectionFallback, ConnectionFallbackTarget, FallbackMode, FallbackTransport,
    MediaModality,
};

/// The owner key of the local ComfyUI; any other owner is a named connection's id.
pub const COMFYUI_OWNER: &str = COMFYUI;

/// The media modality of a mode; `None` for the text and decision modes.
pub fn modality_of(mode: FallbackMode) -> Option<MediaModality> {
    match mode {
        FallbackMode::Image => Some(MediaModality::Image),
        FallbackMode::Video => Some(MediaModality::Video),
        _ => None,
    }
}

pub fn mode_of(modality: MediaModality) -> FallbackMode {
    match modality {
        MediaModality::Image => FallbackMode::Image,
        MediaModality::Video => FallbackMode::Video,
    }
}

/// How a target is reached: the local ComfyUI, or a remote HTTP API.
pub fn transport(connection: &str) -> FallbackTransport {
    if connection == COMFYUI_OWNER {
        FallbackTransport::Local
    } else {
        FallbackTransport::Http
    }
}

/// Why a media fallback is refused.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionChainError {
    /// ComfyUI only has image and video rows.
    NotMedia(FallbackMode),
    SelfFallback(String),
    /// The walk would come back to an owner it already went through.
    Cycle(Vec<String>),
    TooDeep(usize),
    BadModel(&'static str),
    NoOwner,
}

impl std::fmt::Display for ConnectionChainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionChainError::NotMedia(mode) => write!(
                f,
                "{} is not a media mode: ComfyUI only has image and video fallbacks.",
                mode.label()
            ),
            ConnectionChainError::SelfFallback(_) => {
                write!(f, "A source cannot be its own fallback.")
            }
            ConnectionChainError::Cycle(path) => {
                write!(f, "This fallback would loop: {}.", path.join(" → "))
            }
            ConnectionChainError::TooDeep(links) => write!(
                f,
                "A fallback chain would be {links} links long; at most {MAX_CHAIN_DEPTH} are allowed."
            ),
            ConnectionChainError::BadModel(why) => write!(f, "The fallback model {why}."),
            ConnectionChainError::NoOwner => write!(f, "The source of this fallback is missing."),
        }
    }
}

impl std::error::Error for ConnectionChainError {}

/// The fallback `owner` has on its `mode` row.
pub fn row<'a>(
    agents: &'a AgentsConfig,
    owner: &str,
    mode: FallbackMode,
) -> Option<&'a ConnectionFallbackTarget> {
    agents
        .connection_fallbacks
        .iter()
        .find(|entry| entry.owner == owner && entry.row == mode)
        .map(|entry| &entry.fallback)
}

/// Sets (`Some`) or removes (`None`) the `mode` row of `owner`.
pub fn set_row(
    agents: &mut AgentsConfig,
    owner: &str,
    mode: FallbackMode,
    fallback: Option<ConnectionFallbackTarget>,
) {
    agents
        .connection_fallbacks
        .retain(|entry| !(entry.owner == owner && entry.row == mode));
    if let Some(fallback) = fallback {
        agents.connection_fallbacks.push(ConnectionFallback {
            owner: owner.to_string(),
            row: mode,
            fallback,
        });
        agents
            .connection_fallbacks
            .sort_by(|a, b| (&a.owner, a.row).cmp(&(&b.owner, b.row)));
    }
}

/// Checks a row before it is stored: ComfyUI only on a media mode, a target that
/// is not the owner, a sane model, and the chain it would make through the rows of the
/// fallbacks — no loop, not longer than [`MAX_CHAIN_DEPTH`]. Nothing is stored.
pub fn validate_row(
    agents: &AgentsConfig,
    owner: &str,
    mode: FallbackMode,
    target: &ConnectionFallbackTarget,
) -> Result<(), ConnectionChainError> {
    if owner.trim().is_empty() || target.connection.trim().is_empty() {
        return Err(ConnectionChainError::NoOwner);
    }
    if modality_of(mode).is_none() && (owner == COMFYUI_OWNER || target.connection == COMFYUI_OWNER)
    {
        return Err(ConnectionChainError::NotMedia(mode));
    }
    if target.connection == owner {
        return Err(ConnectionChainError::SelfFallback(owner.to_string()));
    }
    if let Some(model) = &target.model {
        if model.trim().is_empty() {
            return Err(ConnectionChainError::BadModel("is empty"));
        }
        if model.chars().count() > 200 || model.chars().any(char::is_control) {
            return Err(ConnectionChainError::BadModel(
                "must be one line of at most 200 characters",
            ));
        }
    }
    let mut trial = agents.clone();
    set_row(&mut trial, owner, mode, Some(target.clone()));
    let mut seen = vec![owner.to_string()];
    let mut at = target.connection.clone();
    seen.push(at.clone());
    while let Some(next) = row(&trial, &at, mode) {
        if seen.contains(&next.connection) {
            let mut path = seen.clone();
            path.push(next.connection.clone());
            return Err(ConnectionChainError::Cycle(path));
        }
        seen.push(next.connection.clone());
        if seen.len() - 1 > MAX_CHAIN_DEPTH {
            return Err(ConnectionChainError::TooDeep(seen.len() - 1));
        }
        at = next.connection.clone();
    }
    Ok(())
}

/// The next target of a generation that left `from` (and the owners in `left`
/// before it) on `mode`: the row of `from`, unless it points back to an owner the
/// job already left (a loop written by hand is simply not walked) or the job
/// already made [`MAX_CHAIN_DEPTH`] hops.
pub fn next_target<'a>(
    agents: &'a AgentsConfig,
    from: &str,
    mode: FallbackMode,
    left: &[String],
) -> Option<&'a ConnectionFallbackTarget> {
    if left.len() >= MAX_CHAIN_DEPTH {
        return None;
    }
    let next = row(agents, from, mode)?;
    (next.connection != from && !left.contains(&next.connection)).then_some(next)
}

/// One link as Settings shows it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ConnectionLinkView {
    pub connection: String,
    #[ts(optional)]
    pub model: Option<String>,
    pub transport: FallbackTransport,
}

/// The fallback of one media row, for display: its own link, the links inherited
/// through the rows of the fallbacks, or why it is refused.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ConnectionChainView {
    pub owner: String,
    pub mode: FallbackMode,
    pub links: Vec<ConnectionLinkView>,
    pub inherited: Vec<ConnectionLinkView>,
    #[ts(optional)]
    pub error: Option<String>,
}

fn link_view(target: &ConnectionFallbackTarget) -> ConnectionLinkView {
    ConnectionLinkView {
        connection: target.connection.clone(),
        model: target.model.clone(),
        transport: transport(&target.connection),
    }
}

pub fn chain_view(agents: &AgentsConfig, owner: &str, mode: FallbackMode) -> ConnectionChainView {
    let own = row(agents, owner, mode);
    let error = own.and_then(|target| {
        // A stored row is validated as if it were being set again over itself.
        let mut without = agents.clone();
        set_row(&mut without, owner, mode, None);
        validate_row(&without, owner, mode, target)
            .err()
            .map(|error| error.to_string())
    });
    let mut inherited = Vec::new();
    if let Some(first) = own {
        let mut left = vec![owner.to_string()];
        let mut at = first;
        while let Some(next) = next_target(agents, &at.connection, mode, &left) {
            left.push(at.connection.clone());
            inherited.push(link_view(next));
            at = next;
        }
    }
    ConnectionChainView {
        owner: owner.to_string(),
        mode,
        links: own.map(link_view).into_iter().collect(),
        inherited,
        error,
    }
}

/// One view per media row that has a fallback.
pub fn overview(agents: &AgentsConfig) -> Vec<ConnectionChainView> {
    agents
        .connection_fallbacks
        .iter()
        .map(|entry| chain_view(agents, &entry.owner, entry.row))
        .collect()
}

/// Resolve a connection's configured model without carrying a foreign model across providers.
pub fn configured_model(
    connection: &crate::models::ExternalApiConnection,
    mode: FallbackMode,
    explicit: Option<&str>,
) -> Option<String> {
    let slots: Vec<&str> = match mode {
        FallbackMode::Eco | FallbackMode::Standard | FallbackMode::Advanced => [
            connection.economy_model.as_deref(),
            connection.default_model.as_deref(),
            connection.reasoning_model.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect(),
        FallbackMode::Decision => connection.decision_model.as_deref().into_iter().collect(),
        FallbackMode::Image => connection.image_model.as_deref().into_iter().collect(),
        FallbackMode::Video => connection.video_model.as_deref().into_iter().collect(),
    };
    if let Some(model) = explicit {
        return slots.contains(&model).then(|| model.to_string());
    }
    let model = match mode {
        FallbackMode::Eco => connection
            .economy_model
            .as_deref()
            .or(connection.default_model.as_deref()),
        FallbackMode::Standard => connection.default_model.as_deref(),
        FallbackMode::Advanced => connection
            .reasoning_model
            .as_deref()
            .or(connection.default_model.as_deref()),
        _ => slots.first().copied(),
    };
    model
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
}

/// Next usable named text/decision target. Media uses the media runner's acceptance guard.
pub async fn next_text_target(
    state: &crate::AppState,
    owner: &str,
    mode: FallbackMode,
) -> Option<(crate::models::ExternalApiConnection, String)> {
    if modality_of(mode).is_some() {
        return None;
    }
    let agents = state.config.read().await.agents.clone();
    let mut from = owner.to_string();
    let mut left = Vec::new();
    while let Some(target) = next_target(&agents, &from, mode, &left) {
        let id = target.connection.clone();
        let connection = state
            .db
            .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &id))
            .await
            .ok()
            .flatten();
        if let Some(connection) = connection {
            let model = configured_model(&connection, mode, target.model.as_deref());
            let tokens = &state.config.read().await.tokens;
            let runtime = crate::http_transport::external_http_runtime(&connection, tokens);
            if let (Some(model), Some(runtime)) = (model, runtime) {
                // KT-2057 — a link missing the key its preset needs is skipped,
                // as a step on that connection is refused (`named_connection_refusal`).
                let keyed =
                    runtime.api_key.is_some() || !connection.origin_preset.requires_credential();
                if keyed && !runtime.endpoint.trim().is_empty() {
                    return Some((connection, model));
                }
            }
        }
        left.push(from);
        from = target.connection.clone();
    }
    None
}

#[cfg(test)]
#[path = "connection_fallback_test.rs"]
mod tests;
