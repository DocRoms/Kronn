use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::models::{
    CreateLivePageDataset, LivePage, LivePageDataset, LivePageDatasetKind, LivePageDatasetPoint,
    LivePageDatasetView, LivePageDetail, LivePageDiscussionLink, LivePageDiscussionRelation,
    LivePagePublication, LivePageRevision, LivePageRunSelector, LivePageWorkflowBinding,
    LivePageWorkflowLink, LivePageWriteOperation, LivePagesCapability, PublishLivePageRequest,
    PublishLivePageResult, UpdateLivePageRequest, UpsertLivePageBindingRequest,
};

pub fn list_live_page_workflows(
    conn: &Connection,
    page_id_or_slug: &str,
) -> Result<Option<Vec<LivePageWorkflowLink>>> {
    let Some(detail) = get_live_page(conn, page_id_or_slug)? else {
        return Ok(None);
    };
    let mut links = crate::db::workflows::list_workflows(conn)?
        .into_iter()
        .filter_map(|workflow| {
            let step_names = workflow
                .steps
                .iter()
                .filter_map(|step| {
                    let publish = step.page_publish.as_ref()?;
                    (publish.page_id == detail.page.id || publish.page_id == detail.page.slug)
                        .then(|| step.name.clone())
                })
                .collect::<Vec<_>>();
            (!step_names.is_empty()).then_some(LivePageWorkflowLink {
                id: workflow.id,
                name: workflow.name,
                enabled: workflow.enabled,
                step_names,
            })
        })
        .collect::<Vec<_>>();
    links.sort_by_key(|link| link.name.to_lowercase());
    Ok(Some(links))
}

/// Return the newest successful Page publications, newest data revision first.
/// Resolving the Page first preserves the API distinction between an existing
/// Page with no refresh yet and an unknown Page id/slug.
pub fn list_live_page_publications(
    conn: &Connection,
    page_id_or_slug: &str,
    limit: usize,
) -> Result<Option<Vec<LivePagePublication>>> {
    let canonical_id: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id_or_slug],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical_id) = canonical_id else {
        return Ok(None);
    };
    let bounded_limit = i64::try_from(limit.clamp(1, 100))
        .context("Page publication limit exceeds SQLite range")?;
    let mut stmt = conn.prepare(
        "SELECT publications.id, publications.page_id, publications.data_revision,
                publications.workflow_id, workflows.name,
                publications.workflow_run_id, publications.datasets_json,
                publications.changed_datasets_json,
                publications.unchanged_datasets_json,
                publications.points_added, publications.points_removed,
                publications.published_at
           FROM live_page_publications publications
           LEFT JOIN workflows ON workflows.id = publications.workflow_id
          WHERE publications.page_id = ?1
          ORDER BY publications.data_revision DESC
          LIMIT ?2",
    )?;
    let publications = stmt
        .query_map(params![canonical_id, bounded_limit], |row| {
            let datasets_updated = parse_string_vec_sql(row.get(6)?, 6)?;
            let changed_datasets = parse_string_vec_sql(row.get(7)?, 7)?;
            let unchanged_datasets = parse_string_vec_sql(row.get(8)?, 8)?;
            let data_revision = u64::try_from(row.get::<_, i64>(2)?).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })?;
            Ok(LivePagePublication {
                id: row.get(0)?,
                page_id: row.get(1)?,
                data_revision,
                workflow_id: row.get(3)?,
                workflow_name: row.get(4)?,
                workflow_run_id: row.get(5)?,
                datasets_updated,
                content_changed: !changed_datasets.is_empty(),
                changed_datasets,
                unchanged_datasets,
                points_added: row.get(9)?,
                points_removed: row.get(10)?,
                published_at: parse_datetime_sql(row.get(11)?)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(publications))
}

pub fn create_live_page(
    conn: &Connection,
    page: &LivePage,
    revision: &LivePageRevision,
    datasets: &[CreateLivePageDataset],
    discussion_id: Option<&str>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO live_pages (
             id, project_id, title, slug, current_revision_id, data_revision,
             created_at, updated_at, last_published_at, pinned, archived
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            page.id,
            page.project_id,
            page.title,
            page.slug,
            page.current_revision_id,
            i64::try_from(page.data_revision).context("Page data revision exceeds SQLite range")?,
            page.created_at.to_rfc3339(),
            page.updated_at.to_rfc3339(),
            page.last_published_at.map(|value| value.to_rfc3339()),
            page.pinned,
            page.archived,
        ],
    )?;
    tx.execute(
        "INSERT INTO live_page_revisions (
             id, page_id, revision, html, created_by_agent, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            revision.id,
            revision.page_id,
            i64::try_from(revision.revision).context("Page revision exceeds SQLite range")?,
            revision.html,
            revision.created_by_agent,
            revision.created_at.to_rfc3339(),
        ],
    )?;
    for dataset in datasets {
        validate_dataset_name(&dataset.name)?;
        let max_points = dataset.max_points.unwrap_or(50_000);
        if max_points == 0 {
            bail!(
                "Dataset '{}' max_points must be greater than zero",
                dataset.name
            );
        }
        if dataset.max_age_days == Some(0) {
            bail!(
                "Dataset '{}' max_age_days must be greater than zero",
                dataset.name
            );
        }
        tx.execute(
            "INSERT INTO live_page_datasets (
                 id, page_id, name, kind, current_json, schema_json,
                 max_points, max_age_days, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                Uuid::new_v4().to_string(),
                page.id,
                dataset.name,
                dataset.kind.as_str(),
                if dataset.kind == LivePageDatasetKind::TimeSeries {
                    None
                } else {
                    dataset
                        .initial
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?
                },
                dataset
                    .schema
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
                max_points,
                dataset.max_age_days,
                page.created_at.to_rfc3339(),
            ],
        )?;
        if dataset.kind == LivePageDatasetKind::TimeSeries {
            let values = match dataset.initial.as_ref() {
                Some(serde_json::Value::Array(values)) => values.as_slice(),
                Some(value) => std::slice::from_ref(value),
                None => &[],
            };
            for (index, value) in values.iter().enumerate() {
                tx.execute(
                    "INSERT INTO live_page_dataset_points (
                         id, dataset_id, observed_at, payload_json, dedupe_key, created_at
                     ) SELECT ?1, id, ?2, ?3, ?4, ?2 FROM live_page_datasets
                       WHERE page_id = ?5 AND name = ?6",
                    params![
                        Uuid::new_v4().to_string(),
                        (page.created_at + chrono::Duration::milliseconds(index as i64))
                            .to_rfc3339(),
                        serde_json::to_string(value)?,
                        format!("seed:{index}"),
                        page.id,
                        dataset.name,
                    ],
                )?;
            }
        }
    }
    if let Some(discussion_id) = discussion_id {
        tx.execute(
            "INSERT INTO live_page_discussion_links (
                 page_id, discussion_id, relation, created_at
             ) VALUES (?1, ?2, 'created_from', ?3)",
            params![page.id, discussion_id, page.created_at.to_rfc3339()],
        )?;
    }
    tx.execute(
        "INSERT OR IGNORE INTO live_pages_capability (singleton, activated_at)
         VALUES (1, ?1)",
        [page.created_at.to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn list_live_pages(conn: &Connection) -> Result<Vec<LivePage>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, slug, current_revision_id, data_revision,
                created_at, updated_at, last_published_at, pinned, archived
           FROM live_pages ORDER BY pinned DESC, updated_at DESC, title COLLATE NOCASE",
    )?;
    let pages = stmt
        .query_map([], map_page)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pages)
}

pub fn update_live_page(
    conn: &Connection,
    page_id: &str,
    request: &UpdateLivePageRequest,
) -> Result<Option<LivePageDetail>> {
    let canonical_id: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical_id) = canonical_id else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE live_pages
            SET title = COALESCE(?2, title),
                pinned = COALESCE(?3, pinned),
                archived = COALESCE(?4, archived),
                updated_at = ?5
          WHERE id = ?1",
        params![
            canonical_id,
            request.title,
            request.pinned,
            request.archived,
            Utc::now().to_rfc3339(),
        ],
    )?;
    get_live_page(conn, &canonical_id)
}

pub fn delete_live_page(conn: &Connection, page_id: &str) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM live_pages WHERE id = ?1 OR slug = ?1",
        [page_id],
    )? > 0)
}

pub fn list_live_page_discussions(
    conn: &Connection,
    page_id_or_slug: &str,
) -> Result<Option<Vec<LivePageDiscussionLink>>> {
    let canonical_id: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id_or_slug],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical_id) = canonical_id else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, links.relation, d.archived
           FROM live_page_discussion_links links
           JOIN discussions d ON d.id = links.discussion_id
          WHERE links.page_id = ?1
          ORDER BY links.created_at DESC",
    )?;
    let links = stmt
        .query_map([canonical_id], |row| {
            let relation: String = row.get(2)?;
            Ok(LivePageDiscussionLink {
                discussion_id: row.get(0)?,
                title: row.get(1)?,
                relation: match relation.as_str() {
                    "created_from" => LivePageDiscussionRelation::CreatedFrom,
                    "attached" => LivePageDiscussionRelation::Attached,
                    _ => return Err(rusqlite::Error::InvalidQuery),
                },
                archived: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(links))
}

pub fn link_live_page_discussion(
    conn: &Connection,
    page_id_or_slug: &str,
    discussion_id: &str,
    relation: LivePageDiscussionRelation,
) -> Result<bool> {
    let canonical_id: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id_or_slug],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical_id) = canonical_id else {
        return Ok(false);
    };
    conn.execute(
        "INSERT INTO live_page_discussion_links (page_id, discussion_id, relation, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(page_id, discussion_id) DO UPDATE SET relation = excluded.relation",
        params![
            canonical_id,
            discussion_id,
            relation.as_str(),
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(true)
}

pub fn unlink_live_page_discussion(
    conn: &Connection,
    page_id_or_slug: &str,
    discussion_id: &str,
) -> Result<bool> {
    Ok(conn.execute(
        "DELETE FROM live_page_discussion_links
          WHERE page_id = (SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1)
            AND discussion_id = ?2",
        params![page_id_or_slug, discussion_id],
    )? > 0)
}

pub fn list_live_page_revisions(conn: &Connection, page_id: &str) -> Result<Vec<LivePageRevision>> {
    let canonical_page_id: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical_page_id) = canonical_page_id else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT id, page_id, revision, html, created_by_agent, created_at
           FROM live_page_revisions WHERE page_id = ?1 ORDER BY revision DESC",
    )?;
    let revisions = stmt
        .query_map([canonical_page_id], map_revision)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(revisions)
}

pub fn update_live_page_html(
    conn: &Connection,
    page_id: &str,
    html: &str,
    created_by_agent: Option<&str>,
) -> Result<LivePageRevision> {
    let tx = conn.unchecked_transaction()?;
    let canonical_page_id: String = tx
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("Page not found"))?;
    let next_revision: i64 = tx.query_row(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM live_page_revisions WHERE page_id = ?1",
        [&canonical_page_id],
        |row| row.get(0),
    )?;
    let now = Utc::now();
    let revision = LivePageRevision {
        id: Uuid::new_v4().to_string(),
        page_id: canonical_page_id.clone(),
        revision: u64::try_from(next_revision).context("Negative Page revision")?,
        html: html.to_string(),
        created_by_agent: created_by_agent.map(str::to_string),
        created_at: now,
    };
    tx.execute(
        "INSERT INTO live_page_revisions (id, page_id, revision, html, created_by_agent, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            revision.id,
            revision.page_id,
            next_revision,
            revision.html,
            revision.created_by_agent,
            revision.created_at.to_rfc3339(),
        ],
    )?;
    tx.execute(
        "UPDATE live_pages SET current_revision_id = ?2, updated_at = ?3 WHERE id = ?1",
        params![canonical_page_id, revision.id, now.to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(revision)
}

pub fn get_live_page(conn: &Connection, page_id: &str) -> Result<Option<LivePageDetail>> {
    let page = conn
        .query_row(
            "SELECT id, project_id, title, slug, current_revision_id, data_revision,
                    created_at, updated_at, last_published_at, pinned, archived
               FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            map_page,
        )
        .optional()?;
    let Some(page) = page else {
        return Ok(None);
    };
    let revision = conn.query_row(
        "SELECT id, page_id, revision, html, created_by_agent, created_at
           FROM live_page_revisions WHERE id = ?1",
        [&page.current_revision_id],
        map_revision,
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, page_id, name, kind, current_json, schema_json,
                max_points, max_age_days, updated_at
           FROM live_page_datasets WHERE page_id = ?1 ORDER BY name COLLATE NOCASE",
    )?;
    let datasets = stmt
        .query_map([&page.id], map_dataset)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut views = Vec::with_capacity(datasets.len());
    for dataset in datasets {
        let points = if dataset.kind == LivePageDatasetKind::TimeSeries {
            list_dataset_points(conn, &dataset.id)?
        } else {
            Vec::new()
        };
        let current_size = dataset
            .current
            .as_ref()
            .map(json_size_bytes)
            .transpose()?
            .unwrap_or_default();
        let data_size_bytes = points.iter().try_fold(current_size, |total, point| {
            total
                .checked_add(json_size_bytes(&point.payload)?)
                .ok_or_else(|| anyhow!("Page dataset size overflow"))
        })?;
        views.push(LivePageDatasetView {
            dataset,
            points,
            data_size_bytes,
        });
    }
    Ok(Some(LivePageDetail {
        page,
        revision,
        datasets: views,
    }))
}

/// Attach one dataset to an already-created Page. `create_live_page` only
/// declares datasets at Page creation; agents that forget one (or need to
/// grow a Page after the fact) had no way to unblock `publish_live_page`'s
/// "Unknown dataset" refusal without this.
///
/// Idempotent on `(page_id, name, kind)`: a repeat call with the same kind
/// is a no-op that returns the existing dataset untouched, so a workflow can
/// call this defensively before every publish. A repeat call with a
/// DIFFERENT kind is a `Conflict`-worthy error instead of silently
/// reinterpreting the dataset's semantics.
pub fn add_live_page_dataset(
    conn: &Connection,
    page_id: &str,
    dataset: &CreateLivePageDataset,
) -> Result<LivePageDataset> {
    validate_dataset_name(&dataset.name)?;
    let max_points = dataset.max_points.unwrap_or(50_000);
    if max_points == 0 {
        bail!(
            "Dataset '{}' max_points must be greater than zero",
            dataset.name
        );
    }
    if dataset.max_age_days == Some(0) {
        bail!(
            "Dataset '{}' max_age_days must be greater than zero",
            dataset.name
        );
    }

    let tx = conn.unchecked_transaction()?;
    let canonical_page_id: String = tx
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("Page not found"))?;

    let existing = tx
        .query_row(
            "SELECT id, page_id, name, kind, current_json, schema_json,
                    max_points, max_age_days, updated_at
               FROM live_page_datasets WHERE page_id = ?1 AND name = ?2",
            params![canonical_page_id, dataset.name],
            map_dataset,
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing.kind == dataset.kind {
            return Ok(existing);
        }
        bail!(
            "Dataset '{}' already exists with kind '{}', cannot redeclare as '{}'",
            dataset.name,
            existing.kind.as_str(),
            dataset.kind.as_str(),
        );
    }

    let now = Utc::now();
    let dataset_id = Uuid::new_v4().to_string();
    let current = if dataset.kind == LivePageDatasetKind::TimeSeries {
        None
    } else {
        dataset
            .initial
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?
    };
    tx.execute(
        "INSERT INTO live_page_datasets (
             id, page_id, name, kind, current_json, schema_json,
             max_points, max_age_days, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            dataset_id,
            canonical_page_id,
            dataset.name,
            dataset.kind.as_str(),
            current,
            dataset
                .schema
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            max_points,
            dataset.max_age_days,
            now.to_rfc3339(),
        ],
    )?;
    if dataset.kind == LivePageDatasetKind::TimeSeries {
        let values = match dataset.initial.as_ref() {
            Some(serde_json::Value::Array(values)) => values.as_slice(),
            Some(value) => std::slice::from_ref(value),
            None => &[],
        };
        for (index, value) in values.iter().enumerate() {
            tx.execute(
                "INSERT INTO live_page_dataset_points (
                     id, dataset_id, observed_at, payload_json, dedupe_key, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?3)",
                params![
                    Uuid::new_v4().to_string(),
                    dataset_id,
                    (now + chrono::Duration::milliseconds(index as i64)).to_rfc3339(),
                    serde_json::to_string(value)?,
                    format!("seed:{index}"),
                ],
            )?;
        }
    }
    tx.commit()?;

    Ok(LivePageDataset {
        id: dataset_id,
        page_id: canonical_page_id,
        name: dataset.name.clone(),
        kind: dataset.kind,
        current: current
            .map(|value| serde_json::from_str(&value))
            .transpose()?,
        schema: dataset.schema.clone(),
        max_points,
        max_age_days: dataset.max_age_days,
        updated_at: now,
    })
}

fn map_binding(row: &Row<'_>) -> rusqlite::Result<LivePageWorkflowBinding> {
    let selector: String = row.get(4)?;
    let allowed: String = row.get(7)?;
    // NULL column → not triggerable; a stored JSON array → the (possibly empty)
    // allowlist. Introduced by migration 155, so pre-existing rows read as NULL.
    let trigger_allowlist: Option<String> = row.get(8)?;
    Ok(LivePageWorkflowBinding {
        id: row.get(0)?,
        page_id: row.get(1)?,
        workflow_id: row.get(2)?,
        dataset: row.get(3)?,
        run_selector: LivePageRunSelector::parse(&selector).ok_or(rusqlite::Error::InvalidQuery)?,
        phase_map: parse_json_sql(row.get(5)?)?,
        meta_map: parse_json_sql(row.get(6)?)?,
        allowed_gate_steps: serde_json::from_str(&allowed)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        trigger_variable_allowlist: trigger_allowlist
            .map(|raw| serde_json::from_str(&raw))
            .transpose()
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
        created_at: parse_datetime_sql(row.get(9)?)?,
        updated_at: parse_datetime_sql(row.get(10)?)?,
    })
}

const BINDING_COLUMNS: &str = "id, page_id, workflow_id, dataset, run_selector, \
     phase_map_json, meta_map_json, allowed_gate_steps_json, \
     trigger_variable_allowlist_json, created_at, updated_at";

/// List every workflow binding attached to a Page. Returns `None` when the Page
/// itself does not exist (so the API can answer 404 vs. an empty list).
pub fn list_live_page_bindings(
    conn: &Connection,
    page_id: &str,
) -> Result<Option<Vec<LivePageWorkflowBinding>>> {
    let canonical: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical) = canonical else {
        return Ok(None);
    };
    let sql = format!(
        "SELECT {BINDING_COLUMNS} FROM live_page_workflow_bindings \
          WHERE page_id = ?1 ORDER BY dataset ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let bindings = stmt
        .query_map([canonical], map_binding)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(bindings))
}

/// Typed failure surface for [`upsert_live_page_binding`]. Replaces the previous
/// `anyhow!`/`bail!` message-text contract that the API boundary recovered by
/// substring-matching — a reword there silently downgraded the response to 500.
/// The mapping to `ApiErrorCode` at the boundary is now total and refactor-safe.
#[derive(Debug)]
pub enum UpsertBindingError {
    /// No live page matches the supplied id/slug (→ 404).
    PageNotFound,
    /// The referenced workflow does not exist (→ 404). Carries the id for the
    /// message.
    WorkflowNotFound(String),
    /// The dataset name failed validation (→ 400). Carries the reason.
    InvalidDataset(String),
    /// A DB / serialization failure (→ 500). Carries the underlying message.
    Db(String),
}

impl std::fmt::Display for UpsertBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpsertBindingError::PageNotFound => write!(f, "Page not found"),
            UpsertBindingError::WorkflowNotFound(id) => write!(f, "Workflow not found: {id}"),
            UpsertBindingError::InvalidDataset(msg) => write!(f, "{msg}"),
            UpsertBindingError::Db(msg) => write!(f, "{msg}"),
        }
    }
}

impl From<rusqlite::Error> for UpsertBindingError {
    fn from(e: rusqlite::Error) -> Self {
        UpsertBindingError::Db(e.to_string())
    }
}

impl From<serde_json::Error> for UpsertBindingError {
    fn from(e: serde_json::Error) -> Self {
        UpsertBindingError::Db(e.to_string())
    }
}

/// Create or replace the binding for `(page, dataset)`. Idempotent on that pair:
/// re-upserting the same dataset updates it in place and preserves `created_at`.
pub fn upsert_live_page_binding(
    conn: &Connection,
    page_id: &str,
    req: &UpsertLivePageBindingRequest,
) -> std::result::Result<LivePageWorkflowBinding, UpsertBindingError> {
    validate_dataset_name(&req.dataset)
        .map_err(|e| UpsertBindingError::InvalidDataset(e.to_string()))?;

    let tx = conn.unchecked_transaction()?;
    let canonical_page_id: String = tx
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(UpsertBindingError::PageNotFound)?;

    let workflow_exists: bool = tx
        .query_row(
            "SELECT 1 FROM workflows WHERE id = ?1",
            [&req.workflow_id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !workflow_exists {
        return Err(UpsertBindingError::WorkflowNotFound(
            req.workflow_id.clone(),
        ));
    }

    let selector = req.run_selector.unwrap_or(LivePageRunSelector::Latest);
    let now = Utc::now().to_rfc3339();
    let phase_map = serde_json::to_string(&req.phase_map)?;
    let meta_map = serde_json::to_string(&req.meta_map)?;
    let allowed = serde_json::to_string(&req.allowed_gate_steps)?;
    // `None` persists as SQL NULL (not triggerable); `Some(list)` as a JSON array.
    let trigger_allowlist = req
        .trigger_variable_allowlist
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    let existing_id: Option<String> = tx
        .query_row(
            "SELECT id FROM live_page_workflow_bindings WHERE page_id = ?1 AND dataset = ?2",
            params![canonical_page_id, req.dataset],
            |row| row.get(0),
        )
        .optional()?;

    let id = match existing_id {
        Some(id) => {
            tx.execute(
                "UPDATE live_page_workflow_bindings
                    SET workflow_id = ?1, run_selector = ?2, phase_map_json = ?3,
                        meta_map_json = ?4, allowed_gate_steps_json = ?5,
                        trigger_variable_allowlist_json = ?6, updated_at = ?7
                  WHERE id = ?8",
                params![
                    req.workflow_id,
                    selector.as_str(),
                    phase_map,
                    meta_map,
                    allowed,
                    trigger_allowlist,
                    now,
                    id,
                ],
            )?;
            id
        }
        None => {
            let id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO live_page_workflow_bindings (
                     id, page_id, workflow_id, dataset, run_selector,
                     phase_map_json, meta_map_json, allowed_gate_steps_json,
                     trigger_variable_allowlist_json, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
                params![
                    id,
                    canonical_page_id,
                    req.workflow_id,
                    req.dataset,
                    selector.as_str(),
                    phase_map,
                    meta_map,
                    allowed,
                    trigger_allowlist,
                    now,
                ],
            )?;
            id
        }
    };

    let sql = format!("SELECT {BINDING_COLUMNS} FROM live_page_workflow_bindings WHERE id = ?1");
    let binding = tx.query_row(&sql, [&id], map_binding)?;
    tx.commit()?;
    Ok(binding)
}

/// Fetch a single binding by `(page, dataset)`. Resolves the page by id or slug.
/// `None` when the page or the binding does not exist.
pub fn get_live_page_binding(
    conn: &Connection,
    page_id: &str,
    dataset: &str,
) -> Result<Option<LivePageWorkflowBinding>> {
    let canonical: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical) = canonical else {
        return Ok(None);
    };
    let sql = format!(
        "SELECT {BINDING_COLUMNS} FROM live_page_workflow_bindings \
          WHERE page_id = ?1 AND dataset = ?2"
    );
    Ok(conn
        .query_row(&sql, params![canonical, dataset], map_binding)
        .optional()?)
}

/// Remove the binding for `(page, dataset)`. Returns whether a row was deleted.
pub fn delete_live_page_binding(conn: &Connection, page_id: &str, dataset: &str) -> Result<bool> {
    let canonical: Option<String> = conn
        .query_row(
            "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(canonical) = canonical else {
        return Ok(false);
    };
    let affected = conn.execute(
        "DELETE FROM live_page_workflow_bindings WHERE page_id = ?1 AND dataset = ?2",
        params![canonical, dataset],
    )?;
    Ok(affected > 0)
}

pub fn pages_capability(conn: &Connection) -> Result<LivePagesCapability> {
    let activated_at: Option<String> = conn
        .query_row(
            "SELECT activated_at FROM live_pages_capability WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(LivePagesCapability {
        activated: activated_at.is_some(),
        activated_at: activated_at.map(parse_datetime).transpose()?,
    })
}

pub fn publish_live_page(
    conn: &Connection,
    page_id: &str,
    request: &PublishLivePageRequest,
) -> Result<PublishLivePageResult> {
    if request.writes.is_empty() {
        bail!("A Page publication requires at least one dataset write");
    }
    let tx = conn.unchecked_transaction()?;
    let current_revision: i64 = tx
        .query_row(
            "SELECT data_revision FROM live_pages WHERE id = ?1 OR slug = ?1",
            [page_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("Page not found"))?;
    let canonical_page_id: String = tx.query_row(
        "SELECT id FROM live_pages WHERE id = ?1 OR slug = ?1",
        [page_id],
        |row| row.get(0),
    )?;

    let published_at = Utc::now();
    let mut updated = Vec::new();
    let mut seen = HashSet::new();
    let mut dataset_changes = HashMap::<String, bool>::new();
    let mut points_added = 0u32;
    let mut points_removed = 0u32;

    for write in &request.writes {
        let dataset = tx
            .query_row(
                "SELECT id, page_id, name, kind, current_json, schema_json,
                        max_points, max_age_days, updated_at
                   FROM live_page_datasets WHERE page_id = ?1 AND name = ?2",
                params![canonical_page_id, write.dataset],
                map_dataset,
            )
            .optional()?
            .ok_or_else(|| anyhow!("Unknown dataset '{}'", write.dataset))?;
        let write_changed = match write.operation {
            LivePageWriteOperation::Replace => {
                if dataset.kind == LivePageDatasetKind::TimeSeries {
                    bail!(
                        "Dataset '{}' is time_series and cannot be replaced",
                        write.dataset
                    );
                }
                let changed = dataset.current.as_ref() != Some(&write.value);
                tx.execute(
                    "UPDATE live_page_datasets SET current_json = ?2, updated_at = ?3 WHERE id = ?1",
                    params![dataset.id, serde_json::to_string(&write.value)?, published_at.to_rfc3339()],
                )?;
                changed
            }
            LivePageWriteOperation::Append => {
                if dataset.kind != LivePageDatasetKind::TimeSeries {
                    bail!("Dataset '{}' must be time_series for append", write.dataset);
                }
                let values: Vec<&serde_json::Value> = match &write.value {
                    serde_json::Value::Array(items) => items.iter().collect(),
                    value => vec![value],
                };
                let observed_at = write.observed_at.unwrap_or(published_at);
                let added_before = points_added;
                let removed_before = points_removed;
                for (index, value) in values.into_iter().enumerate() {
                    let dedupe_key = write.dedupe_key.as_ref().map(|key| {
                        if index == 0 {
                            key.clone()
                        } else {
                            format!("{key}:{index}")
                        }
                    });
                    let changed = tx.execute(
                        "INSERT OR IGNORE INTO live_page_dataset_points (
                             id, dataset_id, observed_at, payload_json, dedupe_key,
                             workflow_run_id, created_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            Uuid::new_v4().to_string(),
                            dataset.id,
                            observed_at.to_rfc3339(),
                            serde_json::to_string(value)?,
                            dedupe_key,
                            request.workflow_run_id,
                            published_at.to_rfc3339(),
                        ],
                    )?;
                    points_added += changed as u32;
                }
                points_removed += enforce_retention(&tx, &dataset)? as u32;
                tx.execute(
                    "UPDATE live_page_datasets SET updated_at = ?2 WHERE id = ?1",
                    params![dataset.id, published_at.to_rfc3339()],
                )?;
                points_added > added_before || points_removed > removed_before
            }
            LivePageWriteOperation::Upsert => {
                if dataset.kind != LivePageDatasetKind::Collection {
                    bail!("Dataset '{}' must be collection for upsert", write.dataset);
                }
                let key_field = write
                    .key_field
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| anyhow!("Upsert on '{}' requires key_field", write.dataset))?;
                let current = dataset.current.unwrap_or_else(|| serde_json::json!([]));
                let merged = upsert_collection(current.clone(), &write.value, key_field)?;
                let changed = merged != current;
                tx.execute(
                    "UPDATE live_page_datasets SET current_json = ?2, updated_at = ?3 WHERE id = ?1",
                    params![dataset.id, serde_json::to_string(&merged)?, published_at.to_rfc3339()],
                )?;
                changed
            }
        };
        dataset_changes
            .entry(write.dataset.clone())
            .and_modify(|changed| *changed |= write_changed)
            .or_insert(write_changed);
        if seen.insert(write.dataset.clone()) {
            updated.push(write.dataset.clone());
        }
    }

    let changed_datasets = updated
        .iter()
        .filter(|dataset| dataset_changes.get(*dataset).copied().unwrap_or(false))
        .cloned()
        .collect::<Vec<_>>();
    let unchanged_datasets = updated
        .iter()
        .filter(|dataset| !dataset_changes.get(*dataset).copied().unwrap_or(false))
        .cloned()
        .collect::<Vec<_>>();
    let content_changed = !changed_datasets.is_empty();

    let data_revision = current_revision
        .checked_add(1)
        .ok_or_else(|| anyhow!("Page data revision overflow"))?;
    tx.execute(
        "UPDATE live_pages SET data_revision = ?2, updated_at = ?3, last_published_at = ?3
          WHERE id = ?1",
        params![canonical_page_id, data_revision, published_at.to_rfc3339()],
    )?;
    tx.execute(
        "INSERT INTO live_page_publications (
             id, page_id, data_revision, workflow_id, workflow_run_id,
             datasets_json, changed_datasets_json, unchanged_datasets_json,
             points_added, points_removed, published_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            Uuid::new_v4().to_string(),
            canonical_page_id,
            data_revision,
            request.workflow_id,
            request.workflow_run_id,
            serde_json::to_string(&updated)?,
            serde_json::to_string(&changed_datasets)?,
            serde_json::to_string(&unchanged_datasets)?,
            points_added,
            points_removed,
            published_at.to_rfc3339(),
        ],
    )?;
    tx.commit()?;

    Ok(PublishLivePageResult {
        page_id: canonical_page_id,
        data_revision: u64::try_from(data_revision).context("Negative Page data revision")?,
        datasets_updated: updated,
        content_changed,
        changed_datasets,
        unchanged_datasets,
        points_added,
        points_removed,
        published_at,
    })
}

fn enforce_retention(conn: &Connection, dataset: &LivePageDataset) -> Result<usize> {
    let mut removed = 0;
    if let Some(days) = dataset.max_age_days {
        removed += conn.execute(
            "DELETE FROM live_page_dataset_points
              WHERE dataset_id = ?1
                AND datetime(observed_at) < datetime('now', ?2)",
            params![dataset.id, format!("-{days} days")],
        )?;
    }
    removed += conn.execute(
        "DELETE FROM live_page_dataset_points
          WHERE id IN (
            SELECT id FROM live_page_dataset_points
             WHERE dataset_id = ?1
             ORDER BY observed_at DESC, rowid DESC
             LIMIT -1 OFFSET ?2
          )",
        params![dataset.id, dataset.max_points],
    )?;
    Ok(removed)
}

fn upsert_collection(
    current: serde_json::Value,
    incoming: &serde_json::Value,
    key_field: &str,
) -> Result<serde_json::Value> {
    let mut items = current
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("Stored collection is not a JSON array"))?;
    let incoming = match incoming {
        serde_json::Value::Array(values) => values.clone(),
        value => vec![value.clone()],
    };
    for value in incoming {
        let key = value
            .get(key_field)
            .filter(|key| !key.is_null())
            .cloned()
            .ok_or_else(|| anyhow!("Collection item is missing key_field '{key_field}'"))?;
        if let Some(index) = items
            .iter()
            .position(|item| item.get(key_field) == Some(&key))
        {
            items[index] = value;
        } else {
            items.push(value);
        }
    }
    Ok(serde_json::Value::Array(items))
}

fn list_dataset_points(conn: &Connection, dataset_id: &str) -> Result<Vec<LivePageDatasetPoint>> {
    let mut stmt = conn.prepare(
        "SELECT id, dataset_id, observed_at, payload_json, workflow_run_id
           FROM live_page_dataset_points WHERE dataset_id = ?1
          ORDER BY observed_at ASC, rowid ASC",
    )?;
    let points = stmt
        .query_map([dataset_id], |row| {
            Ok(LivePageDatasetPoint {
                id: row.get(0)?,
                dataset_id: row.get(1)?,
                observed_at: parse_datetime_sql(row.get(2)?)?,
                payload: parse_json_sql(row.get(3)?)?,
                workflow_run_id: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(points)
}

fn map_page(row: &Row<'_>) -> rusqlite::Result<LivePage> {
    Ok(LivePage {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        slug: row.get(3)?,
        current_revision_id: row.get(4)?,
        data_revision: u64::try_from(row.get::<_, i64>(5)?)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, i64::MAX))?,
        created_at: parse_datetime_sql(row.get(6)?)?,
        updated_at: parse_datetime_sql(row.get(7)?)?,
        last_published_at: row
            .get::<_, Option<String>>(8)?
            .map(parse_datetime_sql)
            .transpose()?,
        pinned: row.get(9)?,
        archived: row.get(10)?,
    })
}

fn map_revision(row: &Row<'_>) -> rusqlite::Result<LivePageRevision> {
    Ok(LivePageRevision {
        id: row.get(0)?,
        page_id: row.get(1)?,
        revision: u64::try_from(row.get::<_, i64>(2)?)
            .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(2, i64::MAX))?,
        html: row.get(3)?,
        created_by_agent: row.get(4)?,
        created_at: parse_datetime_sql(row.get(5)?)?,
    })
}

fn map_dataset(row: &Row<'_>) -> rusqlite::Result<LivePageDataset> {
    let kind: String = row.get(3)?;
    Ok(LivePageDataset {
        id: row.get(0)?,
        page_id: row.get(1)?,
        name: row.get(2)?,
        kind: match kind.as_str() {
            "snapshot" => LivePageDatasetKind::Snapshot,
            "time_series" => LivePageDatasetKind::TimeSeries,
            "collection" => LivePageDatasetKind::Collection,
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        current: row
            .get::<_, Option<String>>(4)?
            .map(parse_json_sql)
            .transpose()?,
        schema: row
            .get::<_, Option<String>>(5)?
            .map(parse_json_sql)
            .transpose()?,
        max_points: row.get(6)?,
        max_age_days: row.get(7)?,
        updated_at: parse_datetime_sql(row.get(8)?)?,
    })
}

fn validate_dataset_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 80
        || !name
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '_' || value == '-')
    {
        bail!("Dataset names must be 1-80 ASCII letters, digits, '-' or '_'");
    }
    Ok(())
}

fn parse_datetime(value: String) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(&value)
        .with_context(|| format!("Invalid datetime '{value}'"))?
        .with_timezone(&Utc))
}

fn parse_datetime_sql(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|datetime| datetime.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn parse_json_sql(value: String) -> rusqlite::Result<serde_json::Value> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn json_size_bytes(value: &serde_json::Value) -> Result<u64> {
    u64::try_from(serde_json::to_vec(value)?.len()).context("Page dataset size overflow")
}

fn parse_string_vec_sql(value: String, column: usize) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LivePageWrite, LivePageWriteOperation};

    fn test_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn fixture(conn: &Connection) -> LivePage {
        let now = Utc::now();
        let page = LivePage {
            id: "page-1".into(),
            project_id: None,
            title: "Adobe indicators".into(),
            slug: "adobe-indicators".into(),
            current_revision_id: "rev-1".into(),
            data_revision: 0,
            created_at: now,
            updated_at: now,
            last_published_at: None,
            pinned: false,
            archived: false,
        };
        let revision = LivePageRevision {
            id: "rev-1".into(),
            page_id: page.id.clone(),
            revision: 1,
            html: "<!doctype html><h1>Adobe</h1>".into(),
            created_by_agent: Some("Codex".into()),
            created_at: now,
        };
        create_live_page(
            conn,
            &page,
            &revision,
            &[
                CreateLivePageDataset {
                    name: "summary".into(),
                    kind: LivePageDatasetKind::Snapshot,
                    initial: None,
                    schema: None,
                    max_points: None,
                    max_age_days: None,
                },
                CreateLivePageDataset {
                    name: "traffic".into(),
                    kind: LivePageDatasetKind::TimeSeries,
                    initial: None,
                    schema: None,
                    max_points: Some(2),
                    max_age_days: None,
                },
                CreateLivePageDataset {
                    name: "alerts".into(),
                    kind: LivePageDatasetKind::Collection,
                    initial: None,
                    schema: None,
                    max_points: None,
                    max_age_days: None,
                },
            ],
            None,
        )
        .unwrap();
        page
    }

    #[test]
    fn workflow_links_distinguish_missing_and_unlinked_pages() {
        let conn = test_connection();
        let page = fixture(&conn);

        assert!(list_live_page_workflows(&conn, "missing-page")
            .unwrap()
            .is_none());
        assert!(list_live_page_workflows(&conn, &page.slug)
            .unwrap()
            .expect("existing page")
            .is_empty());
    }

    fn seed_workflow(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO workflows (id, name, trigger_json, steps_json, created_at, updated_at)
             VALUES (?1, ?2, '{\"type\":\"Manual\"}', '[]', ?3, ?3)",
            params![id, name, Utc::now().to_rfc3339()],
        )
        .unwrap();
    }

    #[test]
    fn workflow_binding_upsert_is_idempotent_on_page_dataset() {
        let conn = test_connection();
        let page = fixture(&conn);
        seed_workflow(&conn, "wf-1", "Pipeline");
        seed_workflow(&conn, "wf-2", "Pipeline v2");

        // Missing page → None (distinct from an existing page with no bindings).
        assert!(list_live_page_bindings(&conn, "missing-page")
            .unwrap()
            .is_none());
        assert!(list_live_page_bindings(&conn, &page.slug)
            .unwrap()
            .expect("existing page")
            .is_empty());

        let created = upsert_live_page_binding(
            &conn,
            &page.slug,
            &UpsertLivePageBindingRequest {
                workflow_id: "wf-1".into(),
                dataset: "pipeline".into(),
                run_selector: Some(LivePageRunSelector::LatestActive),
                phase_map: serde_json::json!([{ "name": "Prep", "steps": [{ "step": "a" }] }]),
                meta_map: serde_json::json!({ "ticket_from": "trigger.jira_ticket_key" }),
                allowed_gate_steps: vec!["gate_prod".into()],
                trigger_variable_allowlist: Some(vec!["besoin".into(), "type".into()]),
            },
        )
        .unwrap();
        assert_eq!(created.workflow_id, "wf-1");
        assert_eq!(created.run_selector, LivePageRunSelector::LatestActive);
        assert_eq!(created.allowed_gate_steps, vec!["gate_prod".to_string()]);
        assert!(created.phase_map.is_array());
        assert_eq!(
            created.trigger_variable_allowlist,
            Some(vec!["besoin".to_string(), "type".to_string()]),
            "trigger allowlist round-trips"
        );

        // Re-upsert the SAME (page, dataset) → updates in place, keeps created_at.
        let updated = upsert_live_page_binding(
            &conn,
            &page.id,
            &UpsertLivePageBindingRequest {
                workflow_id: "wf-2".into(),
                dataset: "pipeline".into(),
                run_selector: None,
                phase_map: serde_json::json!([]),
                meta_map: serde_json::json!({}),
                allowed_gate_steps: vec![],
                trigger_variable_allowlist: None,
            },
        )
        .unwrap();
        assert_eq!(updated.id, created.id, "same binding row reused");
        assert_eq!(updated.workflow_id, "wf-2");
        assert_eq!(updated.run_selector, LivePageRunSelector::Latest);
        assert_eq!(updated.created_at, created.created_at);
        assert_eq!(
            updated.trigger_variable_allowlist, None,
            "re-upsert clears the trigger allowlist back to non-triggerable"
        );

        let bindings = list_live_page_bindings(&conn, &page.slug).unwrap().unwrap();
        assert_eq!(bindings.len(), 1, "upsert did not create a duplicate");

        // Unknown workflow → rejected.
        let err = upsert_live_page_binding(
            &conn,
            &page.slug,
            &UpsertLivePageBindingRequest {
                workflow_id: "nope".into(),
                dataset: "other".into(),
                run_selector: None,
                phase_map: serde_json::json!([]),
                meta_map: serde_json::json!({}),
                allowed_gate_steps: vec![],
                trigger_variable_allowlist: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("Workflow not found"));

        // Delete is idempotent-ish: true once, false after.
        assert!(delete_live_page_binding(&conn, &page.slug, "pipeline").unwrap());
        assert!(!delete_live_page_binding(&conn, &page.slug, "pipeline").unwrap());
        assert!(list_live_page_bindings(&conn, &page.slug)
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn update_live_page_persists_a_renamed_title_without_changing_its_slug() {
        let conn = test_connection();
        let page = fixture(&conn);

        let updated = update_live_page(
            &conn,
            &page.id,
            &UpdateLivePageRequest {
                title: Some("Production health".into()),
                pinned: None,
                archived: None,
            },
        )
        .unwrap()
        .expect("existing Page");

        assert_eq!(updated.page.title, "Production health");
        assert_eq!(updated.page.slug, page.slug);
        assert_eq!(
            get_live_page(&conn, &page.id).unwrap().unwrap().page.title,
            "Production health"
        );
    }

    #[test]
    fn recent_publications_are_limited_and_keep_workflow_provenance() {
        let conn = test_connection();
        let page = fixture(&conn);
        conn.execute(
            "INSERT INTO workflows
             (id, name, trigger_json, steps_json, actions_json, safety_json,
              enabled, created_at, updated_at)
             VALUES ('wf-refresh', 'Adobe refresh', '{\"type\":\"Manual\"}', '[]', '[]',
                     '{}', 1, '2026-08-14T08:00:00Z', '2026-08-14T08:00:00Z')",
            [],
        )
        .unwrap();

        for revision in 1..=4 {
            publish_live_page(
                &conn,
                &page.id,
                &PublishLivePageRequest {
                    workflow_id: Some("wf-refresh".into()),
                    workflow_run_id: None,
                    writes: vec![LivePageWrite {
                        dataset: "summary".into(),
                        operation: LivePageWriteOperation::Replace,
                        value: serde_json::json!({"revision": revision}),
                        observed_at: None,
                        dedupe_key: None,
                        key_field: None,
                    }],
                },
            )
            .unwrap();
        }

        let unchanged = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: Some("wf-refresh".into()),
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "summary".into(),
                    operation: LivePageWriteOperation::Replace,
                    value: serde_json::json!({"revision": 4}),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            },
        )
        .unwrap();
        assert!(!unchanged.content_changed);
        assert!(unchanged.changed_datasets.is_empty());
        assert_eq!(unchanged.unchanged_datasets, vec!["summary"]);

        let publications = list_live_page_publications(&conn, &page.slug, 3)
            .unwrap()
            .expect("existing Page");
        assert_eq!(publications.len(), 3);
        assert_eq!(
            publications
                .iter()
                .map(|publication| publication.data_revision)
                .collect::<Vec<_>>(),
            vec![5, 4, 3]
        );
        assert!(publications
            .iter()
            .all(|publication| publication.workflow_id.as_deref() == Some("wf-refresh")));
        assert!(publications
            .iter()
            .all(|publication| publication.workflow_name.as_deref() == Some("Adobe refresh")));
        assert!(!publications[0].content_changed);
        assert_eq!(publications[0].unchanged_datasets, vec!["summary"]);
        assert!(publications[1].content_changed);
        assert_eq!(publications[1].changed_datasets, vec!["summary"]);
        assert!(list_live_page_publications(&conn, "missing-page", 3)
            .unwrap()
            .is_none());
    }

    #[test]
    fn publish_is_atomic_and_applies_all_operations() {
        let conn = test_connection();
        let page = fixture(&conn);
        let result = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![
                    LivePageWrite {
                        dataset: "summary".into(),
                        operation: LivePageWriteOperation::Replace,
                        value: serde_json::json!({"visits": 42}),
                        observed_at: None,
                        dedupe_key: None,
                        key_field: None,
                    },
                    LivePageWrite {
                        dataset: "traffic".into(),
                        operation: LivePageWriteOperation::Append,
                        value: serde_json::json!({"visits": 42}),
                        observed_at: None,
                        dedupe_key: Some("run-1".into()),
                        key_field: None,
                    },
                    LivePageWrite {
                        dataset: "alerts".into(),
                        operation: LivePageWriteOperation::Upsert,
                        value: serde_json::json!({"id":"a-1","state":"open"}),
                        observed_at: None,
                        dedupe_key: None,
                        key_field: Some("id".into()),
                    },
                ],
            },
        )
        .unwrap();
        assert_eq!(result.data_revision, 1);
        assert_eq!(result.points_added, 1);
        assert!(result.content_changed);
        assert_eq!(
            result.changed_datasets,
            vec!["summary", "traffic", "alerts"]
        );
        assert!(result.unchanged_datasets.is_empty());
        let loaded = get_live_page(&conn, &page.id).unwrap().unwrap();
        let alerts = loaded
            .datasets
            .iter()
            .find(|value| value.dataset.name == "alerts")
            .unwrap();
        let summary = loaded
            .datasets
            .iter()
            .find(|value| value.dataset.name == "summary")
            .unwrap();
        assert_eq!(
            alerts.dataset.current,
            Some(serde_json::json!([{"id":"a-1","state":"open"}]))
        );
        assert_eq!(
            summary.dataset.current,
            Some(serde_json::json!({"visits":42}))
        );
        assert_eq!(
            summary.data_size_bytes,
            u64::try_from(
                serde_json::to_vec(&serde_json::json!({"visits":42}))
                    .unwrap()
                    .len()
            )
            .unwrap()
        );
        let traffic = loaded
            .datasets
            .iter()
            .find(|value| value.dataset.name == "traffic")
            .unwrap();
        assert_eq!(
            traffic.data_size_bytes,
            u64::try_from(
                serde_json::to_vec(&serde_json::json!({"visits":42}))
                    .unwrap()
                    .len()
            )
            .unwrap()
        );

        let identical_upsert = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "alerts".into(),
                    operation: LivePageWriteOperation::Upsert,
                    value: serde_json::json!({"id":"a-1","state":"open"}),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: Some("id".into()),
                }],
            },
        )
        .unwrap();
        assert!(!identical_upsert.content_changed);
        assert_eq!(identical_upsert.unchanged_datasets, vec!["alerts"]);
    }

    #[test]
    fn failed_multi_write_rolls_back_prior_changes() {
        let conn = test_connection();
        let page = fixture(&conn);
        let error = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![
                    LivePageWrite {
                        dataset: "summary".into(),
                        operation: LivePageWriteOperation::Replace,
                        value: serde_json::json!({"visits": 99}),
                        observed_at: None,
                        dedupe_key: None,
                        key_field: None,
                    },
                    LivePageWrite {
                        dataset: "missing".into(),
                        operation: LivePageWriteOperation::Replace,
                        value: serde_json::json!({}),
                        observed_at: None,
                        dedupe_key: None,
                        key_field: None,
                    },
                ],
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unknown dataset"));
        let loaded = get_live_page(&conn, &page.id).unwrap().unwrap();
        assert_eq!(loaded.page.data_revision, 0);
        assert!(loaded
            .datasets
            .iter()
            .find(|value| value.dataset.name == "summary")
            .unwrap()
            .dataset
            .current
            .is_none());
    }

    #[test]
    fn append_dedupes_and_prunes_oldest_points() {
        let conn = test_connection();
        let page = fixture(&conn);
        for index in 0..3 {
            publish_live_page(
                &conn,
                &page.id,
                &PublishLivePageRequest {
                    workflow_id: None,
                    workflow_run_id: None,
                    writes: vec![LivePageWrite {
                        dataset: "traffic".into(),
                        operation: LivePageWriteOperation::Append,
                        value: serde_json::json!({"value":index}),
                        observed_at: Some(Utc::now() + chrono::Duration::seconds(index)),
                        dedupe_key: Some(format!("point-{index}")),
                        key_field: None,
                    }],
                },
            )
            .unwrap();
        }
        let duplicate = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "traffic".into(),
                    operation: LivePageWriteOperation::Append,
                    value: serde_json::json!({"value":2}),
                    observed_at: None,
                    dedupe_key: Some("point-2".into()),
                    key_field: None,
                }],
            },
        )
        .unwrap();
        assert_eq!(duplicate.points_added, 0);
        assert!(!duplicate.content_changed);
        assert_eq!(duplicate.unchanged_datasets, vec!["traffic"]);
        let loaded = get_live_page(&conn, &page.id).unwrap().unwrap();
        let traffic = loaded
            .datasets
            .iter()
            .find(|value| value.dataset.name == "traffic")
            .unwrap();
        assert_eq!(traffic.points.len(), 2);
        assert_eq!(traffic.points[0].payload, serde_json::json!({"value":1}));
    }

    #[test]
    fn append_batch_preserves_insertion_order_when_timestamps_match() {
        let conn = test_connection();
        let page = fixture(&conn);
        let published = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "traffic".into(),
                    operation: LivePageWriteOperation::Append,
                    value: serde_json::json!([
                        {"value": 1},
                        {"value": 2},
                        {"value": 3}
                    ]),
                    observed_at: Some(Utc::now()),
                    dedupe_key: Some("batch".into()),
                    key_field: None,
                }],
            },
        )
        .unwrap();

        assert_eq!(published.points_added, 3);
        assert_eq!(published.points_removed, 1);
        let loaded = get_live_page(&conn, &page.id).unwrap().unwrap();
        let traffic = loaded
            .datasets
            .iter()
            .find(|value| value.dataset.name == "traffic")
            .unwrap();
        assert_eq!(
            traffic
                .points
                .iter()
                .map(|point| point.payload["value"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    fn fixture_without_datasets(conn: &Connection) -> LivePage {
        let now = Utc::now();
        let page = LivePage {
            id: "page-bare".into(),
            project_id: None,
            title: "Bare page".into(),
            slug: "bare-page".into(),
            current_revision_id: "rev-bare".into(),
            data_revision: 0,
            created_at: now,
            updated_at: now,
            last_published_at: None,
            pinned: false,
            archived: false,
        };
        let revision = LivePageRevision {
            id: "rev-bare".into(),
            page_id: page.id.clone(),
            revision: 1,
            html: "<!doctype html><h1>Bare</h1>".into(),
            created_by_agent: None,
            created_at: now,
        };
        create_live_page(conn, &page, &revision, &[], None).unwrap();
        page
    }

    #[test]
    fn add_live_page_dataset_unblocks_a_publish_that_previously_failed() {
        let conn = test_connection();
        let page = fixture_without_datasets(&conn);

        let before = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "auto_reviews".into(),
                    operation: LivePageWriteOperation::Replace,
                    value: serde_json::json!({"count": 1}),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            },
        );
        assert!(before.unwrap_err().to_string().contains("Unknown dataset"));

        let created = add_live_page_dataset(
            &conn,
            &page.id,
            &CreateLivePageDataset {
                name: "auto_reviews".into(),
                kind: LivePageDatasetKind::Snapshot,
                initial: None,
                schema: None,
                max_points: None,
                max_age_days: None,
            },
        )
        .unwrap();
        assert_eq!(created.name, "auto_reviews");
        assert_eq!(created.kind, LivePageDatasetKind::Snapshot);
        assert_eq!(created.max_points, 50_000);

        let after = publish_live_page(
            &conn,
            &page.id,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "auto_reviews".into(),
                    operation: LivePageWriteOperation::Replace,
                    value: serde_json::json!({"count": 1}),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            },
        )
        .unwrap();
        assert_eq!(after.changed_datasets, vec!["auto_reviews"]);
    }

    #[test]
    fn add_live_page_dataset_is_idempotent_and_never_mutates_an_existing_dataset() {
        let conn = test_connection();
        let page = fixture(&conn);

        let repeat = add_live_page_dataset(
            &conn,
            &page.id,
            &CreateLivePageDataset {
                name: "traffic".into(),
                kind: LivePageDatasetKind::TimeSeries,
                initial: None,
                schema: None,
                // A second call carrying a different max_points must not
                // silently change the retention policy of a live dataset.
                max_points: Some(99_999),
                max_age_days: None,
            },
        )
        .unwrap();
        assert_eq!(repeat.max_points, 2, "existing dataset must stay untouched");

        let loaded = get_live_page(&conn, &page.id).unwrap().unwrap();
        assert_eq!(
            loaded
                .datasets
                .iter()
                .filter(|view| view.dataset.name == "traffic")
                .count(),
            1,
            "must not create a duplicate row"
        );
    }

    #[test]
    fn add_live_page_dataset_refuses_to_silently_reinterpret_an_existing_name() {
        let conn = test_connection();
        let page = fixture(&conn);

        let error = add_live_page_dataset(
            &conn,
            &page.id,
            &CreateLivePageDataset {
                name: "summary".into(),
                kind: LivePageDatasetKind::TimeSeries,
                initial: None,
                schema: None,
                max_points: None,
                max_age_days: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("already exists with kind 'snapshot'"));
        assert!(error.contains("'time_series'"));
    }

    #[test]
    fn add_live_page_dataset_rejects_an_unknown_page() {
        let conn = test_connection();
        let error = add_live_page_dataset(
            &conn,
            "missing-page",
            &CreateLivePageDataset {
                name: "auto_reviews".into(),
                kind: LivePageDatasetKind::Snapshot,
                initial: None,
                schema: None,
                max_points: None,
                max_age_days: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Page not found"));
    }
}
