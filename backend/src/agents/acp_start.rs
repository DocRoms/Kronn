//! What an ACP runtime that fails to start, or stops answering, is told to the
//! user: the startup phase it failed or never finished, or a prompt it never
//! answered. The
//! runner's start error is a string, so the phase travels as a JSON header the
//! discussion layer reads back to say it in the user's language.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::models::AgentType;

/// Leads an ACP start error raised after the runtime was spawned. It is
/// settled with a message, never deferred: retrying would repeat it unseen.
pub const ACP_START_FAILED: &str = "acp_start_failed";

/// How much of a runtime's own error a message quotes.
const DETAIL_MAX_CHARS: usize = 300;

/// How many server names a message lists before it stops.
const LISTED_SERVERS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpStartPhase {
    Initialize,
    Session,
    ModelSelection,
    /// Kronn's own preparation once the session is open.
    Start,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcpStartFailure {
    pub phase: AcpStartPhase,
    pub secs: u64,
    /// Project MCP servers declared to the session: the likely blockers.
    pub servers: Vec<String>,
    /// The runtime's error, redacted and bounded; `None` for a timeout.
    #[serde(default)]
    pub detail: Option<String>,
}

impl AcpStartFailure {
    pub fn new(phase: AcpStartPhase, waited: Duration, servers: Vec<String>) -> Self {
        // Rounded: the bound fires a few milliseconds past itself.
        let secs = (waited.as_millis() as u64 + 500) / 1000;
        Self {
            phase,
            secs,
            servers,
            detail: None,
        }
    }

    /// A phase that answered with an error instead of timing out.
    pub fn failed(phase: AcpStartPhase, error: &str) -> Self {
        let redacted = crate::core::redact::redact_for_audit_artifact(error).0;
        let mut detail: String = redacted.chars().take(DETAIL_MAX_CHARS).collect();
        if redacted.chars().nth(DETAIL_MAX_CHARS).is_some() {
            detail.push('…');
        }
        Self {
            phase,
            secs: 0,
            servers: Vec::new(),
            detail: Some(detail),
        }
    }

    /// The start error: the JSON header, then the English message.
    pub fn to_error(&self, agent: &AgentType) -> String {
        let header = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        format!("{ACP_START_FAILED} {header}\n{}", self.message(agent, "en"))
    }

    pub fn from_error(error: &str) -> Option<Self> {
        let header = error.strip_prefix(ACP_START_FAILED)?.lines().next()?.trim();
        serde_json::from_str(header).ok()
    }

    /// The message in `language` (`fr`, `es`, `zh`, else English).
    pub fn message(&self, agent: &AgentType, language: &str) -> String {
        let label = super::runner::agent_settings_label(agent);
        if let Some(detail) = &self.detail {
            return failure_message(label, self.phase, detail, language);
        }
        let secs = self.secs;
        let names = listed(&self.servers);
        match (self.phase, names.is_empty()) {
            (AcpStartPhase::Session, false) => match language {
                "fr" => format!(
                    "{label} n'a pas ouvert sa session en {secs} s. Un serveur MCP du projet bloque peut-être son démarrage : {names}. Kronn a arrêté {label}. Réessayez, ou désactivez ce serveur pour ce projet."
                ),
                "es" => format!(
                    "{label} no abrió su sesión en {secs} s. Puede que un servidor MCP del proyecto bloquee su arranque: {names}. Kronn detuvo {label}. Vuelve a intentarlo o desactiva ese servidor para este proyecto."
                ),
                "zh" => format!(
                    "{label} 未能在 {secs} 秒内打开会话。可能是项目的某个 MCP 服务器阻塞了它的启动：{names}。Kronn 已停止 {label}。请重试，或为此项目停用该服务器。"
                ),
                _ => format!(
                    "{label} did not open its session within {secs} s. A project MCP server may be blocking its start: {names}. Kronn stopped {label}. Retry, or disable that server for this project."
                ),
            },
            (AcpStartPhase::Session, true) => match language {
                "fr" => format!(
                    "{label} n'a pas ouvert sa session en {secs} s. Kronn a arrêté {label}. Réessayez dans un instant."
                ),
                "es" => format!(
                    "{label} no abrió su sesión en {secs} s. Kronn detuvo {label}. Vuelve a intentarlo en un momento."
                ),
                "zh" => format!(
                    "{label} 未能在 {secs} 秒内打开会话。Kronn 已停止 {label}。请稍后重试。"
                ),
                _ => format!(
                    "{label} did not open its session within {secs} s. Kronn stopped {label}. Retry in a moment."
                ),
            },
            (AcpStartPhase::Initialize, _) => match language {
                "fr" => format!(
                    "{label} n'a pas répondu à l'initialisation en {secs} s. Kronn a arrêté {label}. Réessayez, et vérifiez qu'il démarre en dehors de Kronn."
                ),
                "es" => format!(
                    "{label} no respondió a la inicialización en {secs} s. Kronn detuvo {label}. Vuelve a intentarlo y comprueba que arranca fuera de Kronn."
                ),
                "zh" => format!(
                    "{label} 未能在 {secs} 秒内响应初始化。Kronn 已停止 {label}。请重试，并确认它能在 Kronn 之外正常启动。"
                ),
                _ => format!(
                    "{label} did not answer initialization within {secs} s. Kronn stopped {label}. Retry, and check that it starts outside Kronn."
                ),
            },
            (AcpStartPhase::ModelSelection | AcpStartPhase::Start, _) => match language {
                "fr" => format!(
                    "{label} n'a pas appliqué le modèle choisi en {secs} s. Kronn a arrêté {label}. Réessayez, ou choisissez un autre modèle."
                ),
                "es" => format!(
                    "{label} no aplicó el modelo elegido en {secs} s. Kronn detuvo {label}. Vuelve a intentarlo o elige otro modelo."
                ),
                "zh" => format!(
                    "{label} 未能在 {secs} 秒内应用所选模型。Kronn 已停止 {label}。请重试，或选择其他模型。"
                ),
                _ => format!(
                    "{label} did not apply the chosen model within {secs} s. Kronn stopped {label}. Retry, or pick another model."
                ),
            },
        }
    }
}

fn failure_message(label: &str, phase: AcpStartPhase, detail: &str, language: &str) -> String {
    match language {
        "fr" => {
            let what = match phase {
                AcpStartPhase::Initialize => "s'initialiser",
                AcpStartPhase::Session => "ouvrir sa session",
                AcpStartPhase::ModelSelection => "appliquer le modèle choisi",
                AcpStartPhase::Start => "démarrer",
            };
            format!("{label} n'a pas pu {what} : {detail}. Kronn a arrêté {label}. Corrigez la cause indiquée, puis réessayez.")
        }
        "es" => {
            let what = match phase {
                AcpStartPhase::Initialize => "inicializarse",
                AcpStartPhase::Session => "abrir su sesión",
                AcpStartPhase::ModelSelection => "aplicar el modelo elegido",
                AcpStartPhase::Start => "arrancar",
            };
            format!("{label} no pudo {what}: {detail}. Kronn detuvo {label}. Corrige la causa indicada y vuelve a intentarlo.")
        }
        "zh" => {
            let what = match phase {
                AcpStartPhase::Initialize => "完成初始化",
                AcpStartPhase::Session => "打开会话",
                AcpStartPhase::ModelSelection => "应用所选模型",
                AcpStartPhase::Start => "启动",
            };
            format!("{label} 无法{what}：{detail}。Kronn 已停止 {label}。请修正所示原因后重试。")
        }
        _ => {
            let what = match phase {
                AcpStartPhase::Initialize => "initialize",
                AcpStartPhase::Session => "open its session",
                AcpStartPhase::ModelSelection => "apply the chosen model",
                AcpStartPhase::Start => "start",
            };
            format!("{label} could not {what}: {detail}. Kronn stopped {label}. Fix the cause shown, then retry.")
        }
    }
}

/// The message for a prompt the runtime never answered at all, in `language`.
pub fn silent_prompt_message(agent: &AgentType, limit: Duration, language: &str) -> String {
    let label = super::runner::agent_settings_label(agent);
    let secs = limit.as_secs();
    let (minutes, whole) = (secs / 60, secs >= 60 && secs.is_multiple_of(60));
    match language {
        "fr" => {
            let delay = if whole {
                format!("{minutes} min")
            } else {
                format!("{secs} s")
            };
            format!(
                "{label} n'a rien renvoyé pendant {delay} après avoir reçu le message, pas même un premier mot. Kronn a arrêté {label} et ce tour a échoué. Le fournisseur du modèle est peut-être saturé ou lent à démarrer : réessayez, choisissez un autre modèle, ou augmentez le délai d'inactivité des agents (Config › Serveur)."
            )
        }
        "es" => {
            let delay = if whole {
                format!("{minutes} min")
            } else {
                format!("{secs} s")
            };
            format!(
                "{label} no devolvió nada durante {delay} tras recibir el mensaje, ni siquiera una primera palabra. Kronn detuvo {label} y este turno falló. Puede que el proveedor del modelo esté saturado o tarde en arrancar: vuelve a intentarlo, elige otro modelo o aumenta el tiempo de inactividad de los agentes (Configuración › Servidor)."
            )
        }
        "zh" => {
            let delay = if whole {
                format!("{minutes} 分钟")
            } else {
                format!("{secs} 秒")
            };
            format!(
                "{label} 收到消息后 {delay} 内没有返回任何内容，连第一个词都没有。Kronn 已停止 {label}，本轮失败。模型提供方可能过载或启动缓慢：请重试、选择其他模型，或调高智能体空闲超时（配置 › 服务器）。"
            )
        }
        _ => {
            let delay = if whole {
                format!("{minutes} min")
            } else {
                format!("{secs} s")
            };
            format!(
                "{label} returned nothing for {delay} after receiving the message, not even a first word. Kronn stopped {label} and this turn failed. The model provider may be overloaded or slow to start: retry, pick another model, or raise the agent inactivity timeout (Config › Server)."
            )
        }
    }
}

fn listed(servers: &[String]) -> String {
    let mut names: Vec<&str> = servers
        .iter()
        .take(LISTED_SERVERS)
        .map(String::as_str)
        .collect();
    if servers.len() > LISTED_SERVERS {
        names.push("…");
    }
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_timeout_round_trips_through_the_start_error() {
        let timeout = AcpStartFailure::new(
            AcpStartPhase::Session,
            Duration::from_millis(90_004),
            vec!["Memory".into(), "Sequential Thinking".into()],
        );
        let error = timeout.to_error(&AgentType::OpenCode);
        assert!(error.starts_with(ACP_START_FAILED));
        assert!(
            error.contains("OpenCode did not open its session within 90 s"),
            "{error}"
        );
        assert_eq!(AcpStartFailure::from_error(&error), Some(timeout));
        assert_eq!(AcpStartFailure::from_error("ACP spawn failed"), None);
    }

    #[test]
    fn every_language_names_the_agent_the_phase_and_the_servers() {
        let timeout = AcpStartFailure::new(
            AcpStartPhase::Session,
            Duration::from_secs(90),
            vec!["Hang".into()],
        );
        let fr = timeout.message(&AgentType::OpenCode, "fr");
        assert_eq!(
            fr,
            "OpenCode n'a pas ouvert sa session en 90 s. Un serveur MCP du projet bloque peut-être son démarrage : Hang. Kronn a arrêté OpenCode. Réessayez, ou désactivez ce serveur pour ce projet."
        );
        for language in ["en", "es", "zh"] {
            let message = timeout.message(&AgentType::OpenCode, language);
            assert!(
                message.contains("OpenCode") && message.contains("90"),
                "{message}"
            );
            assert!(message.contains("Hang"), "{language}: {message}");
        }
        let initialize =
            AcpStartFailure::new(AcpStartPhase::Initialize, Duration::from_secs(30), vec![]);
        assert!(initialize
            .message(&AgentType::Vibe, "es")
            .starts_with("Vibe no respondió a la inicialización en 30 s"));
    }

    #[test]
    fn a_startup_error_is_quoted_redacted_in_every_language() {
        let failure = AcpStartFailure::failed(
            AcpStartPhase::Session,
            "ACP transport failed: auth refused for key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789",
        );
        let error = failure.to_error(&AgentType::OpenCode);
        assert_eq!(AcpStartFailure::from_error(&error), Some(failure.clone()));
        for language in ["fr", "en", "es", "zh"] {
            let message = failure.message(&AgentType::OpenCode, language);
            assert!(message.contains("auth refused"), "{language}: {message}");
            assert!(
                !message.contains("abcdefghijklmnop"),
                "{language}: {message}"
            );
        }
        assert!(failure
            .message(&AgentType::OpenCode, "fr")
            .starts_with("OpenCode n'a pas pu ouvrir sa session : "));
    }

    #[test]
    fn a_long_server_list_is_cut() {
        let servers: Vec<String> = (0..9).map(|index| format!("s{index}")).collect();
        assert_eq!(listed(&servers), "s0, s1, s2, s3, s4, s5, …");
    }

    #[test]
    fn a_silent_prompt_says_how_long_kronn_waited() {
        let message = silent_prompt_message(&AgentType::OpenCode, Duration::from_secs(300), "fr");
        assert!(
            message.starts_with("OpenCode n'a rien renvoyé pendant 5 min"),
            "{message}"
        );
        let short = silent_prompt_message(&AgentType::OpenCode, Duration::from_secs(2), "en");
        assert!(
            short.starts_with("OpenCode returned nothing for 2 s"),
            "{short}"
        );
        assert!(
            silent_prompt_message(&AgentType::OpenCode, Duration::from_secs(60), "zh")
                .contains("1 分钟")
        );
    }
}
