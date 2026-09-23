//! Publish reached ceilings and a durable human budget question alongside the
//! agent answer, independently of the model's own wording.

use crate::agents::runner::CeilingReport;
use crate::db::discussion_ceiling_requests::{
    tool_step, Ceiling, OPTION_GRANT, OPTION_STOP, OPTION_UNLIMITED, QUESTION_KEY_PREFIX,
    ROUND_STEP,
};

pub(crate) struct CeilingQuestion {
    pub key: String,
    pub ceilings: Vec<Ceiling>,
    /// Appended to the agent's answer: the notice, then the question fence.
    pub markdown: String,
}

struct Words {
    title: &'static str,
    calls: fn(&str, usize, usize) -> String,
    missing: &'static str,
    rounds: fn(usize) -> String,
    question: &'static str,
    context: &'static str,
    grant: fn(&str) -> String,
    grant_detail: &'static str,
    unlimited: &'static str,
    unlimited_detail: fn(&str) -> String,
    stop: &'static str,
    stop_detail: &'static str,
    more_calls: fn(usize, &str) -> String,
    more_rounds: fn(usize) -> String,
}

fn words(language: &str) -> Words {
    match language {
        "fr" => Words {
            title: "⏸️ **Plafond atteint** : cette réponse est partielle.",
            calls: |tool, limit, refused| {
                format!("`{tool}` : {limit} appels autorisés, {refused} refusé(s) ensuite.")
            },
            missing: "Non consulté :",
            rounds: |limit| format!("Tours d'outils : les {limit} autorisés ont été utilisés."),
            question: "Le run s'est arrêté sur un plafond. Aller plus loin ?",
            context: "Une rallonge ne vaut que pour le prochain tour de cet agent. Les gardes \
                      anti-boucle (appels répétés, erreurs en série, durée maximale) restent \
                      actives dans tous les cas.",
            grant: |amount| format!("Autoriser {amount}"),
            grant_detail: "Pour le prochain tour de l'agent seulement. Il reprend là où sa \
                           réponse s'est arrêtée.",
            unlimited: "Aucune limite d'appels pour cette discussion",
            unlimited_detail: |tools| {
                format!(
                    "Lève le compteur de {tools} jusqu'à la fin de la discussion. Les gardes \
                     anti-boucle restent actives."
                )
            },
            stop: "Garder la réponse partielle",
            stop_detail: "Rien n'est relancé.",
            more_calls: |step, tool| format!("{step} appels `{tool}` de plus"),
            more_rounds: |step| format!("{step} tours de plus"),
        },
        "es" => Words {
            title: "⏸️ **Límite alcanzado**: esta respuesta es parcial.",
            calls: |tool, limit, refused| {
                format!("`{tool}`: {limit} llamadas permitidas, {refused} rechazada(s) después.")
            },
            missing: "No consultado:",
            rounds: |limit| format!("Rondas de herramientas: se usaron las {limit} permitidas."),
            question: "La ejecución se detuvo en un límite. ¿Ir más lejos?",
            context: "Una ampliación solo vale para el próximo turno de este agente. Las \
                      protecciones contra bucles (llamadas repetidas, errores en serie, \
                      duración máxima) siguen activas en todos los casos.",
            grant: |amount| format!("Permitir {amount}"),
            grant_detail: "Solo para el próximo turno del agente. Retoma donde su respuesta se \
                           detuvo.",
            unlimited: "Sin límite de llamadas en esta conversación",
            unlimited_detail: |tools| {
                format!(
                    "Levanta el contador de {tools} hasta el final de la conversación. Las \
                     protecciones contra bucles siguen activas."
                )
            },
            stop: "Quedarse con la respuesta parcial",
            stop_detail: "No se relanza nada.",
            more_calls: |step, tool| format!("{step} llamadas `{tool}` más"),
            more_rounds: |step| format!("{step} rondas más"),
        },
        _ => Words {
            title: "⏸️ **Ceiling reached**: this answer is partial.",
            calls: |tool, limit, refused| {
                format!("`{tool}`: {limit} calls allowed, {refused} refused after that.")
            },
            missing: "Not read:",
            rounds: |limit| format!("Tool rounds: all {limit} allowed were used."),
            question: "The run stopped on a ceiling. Go further?",
            context: "A grant only holds for this agent's next turn. The loop guards \
                      (repeated calls, error streaks, maximum duration) stay active in every \
                      case.",
            grant: |amount| format!("Allow {amount}"),
            grant_detail: "For the agent's next turn only. It picks up where its answer \
                           stopped.",
            unlimited: "No call limit for this discussion",
            unlimited_detail: |tools| {
                format!(
                    "Lifts the counter of {tools} until the discussion ends. The loop guards \
                     stay active."
                )
            },
            stop: "Keep the partial answer",
            stop_detail: "Nothing is run again.",
            more_calls: |step, tool| format!("{step} more `{tool}` calls"),
            more_rounds: |step| format!("{step} more rounds"),
        },
    }
}

pub(crate) fn ceiling_question(report: &CeilingReport, language: &str) -> CeilingQuestion {
    let words = words(language);
    let key = format!(
        "{QUESTION_KEY_PREFIX}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let mut ceilings = Vec::new();
    let mut lines = Vec::new();
    let mut amounts = Vec::new();
    for hit in &report.tools {
        let step = tool_step(hit.limit);
        ceilings.push(Ceiling::Tool {
            tool: hit.tool.clone(),
            limit: hit.limit,
            step,
        });
        let mut line = format!("- {}", (words.calls)(&hit.tool, hit.limit, hit.refused));
        if !hit.refused_calls.is_empty() {
            let shown = hit
                .refused_calls
                .iter()
                .map(|call| format!("`{}`", call.replace('`', "'")))
                .collect::<Vec<_>>()
                .join(", ");
            let more = hit.refused.saturating_sub(hit.refused_calls.len());
            line.push_str(&format!(
                " {} {shown}{}",
                words.missing,
                if more > 0 {
                    format!(" (+{more})")
                } else {
                    String::new()
                }
            ));
        }
        lines.push(line);
        amounts.push((words.more_calls)(step, &hit.tool));
    }
    if let Some(limit) = report.rounds {
        ceilings.push(Ceiling::Rounds {
            limit,
            step: ROUND_STEP,
        });
        lines.push(format!("- {}", (words.rounds)(limit)));
        amounts.push((words.more_rounds)(ROUND_STEP));
    }

    let mut options = vec![serde_json::json!({
        "id": OPTION_GRANT,
        "label": (words.grant)(&amounts.join(", ")),
        "description": words.grant_detail,
    })];
    let tools: Vec<String> = report
        .tools
        .iter()
        .map(|hit| format!("`{}`", hit.tool))
        .collect();
    if !tools.is_empty() {
        options.push(serde_json::json!({
            "id": OPTION_UNLIMITED,
            "label": words.unlimited,
            "description": (words.unlimited_detail)(&tools.join(", ")),
        }));
    }
    options.push(serde_json::json!({
        "id": OPTION_STOP,
        "label": words.stop,
        "description": words.stop_detail,
    }));
    let fence = serde_json::json!({
        "version": 1,
        "key": key,
        "question": words.question,
        "context": words.context,
        "options": options,
        "recommended_option_ids": [OPTION_GRANT],
    });
    let markdown = format!(
        "\n\n---\n{}\n{}\n\n```kronn-question\n{}\n```",
        words.title,
        lines.join("\n"),
        serde_json::to_string_pretty(&fence).unwrap_or_default()
    );
    CeilingQuestion {
        key,
        ceilings,
        markdown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::runner::ToolCeilingHit;

    fn report() -> CeilingReport {
        CeilingReport {
            version: 1,
            tools: vec![ToolCeilingHit {
                tool: "web_fetch".into(),
                limit: 120,
                refused: 7,
                refused_calls: vec![
                    "url=https://github.com/o/r/issues/119".into(),
                    "url=https://github.com/o/r/issues/120".into(),
                ],
            }],
            rounds: None,
        }
    }

    #[test]
    fn the_notice_names_the_ceiling_its_value_and_what_was_not_read() {
        let question = ceiling_question(&report(), "fr");
        assert!(question.markdown.contains("**Plafond atteint**"));
        assert!(question
            .markdown
            .contains("`web_fetch` : 120 appels autorisés, 7 refusé(s)"));
        assert!(question
            .markdown
            .contains("`url=https://github.com/o/r/issues/119`"));
        assert!(
            question.markdown.contains("(+5)"),
            "the rest is counted, not hidden"
        );
        assert!(question.key.starts_with(QUESTION_KEY_PREFIX));
        assert_eq!(
            question.ceilings,
            vec![Ceiling::Tool {
                tool: "web_fetch".into(),
                limit: 120,
                step: 50
            }]
        );
    }

    #[test]
    fn the_fence_is_a_question_the_discussion_accepts() {
        let question = ceiling_question(&report(), "en");
        let body = question
            .markdown
            .split("```kronn-question\n")
            .nth(1)
            .and_then(|rest| rest.split("\n```").next())
            .expect("a fence");
        let fence: serde_json::Value = serde_json::from_str(body).unwrap();
        let ids: Vec<&str> = fence["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec![OPTION_GRANT, OPTION_UNLIMITED, OPTION_STOP]);
        assert_eq!(fence["key"], question.key);
        assert_eq!(
            fence["options"][0]["label"],
            "Allow 50 more `web_fetch` calls"
        );
    }

    #[test]
    fn a_round_ceiling_offers_no_unlimited_option() {
        let question = ceiling_question(
            &CeilingReport {
                version: 1,
                tools: Vec::new(),
                rounds: Some(150),
            },
            "fr",
        );
        assert!(question.markdown.contains("les 150 autorisés"));
        assert!(!question.markdown.contains(OPTION_UNLIMITED));
        assert_eq!(
            question.ceilings,
            vec![Ceiling::Rounds {
                limit: 150,
                step: ROUND_STEP
            }]
        );
    }
}
