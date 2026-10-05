//! `GET /api/resources/resolve?ref=<kind>:<slug>&project=<id>` (KT-917): the
//! local id a symbolic reference names, for a script resolving one mid-run.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::{
    models::{ApiErrorCode, ApiResponse},
    AppState,
};

#[derive(Debug, Deserialize)]
pub struct ResolveReferenceQuery {
    /// `<kind>:<slug>`, optionally written `ref:<kind>:<slug>`.
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedReference {
    pub reference: String,
    pub kind: String,
    pub slug: String,
    pub id: String,
}

pub async fn resolve(
    State(state): State<AppState>,
    Query(query): Query<ResolveReferenceQuery>,
) -> Json<ApiResponse<ResolvedReference>> {
    let raw = query.reference.trim();
    let prefixed = if raw.starts_with(crate::core::resource_refs::REF_PREFIX) {
        raw.to_string()
    } else {
        format!("{}{raw}", crate::core::resource_refs::REF_PREFIX)
    };
    let Some((kind, slug)) = crate::core::resource_refs::parse_reference(&prefixed) else {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!(
                "`{raw}` is not a reference: write `<kind>:<slug>` with kind one of {}",
                crate::db::resource_identities::REFERENCE_KINDS.join(", ")
            ),
        ));
    };
    let (kind, slug) = (kind.to_string(), slug.to_string());
    let project = query.project.filter(|project| !project.trim().is_empty());
    let lookup = format!("{kind}:{slug}");
    let outcome = state
        .db
        .with_read_conn(move |conn| {
            crate::db::resource_identities::resolve_symbolic_reference(
                conn,
                &lookup,
                project.as_deref(),
            )
        })
        .await;
    match outcome {
        Ok(Some(id)) => Json(ApiResponse::ok(ResolvedReference {
            reference: format!("{kind}:{slug}"),
            kind,
            slug,
            id,
        })),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            format!("Unknown resource reference `{kind}:{slug}`"),
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            error.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::resource_identities::tests::{
        seed_api, seed_exec, seed_page, seed_plugin, seed_prompt, seed_workflow,
    };

    fn test_state() -> AppState {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn call(state: &AppState, reference: &str) -> ApiResponse<ResolvedReference> {
        resolve(
            State(state.clone()),
            Query(ResolveReferenceQuery {
                reference: reference.into(),
                project: None,
            }),
        )
        .await
        .0
    }

    #[tokio::test]
    async fn the_resolution_route_answers_for_every_kind() {
        let state = test_state();
        state
            .db
            .with_conn(|conn| {
                seed_workflow(conn, "wf-1", "Triage", None);
                seed_prompt(conn, "qp-1", "Review PR", None);
                seed_api(conn, "qa-1", "Fetch Issues", None);
                seed_exec(conn, "qe-1", "Lint", None);
                seed_page(conn, "page-1", "Suivi équipe", "suivi-equipe", None);
                seed_plugin(conn, "cfg-gh", "github");
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        for (reference, expected) in [
            ("workflow:triage", "wf-1"),
            ("prompt:review-pr", "qp-1"),
            ("qa:fetch-issues", "qa-1"),
            ("qe:lint", "qe-1"),
            ("artifact:suivi-equipe", "page-1"),
            ("plugin:github", "cfg-gh"),
            ("skill:rust", "rust"),
            ("ref:workflow:triage", "wf-1"),
        ] {
            let response = call(&state, reference).await;
            let resolved = response
                .data
                .unwrap_or_else(|| panic!("{reference}: {:?}", response.error));
            assert_eq!(resolved.id, expected, "{reference}");
        }
        let unknown = call(&state, "workflow:nope").await;
        assert!(unknown.data.is_none());
        assert!(unknown.error.unwrap().contains("workflow:nope"));
        let malformed = call(&state, "nope").await;
        assert!(malformed.error.unwrap().contains("not a reference"));
    }
}
