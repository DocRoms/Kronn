use axum::{
    extract::{Path, State},
    Json,
};

use crate::{
    db::id_resolver::{AmbiguousKronnId, ResolvedId},
    models::{ApiErrorCode, ApiResponse},
    AppState,
};

fn compact_summary(parts: &[&str]) -> Option<String> {
    let compact = parts
        .iter()
        .filter(|part| !part.trim().is_empty())
        .flat_map(|part| part.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    (!compact.is_empty()).then_some(compact.chars().take(240).collect())
}

fn agent_library_matches(id: &str, unlocked_profiles: &[String]) -> Vec<ResolvedId> {
    let mut matches = Vec::new();
    if let Some(skill) = crate::core::skills::get_skill(id) {
        let origin = if skill.is_builtin {
            "builtin skill"
        } else {
            "custom skill"
        };
        matches.push(ResolvedId {
            kind: "skill".into(),
            id: skill.id,
            reference: None,
            title: skill.name,
            summary: compact_summary(&[origin, "·", &skill.description]),
            parent: None,
            suggested_tool: Some("skill_get".into()),
        });
    }
    let profile_visible = !crate::core::profiles::is_secret_profile(id)
        || unlocked_profiles.iter().any(|unlocked| unlocked == id);
    if profile_visible {
        if let Some(profile) = crate::core::profiles::get_profile(id) {
            let origin = if profile.is_builtin {
                "builtin profile"
            } else {
                "custom profile"
            };
            matches.push(ResolvedId {
                kind: "profile".into(),
                id: profile.id,
                reference: None,
                title: profile.name,
                summary: compact_summary(&[origin, "·", &profile.role]),
                parent: None,
                suggested_tool: Some("profile_get".into()),
            });
        }
    }
    if let Some(directive) = crate::core::directives::get_directive(id) {
        let origin = if directive.is_builtin {
            "builtin directive"
        } else {
            "custom directive"
        };
        matches.push(ResolvedId {
            kind: "directive".into(),
            id: directive.id,
            reference: None,
            title: directive.name,
            summary: compact_summary(&[origin, "·", &directive.description]),
            parent: None,
            suggested_tool: Some("directive_get".into()),
        });
    }
    matches
}

fn resolver_error(error: anyhow::Error) -> Json<ApiResponse<ResolvedId>> {
    let error_code = if error.downcast_ref::<AmbiguousKronnId>().is_some() {
        ApiErrorCode::Conflict
    } else {
        ApiErrorCode::Internal
    };
    Json(ApiResponse::err_coded(error_code, error.to_string()))
}

/// Resolve an opaque Kronn object ID without making the caller probe every
/// object-specific endpoint.
pub async fn resolve(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<ResolvedId>> {
    let query_id = id.clone();
    match state
        .db
        .with_read_conn(move |connection| crate::db::id_resolver::resolve_id(connection, &query_id))
        .await
    {
        Ok(database_match) => {
            let unlocked_profiles = state.config.read().await.unlocked_profiles.clone();
            let mut matches = database_match.into_iter().collect::<Vec<_>>();
            matches.extend(agent_library_matches(&id, &unlocked_profiles));
            match crate::db::id_resolver::select_unique(matches) {
                Ok(Some(resolved)) => Json(ApiResponse::ok(resolved)),
                Ok(None) => Json(ApiResponse::err_coded(
                    ApiErrorCode::NotFound,
                    "Kronn object not found",
                )),
                Err(error) => resolver_error(error),
            }
        }
        Err(error) => resolver_error(error),
    }
}
