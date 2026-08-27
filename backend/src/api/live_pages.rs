use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use uuid::Uuid;

use crate::models::{
    ApiErrorCode, ApiResponse, CreateLivePageDataset, CreateLivePageRequest,
    LinkLivePageDiscussionRequest, LivePage, LivePageDataset, LivePageDiscussionRelation,
    LivePageRevision, PublishLivePageRequest, UpdateLivePageHtmlRequest, UpdateLivePageRequest,
};
use crate::AppState;

const MAX_PAGE_HTML_BYTES: usize = 1_000_000;

pub async fn capability(
    State(state): State<AppState>,
) -> Json<ApiResponse<crate::models::LivePagesCapability>> {
    match state
        .db
        .with_read_conn(crate::db::live_pages::pages_capability)
        .await
    {
        Ok(value) => Json(ApiResponse::ok(value)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to read Pages capability: {error}"),
        )),
    }
}

pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<LivePage>>> {
    match state
        .db
        .with_read_conn(crate::db::live_pages::list_live_pages)
        .await
    {
        Ok(pages) => Json(ApiResponse::ok(pages)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list Pages: {error}"),
        )),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<crate::models::LivePageDetail>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::get_live_page(conn, &id))
        .await
    {
        Ok(Some(page)) => Json(ApiResponse::ok(page)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page: {error}"),
        )),
    }
}

pub async fn revisions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<LivePageRevision>>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_revisions(conn, &id))
        .await
    {
        Ok(revisions) if revisions.is_empty() => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Ok(revisions) => Json(ApiResponse::ok(revisions)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page revisions: {error}"),
        )),
    }
}

pub async fn workflows(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePageWorkflowLink>>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_workflows(conn, &id))
        .await
    {
        Ok(Some(workflows)) => Json(ApiResponse::ok(workflows)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page workflows: {error}"),
        )),
    }
}

pub async fn publications(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePagePublication>>> {
    match state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_pages::list_live_page_publications(conn, &id, 3)
        })
        .await
    {
        Ok(Some(publications)) => Json(ApiResponse::ok(publications)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page publications: {error}"),
        )),
    }
}

pub async fn discussions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePageDiscussionLink>>> {
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_discussions(conn, &id))
        .await
    {
        Ok(Some(discussions)) => Json(ApiResponse::ok(discussions)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page discussions: {error}"),
        )),
    }
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut request): Json<UpdateLivePageRequest>,
) -> Json<ApiResponse<crate::models::LivePageDetail>> {
    if let Some(title) = request.title.take() {
        request.title = match normalize_page_title(&title) {
            Ok(title) => Some(title),
            Err(message) => {
                return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message));
            }
        };
    }
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::update_live_page(conn, &id, &request))
        .await
    {
        Ok(Some(page)) => Json(ApiResponse::ok(page)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to update Page: {error}"),
        )),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::delete_live_page(conn, &id))
        .await
    {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to delete Page: {error}"),
        )),
    }
}

pub async fn link_discussion(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<LinkLivePageDiscussionRequest>,
) -> Json<ApiResponse<Vec<crate::models::LivePageDiscussionLink>>> {
    let discussion_id = request.discussion_id.trim().to_string();
    if discussion_id.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "discussion_id is required",
        ));
    }
    let relation = request
        .relation
        .unwrap_or(LivePageDiscussionRelation::Attached);
    let page_id = id.clone();
    match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::link_live_page_discussion(conn, &id, &discussion_id, relation)
        })
        .await
    {
        Ok(true) => match state
            .db
            .with_read_conn(move |conn| {
                crate::db::live_pages::list_live_page_discussions(conn, &page_id)
            })
            .await
        {
            Ok(Some(links)) => Json(ApiResponse::ok(links)),
            Ok(None) => Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Page not found",
            )),
            Err(error) => Json(ApiResponse::err_coded(
                ApiErrorCode::Internal,
                format!("Unable to reload Page discussions: {error}"),
            )),
        },
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) if error.to_string().contains("FOREIGN KEY constraint") => Json(
            ApiResponse::err_coded(ApiErrorCode::NotFound, "Discussion not found"),
        ),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to link Page discussion: {error}"),
        )),
    }
}

pub async fn unlink_discussion(
    State(state): State<AppState>,
    Path((id, discussion_id)): Path<(String, String)>,
) -> Json<ApiResponse<()>> {
    match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::unlink_live_page_discussion(conn, &id, &discussion_id)
        })
        .await
    {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page discussion link not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to unlink Page discussion: {error}"),
        )),
    }
}

pub async fn update_html(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateLivePageHtmlRequest>,
) -> Json<ApiResponse<LivePageRevision>> {
    if request.html.trim().is_empty() || request.html.len() > MAX_PAGE_HTML_BYTES {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Page HTML must be non-empty and at most 1 MB",
        ));
    }
    let html = request.html;
    let actor = request.created_by_agent;
    match state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::update_live_page_html(conn, &id, &html, actor.as_deref())
        })
        .await
    {
        Ok(revision) => Json(ApiResponse::ok(revision)),
        Err(error) if error.to_string().contains("Page not found") => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to update Page HTML: {error}"),
        )),
    }
}

pub async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateLivePageRequest>,
) -> Json<ApiResponse<crate::models::LivePageDetail>> {
    let title = match normalize_page_title(&request.title) {
        Ok(title) => title,
        Err(message) => {
            return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message));
        }
    };
    if request.html.trim().is_empty() || request.html.len() > MAX_PAGE_HTML_BYTES {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Page HTML must be non-empty and at most 1 MB",
        ));
    }
    let slug = request
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| slugify(&title));
    if !valid_slug(&slug) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Page slug must contain lowercase ASCII letters, digits and single '-' separators",
        ));
    }

    let now = Utc::now();
    let page_id = Uuid::new_v4().to_string();
    let revision_id = Uuid::new_v4().to_string();
    let page = LivePage {
        id: page_id.clone(),
        project_id: request.project_id,
        title,
        slug,
        current_revision_id: revision_id.clone(),
        data_revision: 0,
        created_at: now,
        updated_at: now,
        last_published_at: None,
        pinned: false,
        archived: false,
    };
    let revision = LivePageRevision {
        id: revision_id,
        page_id: page_id.clone(),
        revision: 1,
        html: request.html,
        created_by_agent: request.created_by_agent,
        created_at: now,
    };
    let page_for_insert = page.clone();
    let revision_for_insert = revision.clone();
    let datasets = request.datasets;
    let discussion_id = request.discussion_id;
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::create_live_page(
                conn,
                &page_for_insert,
                &revision_for_insert,
                &datasets,
                discussion_id.as_deref(),
            )
        })
        .await
    {
        let message = error.to_string();
        let code = if message.contains("UNIQUE constraint") || message.contains("Dataset names") {
            ApiErrorCode::Conflict
        } else {
            ApiErrorCode::Internal
        };
        return Json(ApiResponse::err_coded(
            code,
            format!("Unable to create Page: {error}"),
        ));
    }
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::get_live_page(conn, &page_id))
        .await
    {
        Ok(Some(detail)) => Json(ApiResponse::ok(detail)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            "Page was created but could not be reloaded",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Page was created but could not be reloaded: {error}"),
        )),
    }
}

pub async fn add_dataset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(dataset): Json<CreateLivePageDataset>,
) -> Json<ApiResponse<LivePageDataset>> {
    match state
        .db
        .with_conn(move |conn| crate::db::live_pages::add_live_page_dataset(conn, &id, &dataset))
        .await
    {
        Ok(dataset) => Json(ApiResponse::ok(dataset)),
        Err(error) if error.to_string().contains("Page not found") => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => {
            let message = error.to_string();
            let code = if message.contains("already exists with kind")
                || message.contains("UNIQUE constraint")
            {
                ApiErrorCode::Conflict
            } else if message.contains("Dataset names")
                || message.contains("max_points")
                || message.contains("max_age_days")
            {
                ApiErrorCode::Validation
            } else {
                ApiErrorCode::Internal
            };
            Json(ApiResponse::err_coded(
                code,
                format!("Unable to add dataset: {error}"),
            ))
        }
    }
}

pub async fn publish(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<PublishLivePageRequest>,
) -> Json<ApiResponse<crate::models::PublishLivePageResult>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::live_pages::publish_live_page(conn, &id, &request))
        .await;
    match result {
        Ok(publication) => Json(ApiResponse::ok(publication)),
        Err(error) => {
            let message = error.to_string();
            let code = if message == "Page not found" {
                ApiErrorCode::NotFound
            } else if message.contains("requires")
                || message.contains("Unknown dataset")
                || message.contains("must be")
                || message.contains("cannot be")
                || message.contains("missing key_field")
            {
                ApiErrorCode::Validation
            } else {
                ApiErrorCode::Internal
            };
            Json(ApiResponse::err_coded(code, message))
        }
    }
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for value in title.to_lowercase().chars() {
        if value.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(value);
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        format!("page-{}", &Uuid::new_v4().simple().to_string()[..8])
    } else {
        slug
    }
}

fn normalize_page_title(title: &str) -> Result<String, &'static str> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        Err("Page title must contain 1-200 characters")
    } else {
        Ok(title.to_string())
    }
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 100
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
        && slug
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_is_stable_and_url_safe() {
        assert_eq!(
            slugify("Adobe — Tests & Indicateurs"),
            "adobe-tests-indicateurs"
        );
        assert!(valid_slug("adobe-tests-indicateurs"));
        assert!(!valid_slug("Adobe--tests"));
    }

    #[test]
    fn page_title_is_trimmed_and_limited_to_two_hundred_characters() {
        assert_eq!(
            normalize_page_title("  Production health  ").unwrap(),
            "Production health"
        );
        assert!(normalize_page_title("   ").is_err());
        assert!(normalize_page_title(&"é".repeat(200)).is_ok());
        assert!(normalize_page_title(&"é".repeat(201)).is_err());
    }
}
