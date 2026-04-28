//! Hydratation d'un `WorkflowStep` depuis une `QuickApi` référencée via
//! `step.quick_api_id`. Utilisé par `batch_apicall_step.rs` (existant) et
//! par `api_call_executor.rs` (single ApiCall, 0.7+).
//!
//! Sémantique : per-field override. Le step gagne quand il a une valeur
//! explicite, sinon on utilise la valeur du QuickApi. Permet à l'utilisateur
//! de définir un appel canonique côté QuickApi et de l'override ponctuellement
//! au niveau workflow (ex: même endpoint, extract path différent).
//!
//! Le helper accepte un `&Database` plutôt qu'un `&AppState` complet — la
//! seule dépendance runtime est l'accès aux QuickApis. Permet aux tests
//! d'instancier juste un Db en mémoire sans monter un AppState.

use crate::db::Database;
use crate::models::WorkflowStep;

/// Tag pour le diagnostic d'erreur — quel step type a déclenché l'hydratation.
fn step_kind_label(step: &WorkflowStep) -> &'static str {
    match step.step_type {
        crate::models::StepType::BatchApiCall => "BatchApiCall",
        crate::models::StepType::ApiCall => "ApiCall",
        _ => "ApiCall-like",
    }
}

/// Si `step.quick_api_id` est set, charge le QuickApi correspondant et applique
/// l'override per-field (step wins). Mutate `step` in place.
///
/// Retourne `Err(message)` si :
///   - Le QuickApi référencé est introuvable
///   - L'accès DB échoue
///
/// No-op si `quick_api_id` est `None`.
pub async fn hydrate_step_from_quick_api(
    step: &mut WorkflowStep,
    db: &Database,
) -> Result<(), String> {
    let qa_id = match step.quick_api_id.clone() {
        Some(id) => id,
        None => return Ok(()),
    };
    let kind = step_kind_label(step);

    let qa_lookup = qa_id.clone();
    let qa = match db
        .with_conn(move |conn| crate::db::quick_apis::get_quick_api(conn, &qa_lookup))
        .await
    {
        Ok(Some(q)) => q,
        Ok(None) => {
            return Err(format!(
                "{} step references QuickApi `{}` which does not exist.",
                kind, qa_id
            ));
        }
        Err(e) => return Err(format!("DB error loading QuickApi: {}", e)),
    };

    // Per-field override : la valeur du step gagne si présente, sinon fallback
    // sur la valeur du QuickApi. Aligne batch + single sur la même règle.
    if step.api_plugin_slug.is_none() {
        step.api_plugin_slug = Some(qa.api_plugin_slug);
    }
    if step.api_config_id.is_none() {
        step.api_config_id = Some(qa.api_config_id);
    }
    if step.api_endpoint_path.is_none() {
        step.api_endpoint_path = Some(qa.api_endpoint_path);
    }
    if step.api_method.is_none() {
        step.api_method = qa.api_method;
    }
    if step.api_query.is_none() {
        step.api_query = qa.api_query;
    }
    if step.api_path_params.is_none() {
        step.api_path_params = qa.api_path_params;
    }
    if step.api_headers.is_none() {
        step.api_headers = qa.api_headers;
    }
    if step.api_body.is_none() {
        step.api_body = qa.api_body;
    }
    if step.api_extract.is_none() {
        step.api_extract = qa.api_extract;
    }
    if step.api_pagination.is_none() {
        step.api_pagination = qa.api_pagination;
    }
    if step.api_timeout_ms.is_none() {
        step.api_timeout_ms = qa.api_timeout_ms;
    }
    if step.api_max_retries.is_none() {
        step.api_max_retries = qa.api_max_retries;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AgentType, ExtractSpec, QuickApi, StepMode, StepOutputFormat, StepType, WorkflowStep,
    };
    use chrono::Utc;

    /// Construit un WorkflowStep par défaut (tous les champs optionnels à
    /// None / vec![]). `WorkflowStep` n'implémente pas `Default`, on
    /// reproduit donc le pattern utilisé par notify_step::tests::make_step.
    fn blank_step(name: &str, step_type: StepType) -> WorkflowStep {
        WorkflowStep {
            name: name.to_string(),
            step_type,
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
            quick_prompt_id: None,
            json_data_payload: None,
        }
    }

    /// Insère un QuickApi minimal dans le Db et retourne son id.
    async fn seed_qa(db: &Database, qa: QuickApi) -> String {
        let qa_clone = qa.clone();
        db.with_conn(move |conn| crate::db::quick_apis::insert_quick_api(conn, &qa_clone))
            .await
            .expect("insert QA");
        qa.id
    }

    fn make_qa(id: &str) -> QuickApi {
        QuickApi {
            id: id.to_string(),
            name: "Test QA".to_string(),
            icon: "P".to_string(),
            description: String::new(),
            project_id: None,
            api_plugin_slug: "test-plugin".to_string(),
            api_config_id: "cfg-1".to_string(),
            api_endpoint_path: "/v1/items".to_string(),
            api_method: Some("GET".to_string()),
            api_query: None,
            api_path_params: None,
            api_headers: None,
            api_body: None,
            api_extract: Some(ExtractSpec {
                path: "$.items[*]".to_string(),
                fallback: None,
                fail_on_empty: false,
            }),
            api_pagination: None,
            api_timeout_ms: Some(5000),
            api_max_retries: Some(3),
            variables: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn empty_step(quick_api_id: Option<String>) -> WorkflowStep {
        let mut s = blank_step("test_step", StepType::ApiCall);
        s.quick_api_id = quick_api_id;
        s
    }

    #[tokio::test]
    async fn no_quick_api_id_is_noop() {
        let db = Database::open_in_memory().unwrap();
        let mut step = empty_step(None);
        let before = step.clone();
        hydrate_step_from_quick_api(&mut step, &db).await.unwrap();
        assert_eq!(step.api_plugin_slug, before.api_plugin_slug);
        assert_eq!(step.api_endpoint_path, before.api_endpoint_path);
    }

    #[tokio::test]
    async fn missing_quick_api_returns_clear_error() {
        let db = Database::open_in_memory().unwrap();
        let mut step = empty_step(Some("nonexistent-id".to_string()));
        let err = hydrate_step_from_quick_api(&mut step, &db)
            .await
            .unwrap_err();
        assert!(err.contains("QuickApi"), "error mentions QuickApi: {}", err);
        assert!(err.contains("nonexistent-id"), "error mentions id: {}", err);
        assert!(err.contains("ApiCall"), "error mentions step kind: {}", err);
    }

    #[tokio::test]
    async fn hydrates_missing_fields_from_quick_api() {
        let db = Database::open_in_memory().unwrap();
        let qa_id = seed_qa(&db, make_qa("qa-hydrate-1")).await;
        let mut step = empty_step(Some(qa_id));
        hydrate_step_from_quick_api(&mut step, &db).await.unwrap();
        assert_eq!(step.api_plugin_slug.as_deref(), Some("test-plugin"));
        assert_eq!(step.api_config_id.as_deref(), Some("cfg-1"));
        assert_eq!(step.api_endpoint_path.as_deref(), Some("/v1/items"));
        assert_eq!(step.api_method.as_deref(), Some("GET"));
        assert_eq!(step.api_timeout_ms, Some(5000));
        assert_eq!(step.api_max_retries, Some(3));
        assert!(step.api_extract.is_some());
    }

    #[tokio::test]
    async fn step_overrides_win_per_field() {
        let db = Database::open_in_memory().unwrap();
        let qa_id = seed_qa(&db, make_qa("qa-hydrate-2")).await;
        let mut step = blank_step("override_step", StepType::ApiCall);
        step.quick_api_id = Some(qa_id);
        // Override : le step déclare son propre endpoint + timeout. Le
        // plugin_slug / config_id viennent du QA (non-overridden).
        step.api_endpoint_path = Some("/step/override-path".to_string());
        step.api_timeout_ms = Some(9999);
        hydrate_step_from_quick_api(&mut step, &db).await.unwrap();
        assert_eq!(
            step.api_endpoint_path.as_deref(),
            Some("/step/override-path"),
            "step endpoint wins"
        );
        assert_eq!(step.api_timeout_ms, Some(9999), "step timeout wins");
        assert_eq!(
            step.api_plugin_slug.as_deref(),
            Some("test-plugin"),
            "QA plugin used (no step override)"
        );
        assert_eq!(
            step.api_config_id.as_deref(),
            Some("cfg-1"),
            "QA config used"
        );
    }

    #[tokio::test]
    async fn hydration_label_reflects_step_type() {
        let db = Database::open_in_memory().unwrap();
        let mut step = blank_step("batch_step", StepType::BatchApiCall);
        step.quick_api_id = Some("missing".to_string());
        let err = hydrate_step_from_quick_api(&mut step, &db)
            .await
            .unwrap_err();
        assert!(
            err.contains("BatchApiCall"),
            "label adapts to step type: {}",
            err
        );
    }
}
