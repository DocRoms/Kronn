//! Refusals decided before an agent starts (KT-1154): an endpoint nothing
//! answers at, and a model the catalogue refuses. Each one leaves a System
//! card in the discussion that says why, typed, instead of a transient toast.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::core::endpoint_reachability::{StartRefusal, UnreachableReason, SHARED, START_BUDGET};
use crate::models::setup::HttpEndpoints;
use crate::models::{
    AgentType, CatalogPreflightFailure, DiscussionMessage, MessageChannel, MessageRole, ModelTier,
    ModelUnavailableReason,
};
use crate::AppState;

/// The address the runner will call for this HTTP agent, when Kronn knows it.
pub(crate) fn launch_endpoint(
    agent_type: &AgentType,
    external_endpoint: Option<&str>,
    endpoints: &HttpEndpoints,
) -> Option<String> {
    if let Some(endpoint) = external_endpoint
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
    {
        return Some(endpoint.to_string());
    }
    match agent_type {
        AgentType::LiteLlm => Some(crate::api::lite_llm::resolve_base_url_pub(
            endpoints.lite_llm.as_deref(),
        )),
        AgentType::Ollama => Some(crate::api::ollama::resolve_base_url_pub(
            endpoints.for_agent(agent_type),
        )),
        _ => None,
    }
}

/// The transport verdict for a launch, bounded by `START_BUDGET` and served
/// from the KT-697 cache when an observation is fresh.
pub(crate) async fn unreachable_endpoint(
    agent_type: &AgentType,
    external_endpoint: Option<&str>,
    endpoints: &HttpEndpoints,
) -> Option<StartRefusal> {
    if !crate::agents::runner::is_http_chat_agent(agent_type) {
        return None;
    }
    let endpoint = launch_endpoint(agent_type, external_endpoint, endpoints)?;
    SHARED.start_refusal(&endpoint, START_BUDGET).await
}

fn tier_key(tier: ModelTier) -> &'static str {
    match tier {
        ModelTier::Economy => "economy",
        ModelTier::Default => "default",
        ModelTier::Reasoning => "reasoning",
    }
}

fn stamp(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

fn card(
    kind: &str,
    status: Option<u16>,
    summary: &str,
    detail: &str,
    tier: ModelTier,
    retry_dispatch_id: Option<&str>,
) -> String {
    let payload = serde_json::json!({
        "kind": kind,
        "status": status,
        "summary": summary,
        "detail": detail,
        "tier": tier_key(tier),
        "retry_dispatch_id": retry_dispatch_id,
        "retried": false,
    });
    format!("[kronn:agent-error]\n{payload}")
}

fn unreachable_phrase(language: &str, reason: UnreachableReason) -> &'static str {
    use UnreachableReason as R;
    match (language, reason) {
        ("fr", R::Dns) => "nom introuvable (DNS)",
        ("fr", R::Refused) => "connexion refusée",
        ("fr", R::Tls) => "échec TLS",
        ("fr", R::InvalidEndpoint) => "adresse invalide",
        ("fr", R::Timeout) => "pas de réponse dans le délai",
        ("fr", R::HttpStatus) => "erreur du serveur",
        ("fr", R::Connect) => "connexion impossible",
        ("es", R::Dns) => "nombre no encontrado (DNS)",
        ("es", R::Refused) => "conexión rechazada",
        ("es", R::Tls) => "fallo TLS",
        ("es", R::InvalidEndpoint) => "dirección no válida",
        ("es", R::Timeout) => "sin respuesta a tiempo",
        ("es", R::HttpStatus) => "error del servidor",
        ("es", R::Connect) => "conexión imposible",
        ("zh", R::Dns) => "无法解析域名（DNS）",
        ("zh", R::Refused) => "连接被拒绝",
        ("zh", R::Tls) => "TLS 失败",
        ("zh", R::InvalidEndpoint) => "地址无效",
        ("zh", R::Timeout) => "响应超时",
        ("zh", R::HttpStatus) => "服务器错误",
        ("zh", R::Connect) => "无法连接",
        (_, R::Dns) => "name not resolved (DNS)",
        (_, R::Refused) => "connection refused",
        (_, R::Tls) => "TLS failure",
        (_, R::InvalidEndpoint) => "invalid address",
        (_, R::Timeout) => "no answer in time",
        (_, R::HttpStatus) => "server error",
        (_, R::Connect) => "connection failed",
    }
}

/// The VPN card for an endpoint nothing answers at. The address stays out of
/// it, as it stays out of every KT-697 result.
pub(crate) fn unreachable_card(
    label: &str,
    language: &str,
    tier: ModelTier,
    refusal: &StartRefusal,
    retry_dispatch_id: Option<&str>,
) -> String {
    let why = unreachable_phrase(language, refusal.reason);
    let summary = match language {
        "fr" => format!("{label} est injoignable ({why}). Vérifiez la connexion, le VPN ou le service, puis relancez uniquement cet agent."),
        "es" => format!("{label} no es accesible ({why}). Comprueba la conexión, la VPN o el servicio y vuelve a ejecutar solo este agente."),
        "zh" => format!("{label} 无法访问（{why}）。请检查网络、VPN 或服务，然后仅重试此智能体。"),
        _ => format!("{label} is unreachable ({why}). Check the connection, VPN or service, then retry only this agent."),
    };
    let detail = format!(
        "endpoint probe before start: {} (observed {})",
        refusal.reason.as_str(),
        refusal.checked_at.to_rfc3339()
    );
    card(
        "agent_error",
        None,
        &summary,
        &detail,
        tier,
        retry_dispatch_id,
    )
}

/// One line for `last_error`, so a refused start is diagnosable from the job.
pub(crate) fn unreachable_diagnostic(refusal: &StartRefusal) -> String {
    format!(
        "endpoint unreachable before start ({}), observed {}",
        refusal.reason.as_str(),
        refusal.checked_at.to_rfc3339()
    )
}

fn refusal_phrase(language: &str, reason: ModelUnavailableReason) -> &'static str {
    use ModelUnavailableReason as R;
    match (language, reason) {
        ("fr", R::AccessDenied) => "accès refusé pour cette clé ou ce projet",
        ("fr", R::NotFound) => "introuvable chez le fournisseur",
        ("fr", R::Disappeared) => "absent du dernier catalogue, sans remplaçant",
        ("fr", R::AuthRequired) => "authentification requise",
        ("fr", R::Timeout) => "le catalogue n'a pas répondu à temps",
        ("fr", R::CliMissing) => "CLI non installée",
        ("fr", R::InvalidCatalog) => "catalogue illisible",
        ("fr", R::ProviderError) => "erreur du fournisseur",
        ("fr", R::Unsupported) => "inutilisable pour une discussion",
        ("es", R::AccessDenied) => "acceso denegado para esta clave o proyecto",
        ("es", R::NotFound) => "no encontrado en el proveedor",
        ("es", R::Disappeared) => "ausente del último catálogo, sin reemplazo",
        ("es", R::AuthRequired) => "autenticación requerida",
        ("es", R::Timeout) => "el catálogo no respondió a tiempo",
        ("es", R::CliMissing) => "CLI no instalada",
        ("es", R::InvalidCatalog) => "catálogo ilegible",
        ("es", R::ProviderError) => "error del proveedor",
        ("es", R::Unsupported) => "no utilizable para una conversación",
        ("zh", R::AccessDenied) => "此密钥或项目被拒绝访问",
        ("zh", R::NotFound) => "提供方找不到该模型",
        ("zh", R::Disappeared) => "已从最新目录中消失且无替代",
        ("zh", R::AuthRequired) => "需要身份验证",
        ("zh", R::Timeout) => "目录未及时响应",
        ("zh", R::CliMissing) => "CLI 未安装",
        ("zh", R::InvalidCatalog) => "目录无法解析",
        ("zh", R::ProviderError) => "提供方错误",
        ("zh", R::Unsupported) => "不能用于对话",
        (_, R::AccessDenied) => "access denied for this key or project",
        (_, R::NotFound) => "not found at the provider",
        (_, R::Disappeared) => "gone from the latest catalogue, with no replacement",
        (_, R::AuthRequired) => "authentication required",
        (_, R::Timeout) => "the catalogue did not answer in time",
        (_, R::CliMissing) => "CLI not installed",
        (_, R::InvalidCatalog) => "unreadable catalogue",
        (_, R::ProviderError) => "provider error",
        (_, R::Unsupported) => "not usable for a discussion",
    }
}

fn remedy(language: &str, action: &str, settings: &str) -> String {
    match (language, action) {
        ("fr", "authenticate") => format!("Reconnectez l'agent dans {settings}."),
        ("fr", "install_cli") => format!("Installez la CLI dans {settings}."),
        ("fr", "recheck_catalog") => {
            format!("Relancez la vérification du catalogue dans {settings}.")
        }
        ("fr", "configure_manual_model") => {
            format!("Déclarez le modèle à la main dans {settings}.")
        }
        ("fr", _) => format!("Choisissez un autre modèle dans {settings}."),
        ("es", "authenticate") => format!("Vuelve a conectar el agente en {settings}."),
        ("es", "install_cli") => format!("Instala la CLI en {settings}."),
        ("es", "recheck_catalog") => format!("Vuelve a comprobar el catálogo en {settings}."),
        ("es", "configure_manual_model") => format!("Declara el modelo a mano en {settings}."),
        ("es", _) => format!("Elige otro modelo en {settings}."),
        ("zh", "authenticate") => format!("请在 {settings} 中重新连接该智能体。"),
        ("zh", "install_cli") => format!("请在 {settings} 中安装 CLI。"),
        ("zh", "recheck_catalog") => format!("请在 {settings} 中重新检查目录。"),
        ("zh", "configure_manual_model") => format!("请在 {settings} 中手动声明该模型。"),
        ("zh", _) => format!("请在 {settings} 中选择其他模型。"),
        (_, "authenticate") => format!("Sign the agent in again in {settings}."),
        (_, "install_cli") => format!("Install the CLI in {settings}."),
        (_, "recheck_catalog") => format!("Re-check the catalogue in {settings}."),
        (_, "configure_manual_model") => format!("Declare the model by hand in {settings}."),
        (_, _) => format!("Choose another model in {settings}."),
    }
}

/// The sentence a catalogue refusal reads as: which model, why, since when,
/// and the one step that applies. The date is the catalogue's own, so a
/// refusal remembered from days ago says so.
pub(crate) fn preflight_refusal_summary(
    agent_type: &AgentType,
    language: &str,
    failure: &CatalogPreflightFailure,
) -> String {
    let label = format!("{agent_type:?}");
    let model = failure.model_id.as_deref().unwrap_or("default");
    let why = refusal_phrase(language, failure.reason);
    let since = stamp(failure.last_checked_at);
    let settings = match (language, agent_type) {
        ("fr", AgentType::LiteLlm) => "Config › Agents › LiteLLM",
        ("es", AgentType::LiteLlm) => "Config › Agentes › LiteLLM",
        ("zh", AgentType::LiteLlm) => "配置 › 智能体 › LiteLLM",
        (_, AgentType::LiteLlm) => "Config › Agents › LiteLLM",
        ("es", _) => "Config › Agentes",
        ("zh", _) => "配置 › 智能体",
        _ => "Config › Agents",
    };
    let next = remedy(language, &failure.recommended_action, settings);
    match language {
        "fr" => {
            format!("{label} refuse le modèle « {model} » : {why} (constaté le {since}). {next}")
        }
        "es" => {
            format!("{label} rechaza el modelo «{model}»: {why} (constatado el {since}). {next}")
        }
        "zh" => format!("{label} 拒绝模型“{model}”：{why}（记录于 {since}）。{next}"),
        _ => format!("{label} refuses model “{model}”: {why} (recorded {since}). {next}"),
    }
}

/// `HTTP 401` in a stored refusal's detail, when the proxy gave one.
fn detail_http_status(detail: &str) -> Option<u16> {
    let (_, rest) = detail.split_once("HTTP ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (digits.len() == 3).then(|| digits.parse().ok()).flatten()
}

pub(crate) fn preflight_refusal_card(
    summary: &str,
    tier: ModelTier,
    failure: &CatalogPreflightFailure,
    retry_dispatch_id: Option<&str>,
) -> String {
    use ModelUnavailableReason as R;
    let kind = match failure.reason {
        R::AccessDenied | R::NotFound | R::Disappeared | R::Unsupported | R::InvalidCatalog => {
            "model_error"
        }
        R::AuthRequired | R::Timeout | R::CliMissing | R::ProviderError => "agent_error",
    };
    card(
        kind,
        detail_http_status(&failure.detail),
        summary,
        &crate::core::model_catalog::preflight_refusal_diagnostic(failure),
        tier,
        retry_dispatch_id,
    )
}

/// Who a refused start belongs to, for the System card it leaves.
pub(crate) struct RefusedStart<'a> {
    pub discussion_id: &'a str,
    pub agent_type: &'a AgentType,
    pub model: Option<String>,
    pub tier: ModelTier,
    pub dispatch_id: Option<&'a str>,
    pub tracked_dispatch: bool,
    pub reply_to: Option<String>,
}

/// Write the card the way a failed spawn writes its own: linked to its
/// dispatch, federated, and clearing an untracked awaiting flag.
pub(crate) async fn record_refusal_card(
    state: &AppState,
    start: RefusedStart<'_>,
    content: String,
) {
    let message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        session_continuity: None,
        kind: Default::default(),
        model: start.model,
        lint_report: None,
        id: Uuid::new_v4().to_string(),
        role: MessageRole::System,
        channel: MessageChannel::Main,
        content,
        agent_type: Some(start.agent_type.clone()),
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: Some(tier_key(start.tier).to_string()),
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: start.reply_to,
    };
    let discussion_id = start.discussion_id.to_string();
    let dispatch_id = start.dispatch_id.map(str::to_string);
    let tracked = start.tracked_dispatch;
    let stored = message.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            super::streaming::persist_agent_start_error(
                conn,
                &discussion_id,
                &stored,
                dispatch_id.as_deref(),
                tracked,
            )
        })
        .await
    {
        tracing::error!("Failed to save the refused-start card: {error}");
    }
    crate::api::federation::federate_message(state, start.discussion_id, &message).await;
}

#[cfg(test)]
#[path = "start_refusal_tests.rs"]
mod tests;
