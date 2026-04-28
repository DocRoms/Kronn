//! Hydratation d'un `WorkflowStep` Agent depuis un `QuickPrompt` référencé via
//! `step.quick_prompt_id`. Mirror de `quick_api_hydrate.rs` mais pour les
//! steps Agent qui réutilisent un Quick Prompt sauvegardé.
//!
//! Sémantique : per-field override. Le step gagne quand il a une valeur
//! explicite, sinon on utilise la valeur du QuickPrompt. Permet de définir
//! un prompt canonique côté QP et de l'override ponctuellement (ex: même
//! template, agent différent pour un workflow spécifique).
//!
//! Champs hydratés depuis le QP :
//! - `prompt_template` (si vide / whitespace côté step)
//! - `agent` (toujours overridable — le step déclare un agent par défaut)
//! - `tier` via `agent_settings.tier`
//! - `skill_ids` (si vide côté step)
//!
//! Pas de variables au niveau step : les `{{var}}` du QP sont résolus avec
//! le `TemplateContext` du workflow (launch_variables / state / steps.X).
//! Si le QP utilise `{{host}}` et que le workflow ne l'expose pas, le
//! `fail_fast_on_unresolved` côté `execute_step` flagge l'erreur — pas
//! besoin d'un mécanisme dédié.

use crate::db::Database;
use crate::models::WorkflowStep;

/// Si `step.quick_prompt_id` est set, charge le QuickPrompt correspondant et
/// applique l'override per-field. Mutate `step` in place.
///
/// Retourne `Err(message)` si :
///   - Le QuickPrompt référencé est introuvable
///   - L'accès DB échoue
///
/// No-op si `quick_prompt_id` est `None`.
pub async fn hydrate_step_from_quick_prompt(
    step: &mut WorkflowStep,
    db: &Database,
) -> Result<(), String> {
    let qp_id = match step.quick_prompt_id.clone() {
        Some(id) => id,
        None => return Ok(()),
    };

    let qp_lookup = qp_id.clone();
    let qp = match db
        .with_conn(move |conn| crate::db::quick_prompts::get_quick_prompt(conn, &qp_lookup))
        .await
    {
        Ok(Some(q)) => q,
        Ok(None) => {
            return Err(format!(
                "Agent step references QuickPrompt `{}` which does not exist.",
                qp_id
            ));
        }
        Err(e) => return Err(format!("DB error loading QuickPrompt: {}", e)),
    };

    // `prompt_template` : un step a TOUJOURS un prompt_template (String,
    // pas Option). On considère qu'un prompt_template vide ou whitespace
    // signale "hérite du QP" — l'utilisateur qui veut override met du
    // texte non-vide.
    if step.prompt_template.trim().is_empty() {
        step.prompt_template = qp.prompt_template;
    }

    // `agent` : c'est aussi un champ obligatoire (pas Option). On laisse
    // toujours le step gagner, sauf si le step ne contient pas d'agent_settings
    // ET que le QP a un tier différent. Pour rester simple, on respecte le
    // step.agent toujours et on pousse seulement le tier du QP dans les
    // agent_settings si le step n'en a pas. Ça permet à l'utilisateur de
    // surcharger l'agent au niveau step (ex: le QP est calé sur Claude mais
    // ce workflow tourne avec Codex).
    //
    // Note d'override "subtile" : on pourrait imaginer écraser step.agent
    // par qp.agent quand le step est `ClaudeCode` (= default). Mais ça
    // créerait du magic invisible qu'on regretterait. Régle simple :
    // step.agent = source-of-truth, point.

    // `agent_settings` : si le step n'en a pas, on injecte un settings
    // minimal avec le `tier` du QP. Si le step en a un mais pas de tier,
    // on remplit. Sinon on respecte.
    match step.agent_settings.as_mut() {
        Some(settings) => {
            if settings.tier.is_none() {
                settings.tier = Some(qp.tier);
            }
        }
        None => {
            step.agent_settings = Some(crate::models::AgentSettings {
                model: None,
                tier: Some(qp.tier),
                reasoning_effort: None,
                max_tokens: None,
            });
        }
    }

    // `skill_ids` : merge logique = step gagne si non-vide.
    if step.skill_ids.is_empty() {
        step.skill_ids = qp.skill_ids;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AgentType, ModelTier, PromptVariable, QuickPrompt, StepMode, StepOutputFormat, StepType,
    };
    use chrono::Utc;

    async fn seed_qp(db: &Database, qp: QuickPrompt) -> String {
        let qp_clone = qp.clone();
        db.with_conn(move |conn| crate::db::quick_prompts::insert_quick_prompt(conn, &qp_clone))
            .await
            .expect("insert QP");
        qp.id
    }

    fn make_qp(id: &str, prompt: &str) -> QuickPrompt {
        QuickPrompt {
            id: id.to_string(),
            name: "Test QP".to_string(),
            icon: "P".to_string(),
            prompt_template: prompt.to_string(),
            variables: vec![PromptVariable {
                name: "host".to_string(),
                label: "Host".to_string(),
                placeholder: "fr.example.com".to_string(),
                description: None,
                required: true,
            }],
            agent: AgentType::ClaudeCode,
            project_id: None,
            skill_ids: vec!["skill-a".to_string(), "skill-b".to_string()],
            tier: ModelTier::Reasoning,
            description: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn blank_step(quick_prompt_id: Option<String>) -> WorkflowStep {
        WorkflowStep {
            name: "agent_step".to_string(),
            step_type: StepType::Agent,
            description: None,
            agent: AgentType::ClaudeCode,
            prompt_template: String::new(),
            mode: StepMode::Normal,
            output_format: StepOutputFormat::FreeText,
            mcp_config_ids: vec![],
            agent_settings: None,
            on_result: vec![],
            stall_timeout_secs: None,
            retry: None,
            delay_after_secs: None,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            batch_quick_prompt_id: None,
            batch_items_from: None,
            batch_wait_for_completion: None,
            batch_max_items: None,
            batch_workspace_mode: None,
            batch_chain_prompt_ids: vec![],
            batch_concurrent_limit: None,
            quick_api_id: None,
            quick_prompt_id,
            notify_config: None,
            api_plugin_slug: None,
            api_config_id: None,
            api_endpoint_path: None,
            api_method: None,
            api_path_params: None,
            api_query: None,
            api_headers: None,
            api_body: None,
            api_extract: None,
            api_pagination: None,
            api_timeout_ms: None,
            api_max_retries: None,
            api_output_var: None,
            gate_message: None,
            gate_request_changes_target: None,
            gate_notify_url: None,
            exec_command: None,
            exec_args: vec![],
            exec_timeout_secs: None,
            json_data_payload: None,
        }
    }

    #[tokio::test]
    async fn no_quick_prompt_id_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let mut step = blank_step(None);
        let before = step.clone();
        hydrate_step_from_quick_prompt(&mut step, &db).await.unwrap();
        assert_eq!(step.prompt_template, before.prompt_template);
        assert_eq!(step.skill_ids, before.skill_ids);
    }

    #[tokio::test]
    async fn missing_quick_prompt_returns_clear_error() {
        let db = Database::open_in_memory().unwrap();
        let mut step = blank_step(Some("nonexistent".to_string()));
        let err = hydrate_step_from_quick_prompt(&mut step, &db)
            .await
            .unwrap_err();
        assert!(err.contains("QuickPrompt"), "error mentions QP: {}", err);
        assert!(err.contains("nonexistent"), "error mentions id: {}", err);
    }

    #[tokio::test]
    async fn hydrates_empty_prompt_template_from_qp() {
        let db = Database::open_in_memory().unwrap();
        let qp_id = seed_qp(&db, make_qp("qp-1", "Audit le host {{host}}")).await;
        let mut step = blank_step(Some(qp_id));
        hydrate_step_from_quick_prompt(&mut step, &db).await.unwrap();
        assert_eq!(step.prompt_template, "Audit le host {{host}}");
        assert_eq!(step.skill_ids, vec!["skill-a", "skill-b"]);
        // tier injecté dans agent_settings (créé par le helper)
        assert!(step.agent_settings.is_some());
        assert_eq!(
            step.agent_settings.as_ref().unwrap().tier,
            Some(ModelTier::Reasoning)
        );
    }

    #[tokio::test]
    async fn step_prompt_template_wins_when_set() {
        let db = Database::open_in_memory().unwrap();
        let qp_id = seed_qp(&db, make_qp("qp-2", "QP version")).await;
        let mut step = blank_step(Some(qp_id));
        // L'utilisateur a écrit son propre prompt — il doit gagner.
        step.prompt_template = "Step override version".to_string();
        hydrate_step_from_quick_prompt(&mut step, &db).await.unwrap();
        assert_eq!(step.prompt_template, "Step override version");
    }

    #[tokio::test]
    async fn step_skill_ids_win_when_non_empty() {
        let db = Database::open_in_memory().unwrap();
        let qp_id = seed_qp(&db, make_qp("qp-3", "...")).await;
        let mut step = blank_step(Some(qp_id));
        step.skill_ids = vec!["step-skill".to_string()];
        hydrate_step_from_quick_prompt(&mut step, &db).await.unwrap();
        assert_eq!(step.skill_ids, vec!["step-skill"]);
    }

    #[tokio::test]
    async fn whitespace_only_prompt_treated_as_empty() {
        // Cohérent avec l'interprétation "vide" — un user qui colle des
        // espaces n'a pas écrit de vrai override.
        let db = Database::open_in_memory().unwrap();
        let qp_id = seed_qp(&db, make_qp("qp-ws", "Real QP prompt")).await;
        let mut step = blank_step(Some(qp_id));
        step.prompt_template = "   \n  ".to_string();
        hydrate_step_from_quick_prompt(&mut step, &db).await.unwrap();
        assert_eq!(step.prompt_template, "Real QP prompt");
    }
}
