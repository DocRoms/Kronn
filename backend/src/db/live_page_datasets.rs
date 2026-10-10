//! Dataset lifecycle after creation (KT-1104): what uses a dataset, deleting
//! it, and changing its retention limits.
use super::*;
use crate::models::{
    DeleteLivePageDatasetResult, LivePageDatasetUsage, LivePageDatasetWriter,
    UpdateLivePageDatasetRequest, UpdateLivePageDatasetResult,
};

/// The dataset does not exist on this Page.
#[derive(Debug)]
pub struct DatasetNotFound(pub String);

impl std::fmt::Display for DatasetNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Unknown dataset '{}'", self.0)
    }
}

impl std::error::Error for DatasetNotFound {}

/// Deletion refused: something still reads or writes the dataset.
#[derive(Debug)]
pub struct DatasetReferenced(pub LivePageDatasetUsage);

impl std::fmt::Display for DatasetReferenced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let usage = &self.0;
        let mut references = Vec::new();
        if !usage.writers.is_empty() {
            let names = usage
                .writers
                .iter()
                .map(|writer| format!("'{}' ({})", writer.workflow_name, writer.workflow_id))
                .collect::<Vec<_>>()
                .join(", ");
            references.push(format!("written by workflow {names}"));
        }
        if usage.html_referenced {
            references.push("named in the Page HTML".to_string());
        }
        if !usage.action_refs.is_empty() {
            references.push(format!(
                "bound by Page button {}",
                usage.action_refs.join(", ")
            ));
        }
        write!(
            f,
            "Dataset '{}' is still in use: {}",
            usage.name,
            references.join("; ")
        )
    }
}

impl std::error::Error for DatasetReferenced {}

/// Whether a `PublishPageData` target names this Page: by id, live slug or a
/// slug it was renamed from.
fn targets_page(target: &str, page: &LivePage, aliases: &HashSet<String>) -> bool {
    let target = target.trim();
    target == page.id || target == page.slug || aliases.contains(target)
}

/// `name` as a whole word of `html`: neither neighbour is a character a
/// dataset name may contain.
fn html_names(html: &str, name: &str) -> bool {
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    html.match_indices(name).any(|(start, _)| {
        let before = html[..start].chars().next_back();
        let after = html[start + name.len()..].chars().next();
        !before.is_some_and(is_name_char) && !after.is_some_and(is_name_char)
    })
}

/// The dataset a `<page.dataset.<name>.<path>>` binding reads, if any.
fn bound_dataset(source_ref: &str) -> Option<&str> {
    source_ref
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .strip_prefix("page.dataset.")?
        .split('.')
        .next()
}

/// Usage of each named dataset of `page`, in the order given.
fn usage_of(
    conn: &Connection,
    page: &LivePage,
    names: &[String],
) -> Result<Vec<LivePageDatasetUsage>> {
    let html: String = conn.query_row(
        "SELECT html FROM live_page_revisions WHERE id = ?1",
        [&page.current_revision_id],
        |row| row.get(0),
    )?;
    let aliases: HashSet<String> = list_live_page_slug_aliases(conn, &page.id)?
        .into_iter()
        .collect();
    let mut writers = HashMap::<String, Vec<LivePageDatasetWriter>>::new();
    for workflow in crate::db::workflows::list_workflows(conn)? {
        // The rollback chain publishes too; a child workflow is listed on its own.
        let written = workflow
            .steps
            .iter()
            .chain(&workflow.on_failure)
            .filter_map(|step| step.page_publish.as_ref())
            .filter(|publish| targets_page(&publish.page_id, page, &aliases))
            .flat_map(|publish| publish.writes.iter().map(|write| write.dataset.trim()))
            .collect::<HashSet<_>>();
        for dataset in written {
            writers
                .entry(dataset.to_string())
                .or_default()
                .push(LivePageDatasetWriter {
                    workflow_id: workflow.id.clone(),
                    workflow_name: workflow.name.clone(),
                    enabled: workflow.enabled,
                });
        }
    }
    let mut bindings = HashMap::<String, Vec<String>>::new();
    let mut stmt = conn.prepare(
        "SELECT action_ref, values_json FROM live_page_actions
          WHERE live_page_id = ?1 ORDER BY action_ref",
    )?;
    let actions = stmt
        .query_map([&page.id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (action_ref, values_json) in actions {
        let values: Vec<serde_json::Value> = serde_json::from_str(&values_json)?;
        let bound = values
            .iter()
            .filter_map(|value| value.get("source_ref")?.as_str())
            .filter_map(bound_dataset)
            .collect::<HashSet<_>>();
        for dataset in bound {
            bindings
                .entry(dataset.to_string())
                .or_default()
                .push(action_ref.clone());
        }
    }
    Ok(names
        .iter()
        .map(|name| {
            let mut dataset_writers = writers.remove(name).unwrap_or_default();
            dataset_writers.sort_by_key(|writer| writer.workflow_name.to_lowercase());
            LivePageDatasetUsage {
                name: name.clone(),
                writers: dataset_writers,
                html_referenced: html_names(&html, name),
                action_refs: bindings.remove(name).unwrap_or_default(),
            }
        })
        .collect())
}

/// What reads or writes each dataset of a Page; `None` for an unknown Page.
pub fn list_live_page_dataset_usage(
    conn: &Connection,
    page_ref: &str,
) -> Result<Option<Vec<LivePageDatasetUsage>>> {
    let Some(page) = get_live_page_summary(conn, page_ref)? else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT name FROM live_page_datasets WHERE page_id = ?1 ORDER BY name COLLATE NOCASE",
    )?;
    let names = stmt
        .query_map([&page.id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(usage_of(conn, &page, &names)?))
}

fn find_dataset(conn: &Connection, page_id: &str, name: &str) -> Result<LivePageDataset> {
    conn.query_row(
        "SELECT id, page_id, name, kind, current_json, schema_json,
                max_points, max_age_days, updated_at
           FROM live_page_datasets WHERE page_id = ?1 AND name = ?2",
        params![page_id, name],
        map_dataset,
    )
    .optional()?
    .ok_or_else(|| DatasetNotFound(name.to_string()).into())
}

/// Moves the Page to a new data revision so open views reload it.
fn bump_data_revision(conn: &Connection, page_id: &str) -> Result<u64> {
    let revision: i64 = conn.query_row(
        "UPDATE live_pages SET data_revision = data_revision + 1, updated_at = ?2
          WHERE id = ?1 RETURNING data_revision",
        params![page_id, Utc::now().to_rfc3339()],
        |row| row.get(0),
    )?;
    u64::try_from(revision).context("Negative Page data revision")
}

/// Deletes a dataset and its points. Refused with [`DatasetReferenced`] while
/// a workflow writes it, the HTML names it or a Page button binds to it,
/// unless `force`. The check and the deletion share one transaction, so a
/// publication lands before (and is deleted) or after (and finds no dataset).
pub fn delete_live_page_dataset(
    conn: &Connection,
    page_ref: &str,
    name: &str,
    force: bool,
) -> Result<DeleteLivePageDatasetResult> {
    let tx = conn.unchecked_transaction()?;
    let page = get_live_page_summary(&tx, page_ref)?.ok_or_else(|| anyhow!("Page not found"))?;
    let dataset = find_dataset(&tx, &page.id, name)?;
    let usage = usage_of(&tx, &page, std::slice::from_ref(&dataset.name))?
        .pop()
        .ok_or_else(|| anyhow!("Dataset usage missing"))?;
    if usage.is_referenced() && !force {
        return Err(DatasetReferenced(usage).into());
    }
    tx.execute(
        "DELETE FROM live_page_datasets WHERE id = ?1",
        [&dataset.id],
    )?;
    let data_revision = bump_data_revision(&tx, &page.id)?;
    tx.commit()?;
    let overridden = if usage.is_referenced() {
        usage
    } else {
        LivePageDatasetUsage {
            name: dataset.name.clone(),
            writers: Vec::new(),
            html_referenced: false,
            action_refs: Vec::new(),
        }
    };
    Ok(DeleteLivePageDatasetResult {
        page_id: page.id,
        name: dataset.name,
        data_revision,
        overridden,
    })
}

/// Changes a dataset's retention limits and prunes the points beyond them
/// at once.
pub fn update_live_page_dataset_limits(
    conn: &Connection,
    page_ref: &str,
    name: &str,
    request: &UpdateLivePageDatasetRequest,
) -> Result<UpdateLivePageDatasetResult> {
    if request.max_points == Some(0) {
        bail!("Dataset '{name}' max_points must be greater than zero");
    }
    if request.max_age_days == Some(Some(0)) {
        bail!("Dataset '{name}' max_age_days must be greater than zero");
    }
    let tx = conn.unchecked_transaction()?;
    let page = get_live_page_summary(&tx, page_ref)?.ok_or_else(|| anyhow!("Page not found"))?;
    let mut dataset = find_dataset(&tx, &page.id, name)?;
    if let Some(max_points) = request.max_points {
        dataset.max_points = max_points;
    }
    if let Some(max_age_days) = request.max_age_days {
        dataset.max_age_days = max_age_days;
    }
    tx.execute(
        "UPDATE live_page_datasets SET max_points = ?2, max_age_days = ?3 WHERE id = ?1",
        params![dataset.id, dataset.max_points, dataset.max_age_days],
    )?;
    let points_removed =
        u32::try_from(enforce_retention(&tx, &dataset)?).context("Pruned point count overflow")?;
    let data_revision = if points_removed > 0 {
        bump_data_revision(&tx, &page.id)?
    } else {
        page.data_revision
    };
    tx.commit()?;
    Ok(UpdateLivePageDatasetResult {
        dataset,
        points_removed,
        data_revision,
    })
}

#[cfg(test)]
#[path = "live_page_datasets_tests.rs"]
pub(crate) mod tests;
