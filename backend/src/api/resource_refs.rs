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
