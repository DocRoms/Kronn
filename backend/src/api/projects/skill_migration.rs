//! "Migrate everything to `.agents/skills`" (KT-903): the recap, then the move.
//! The file work is `core::skill_migration`; this layer adds what lives in
//! Kronn's database — the alignment baselines and the read-only references that
//! name a skill by path — so they follow the skill to its new place.

use std::path::{Path, PathBuf};

use axum::{
    extract::{Path as AxumPath, State},
    Json,
};
use chrono::Utc;

use crate::models::{
    ApiErrorCode, ApiResponse, ProjectRepositoryResourceKind, SkillMigrationPlan,
    SkillMigrationRequest, SkillMigrationResult,
};
use crate::AppState;

/// GET /api/projects/:id/repository-resources/skills/migration
///
/// What a migration would do, without touching the repository.
pub async fn skill_migration_plan(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
) -> Json<ApiResponse<SkillMigrationPlan>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            Ok(crate::db::projects::get_project(conn, &project_id)?
                .map(|project| crate::core::skill_migration::plan(Path::new(&project.path))))
        })
        .await;
    match result {
        Ok(Some(plan)) => Json(ApiResponse::ok(plan)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to plan the skill migration: {error}"),
        )),
    }
}

/// A skill Kronn tracks that moved: its baseline is set against the file where
/// it now stands. The Kronn side is compared with what the repository now
/// holds; equal, the two stay aligned, otherwise the baseline is dropped so the
/// difference is a conflict a human settles, never a guess.
fn rebaseline_relocated_skill(
    conn: &rusqlite::Connection,
    project_key: &str,
    root: &Path,
    entry: &crate::core::repository_resources::RepositoryLockResource,
) -> anyhow::Result<()> {
    let Some(alignment) =
        crate::db::repository_resources::find_alignment(conn, project_key, "skill", &entry.slug)?
    else {
        return Ok(());
    };
    let (_, repository_hash) = crate::core::repository_resources::read_resource(root, entry)
        .map_err(anyhow::Error::msg)?;
    let rendered = super::resources::render_database_resource(
        conn,
        ProjectRepositoryResourceKind::Skill,
        &alignment.target_id,
        &entry.slug,
        super::resources::parse_identity_time(&alignment.aligned_at),
    );
    match rendered {
        Ok(rendered) if rendered.hash == repository_hash => {
            crate::db::repository_resources::upsert_alignment(
                conn,
                project_key,
                "skill",
                &entry.slug,
                &alignment.target_id,
                &repository_hash,
                &rendered.hash,
                &alignment.aligned_at,
                alignment.imported,
            )?;
        }
        _ => crate::db::repository_resources::delete_alignment(
            conn,
            project_key,
            "skill",
            &entry.slug,
        )?,
    }
    Ok(())
}

/// A read-only reference ("Use in Kronn") that named a folder the skill was
/// moved out of now names its new place.
fn follow_references(
    conn: &rusqlite::Connection,
    project_id: &str,
    result: &SkillMigrationResult,
) -> anyhow::Result<()> {
    for moved in &result.moved {
        let Some(reference) =
            crate::db::project_skill_references::find(conn, project_id, &moved.slug)?
        else {
            continue;
        };
        if reference
            .relative_path
            .strip_prefix(&moved.source)
            .is_some_and(|rest| rest.starts_with('/'))
        {
            crate::db::project_skill_references::upsert(
                conn,
                project_id,
                &moved.slug,
                &crate::core::repository_resources::skill_path(&moved.slug),
                &reference.name,
                &Utc::now().to_rfc3339(),
            )?;
        }
    }
    Ok(())
}

fn migrate(
    conn: &rusqlite::Connection,
    project_id: &str,
    request: &SkillMigrationRequest,
) -> anyhow::Result<Option<SkillMigrationResult>> {
    let Some(project) = crate::db::projects::get_project(conn, project_id)? else {
        return Ok(None);
    };
    let root = PathBuf::from(&project.path);
    let (writable, _) = crate::core::repository_resources::can_write_repository(&root);
    if !writable {
        anyhow::bail!("the repository cannot be written to");
    }
    let project_key = crate::db::resource_identities::project_key(conn, Some(project_id))?;
    let result = crate::core::skill_migration::apply(&root, &request.resolutions)
        .map_err(anyhow::Error::msg)?;
    let managed: Vec<String> = result
        .moved
        .iter()
        .filter(|moved| moved.kronn_managed)
        .map(|moved| moved.slug.clone())
        .collect();
    let relocated = crate::core::repository_resources::relocate_skill_entries(&root, &managed)
        .map_err(anyhow::Error::msg)?;
    for entry in &relocated {
        rebaseline_relocated_skill(conn, &project_key, &root, entry)?;
    }
    follow_references(conn, project_id, &result)?;
    Ok(Some(result))
}

/// POST /api/projects/:id/repository-resources/skills/migrate
///
/// Moves the skills the recap listed into `.agents/skills`, and only those:
/// a conflict is settled by the versions the request names and skipped when it
/// names none. Nothing is committed.
pub async fn migrate_skills(
    State(state): State<AppState>,
    AxumPath(project_id): AxumPath<String>,
    Json(request): Json<SkillMigrationRequest>,
) -> Json<ApiResponse<SkillMigrationResult>> {
    let result = state
        .db
        .with_conn(move |conn| migrate(conn, &project_id, &request))
        .await;
    match result {
        Ok(Some(result)) => Json(ApiResponse::ok(result)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Project not found",
        )),
        Err(error) => Json(ApiResponse::err(format!(
            "Unable to migrate the skills: {error}"
        ))),
    }
}
