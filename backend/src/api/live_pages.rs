use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{
    ApiErrorCode, ApiResponse, CreateLivePageDataset, CreateLivePageRequest,
    LinkLivePageDiscussionRequest, LivePage, LivePageDataset, LivePageDiscussionRelation,
    LivePageRevision, PublishLivePageRequest, UpdateLivePageHtmlRequest, UpdateLivePageRequest,
};
use crate::AppState;

const MAX_PAGE_HTML_BYTES: usize = 1_000_000;
const INVALID_SLUG_MESSAGE: &str = "Page slug must contain lowercase ASCII letters, digits and single '-' separators, and must not look like a page id";

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

#[derive(Debug, Default, Deserialize)]
pub struct ListLivePagesQuery {
    pub project_id: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListLivePagesQuery>,
) -> Json<ApiResponse<Vec<LivePage>>> {
    let project_id = query.project_id.filter(|id| !id.trim().is_empty());
    match state
        .db
        .with_read_conn(move |conn| match project_id {
            Some(project_id) => {
                crate::db::live_pages::list_live_pages_for_project(conn, &project_id)
            }
            None => crate::db::live_pages::list_live_pages(conn),
        })
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
    if let Some(slug) = request.slug.take() {
        let slug = slug.trim().to_string();
        if !valid_slug(&slug) {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                INVALID_SLUG_MESSAGE,
            ));
        }
        request.slug = Some(slug);
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
        Err(error)
            if error.is::<crate::db::live_pages::SlugUnavailable>()
                || error.to_string().contains("live_pages.slug") =>
        {
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                crate::db::live_pages::SlugUnavailable.to_string(),
            ))
        }
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
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
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
            INVALID_SLUG_MESSAGE,
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
    let mut page_for_insert = page.clone();
    let revision_for_insert = revision.clone();
    let datasets = request.datasets;
    let discussion_id = request.discussion_id;
    let source_message_id = request.source_message_id;
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            if let Some(message_id) = &source_message_id {
                let origin: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND discussion_id = ?2)",
                    rusqlite::params![message_id, discussion_id], |row| row.get(0),
                )?;
                if !origin {
                    anyhow::bail!("Source message must belong to the originating discussion");
                }
                // Promoting another preview with the same title creates a new
                // Artifact. Ordinary explicit-slug creation keeps its conflict contract.
                let base = page_for_insert.slug.clone();
                while crate::db::live_pages::slug_is_taken(&tx, &page_for_insert.slug)? {
                    page_for_insert.slug = format!("{}-{}", base.chars().take(80).collect::<String>().trim_end_matches('-'), Uuid::new_v4().simple());
                }
                if page_for_insert.project_id.is_none() {
                    page_for_insert.project_id = tx.query_row("SELECT project_id FROM discussions WHERE id = ?1",
                        [&discussion_id], |row| row.get(0))?;
                }
            }
            crate::db::live_pages::create_live_page_in_transaction(
                &tx,
                &page_for_insert,
                &revision_for_insert,
                &datasets,
                discussion_id.as_deref(),
            )?;
            if let Some(message_id) = source_message_id {
                tx.execute("UPDATE live_page_discussion_links SET source_message_id = ?1 WHERE page_id = ?2 AND discussion_id = ?3",
                    rusqlite::params![message_id, page_for_insert.id, discussion_id])?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    {
        let message = error.to_string();
        // Slugs are unique across projects: a token learns only that this one
        // is taken, never where.
        if error.is::<crate::db::live_pages::SlugUnavailable>()
            || (bridge.is_some() && message.contains("live_pages.slug"))
        {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "This Page slug is not available; choose another",
            ));
        }
        let code = if message.starts_with("Source message") {
            ApiErrorCode::Validation
        } else if message.contains("UNIQUE constraint") || message.contains("Dataset names") {
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

#[derive(Debug, Default, Deserialize)]
pub struct DeleteLivePageDatasetQuery {
    #[serde(default)]
    pub force: bool,
}

/// What reads or writes each dataset of a Page. Human only: it names
/// workflows a bridge token may not see.
pub async fn dataset_usage(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Vec<crate::models::LivePageDatasetUsage>>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Dataset usage is shown to a human only",
        ));
    }
    match state
        .db
        .with_read_conn(move |conn| crate::db::live_pages::list_live_page_dataset_usage(conn, &id))
        .await
    {
        Ok(Some(usage)) => Json(ApiResponse::ok(usage)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Page not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to read dataset usage: {error}"),
        )),
    }
}

/// Deletes a dataset nothing uses. A dataset still written, named or bound is
/// refused with its references; only a human may delete it anyway (`force`).
pub async fn delete_dataset(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path((id, name)): Path<(String, String)>,
    Query(query): Query<DeleteLivePageDatasetQuery>,
) -> Json<ApiResponse<crate::models::DeleteLivePageDatasetResult>> {
    if query.force && bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can delete a dataset that is still in use",
        ));
    }
    let force = query.force;
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::delete_live_page_dataset(conn, &id, &name, force)
        })
        .await;
    match result {
        Ok(deleted) => {
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::LivePageDataChanged {
                    page_id: deleted.page_id.clone(),
                    data_revision: deleted.data_revision,
                });
            Json(ApiResponse::ok(deleted))
        }
        Err(error) => Json(dataset_error(error, "Unable to delete dataset")),
    }
}

/// Changes a dataset's retention limits; existing points are pruned at once.
/// Human only, like `clear`: pruning destroys data.
pub async fn update_dataset(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path((id, name)): Path<(String, String)>,
    Json(request): Json<crate::models::UpdateLivePageDatasetRequest>,
) -> Json<ApiResponse<crate::models::UpdateLivePageDatasetResult>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can change a dataset's retention limits",
        ));
    }
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::live_pages::update_live_page_dataset_limits(conn, &id, &name, &request)
        })
        .await;
    match result {
        Ok(updated) => {
            if updated.points_removed > 0 {
                let _ = state
                    .ws_broadcast
                    .send(crate::models::WsMessage::LivePageDataChanged {
                        page_id: updated.dataset.page_id.clone(),
                        data_revision: updated.data_revision,
                    });
            }
            Json(ApiResponse::ok(updated))
        }
        Err(error) => Json(dataset_error(error, "Unable to update dataset")),
    }
}

fn dataset_error<T: serde::Serialize>(error: anyhow::Error, context: &str) -> ApiResponse<T> {
    let message = error.to_string();
    let code =
        if message == "Page not found" || error.is::<crate::db::live_pages::DatasetNotFound>() {
            ApiErrorCode::NotFound
        } else if error.is::<crate::db::live_pages::DatasetReferenced>() {
            ApiErrorCode::Conflict
        } else if message.contains("must be greater than zero") {
            ApiErrorCode::Validation
        } else {
            return ApiResponse::err_coded(ApiErrorCode::Internal, format!("{context}: {message}"));
        };
    ApiResponse::err_coded(code, message)
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
        Ok(publication) => {
            announce_page_data_changed(&state, &publication);
            Json(ApiResponse::ok(publication))
        }
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

/// A slug never looks like a page id: pages resolve by id or slug, and one
/// page's slug equal to another's id would make the id ambiguous.
pub(crate) fn valid_slug(slug: &str) -> bool {
    Uuid::parse_str(slug).is_err()
        && !slug.is_empty()
        && slug.len() <= 100
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--")
        && slug
            .chars()
            .all(|value| value.is_ascii_lowercase() || value.is_ascii_digit() || value == '-')
}

/// GET /api/config/embed-origins: the origins every Live Page of this Kronn
/// may embed content from (`data-kronn-embed`), normalized.
pub async fn embed_origins(State(state): State<AppState>) -> Json<ApiResponse<Vec<String>>> {
    let config = state.config.read().await;
    let current = &config.embed_allowed_origins;
    Json(ApiResponse::ok(
        crate::core::embed_origins::apply_changes(current, &[], &[]).unwrap_or_default(),
    ))
}

/// Path the Docker gateway asks, through `auth_request`, for each app
/// document's `frame-src`. Open like `/api/health`: the gateway cannot present
/// the browser's credentials, and the value is the CSP header of that document.
pub const EMBED_FRAME_SRC_PATH: &str = "/api/embed-origins/frame-src";

/// Header carrying the sources; the gateway copies it into its CSP.
pub const EMBED_FRAME_SRC_HEADER: &str = "x-kronn-frame-src";

/// GET /api/embed-origins/frame-src: 204 with `X-Kronn-Frame-Src`.
pub async fn embed_frame_src(
    State(state): State<AppState>,
) -> (
    axum::http::StatusCode,
    [(&'static str, axum::http::HeaderValue); 2],
) {
    let config = state.config.read().await;
    let sources = crate::core::embed_origins::frame_src_sources(&config.embed_allowed_origins);
    drop(config);
    let value = axum::http::HeaderValue::from_str(&sources)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("'self'"));
    (
        axum::http::StatusCode::NO_CONTENT,
        [
            (EMBED_FRAME_SRC_HEADER, value),
            (
                "cache-control",
                axum::http::HeaderValue::from_static("no-store"),
            ),
        ],
    )
}

/// Tell every open tab that the allowed sites changed: they re-read the list
/// and take down what was revoked. The event names the change, never the list.
/// Tell open views of the Page its data changed; nothing when it did not.
pub fn announce_page_data_changed(
    state: &AppState,
    publication: &crate::models::PublishLivePageResult,
) {
    if publication.content_changed || publication.points_added > 0 {
        let _ = state
            .ws_broadcast
            .send(crate::models::WsMessage::LivePageDataChanged {
                page_id: publication.page_id.clone(),
                data_revision: publication.data_revision,
            });
    }
}

pub fn announce_embed_origins_changed(state: &AppState) {
    let _ = state
        .ws_broadcast
        .send(crate::models::WsMessage::EmbedOriginsChanged);
}

fn accepts_html(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html"))
}

/// Puts the host frame policy on every response of the app's static files,
/// so the browser itself refuses to frame a site that is not allowed, even
/// after a redirect. Documents lose their validators: a reload must carry the
/// current list, never a 304 for an older one.
pub async fn document_frame_policy(
    State(state): State<AppState>,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{header, HeaderValue};
    use axum::response::IntoResponse;
    if accepts_html(request.headers()) {
        request.headers_mut().remove(header::IF_NONE_MATCH);
        request.headers_mut().remove(header::IF_MODIFIED_SINCE);
    }
    let mut response = next.run(request).await;
    let sources = {
        let config = state.config.read().await;
        crate::core::embed_origins::frame_src_sources(&config.embed_allowed_origins)
    };
    let policy = crate::core::embed_origins::policy_for_sources(&sources);
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_str(&policy).unwrap_or_else(|_| {
            HeaderValue::from_static("frame-src 'self'; child-src 'self'; worker-src 'self' blob:")
        }),
    );
    let is_html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/html"));
    if !is_html {
        return response;
    }
    headers.remove(header::ETAG);
    headers.remove(header::LAST_MODIFIED);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // The page reads the sources its CSP was served with from this marker.
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_APP_DOCUMENT_BYTES).await else {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "document too large",
        )
            .into_response();
    };
    let body = crate::core::embed_origins::inject_served_frame_src(&bytes, &sources)
        .map(axum::body::Body::from)
        .unwrap_or_else(|| axum::body::Body::from(bytes));
    parts.headers.remove(header::CONTENT_LENGTH);
    axum::response::Response::from_parts(parts, body)
}

/// Bound on an app document buffered to carry the served-sources marker.
const MAX_APP_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

/// The app's built frontend, served with the host frame policy. The desktop
/// serves its documents through this, the only place they get a CSP there.
pub fn serve_app_documents(dist_dir: &std::path::Path, state: AppState) -> axum::Router {
    axum::Router::new()
        .fallback_service(
            tower_http::services::ServeDir::new(dist_dir).append_index_html_on_directories(true),
        )
        .layer(axum::middleware::from_fn_with_state(
            state,
            document_frame_policy,
        ))
}

/// POST /api/config/embed-origins: allow and revoke origins, then persist.
/// Returns the new list.
pub async fn change_embed_origins(
    State(state): State<AppState>,
    Json(change): Json<crate::models::EmbedOriginsChange>,
) -> Json<ApiResponse<Vec<String>>> {
    let mut config = state.config.write().await;
    let next = match crate::core::embed_origins::apply_changes(
        &config.embed_allowed_origins,
        &change.add,
        &change.remove,
    ) {
        Ok(next) => next,
        Err(error) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, error)),
    };
    let previous = std::mem::replace(&mut config.embed_allowed_origins, next.clone());
    if let Err(error) = crate::core::config::save(&config).await {
        config.embed_allowed_origins = previous;
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to save allowed sites: {error}"),
        ));
    }
    drop(config);
    announce_embed_origins_changed(&state);
    Json(ApiResponse::ok(next))
}

#[cfg(test)]
#[path = "live_page_dataset_api_tests.rs"]
mod dataset_api_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_never_looks_like_a_page_id() {
        assert!(!valid_slug("0b8f5c2e-3a7d-4f1e-9c6b-2d4e8a1f7c3b"));
        assert!(!valid_slug("0b8f5c2e3a7d4f1e9c6b2d4e8a1f7c3b"));
        assert!(valid_slug("adobe-indicators"));
    }

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
