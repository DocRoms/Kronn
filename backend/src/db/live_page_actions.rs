//! Durable, human-gated Kronn action proposals authored inline in a Live
//! Page's HTML (KT-538). Builds on the KT-476 `discussion_actions` contract:
//! same `DiscussionActionKind`/`DiscussionActionState`/`DiscussionActionValue`
//! shape, same `target_contract`/`resolved_values` target validation, and the
//! same shared launch state machine (`kronn_action_engine`) — only the origin
//! anchor differs (a Page action block instead of a discussion message).
//!
//! An agent embeds one typed block per CTA:
//! ```html
//! <button data-kronn-action="collect-logs">Collect logs</button>
//! <script type="application/kronn-action" data-action-id="collect-logs">
//! {"kind":"quick_exec","target_id":"qe-1"}
//! </script>
//! ```
//! `application/kronn-action` is not an executable script MIME type, so the
//! sandboxed iframe never runs it — it is an inert data island, exactly like
//! `type="application/json"`. The block is parsed exactly once, in the same
//! transaction as the HTML revision that introduced or last matched it
//! (`ingest_page_actions`, called from `create_live_page` /
//! `update_live_page_html`); the sandboxed document is never reparsed at
//! render time. See `docs/architecture/live-pages.md`.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::discussion_actions::{
    self, ActionFence, DiscussionActionKind, DiscussionActionState, DiscussionActionValue,
    DiscussionActionValueProvenance,
};
use super::kronn_action_engine::{self, ActionTable};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LivePageAction {
    pub id: String,
    pub live_page_id: String,
    pub live_page_revision_id: String,
    pub action_ref: String,
    pub kind: DiscussionActionKind,
    pub target_id: String,
    pub target_name: String,
    pub project_id: Option<String>,
    pub state: DiscussionActionState,
    pub values: Vec<DiscussionActionValue>,
    pub shared_run_id: Option<String>,
    pub result_discussion_id: Option<String>,
    pub deep_link: Option<String>,
    pub diagnostic: Option<String>,
    pub launched_at: Option<String>,
    pub finished_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// True when `live_page_revision_id` no longer matches the Page's live
    /// `current_revision_id`. The `(live_page_id, action_ref)` anchor itself
    /// always survives a refresh or a content update — this flag exists so
    /// the human sees an explicit explanation instead of silently trusting
    /// values that may no longer reflect the currently displayed Page.
    pub stale_source: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaunchLivePageActionRequest {
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: HashMap<String, String>,
    /// Row/card SELECTOR for `dynamic_binding` fields, keyed by variable
    /// name — never a resolved value. For example the clicked collection
    /// item's key-field value. The real field value is always looked up
    /// server-side from the live dataset/page row (`resolve_dynamic_binding`)
    /// — a caller can choose which existing row to bind to, never inject an
    /// arbitrary resolved value.
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub bindings: HashMap<String, String>,
}

pub enum LivePageActionClaimOutcome {
    Claimed {
        action: LivePageAction,
        variables: HashMap<String, String>,
    },
    Existing(LivePageAction),
}

/// Extract `(action_ref, json_body)` pairs from `<script
/// type="application/kronn-action" data-action-id="...">...</script>` blocks.
/// A hand-rolled scanner (not a full HTML parser or `regex`) is deliberate:
/// Page HTML is bounded to 1 MB and author-controlled, and the block shape is
/// fixed, so a tag/attribute scan is sufficient and keeps this dependency
/// footprint at zero. `to_ascii_lowercase` (not `to_lowercase`) preserves
/// byte offsets even when the surrounding HTML contains non-ASCII text.
fn extract_page_action_blocks(html: &str) -> Vec<(String, String)> {
    let lower = html.to_ascii_lowercase();
    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    while let Some(open_rel) = lower[cursor..].find("<script") {
        let open = cursor + open_rel;
        let after_name = open + "<script".len();
        if !lower[after_name..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace() || character == '>')
        {
            cursor = after_name;
            continue;
        }
        let Some(tag_end) = find_tag_end(html, after_name) else {
            break;
        };
        let opening_tag = &html[open..=tag_end];
        let body_start = tag_end + 1;
        let Some(close_rel) = lower[body_start..].find("</script>") else {
            break;
        };
        let body_end = body_start + close_rel;
        cursor = body_end + "</script>".len();
        if !extract_attribute(opening_tag, "type").is_some_and(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("application/kronn-action")
        }) {
            continue;
        }
        let Some(action_ref) = extract_attribute(opening_tag, "data-action-id") else {
            continue;
        };
        let action_ref = action_ref.trim();
        if !valid_action_ref(action_ref) {
            continue;
        }
        blocks.push((
            action_ref.to_string(),
            html[body_start..body_end].trim().to_string(),
        ));
    }
    blocks
}

fn find_tag_end(tag: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in tag[start..].char_indices() {
        match (quote, character) {
            (Some(active), current) if current == active => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn extract_attribute(tag: &str, name: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut cursor = tag.find(char::is_whitespace).unwrap_or(tag.len());
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || matches!(bytes[cursor], b'>' | b'/') {
            break;
        }
        let attribute_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric()
                || matches!(bytes[cursor], b'-' | b'_' | b':' | b'.'))
        {
            cursor += 1;
        }
        if cursor == attribute_start {
            cursor += 1;
            continue;
        }
        let attribute_name = &tag[attribute_start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || !matches!(bytes[cursor], b'\'' | b'"') {
            continue;
        }
        let quote = bytes[cursor];
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != quote {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            return None;
        }
        let value = &tag[value_start..cursor];
        cursor += 1;
        if attribute_name.eq_ignore_ascii_case(name) {
            return Some(value.to_string());
        }
    }
    None
}

fn valid_action_ref(action_ref: &str) -> bool {
    !action_ref.is_empty()
        && action_ref.len() <= 256
        && action_ref
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

/// Persist every valid action block in the caller's HTML-revision
/// transaction. A block Kronn cannot run at all (invalid JSON, blank
/// `target_id`, unknown/mismatched target) is retained as an actionable
/// `preflight_failed` card. Only a still-`proposed`/`preflight_failed` row is
/// refreshed to a newer revision's definition — once a row has genuinely
/// launched (`launching`/`running`/terminal), its definition is frozen, and a
/// later edit to that CTA in the Page simply leaves the row `stale_source`.
pub fn ingest_page_actions(
    conn: &Connection,
    live_page_id: &str,
    revision_id: &str,
    html: &str,
) -> Result<()> {
    if !html.contains("application/kronn-action") {
        return Ok(());
    }
    let page_project: Option<String> = conn.query_row(
        "SELECT project_id FROM live_pages WHERE id = ?1",
        [live_page_id],
        |row| row.get(0),
    )?;
    let now = Utc::now().to_rfc3339();
    let mut seen_refs = HashSet::new();
    for (action_ref, raw) in extract_page_action_blocks(html) {
        // First occurrence of a given `action_ref` in this revision wins;
        // a later duplicate is ignored defensively rather than clobbering it.
        if !seen_refs.insert(action_ref.clone()) {
            continue;
        }
        let action_id = format!("page-action:{live_page_id}:{action_ref}");
        let fence = match serde_json::from_str::<ActionFence>(&raw) {
            Ok(fence) => fence,
            Err(error) => {
                upsert_action_row(
                    conn,
                    &action_id,
                    live_page_id,
                    revision_id,
                    &action_ref,
                    DiscussionActionKind::Invalid,
                    "",
                    "(bloc invalide)",
                    None,
                    "[]",
                    Some(format!(
                        "Ce bloc d’action n’a pas pu être lu (JSON invalide) : {error}."
                    )),
                    &now,
                )?;
                continue;
            }
        };
        if fence.target_id.trim().is_empty() {
            upsert_action_row(
                conn,
                &action_id,
                live_page_id,
                revision_id,
                &action_ref,
                fence.kind,
                "",
                "(bloc invalide)",
                None,
                "[]",
                Some("Ce bloc d’action ne précise aucune cible (target_id vide).".into()),
                &now,
            )?;
            continue;
        }
        let contract = discussion_actions::target_contract(conn, fence.kind, &fence.target_id)?;
        let mut diagnostic = None;
        let (target_name, target_project, values) = match contract {
            Some(contract) => {
                let values = match discussion_actions::resolved_values(
                    &contract.variables,
                    fence.values,
                    true,
                ) {
                    Ok(values) => values,
                    Err(error) => {
                        diagnostic = Some(error.to_string());
                        Vec::new()
                    }
                };
                (contract.name, contract.project_id, values)
            }
            None => {
                diagnostic = Some("La cible n’existe plus ou n’est pas accessible.".into());
                (fence.target_id.clone(), None, Vec::new())
            }
        };
        let project_id = fence
            .project_id
            .clone()
            .or(target_project.clone())
            .or(page_project.clone());
        if target_project.is_some()
            && fence.project_id.is_some()
            && target_project != fence.project_id
        {
            diagnostic = Some("Le projet proposé ne correspond pas au projet de la cible.".into());
        }
        if page_project.is_some() && project_id != page_project {
            diagnostic = Some("Cette action n’est pas autorisée dans le projet de la Page.".into());
        }
        if let Some(project_id) = project_id.as_deref() {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM projects WHERE id = ?1)",
                [project_id],
                |row| row.get(0),
            )?;
            if !exists {
                diagnostic = Some("Le projet proposé n’existe plus.".into());
            }
        }
        upsert_action_row(
            conn,
            &action_id,
            live_page_id,
            revision_id,
            &action_ref,
            fence.kind,
            &fence.target_id,
            &target_name,
            project_id.as_deref(),
            &serde_json::to_string(&values)?,
            diagnostic,
            &now,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upsert_action_row(
    conn: &Connection,
    action_id: &str,
    live_page_id: &str,
    revision_id: &str,
    action_ref: &str,
    kind: DiscussionActionKind,
    target_id: &str,
    target_name: &str,
    project_id: Option<&str>,
    values_json: &str,
    diagnostic: Option<String>,
    now: &str,
) -> Result<()> {
    let state = if diagnostic.is_some() {
        "preflight_failed"
    } else {
        "proposed"
    };
    conn.execute(
        "INSERT INTO live_page_actions (
             id, live_page_id, live_page_revision_id, action_ref, kind,
             target_id, target_name, project_id, state, values_json,
             diagnostic, finished_at, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)
         ON CONFLICT(live_page_id, action_ref) DO UPDATE SET
             live_page_revision_id = excluded.live_page_revision_id,
             kind = excluded.kind,
             target_id = excluded.target_id,
             target_name = excluded.target_name,
             project_id = excluded.project_id,
             state = excluded.state,
             values_json = excluded.values_json,
             diagnostic = excluded.diagnostic,
             finished_at = excluded.finished_at,
             updated_at = excluded.updated_at
         WHERE live_page_actions.state IN ('proposed', 'preflight_failed')",
        params![
            action_id,
            live_page_id,
            revision_id,
            action_ref,
            kind.as_db_str(),
            target_id,
            target_name,
            project_id,
            state,
            values_json,
            diagnostic.clone(),
            diagnostic.as_ref().map(|_| now.to_string()),
            now,
        ],
    )?;
    Ok(())
}

const SELECT_LIVE_PAGE_ACTION: &str = "SELECT a.id, a.live_page_id, a.live_page_revision_id,
    a.action_ref, a.kind, a.target_id, a.target_name, a.project_id, a.state, a.values_json,
    a.shared_run_id, a.result_discussion_id, a.deep_link, a.diagnostic, a.launched_at,
    a.finished_at, a.created_at, a.updated_at,
    (a.live_page_revision_id != p.current_revision_id) AS stale_source
    FROM live_page_actions a JOIN live_pages p ON p.id = a.live_page_id";

fn map_action(row: &rusqlite::Row<'_>) -> rusqlite::Result<LivePageAction> {
    let kind_raw = row.get::<_, String>(4)?;
    let state_raw = row.get::<_, String>(8)?;
    let values_raw = row.get::<_, String>(9)?;
    Ok(LivePageAction {
        id: row.get(0)?,
        live_page_id: row.get(1)?,
        live_page_revision_id: row.get(2)?,
        action_ref: row.get(3)?,
        kind: DiscussionActionKind::from_db_str(&kind_raw).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                4,
                rusqlite::types::Type::Text,
                format!("invalid live page action kind `{kind_raw}`").into(),
            )
        })?,
        target_id: row.get(5)?,
        target_name: row.get(6)?,
        project_id: row.get(7)?,
        state: DiscussionActionState::from_db_str(&state_raw).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                format!("invalid live page action state `{state_raw}`").into(),
            )
        })?,
        values: serde_json::from_str(&values_raw).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        shared_run_id: row.get(10)?,
        result_discussion_id: row.get(11)?,
        deep_link: row.get(12)?,
        diagnostic: row.get(13)?,
        launched_at: row.get(14)?,
        finished_at: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        stale_source: row.get(18)?,
    })
}

fn refresh_from_shared_run(conn: &Connection, action: &mut LivePageAction) -> Result<()> {
    let mut core = kronn_action_engine::ActionCore {
        id: action.id.clone(),
        state: action.state,
        values: std::mem::take(&mut action.values),
        shared_run_id: action.shared_run_id.clone(),
        diagnostic: action.diagnostic.clone(),
        launched_at: action.launched_at.clone(),
        finished_at: action.finished_at.clone(),
        updated_at: action.updated_at.clone(),
    };
    kronn_action_engine::refresh_from_shared_run(conn, ActionTable::LivePage, &mut core)?;
    action.state = core.state;
    action.values = core.values;
    action.diagnostic = core.diagnostic;
    action.finished_at = core.finished_at;
    action.updated_at = core.updated_at;
    Ok(())
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<LivePageAction>> {
    let mut action = conn
        .query_row(
            &format!("{SELECT_LIVE_PAGE_ACTION} WHERE a.id = ?1"),
            [id],
            map_action,
        )
        .optional()?;
    if let Some(action) = action.as_mut() {
        refresh_from_shared_run(conn, action)?;
    }
    Ok(action)
}

pub fn list_for_live_page(conn: &Connection, live_page_id: &str) -> Result<Vec<LivePageAction>> {
    let mut statement = conn.prepare(&format!(
        "{SELECT_LIVE_PAGE_ACTION} WHERE a.live_page_id = ?1 ORDER BY a.created_at, a.action_ref"
    ))?;
    let mut actions = statement
        .query_map([live_page_id], map_action)?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for action in &mut actions {
        refresh_from_shared_run(conn, action)?;
    }
    Ok(actions)
}

pub fn cancel(conn: &Connection, id: &str) -> Result<Option<LivePageAction>> {
    kronn_action_engine::cancel(conn, ActionTable::LivePage, id)?;
    get(conn, id)
}

/// Resolve one `dynamic_binding` `source_ref` against real, current Page or
/// dataset content. `binding_key` is a row/card SELECTOR reported by the
/// click — never a value. Supported shapes:
///   - `<page.id>` / `<page.slug>` / `<page.title>` — the Page row itself,
///     no selector needed.
///   - `<page.dataset.<name>.<path>>` — a dot-path into that dataset's
///     current JSON. A `find(<field>)` path segment requires `binding_key`
///     and selects the array element whose `<field>` matches it (a
///     `collection` dataset row); without it the path applies directly (a
///     `snapshot`/"card" dataset, or a known field of a `collection` value).
///
/// `time_series` datasets are out of scope for KT-538 and fail closed with an
/// actionable diagnostic.
fn resolve_dynamic_binding(
    conn: &Connection,
    live_page_id: &str,
    source_ref: &str,
    binding_key: Option<&str>,
) -> Result<String> {
    let reference = source_ref
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>');
    let Some(field) = reference.strip_prefix("page.") else {
        anyhow::bail!("unsupported dynamic_binding reference `{source_ref}`");
    };
    if let Some(rest) = field.strip_prefix("dataset.") {
        let Some((dataset_name, path)) = rest.split_once('.') else {
            anyhow::bail!(
                "dynamic_binding reference `{source_ref}` is missing a dataset field path"
            );
        };
        let row: Option<(String, Option<String>)> = conn
            .query_row(
                "SELECT kind, current_json FROM live_page_datasets WHERE page_id = ?1 AND name = ?2",
                params![live_page_id, dataset_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((kind, current_json)) = row else {
            anyhow::bail!("dynamic_binding dataset `{dataset_name}` does not exist on this Page");
        };
        if kind == "time_series" {
            anyhow::bail!(
                "dynamic_binding does not support time_series datasets yet (`{dataset_name}`)"
            );
        }
        let current: serde_json::Value = current_json
            .map(|raw| serde_json::from_str(&raw))
            .transpose()?
            .unwrap_or(serde_json::Value::Null);
        let resolved = resolve_json_path(&current, path, binding_key)?;
        return Ok(json_value_as_string(&resolved));
    }
    let page: (String, String, String) = conn.query_row(
        "SELECT id, slug, title FROM live_pages WHERE id = ?1",
        [live_page_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    match field {
        "id" => Ok(page.0),
        "slug" => Ok(page.1),
        "title" => Ok(page.2),
        other => anyhow::bail!("unknown page context field `{other}`"),
    }
}

fn resolve_json_path(
    value: &serde_json::Value,
    path: &str,
    binding_key: Option<&str>,
) -> Result<serde_json::Value> {
    let mut current = value.clone();
    for segment in path.split('.') {
        if let Some(field) = segment
            .strip_prefix("find(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let key = binding_key.ok_or_else(|| {
                anyhow::anyhow!(
                    "dynamic_binding path `{path}` requires a row selector but none was supplied"
                )
            })?;
            let array = current.as_array().ok_or_else(|| {
                anyhow::anyhow!(
                    "dynamic_binding path `{path}` expected an array at `find({field})`"
                )
            })?;
            current = array
                .iter()
                .find(|item| item.get(field).map(json_value_as_string).as_deref() == Some(key))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no row found where `{field}` = `{key}`"))?;
        } else if let Ok(index) = segment.parse::<usize>() {
            let array = current.as_array().ok_or_else(|| {
                anyhow::anyhow!(
                    "dynamic_binding path `{path}` expected an array at index `{index}`"
                )
            })?;
            current = array.get(index).cloned().ok_or_else(|| {
                anyhow::anyhow!("dynamic_binding path `{path}` index `{index}` out of range")
            })?;
        } else {
            let object = current.as_object().ok_or_else(|| {
                anyhow::anyhow!("dynamic_binding path `{path}` expected an object at `{segment}`")
            })?;
            current = object.get(segment).cloned().ok_or_else(|| {
                anyhow::anyhow!("dynamic_binding path `{path}` field `{segment}` is missing")
            })?;
        }
    }
    Ok(current)
}

fn json_value_as_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub fn claim_launch(
    conn: &Connection,
    id: &str,
    supplied: &HashMap<String, String>,
    bindings: &HashMap<String, String>,
) -> Result<Option<LivePageActionClaimOutcome>> {
    let transaction = conn.unchecked_transaction()?;
    let Some(mut action) = get(&transaction, id)? else {
        transaction.commit()?;
        return Ok(None);
    };
    if action.state == DiscussionActionState::Proposed && action.stale_source {
        anyhow::bail!(
            "this action is no longer present in the current Page revision; reload the Page and use a current CTA"
        );
    }
    if action.state != DiscussionActionState::Proposed {
        transaction.commit()?;
        return Ok(Some(LivePageActionClaimOutcome::Existing(action)));
    }
    for (name, selector) in bindings {
        let declared = action.values.iter().any(|value| {
            value.name == *name
                && value.provenance == DiscussionActionValueProvenance::DynamicBinding
        });
        if !declared {
            anyhow::bail!("unknown dynamic action binding `{name}`");
        }
        if selector.len() > 4_096 {
            anyhow::bail!("dynamic action binding `{name}` is too large");
        }
    }
    // Resolve every `dynamic_binding` value server-side before the shared
    // engine ever sees it. Any value the caller placed in `supplied` for one
    // of these variables is ignored: the resolved value always comes from
    // this lookup, never from the wire.
    let mut resolved_supplied = supplied.clone();
    for value in &action.values {
        if value.provenance != DiscussionActionValueProvenance::DynamicBinding {
            continue;
        }
        let source_ref = value.source_ref.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "dynamic_binding variable `{}` has no source_ref",
                value.name
            )
        })?;
        let binding_key = bindings.get(&value.name).map(String::as_str);
        let resolved =
            resolve_dynamic_binding(&transaction, &action.live_page_id, source_ref, binding_key)?;
        resolved_supplied.insert(value.name.clone(), resolved);
    }
    let mut core = kronn_action_engine::ActionCore {
        id: action.id.clone(),
        state: action.state,
        values: std::mem::take(&mut action.values),
        shared_run_id: action.shared_run_id.clone(),
        diagnostic: action.diagnostic.clone(),
        launched_at: action.launched_at.clone(),
        finished_at: action.finished_at.clone(),
        updated_at: action.updated_at.clone(),
    };
    let claimed_variables = kronn_action_engine::claim_launch(
        &transaction,
        ActionTable::LivePage,
        &mut core,
        &resolved_supplied,
    )?;
    action.state = core.state;
    action.values = core.values;
    action.launched_at = core.launched_at;
    action.updated_at = core.updated_at;
    transaction.commit()?;
    Ok(Some(match claimed_variables {
        Some(variables) => LivePageActionClaimOutcome::Claimed { action, variables },
        None => LivePageActionClaimOutcome::Existing(action),
    }))
}

pub fn complete(
    conn: &Connection,
    id: &str,
    completion: kronn_action_engine::ActionCompletion,
) -> Result<()> {
    kronn_action_engine::complete(conn, ActionTable::LivePage, id, completion)
}

/// Complete a Page-authored QP and register its result discussion atomically.
/// This is the durable reverse edge for the action's own
/// `result_discussion_id`: Page history and discussion-origin lookups cannot
/// disagree after a crash between two separate commits.
pub fn complete_quick_prompt(
    conn: &Connection,
    id: &str,
    live_page_id: &str,
    discussion_id: &str,
) -> Result<()> {
    let transaction = conn.unchecked_transaction()?;
    crate::db::live_pages::link_live_page_discussion(
        &transaction,
        live_page_id,
        discussion_id,
        crate::models::LivePageDiscussionRelation::Attached,
    )?;
    complete(
        &transaction,
        id,
        kronn_action_engine::ActionCompletion {
            state: DiscussionActionState::Succeeded,
            shared_run_id: None,
            result_discussion_id: Some(discussion_id.to_string()),
            deep_link: Some(format!("discussion:{discussion_id}")),
            diagnostic: None,
        },
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AgentType, CollectQuickExecOutputFormat, ModelTier, PromptVariable, PromptVariableSource,
        QuickApi, QuickExec, QuickPrompt,
    };

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    fn insert_page(conn: &Connection, page_id: &str, revision_id: &str, html: &str) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO live_pages (
                 id, project_id, title, slug, current_revision_id, data_revision,
                 created_at, updated_at, last_published_at, pinned, archived
             ) VALUES (?1, NULL, 'Test Page', ?1, ?2, 0, ?3, ?3, NULL, 0, 0)",
            params![page_id, revision_id, now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO live_page_revisions (id, page_id, revision, html, created_by_agent, created_at)
             VALUES (?1, ?2, 1, ?3, NULL, ?4)",
            params![revision_id, page_id, html, now],
        )
        .unwrap();
    }

    fn republish_revision(conn: &Connection, page_id: &str, new_revision_id: &str, html: &str) {
        let now = Utc::now().to_rfc3339();
        let next_revision: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(revision), 0) + 1 FROM live_page_revisions WHERE page_id = ?1",
                [page_id],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO live_page_revisions (id, page_id, revision, html, created_by_agent, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![new_revision_id, page_id, next_revision, html, now],
        )
        .unwrap();
        conn.execute(
            "UPDATE live_pages SET current_revision_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![page_id, new_revision_id, now],
        )
        .unwrap();
    }

    fn insert_dataset(
        conn: &Connection,
        page_id: &str,
        name: &str,
        kind: &str,
        current_json: &str,
    ) {
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO live_page_datasets (
                 id, page_id, name, kind, current_json, schema_json, max_points, max_age_days, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 50000, NULL, ?6)",
            params![format!("ds-{page_id}-{name}"), page_id, name, kind, current_json, now],
        )
        .unwrap();
    }

    fn action_block(action_ref: &str, json: &str) -> String {
        format!(
            r#"<div><button data-kronn-action="{action_ref}">Go</button>
<script type="application/kronn-action" data-action-id="{action_ref}">{json}</script></div>"#
        )
    }

    fn insert_target(conn: &Connection) {
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            conn,
            &QuickExec {
                id: "qe-1".into(),
                name: "Collect logs".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "service".into(),
                    label: "Service".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: None,
                    source_ref: None,
                    allow_manual_override: false,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
    }

    fn insert_second_target(conn: &Connection) {
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            conn,
            &QuickExec {
                id: "qe-2".into(),
                name: "Restart service".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "service".into(),
                    label: "Service".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: None,
                    source_ref: None,
                    allow_manual_override: false,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
    }

    fn insert_all_target_kinds(conn: &Connection) {
        let now = Utc::now();
        crate::db::quick_prompts::insert_quick_prompt(
            conn,
            &QuickPrompt {
                id: "qp-1".into(),
                pinned: false,
                name: "Frame issue".into(),
                icon: "✨".into(),
                prompt_template: "Frame this issue".into(),
                variables: vec![],
                agent: AgentType::ClaudeCode,
                connection_id: None,
                project_id: None,
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                tier: ModelTier::Default,
                agent_settings: None,
                description: String::new(),
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        crate::db::quick_apis::insert_quick_api(
            conn,
            &QuickApi {
                id: "qa-1".into(),
                pinned: false,
                name: "Read ticket".into(),
                description: String::new(),
                icon: "🔌".into(),
                project_id: None,
                api_plugin_slug: "tracker".into(),
                api_config_id: "config".into(),
                api_endpoint_path: "/ticket".into(),
                api_method: Some("GET".into()),
                api_query: None,
                api_path_params: None,
                api_headers: None,
                api_body: None,
                api_extract: None,
                api_pagination: None,
                api_timeout_ms: None,
                api_max_retries: None,
                variables: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        conn.execute(
            "INSERT INTO workflows
                (id, name, trigger_json, steps_json, actions_json, safety_json,
                 variables, enabled, created_at, updated_at)
             VALUES ('wf-1', 'Publish report', '\"Manual\"', '[]', '[]', '{}',
                     '[]', 1, ?1, ?1)",
            [now.to_rfc3339()],
        )
        .unwrap();
        insert_target(conn);
    }

    #[test]
    fn page_action_block_is_ingested_once_and_stable_across_reingestion() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "collect-logs",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","value":"api","provenance":"agent_suggestion"}]}"#,
        );
        insert_page(&conn, "page-1", "rev-1", &html);

        ingest_page_actions(&conn, "page-1", "rev-1", &html).unwrap();
        ingest_page_actions(&conn, "page-1", "rev-1", &html).unwrap();

        let actions = list_for_live_page(&conn, "page-1").unwrap();
        assert_eq!(actions.len(), 1, "re-ingestion must remain idempotent");
        let action = &actions[0];
        assert_eq!(action.id, "page-action:page-1:collect-logs");
        assert_eq!(action.kind, DiscussionActionKind::QuickExec);
        assert_eq!(action.target_name, "Collect logs");
        assert_eq!(action.state, DiscussionActionState::Proposed);
        assert!(!action.stale_source);
        assert_eq!(action.values[0].value.as_deref(), Some("api"));
    }

    #[test]
    fn one_registry_validates_all_four_target_kinds_from_a_page() {
        let conn = connection();
        insert_all_target_kinds(&conn);
        let html = format!(
            "{}{}{}{}",
            action_block("1-qp", r#"{"kind":"quick_prompt","target_id":"qp-1"}"#),
            action_block("2-qa", r#"{"kind":"quick_api","target_id":"qa-1"}"#),
            action_block("3-qe", r#"{"kind":"quick_exec","target_id":"qe-1"}"#),
            action_block("4-wf", r#"{"kind":"workflow","target_id":"wf-1"}"#),
        );
        insert_page(&conn, "page-all", "rev-1", &html);
        ingest_page_actions(&conn, "page-all", "rev-1", &html).unwrap();

        // All four blocks land in the same `ingest_page_actions` call and
        // therefore share one `created_at` timestamp; `list_for_live_page`'s
        // secondary sort by `action_ref` is what makes the order below
        // deterministic (document position isn't tracked for Page blocks the
        // way `fence_index` tracks it for discussion fences).
        let actions = list_for_live_page(&conn, "page-all").unwrap();
        assert_eq!(
            actions.iter().map(|action| action.kind).collect::<Vec<_>>(),
            vec![
                DiscussionActionKind::QuickPrompt,
                DiscussionActionKind::QuickApi,
                DiscussionActionKind::QuickExec,
                DiscussionActionKind::Workflow,
            ]
        );
        assert!(actions
            .iter()
            .all(|action| action.state == DiscussionActionState::Proposed));
    }

    #[test]
    fn missing_target_becomes_an_actionable_preflight_failure() {
        let conn = connection();
        let html = action_block("missing", r#"{"kind":"workflow","target_id":"missing"}"#);
        insert_page(&conn, "page-2", "rev-1", &html);
        ingest_page_actions(&conn, "page-2", "rev-1", &html).unwrap();

        let action = get(&conn, "page-action:page-2:missing").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert!(action.diagnostic.unwrap().contains("n’existe plus"));
    }

    #[test]
    fn a_cross_project_page_action_is_refused_before_launch() {
        let conn = connection();
        insert_target(&conn);
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at, updated_at)
             VALUES ('project-page', 'Page', '/tmp/page', ?1, ?1),
                    ('project-other', 'Other', '/tmp/other', ?1, ?1)",
            [&now],
        )
        .unwrap();
        let html = action_block(
            "cross-project",
            r#"{"kind":"quick_exec","target_id":"qe-1","project_id":"project-other"}"#,
        );
        insert_page(&conn, "page-project", "rev-project", &html);
        conn.execute(
            "UPDATE live_pages SET project_id = 'project-page' WHERE id = 'page-project'",
            [],
        )
        .unwrap();
        ingest_page_actions(&conn, "page-project", "rev-project", &html).unwrap();

        let action = get(&conn, "page-action:page-project:cross-project")
            .unwrap()
            .unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert!(action
            .diagnostic
            .as_deref()
            .is_some_and(|text| text.contains("n’est pas autorisée")));
        let outcome = claim_launch(&conn, &action.id, &HashMap::new(), &HashMap::new()).unwrap();
        assert!(matches!(
            outcome,
            Some(LivePageActionClaimOutcome::Existing(existing))
                if existing.state == DiscussionActionState::PreflightFailed
        ));
    }

    #[test]
    fn launch_claim_is_atomic_and_a_second_click_reuses_the_same_action() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "collect-logs",
            r#"{"kind":"quick_exec","target_id":"qe-1"}"#,
        );
        insert_page(&conn, "page-3", "rev-1", &html);
        ingest_page_actions(&conn, "page-3", "rev-1", &html).unwrap();

        let supplied = HashMap::from([("service".into(), "api".into())]);
        let bindings = HashMap::new();
        let first = claim_launch(
            &conn,
            "page-action:page-3:collect-logs",
            &supplied,
            &bindings,
        )
        .unwrap()
        .unwrap();
        let LivePageActionClaimOutcome::Claimed { action, variables } = first else {
            panic!("expected a fresh claim");
        };
        assert_eq!(variables["service"], "api");
        assert!(action.values[0].value.is_none());

        let second = claim_launch(
            &conn,
            "page-action:page-3:collect-logs",
            &supplied,
            &bindings,
        )
        .unwrap()
        .unwrap();
        assert!(matches!(second, LivePageActionClaimOutcome::Existing(_)));
        let reloaded = get(&conn, "page-action:page-3:collect-logs")
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.state, DiscussionActionState::Launching);
    }

    #[test]
    fn a_page_agent_suggestion_keeps_its_authored_prefill_but_scrubs_the_runtime_value() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "suggested",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","value":"api","provenance":"agent_suggestion","suggested_by":"@claude-cli"}]}"#,
        );
        insert_page(&conn, "page-suggested", "rev-suggested", &html);
        ingest_page_actions(&conn, "page-suggested", "rev-suggested", &html).unwrap();

        let proposed = get(&conn, "page-action:page-suggested:suggested")
            .unwrap()
            .unwrap();
        assert_eq!(proposed.values[0].value.as_deref(), Some("api"));
        assert_eq!(
            proposed.values[0].provenance,
            DiscussionActionValueProvenance::AgentSuggestion
        );
        let claimed = claim_launch(&conn, &proposed.id, &HashMap::new(), &HashMap::new())
            .unwrap()
            .unwrap();
        let LivePageActionClaimOutcome::Claimed { action, variables } = claimed else {
            panic!("expected a fresh claim");
        };
        assert_eq!(variables["service"], "api");
        assert!(action.values[0].value.is_none());
        let stored: String = conn
            .query_row(
                "SELECT values_json FROM live_page_actions WHERE id = ?1",
                [&proposed.id],
                |row| row.get(0),
            )
            .unwrap();
        let stored: Vec<DiscussionActionValue> = serde_json::from_str(&stored).unwrap();
        assert!(stored[0].value.is_none());
        assert_eq!(stored[0].suggested_value.as_deref(), Some("api"));
    }

    #[test]
    fn allow_manual_override_lets_a_human_override_an_environment_value_without_persisting_it() {
        let conn = connection();
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            &conn,
            &QuickExec {
                id: "qe-env".into(),
                name: "Push status".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "token".into(),
                    label: "Token".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: Some(PromptVariableSource::ProjectEnv),
                    source_ref: Some("<env.TOKEN>".into()),
                    allow_manual_override: true,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        let html = action_block("push", r#"{"kind":"quick_exec","target_id":"qe-env"}"#);
        insert_page(&conn, "page-env", "rev-1", &html);
        ingest_page_actions(&conn, "page-env", "rev-1", &html).unwrap();

        let proposed = get(&conn, "page-action:page-env:push").unwrap().unwrap();
        assert!(proposed.values[0].allow_manual_override);
        assert_eq!(
            proposed.values[0].provenance,
            DiscussionActionValueProvenance::ProjectEnv
        );

        let supplied = HashMap::from([("token".into(), "override-secret".into())]);
        let claimed = claim_launch(
            &conn,
            "page-action:page-env:push",
            &supplied,
            &HashMap::new(),
        )
        .unwrap()
        .unwrap();
        let LivePageActionClaimOutcome::Claimed { action, variables } = claimed else {
            panic!("an allow_manual_override variable must be claimable");
        };
        assert_eq!(variables["token"], "override-secret");
        assert!(action.values[0].value.is_none());
    }

    #[test]
    fn an_environment_value_without_override_stays_read_only() {
        let conn = connection();
        let now = Utc::now();
        crate::db::quick_execs::insert_quick_exec(
            &conn,
            &QuickExec {
                id: "qe-locked".into(),
                name: "Push status".into(),
                icon: "⌨".into(),
                description: String::new(),
                project_id: None,
                command: "printf".into(),
                args: vec![],
                timeout_secs: 30,
                output_format: CollectQuickExecOutputFormat::Json,
                variables: vec![PromptVariable {
                    name: "token".into(),
                    label: "Token".into(),
                    placeholder: String::new(),
                    description: None,
                    required: true,
                    pattern: None,
                    source: Some(PromptVariableSource::ProjectEnv),
                    source_ref: Some("<env.TOKEN>".into()),
                    allow_manual_override: false,
                    control: None,
                }],
                pinned: false,
                created_at: now,
                updated_at: now,
            },
        )
        .unwrap();
        let html = action_block("push", r#"{"kind":"quick_exec","target_id":"qe-locked"}"#);
        insert_page(&conn, "page-locked", "rev-1", &html);
        ingest_page_actions(&conn, "page-locked", "rev-1", &html).unwrap();

        let supplied = HashMap::from([("token".into(), "attempted-override".into())]);
        let result = claim_launch(
            &conn,
            "page-action:page-locked:push",
            &supplied,
            &HashMap::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn invalid_json_block_becomes_an_actionable_preflight_failure() {
        let conn = connection();
        let html = r#"<script type="application/kronn-action" data-action-id="bad">{not valid json</script>"#;
        insert_page(&conn, "page-bad", "rev-1", html);
        ingest_page_actions(&conn, "page-bad", "rev-1", html).unwrap();

        let action = get(&conn, "page-action:page-bad:bad").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert_eq!(action.kind, DiscussionActionKind::Invalid);
        assert!(action.diagnostic.unwrap().contains("JSON invalide"));
    }

    #[test]
    fn empty_target_id_becomes_an_actionable_preflight_failure() {
        let conn = connection();
        let html = action_block("empty", r#"{"kind":"workflow","target_id":""}"#);
        insert_page(&conn, "page-empty", "rev-1", &html);
        ingest_page_actions(&conn, "page-empty", "rev-1", &html).unwrap();

        let action = get(&conn, "page-action:page-empty:empty").unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert!(action.diagnostic.unwrap().contains("aucune cible"));
    }

    #[test]
    fn interrupted_claim_fails_closed_instead_of_replaying_side_effects() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "collect-logs",
            r#"{"kind":"quick_exec","target_id":"qe-1"}"#,
        );
        insert_page(&conn, "page-stale", "rev-1", &html);
        ingest_page_actions(&conn, "page-stale", "rev-1", &html).unwrap();
        let supplied = HashMap::from([("service".into(), "api".into())]);
        claim_launch(
            &conn,
            "page-action:page-stale:collect-logs",
            &supplied,
            &HashMap::new(),
        )
        .unwrap();
        conn.execute(
            "UPDATE live_page_actions SET launched_at = ?2 WHERE id = ?1",
            params![
                "page-action:page-stale:collect-logs",
                (Utc::now() - chrono::Duration::minutes(6)).to_rfc3339()
            ],
        )
        .unwrap();

        let action = get(&conn, "page-action:page-stale:collect-logs")
            .unwrap()
            .unwrap();
        assert_eq!(action.state, DiscussionActionState::Failed);
        assert!(action
            .diagnostic
            .unwrap()
            .contains("not retried automatically"));
    }

    #[test]
    fn dynamic_binding_resolves_a_snapshot_dataset_field_at_launch() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "show-kpi",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.kpis.label>"}]}"#,
        );
        insert_page(&conn, "page-kpi", "rev-1", &html);
        insert_dataset(
            &conn,
            "page-kpi",
            "kpis",
            "snapshot",
            r#"{"label":"Users"}"#,
        );
        ingest_page_actions(&conn, "page-kpi", "rev-1", &html).unwrap();

        let proposed = get(&conn, "page-action:page-kpi:show-kpi")
            .unwrap()
            .unwrap();
        assert_eq!(
            proposed.values[0].provenance,
            DiscussionActionValueProvenance::DynamicBinding
        );
        assert!(proposed.values[0].value.is_none());

        let claimed = claim_launch(
            &conn,
            "page-action:page-kpi:show-kpi",
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap()
        .unwrap();
        let LivePageActionClaimOutcome::Claimed { variables, .. } = claimed else {
            panic!("expected a fresh claim");
        };
        assert_eq!(variables["service"], "Users");
    }

    #[test]
    fn dynamic_binding_resolves_a_collection_row_via_binding_key() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "open-ticket",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).title>"}]}"#,
        );
        insert_page(&conn, "page-tix", "rev-1", &html);
        insert_dataset(
            &conn,
            "page-tix",
            "tickets",
            "collection",
            r#"[{"key":"K1","title":"Fix bug"},{"key":"K2","title":"Add feature"}]"#,
        );
        ingest_page_actions(&conn, "page-tix", "rev-1", &html).unwrap();

        let bindings = HashMap::from([("service".into(), "K2".into())]);
        let claimed = claim_launch(
            &conn,
            "page-action:page-tix:open-ticket",
            &HashMap::new(),
            &bindings,
        )
        .unwrap()
        .unwrap();
        let LivePageActionClaimOutcome::Claimed { variables, .. } = claimed else {
            panic!("expected a fresh claim");
        };
        assert_eq!(variables["service"], "Add feature");
    }

    #[test]
    fn dynamic_binding_ignores_a_client_supplied_value_and_uses_the_real_dataset_value() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "open-ticket",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).title>"}]}"#,
        );
        insert_page(&conn, "page-tix2", "rev-1", &html);
        insert_dataset(
            &conn,
            "page-tix2",
            "tickets",
            "collection",
            r#"[{"key":"K1","title":"Fix bug"},{"key":"K2","title":"Add feature"}]"#,
        );
        ingest_page_actions(&conn, "page-tix2", "rev-1", &html).unwrap();

        let supplied = HashMap::from([("service".into(), "malicious-value".into())]);
        let bindings = HashMap::from([("service".into(), "K2".into())]);
        let claimed = claim_launch(
            &conn,
            "page-action:page-tix2:open-ticket",
            &supplied,
            &bindings,
        )
        .unwrap()
        .unwrap();
        let LivePageActionClaimOutcome::Claimed { variables, .. } = claimed else {
            panic!("expected a fresh claim");
        };
        assert_eq!(
            variables["service"], "Add feature",
            "a dynamic_binding value must always be resolved server-side, never trusted from the wire"
        );
    }

    #[test]
    fn undeclared_dynamic_binding_selector_is_rejected() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "show-kpi",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.title>"}]}"#,
        );
        insert_page(&conn, "page-strict-bindings", "rev-1", &html);
        ingest_page_actions(&conn, "page-strict-bindings", "rev-1", &html).unwrap();

        let result = claim_launch(
            &conn,
            "page-action:page-strict-bindings:show-kpi",
            &HashMap::new(),
            &HashMap::from([("forged".into(), "other-row".into())]),
        );
        match result {
            Err(error) => assert!(error.to_string().contains("unknown dynamic action binding")),
            Ok(_) => panic!("an undeclared dynamic binding must be rejected"),
        }
    }

    #[test]
    fn dynamic_binding_without_a_matching_row_fails_closed() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "open-ticket",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).title>"}]}"#,
        );
        insert_page(&conn, "page-tix3", "rev-1", &html);
        insert_dataset(
            &conn,
            "page-tix3",
            "tickets",
            "collection",
            r#"[{"key":"K1","title":"Fix bug"}]"#,
        );
        ingest_page_actions(&conn, "page-tix3", "rev-1", &html).unwrap();

        let result = claim_launch(
            &conn,
            "page-action:page-tix3:open-ticket",
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            result.is_err(),
            "a missing row selector must fail closed, never launch with a guessed value"
        );
    }

    #[test]
    fn dynamic_binding_resolves_page_level_context() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "show-title",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.title>"}]}"#,
        );
        insert_page(&conn, "page-ctx", "rev-1", &html);
        ingest_page_actions(&conn, "page-ctx", "rev-1", &html).unwrap();

        let claimed = claim_launch(
            &conn,
            "page-action:page-ctx:show-title",
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap()
        .unwrap();
        let LivePageActionClaimOutcome::Claimed { variables, .. } = claimed else {
            panic!("expected a fresh claim");
        };
        assert_eq!(variables["service"], "Test Page");
    }

    #[test]
    fn a_still_proposed_action_is_refreshed_when_the_block_is_republished_in_a_new_revision() {
        let conn = connection();
        insert_target(&conn);
        insert_second_target(&conn);
        let html_v1 = action_block("cta", r#"{"kind":"quick_exec","target_id":"qe-1"}"#);
        insert_page(&conn, "page-refresh", "rev-1", &html_v1);
        ingest_page_actions(&conn, "page-refresh", "rev-1", &html_v1).unwrap();

        let html_v2 = action_block("cta", r#"{"kind":"quick_exec","target_id":"qe-2"}"#);
        republish_revision(&conn, "page-refresh", "rev-2", &html_v2);
        ingest_page_actions(&conn, "page-refresh", "rev-2", &html_v2).unwrap();

        let actions = list_for_live_page(&conn, "page-refresh").unwrap();
        assert_eq!(
            actions.len(),
            1,
            "the same action_ref must refresh in place, not duplicate"
        );
        let action = &actions[0];
        assert_eq!(action.target_id, "qe-2");
        assert_eq!(action.target_name, "Restart service");
        assert_eq!(action.live_page_revision_id, "rev-2");
        assert!(!action.stale_source);
    }

    #[test]
    fn a_removed_proposal_cannot_be_launched_through_its_old_api_id() {
        let conn = connection();
        insert_target(&conn);
        let html_v1 = action_block("removed", r#"{"kind":"quick_exec","target_id":"qe-1"}"#);
        insert_page(&conn, "page-removed", "rev-1", &html_v1);
        ingest_page_actions(&conn, "page-removed", "rev-1", &html_v1).unwrap();
        republish_revision(
            &conn,
            "page-removed",
            "rev-2",
            "<p>The CTA was removed.</p>",
        );

        let action = get(&conn, "page-action:page-removed:removed")
            .unwrap()
            .unwrap();
        assert!(action.stale_source);
        assert_eq!(action.state, DiscussionActionState::Proposed);
        let error = match claim_launch(
            &conn,
            &action.id,
            &HashMap::from([("service".into(), "api".into())]),
            &HashMap::new(),
        ) {
            Err(error) => error,
            Ok(_) => panic!("a removed proposal must fail closed"),
        };
        assert!(error.to_string().contains("no longer present"));
        assert_eq!(
            get(&conn, &action.id).unwrap().unwrap().state,
            DiscussionActionState::Proposed,
            "a rejected stale launch must not claim the old proposal"
        );
    }

    #[test]
    fn a_launched_action_is_frozen_and_becomes_stale_source_after_a_later_revision() {
        let conn = connection();
        insert_target(&conn);
        let html_v1 = action_block("cta", r#"{"kind":"quick_exec","target_id":"qe-1"}"#);
        insert_page(&conn, "page-frozen", "rev-1", &html_v1);
        ingest_page_actions(&conn, "page-frozen", "rev-1", &html_v1).unwrap();
        let supplied = HashMap::from([("service".into(), "api".into())]);
        claim_launch(
            &conn,
            "page-action:page-frozen:cta",
            &supplied,
            &HashMap::new(),
        )
        .unwrap();
        complete(
            &conn,
            "page-action:page-frozen:cta",
            kronn_action_engine::ActionCompletion {
                state: DiscussionActionState::Succeeded,
                shared_run_id: None,
                result_discussion_id: None,
                deep_link: Some("automation:quick_exec:run-1".into()),
                diagnostic: None,
            },
        )
        .unwrap();

        // The agent removes the CTA entirely in a later revision.
        let html_v2 = "<p>No more actions here.</p>";
        republish_revision(&conn, "page-frozen", "rev-2", html_v2);
        ingest_page_actions(&conn, "page-frozen", "rev-2", html_v2).unwrap();

        let action = get(&conn, "page-action:page-frozen:cta").unwrap().unwrap();
        assert_eq!(
            action.state,
            DiscussionActionState::Succeeded,
            "a launched action's definition must never be mutated by a later edit"
        );
        assert_eq!(action.live_page_revision_id, "rev-1");
        assert!(
            action.stale_source,
            "the Page has moved to a newer revision than the one this action ran against"
        );
    }

    #[test]
    fn multiple_action_refs_on_the_same_page_are_isolated() {
        let conn = connection();
        insert_target(&conn);
        insert_second_target(&conn);
        let html = format!(
            "{}{}",
            action_block("cta-a", r#"{"kind":"quick_exec","target_id":"qe-1"}"#),
            action_block("cta-b", r#"{"kind":"quick_exec","target_id":"qe-2"}"#),
        );
        insert_page(&conn, "page-multi", "rev-1", &html);
        ingest_page_actions(&conn, "page-multi", "rev-1", &html).unwrap();

        let actions = list_for_live_page(&conn, "page-multi").unwrap();
        assert_eq!(actions.len(), 2);

        cancel(&conn, "page-action:page-multi:cta-a").unwrap();
        let a = get(&conn, "page-action:page-multi:cta-a").unwrap().unwrap();
        let b = get(&conn, "page-action:page-multi:cta-b").unwrap().unwrap();
        assert_eq!(a.state, DiscussionActionState::Cancelled);
        assert_eq!(
            b.state,
            DiscussionActionState::Proposed,
            "cancelling one CTA must never affect another CTA's own instance"
        );
    }

    #[test]
    fn quick_prompt_completion_persists_both_page_and_discussion_anchors() {
        let conn = connection();
        insert_all_target_kinds(&conn);
        let html = action_block("frame", r#"{"kind":"quick_prompt","target_id":"qp-1"}"#);
        insert_page(&conn, "page-trace", "rev-trace", &html);
        ingest_page_actions(&conn, "page-trace", "rev-trace", &html).unwrap();
        let action_id = "page-action:page-trace:frame";
        let claimed = claim_launch(&conn, action_id, &HashMap::new(), &HashMap::new()).unwrap();
        assert!(matches!(
            claimed,
            Some(LivePageActionClaimOutcome::Claimed { .. })
        ));
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
             VALUES ('disc-page-result', 'Page result', 'ClaudeCode', 'fr', '[]', ?1, ?1)",
            [&now],
        )
        .unwrap();

        complete_quick_prompt(&conn, action_id, "page-trace", "disc-page-result").unwrap();

        let action = get(&conn, action_id).unwrap().unwrap();
        assert_eq!(action.state, DiscussionActionState::Succeeded);
        assert_eq!(
            action.result_discussion_id.as_deref(),
            Some("disc-page-result")
        );
        let link: (String, String) = conn
            .query_row(
                "SELECT page_id, relation FROM live_page_discussion_links WHERE discussion_id = 'disc-page-result'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(link, ("page-trace".into(), "attached".into()));
    }

    #[test]
    fn extract_page_action_blocks_ignores_prose_and_other_script_tags() {
        let html = r#"<p>Some prose</p>
<script>console.log('not an action');</script>
<script type="application/kronn-action" data-action-id="only-one">{"kind":"workflow","target_id":"wf-1"}</script>
<p>after</p>"#;
        let blocks = extract_page_action_blocks(html);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "only-one");
        assert_eq!(blocks[0].1, r#"{"kind":"workflow","target_id":"wf-1"}"#);
    }

    #[test]
    fn extract_page_action_blocks_requires_exact_attributes_and_a_url_safe_ref() {
        let oversized = "x".repeat(257);
        let html = format!(
            r#"<script data-note="application/kronn-action" type="application/json" data-action-id="wrong-type">{{}}</script>
<script type="application/kronn-action" x-data-action-id="prefixed">{{}}</script>
<scripture type="application/kronn-action" data-action-id="wrong-tag">{{}}</scripture>
<script type="application/kronn-action" data-action-id="../route">{{}}</script>
<script type="application/kronn-action" data-action-id="{oversized}">{{}}</script>
<script data-note="a > b" TYPE = 'APPLICATION/KRONN-ACTION' DATA-ACTION-ID = "safe_ref-1.0~">{{"kind":"workflow","target_id":"wf-1"}}</script>"#
        );

        let blocks = extract_page_action_blocks(&html);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, "safe_ref-1.0~");
    }
}
