//! The workflows shipped in `docs/examples/` import through the real route,
//! so a model change that breaks them fails here rather than for a reader.

use super::*;

const EXAMPLES: &[(&str, &str)] = &[
    (
        "design-system-first-visual.cloud.json",
        include_str!("../../../docs/examples/design-system-first-visual.cloud.json"),
    ),
    (
        "design-system-first-visual.local.json",
        include_str!("../../../docs/examples/design-system-first-visual.local.json"),
    ),
    (
        "design-system-first-visual.hybrid.json",
        include_str!("../../../docs/examples/design-system-first-visual.hybrid.json"),
    ),
];

fn state() -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("db"));
    let config = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::core::config::default_config(),
    ));
    AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

#[tokio::test]
async fn docs_example_workflows_import() {
    let state = state();
    for (file, content) in EXAMPLES {
        let envelope = parse_import_envelope(content).unwrap_or_else(|e| panic!("{file}: {e}"));
        validate_workflow_for_import(&envelope.workflow).unwrap_or_else(|e| panic!("{file}: {e}"));
        let kinds: Vec<StepType> = envelope
            .workflow
            .steps
            .iter()
            .map(|step| step.step_type.clone())
            .collect();
        assert_eq!(kinds, vec![StepType::Agent; 4], "{file}");

        let Json(imported) = import_workflow(
            State(state.clone()),
            None,
            None,
            Json(ImportWorkflowRequest {
                skill_choices: vec![],
                content: (*content).to_string(),
                project_id: None,
            }),
        )
        .await;
        assert!(imported.success, "{file}: {:?}", imported.error);
        let imported = imported.data.expect("imported workflow");
        assert_eq!(imported.steps.len(), 4, "{file}");
        assert!(!imported.enabled, "{file}: an import is never armed");
    }
}
