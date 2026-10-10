use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::models::{
    CreateLivePageDataset, LivePage, LivePageDataset, LivePageDatasetKind, LivePageDatasetPoint,
    LivePageDatasetView, LivePageDetail, LivePageDiscussionLink, LivePageDiscussionRelation,
    LivePagePublication, LivePageRevision, LivePageWorkflowLink, LivePageWriteOperation,
    LivePagesCapability, PublishLivePageRequest, PublishLivePageResult, UpdateLivePageRequest,
};

pub fn list_live_page_workflows(
    conn: &Connection,
    page_id_or_slug: &str,
) -> Result<Option<Vec<LivePageWorkflowLink>>> {
    let Some(detail) = get_live_page(conn, page_id_or_slug)? else {
        return Ok(None);
    };
    let aliases: HashSet<String> = list_live_page_slug_aliases(conn, &detail.page.id)?
        .into_iter()
        .collect();
    let mut links = crate::db::workflows::list_workflows(conn)?
        .into_iter()
        .filter_map(|workflow| {
            let step_names = workflow
                .steps
                .iter()
                .filter_map(|step| {
                    let publish = step.page_publish.as_ref()?;
                    (publish.page_id == detail.page.id
                        || publish.page_id == detail.page.slug
                        || aliases.contains(&publish.page_id))
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
    let canonical_id = resolve_live_page_id(conn, page_id_or_slug)?;
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
    create_live_page_in_transaction(&tx, page, revision, datasets, discussion_id)?;
    tx.commit()?;
    Ok(())
}

/// Add a complete Page to a caller-owned transaction. The caller commits the
/// whole bundle; this helper must never make part of an import durable alone.
pub(crate) fn create_live_page_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    page: &LivePage,
    revision: &LivePageRevision,
    datasets: &[CreateLivePageDataset],
    discussion_id: Option<&str>,
) -> Result<()> {
    // A renamed page's old slug is not free: taking it would capture its links.
    let retired: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_page_slug_aliases WHERE slug = ?1)",
        [&page.slug],
        |row| row.get(0),
    )?;
    if retired {
        return Err(SlugUnavailable.into());
    }
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
    crate::db::live_page_actions::ingest_page_actions(tx, &page.id, &revision.id, &revision.html)?;
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

pub fn list_live_pages_for_project(conn: &Connection, project_id: &str) -> Result<Vec<LivePage>> {
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, slug, current_revision_id, data_revision,
                created_at, updated_at, last_published_at, pinned, archived
           FROM live_pages WHERE project_id = ?1
           ORDER BY pinned DESC, updated_at DESC, title COLLATE NOCASE",
    )?;
    let pages = stmt
        .query_map([project_id], map_page)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pages)
}

pub fn update_live_page(
    conn: &Connection,
    page_id: &str,
    request: &UpdateLivePageRequest,
) -> Result<Option<LivePageDetail>> {
    let tx = conn.unchecked_transaction()?;
    let Some(canonical_id) = resolve_live_page_id(&tx, page_id)? else {
        return Ok(None);
    };
    if let Some(slug) = request.slug.as_deref() {
        rename_live_page_slug(&tx, &canonical_id, slug)?;
    }
    tx.execute(
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
    tx.commit()?;
    get_live_page(conn, &canonical_id)
}

/// Refusal for a slug another page holds, live or retired. It never says
/// which page, since that page may sit in a project the caller cannot see.
#[derive(Debug)]
pub struct SlugUnavailable;

impl std::fmt::Display for SlugUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("This Page slug is not available; choose another")
    }
}

impl std::error::Error for SlugUnavailable {}

/// The page an id, slug or retired slug names. An id wins over a slug and a
/// live slug over a retired one, so an alias can never shadow a page.
pub fn resolve_live_page_id(conn: &Connection, page_ref: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM (
                 SELECT id, 0 AS rank FROM live_pages WHERE id = ?1
                 UNION ALL SELECT id, 1 FROM live_pages WHERE slug = ?1
                 UNION ALL SELECT page_id, 2 FROM live_page_slug_aliases WHERE slug = ?1
             ) ORDER BY rank LIMIT 1",
            [page_ref],
            |row| row.get(0),
        )
        .optional()?)
}

/// Whether a new page may not take `slug`: a live slug, or one a renamed page
/// keeps for its old links.
pub fn slug_is_taken(conn: &Connection, slug: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_pages WHERE slug = ?1)
             OR EXISTS(SELECT 1 FROM live_page_slug_aliases WHERE slug = ?1)",
        [slug],
        |row| row.get(0),
    )?)
}

/// The slugs a page answered to before it was renamed.
pub fn list_live_page_slug_aliases(conn: &Connection, page_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT slug FROM live_page_slug_aliases WHERE page_id = ?1 ORDER BY created_at, slug",
    )?;
    let slugs = stmt
        .query_map([page_id], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(slugs)
}

/// Give a page a new, already validated slug. The old one becomes an alias, and
/// the page may take back one of its own aliases but never another page's.
fn rename_live_page_slug(conn: &Connection, page_id: &str, slug: &str) -> Result<()> {
    let current: String = conn.query_row(
        "SELECT slug FROM live_pages WHERE id = ?1",
        [page_id],
        |row| row.get(0),
    )?;
    if current == slug {
        return Ok(());
    }
    let alias_owner: Option<String> = conn
        .query_row(
            "SELECT page_id FROM live_page_slug_aliases WHERE slug = ?1",
            [slug],
            |row| row.get(0),
        )
        .optional()?;
    let live_elsewhere: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_pages WHERE slug = ?1 AND id != ?2)",
        params![slug, page_id],
        |row| row.get(0),
    )?;
    if live_elsewhere || alias_owner.as_deref().is_some_and(|owner| owner != page_id) {
        return Err(SlugUnavailable.into());
    }
    let now = Utc::now().to_rfc3339();
    conn.execute("DELETE FROM live_page_slug_aliases WHERE slug = ?1", [slug])?;
    conn.execute(
        "UPDATE live_pages SET slug = ?2 WHERE id = ?1",
        params![page_id, slug],
    )?;
    conn.execute(
        "INSERT INTO live_page_slug_aliases (slug, page_id, created_at) VALUES (?1, ?2, ?3)",
        params![current, page_id, now],
    )?;
    Ok(())
}

pub fn delete_live_page(conn: &Connection, page_id: &str) -> Result<bool> {
    let Some(canonical_id) = resolve_live_page_id(conn, page_id)? else {
        return Ok(false);
    };
    Ok(conn.execute("DELETE FROM live_pages WHERE id = ?1", [canonical_id])? > 0)
}

pub fn list_live_page_discussions(
    conn: &Connection,
    page_id_or_slug: &str,
) -> Result<Option<Vec<LivePageDiscussionLink>>> {
    let canonical_id = resolve_live_page_id(conn, page_id_or_slug)?;
    let Some(canonical_id) = canonical_id else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT d.id, d.title, links.relation, d.archived, links.source_message_id
           FROM live_page_discussion_links links
           JOIN discussions d ON d.id = links.discussion_id
          WHERE links.page_id = ?1
          ORDER BY links.created_at DESC",
    )?;
    let links = stmt
        .query_map([canonical_id], |row| {
            let relation: String = row.get(2)?;
            Ok(LivePageDiscussionLink {
                source_message_id: row.get(4)?,
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
    let canonical_id = resolve_live_page_id(conn, page_id_or_slug)?;
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
    let Some(canonical_id) = resolve_live_page_id(conn, page_id_or_slug)? else {
        return Ok(false);
    };
    Ok(conn.execute(
        "DELETE FROM live_page_discussion_links WHERE page_id = ?1 AND discussion_id = ?2",
        params![canonical_id, discussion_id],
    )? > 0)
}

pub fn list_live_page_revisions(conn: &Connection, page_id: &str) -> Result<Vec<LivePageRevision>> {
    let canonical_page_id = resolve_live_page_id(conn, page_id)?;
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
    let canonical_page_id =
        resolve_live_page_id(&tx, page_id)?.ok_or_else(|| anyhow!("Page not found"))?;
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
    crate::db::live_page_actions::ingest_page_actions(
        &tx,
        &canonical_page_id,
        &revision.id,
        &revision.html,
    )?;
    tx.commit()?;
    Ok(revision)
}

/// The page row alone, by id or slug: `get_live_page` without the revision,
/// the datasets and their points, for a caller that needs the page's own facts.
pub fn get_live_page_summary(conn: &Connection, page_id: &str) -> Result<Option<LivePage>> {
    let Some(canonical_id) = resolve_live_page_id(conn, page_id)? else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id, project_id, title, slug, current_revision_id, data_revision,
                    created_at, updated_at, last_published_at, pinned, archived
               FROM live_pages WHERE id = ?1",
            [canonical_id],
            map_page,
        )
        .optional()?)
}

pub fn get_live_page(conn: &Connection, page_id: &str) -> Result<Option<LivePageDetail>> {
    let page = get_live_page_summary(conn, page_id)?;
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
    let slug_aliases = list_live_page_slug_aliases(conn, &page.id)?;
    Ok(Some(LivePageDetail {
        page,
        revision,
        datasets: views,
        slug_aliases,
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
    let canonical_page_id =
        resolve_live_page_id(&tx, page_id)?.ok_or_else(|| anyhow!("Page not found"))?;

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
    let canonical_page_id =
        resolve_live_page_id(&tx, page_id)?.ok_or_else(|| anyhow!("Page not found"))?;
    let current_revision: i64 = tx.query_row(
        "SELECT data_revision FROM live_pages WHERE id = ?1",
        [&canonical_page_id],
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
    fn create_live_page_in_transaction_leaves_commit_or_rollback_to_the_caller() {
        let source = test_connection();
        let page = fixture(&source);
        let revision = get_live_page(&source, &page.id).unwrap().unwrap().revision;
        let conn = test_connection();
        for commit in [false, true] {
            let tx = conn.unchecked_transaction().unwrap();
            create_live_page_in_transaction(&tx, &page, &revision, &[], None).unwrap();
            assert!(get_live_page(&tx, &page.id).unwrap().is_some());
            assert!(pages_capability(&tx).unwrap().activated);
            if commit {
                tx.commit().unwrap();
            } else {
                tx.rollback().unwrap();
            }
            assert_eq!(get_live_page(&conn, &page.id).unwrap().is_some(), commit);
            assert_eq!(pages_capability(&conn).unwrap().activated, commit);
        }
    }

    /// B5-10 — when one page's slug is another page's id, the id wins.
    #[test]
    fn an_id_resolves_before_a_colliding_slug() {
        let conn = test_connection();
        let now = Utc::now();
        // Created first, so a scan without an order would meet it first.
        create_live_page(
            &conn,
            &LivePage {
                id: "page-0".into(),
                project_id: None,
                title: "Squatter".into(),
                slug: "page-1".into(),
                current_revision_id: "rev-0".into(),
                data_revision: 0,
                created_at: now,
                updated_at: now,
                last_published_at: None,
                pinned: false,
                archived: false,
            },
            &LivePageRevision {
                id: "rev-0".into(),
                page_id: "page-0".into(),
                revision: 1,
                html: "<!doctype html><h1>Squatter</h1>".into(),
                created_by_agent: None,
                created_at: now,
            },
            &[CreateLivePageDataset {
                name: "summary".into(),
                kind: LivePageDatasetKind::Snapshot,
                initial: None,
                schema: None,
                max_points: None,
                max_age_days: None,
            }],
            None,
        )
        .unwrap();
        fixture(&conn);
        let result = publish_live_page(
            &conn,
            "page-1",
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "summary".into(),
                    operation: LivePageWriteOperation::Replace,
                    value: serde_json::json!({"value": 1}),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            },
        )
        .unwrap();
        assert_eq!(result.page_id, "page-1");
        let squatter: i64 = conn
            .query_row(
                "SELECT data_revision FROM live_pages WHERE id = 'page-0'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(squatter, 0);
        assert_eq!(
            get_live_page_summary(&conn, "page-1").unwrap().unwrap().id,
            "page-1"
        );
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

    #[test]
    fn update_live_page_persists_a_renamed_title_without_changing_its_slug() {
        let conn = test_connection();
        let page = fixture(&conn);

        let updated = update_live_page(
            &conn,
            &page.id,
            &UpdateLivePageRequest {
                title: Some("Production health".into()),
                slug: None,
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

    fn rename(conn: &Connection, page: &str, slug: &str) -> Result<Option<LivePageDetail>> {
        update_live_page(
            conn,
            page,
            &UpdateLivePageRequest {
                title: None,
                slug: Some(slug.into()),
                pinned: None,
                archived: None,
            },
        )
    }

    /// KT-1098 — every id-or-slug entry point follows a renamed page's old slug.
    #[test]
    fn an_old_slug_reaches_the_page_through_every_lookup() {
        let conn = test_connection();
        let page = fixture(&conn);
        let renamed = rename(&conn, "adobe-indicators", "adobe-kpis")
            .unwrap()
            .unwrap();
        assert_eq!(renamed.page.slug, "adobe-kpis");
        assert_eq!(renamed.slug_aliases, vec!["adobe-indicators".to_string()]);
        assert_eq!(
            rename(&conn, "adobe-kpis", "adobe-kpis")
                .unwrap()
                .unwrap()
                .slug_aliases
                .len(),
            1
        );

        let old = "adobe-indicators";
        assert_eq!(
            resolve_live_page_id(&conn, old).unwrap().as_deref(),
            Some("page-1")
        );
        assert_eq!(list_live_page_revisions(&conn, old).unwrap().len(), 1);
        assert!(list_live_page_publications(&conn, old, 3)
            .unwrap()
            .is_some());
        assert!(list_live_page_discussions(&conn, old).unwrap().is_some());
        update_live_page_html(&conn, old, "<p>v2</p>", None).unwrap();
        let published = publish_live_page(
            &conn,
            old,
            &PublishLivePageRequest {
                workflow_id: None,
                workflow_run_id: None,
                writes: vec![LivePageWrite {
                    dataset: "summary".into(),
                    operation: LivePageWriteOperation::Replace,
                    value: serde_json::json!({"count": 1}),
                    observed_at: None,
                    dedupe_key: None,
                    key_field: None,
                }],
            },
        )
        .unwrap();
        assert_eq!(published.data_revision, 1);
        assert_eq!(
            crate::workflows::run_scope::page_projects(&conn, old).unwrap(),
            vec![None]
        );
        assert!(!unlink_live_page_discussion(&conn, old, "none").unwrap());
        assert!(delete_live_page(&conn, old).unwrap());
        assert!(get_live_page(&conn, &page.id).unwrap().is_none());
        assert!(resolve_live_page_id(&conn, old).unwrap().is_none());
        assert!(!slug_is_taken(&conn, old).unwrap());
    }

    /// KT-1098 — a live slug wins over a retired one, and no create path, the
    /// import included, can take a retired slug.
    #[test]
    fn a_retired_slug_never_shadows_or_yields_to_another_page() {
        let conn = test_connection();
        let page = fixture(&conn);
        rename(&conn, &page.id, "adobe-kpis").unwrap();
        let other = fixture_without_datasets(&conn);
        assert!(rename(&conn, &other.id, "adobe-indicators")
            .unwrap_err()
            .is::<SlugUnavailable>());
        assert!(rename(&conn, &other.id, "adobe-kpis")
            .unwrap_err()
            .is::<SlugUnavailable>());

        let revision = get_live_page(&conn, &other.id).unwrap().unwrap().revision;
        let squatter = LivePage {
            id: "page-squat".into(),
            slug: "adobe-indicators".into(),
            ..other.clone()
        };
        let tx = conn.unchecked_transaction().unwrap();
        let refused = create_live_page_in_transaction(&tx, &squatter, &revision, &[], None);
        assert!(refused.unwrap_err().is::<SlugUnavailable>());
        drop(tx);
        assert!(slug_is_taken(&conn, "adobe-indicators").unwrap());

        // Even a stray alias row naming another page's live slug loses to it.
        conn.execute(
            "INSERT INTO live_page_slug_aliases (slug, page_id, created_at) VALUES ('bare-page', 'page-1', 'x')",
            [],
        )
        .unwrap();
        assert_eq!(
            resolve_live_page_id(&conn, "bare-page").unwrap().as_deref(),
            Some(other.id.as_str())
        );
    }

    /// KT-1098 — a workflow that publishes into the old slug still counts as
    /// one of the page's publishers.
    #[test]
    fn a_workflow_naming_the_old_slug_still_lists_as_a_publisher() {
        let conn = test_connection();
        fixture(&conn);
        let now = Utc::now();
        let workflow = crate::models::Workflow {
            retention: None,
            project_scope: None,
            id: "wf-old-slug".into(),
            name: "Feeder".into(),
            project_id: None,
            trigger: crate::models::WorkflowTrigger::Manual,
            steps: vec![crate::models::WorkflowStep {
                name: "publish".into(),
                step_type: crate::models::StepType::PublishPageData,
                page_publish: Some(crate::models::PublishPageDataConfig {
                    page_id: "adobe-indicators".into(),
                    writes: vec![],
                }),
                ..Default::default()
            }],
            actions: vec![],
            safety: crate::models::WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            concurrency_key: None,
            guards: None,
            artifacts: HashMap::new(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            pinned: false,
            created_at: now,
            updated_at: now,
        };
        crate::db::workflows::insert_workflow(&conn, &workflow).unwrap();
        rename(&conn, "page-1", "adobe-kpis").unwrap();
        let links = list_live_page_workflows(&conn, "adobe-kpis")
            .unwrap()
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].id, "wf-old-slug");
    }
}
