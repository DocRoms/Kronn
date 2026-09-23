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
//!
//! **A block is a gabarit, not a proposal** (KT-678). The page instantiates it
//! once per dataset row, so one declaration draws as many buttons as the list
//! is long — 39 tickets, 39 buttons, 39 liaisons, one `action_ref`. The
//! declaration here is therefore the *offer* and is never consumed: it carries
//! no execution state at all. Each click becomes a row of its own in
//! `live_page_action_launches`, identified by the binding it was clicked on,
//! and that row is what the shared state machine drives. What the API speaks
//! stays a single `LivePageAction`: the declaration alone before any click,
//! the declaration joined to one of its launches afterwards.

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

/// One Page action as the API speaks it: a declaration, plus the launch it
/// produced when there is one.
///
/// `id` is the handle for *this* card, not for the block: the declaration's id
/// before a click, that launch's id afterwards. A card therefore polls its own
/// launch instead of the last one anybody happened to start — which is what
/// made 39 buttons report the first ticket's success (KT-678).
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
    /// KT-582 — the target's project, by name, for the same reason as the
    /// discussion card: the guard is gone, so the card has to say where it runs.
    pub project_name: Option<String>,
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
    /// The row a launch was clicked on — sorted `name=selector` pairs joined by
    /// U+001F, empty for an unbound CTA. `None` on a declaration, which belongs
    /// to every row at once.
    pub binding_key: Option<String>,
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
        // KT-582 — lifted with the discussion one, and for the same reason. The
        // two surfaces must behave alike: the same gesture cannot mean two
        // things depending on where the card is drawn. Server-side resolution
        // of dataset-bound values is untouched and still refuses anything the
        // client declares on that path.
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
    // No `WHERE state IN (...)` guard any more: a declaration only ever holds
    // `proposed` or `preflight_failed`, so republishing always refreshes it.
    // The guard used to exclude rows that had launched, which is precisely how
    // a `succeeded` CTA stayed dead through every later republish (KT-678).
    conn.execute(
        "INSERT INTO live_page_actions (
             id, live_page_id, live_page_revision_id, action_ref, kind,
             target_id, target_name, project_id, state, values_json,
             diagnostic, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)
         ON CONFLICT(live_page_id, action_ref) DO UPDATE SET
             live_page_revision_id = excluded.live_page_revision_id,
             kind = excluded.kind,
             target_id = excluded.target_id,
             target_name = excluded.target_name,
             project_id = excluded.project_id,
             state = excluded.state,
             values_json = excluded.values_json,
             diagnostic = excluded.diagnostic,
             updated_at = excluded.updated_at",
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
            diagnostic,
            now,
        ],
    )?;
    Ok(())
}

/// The offer on its own — what every button shows before it is clicked. The
/// execution columns are literal NULLs because a declaration has none: it is
/// never launched, only instantiated.
const SELECT_DECLARATION: &str = "SELECT a.id, a.live_page_id, a.live_page_revision_id,
    a.action_ref, a.kind, a.target_id, a.target_name, a.project_id, a.state, a.values_json,
    NULL, NULL, NULL, a.diagnostic, NULL, NULL, a.created_at, a.updated_at,
    (a.live_page_revision_id != p.current_revision_id) AS stale_source,
    proj.name, NULL
    -- LEFT for the project: one deleted after the proposal must still return
    -- the card, with no name rather than no card.
    FROM live_page_actions a JOIN live_pages p ON p.id = a.live_page_id
    LEFT JOIN projects proj ON proj.id = a.project_id";

/// One launch, presented under its declaration's identity so the client keeps
/// reading a single shape. Same column order as `SELECT_DECLARATION`, so both
/// go through `map_action`. Everything but the Page anchor comes from the
/// launch itself, frozen at the click: a later republish moves the
/// declaration on, never what already ran.
const SELECT_LAUNCH: &str = "SELECT l.id, a.live_page_id, l.live_page_revision_id,
    a.action_ref, l.kind, l.target_id, l.target_name, l.project_id, l.state, l.values_json,
    l.shared_run_id, l.result_discussion_id, l.deep_link, l.diagnostic, l.launched_at,
    l.finished_at, l.created_at, l.updated_at,
    (l.live_page_revision_id != p.current_revision_id) AS stale_source,
    proj.name, l.binding_key
    FROM live_page_action_launches l
    JOIN live_page_actions a ON a.id = l.action_id
    JOIN live_pages p ON p.id = a.live_page_id
    LEFT JOIN projects proj ON proj.id = l.project_id";

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
        project_name: row.get(19)?,
        binding_key: row.get(20)?,
    })
}

fn reconcile(
    mode: kronn_action_engine::Reconcile,
    conn: &Connection,
    action: &mut LivePageAction,
) -> Result<()> {
    let mut core = kronn_action_engine::ActionCore {
        id: action.id.clone(),
        state: action.state,
        values: std::mem::take(&mut action.values),
        shared_run_id: action.shared_run_id.clone(),
        result_discussion_id: action.result_discussion_id.clone(),
        diagnostic: action.diagnostic.clone(),
        launched_at: action.launched_at.clone(),
        finished_at: action.finished_at.clone(),
        updated_at: action.updated_at.clone(),
    };
    kronn_action_engine::reconcile(conn, ActionTable::LivePageLaunch, &mut core, mode)?;
    action.state = core.state;
    action.values = core.values;
    action.diagnostic = core.diagnostic;
    action.finished_at = core.finished_at;
    action.updated_at = core.updated_at;
    Ok(())
}

fn launch_by_id(
    mode: kronn_action_engine::Reconcile,
    conn: &Connection,
    launch_id: &str,
) -> Result<Option<LivePageAction>> {
    let mut action = conn
        .query_row(
            &format!("{SELECT_LAUNCH} WHERE l.id = ?1"),
            [launch_id],
            map_action,
        )
        .optional()?;
    if let Some(action) = action.as_mut() {
        reconcile(mode, conn, action)?;
    }
    Ok(action)
}

fn declaration_by_id(conn: &Connection, action_id: &str) -> Result<Option<LivePageAction>> {
    // No reconciliation: a declaration has no run behind it to reconcile with.
    Ok(conn
        .query_row(
            &format!("{SELECT_DECLARATION} WHERE a.id = ?1"),
            [action_id],
            map_action,
        )
        .optional()?)
}

/// Resolve a card handle, whichever half of the model it names — the launch a
/// card is following, or the declaration it has not launched yet.
pub fn get(
    mode: kronn_action_engine::Reconcile,
    conn: &Connection,
    id: &str,
) -> Result<Option<LivePageAction>> {
    match launch_by_id(mode, conn, id)? {
        Some(action) => Ok(Some(action)),
        None => declaration_by_id(conn, id),
    }
}

/// Every offer the Page currently carries. Launches are deliberately absent:
/// this is what arms the buttons, and a button is armed as long as its block
/// is on the page — no matter how many times it, or its neighbours, have run.
pub fn list_for_live_page(conn: &Connection, live_page_id: &str) -> Result<Vec<LivePageAction>> {
    let mut statement = conn.prepare(&format!(
        "{SELECT_DECLARATION} WHERE a.live_page_id = ?1 ORDER BY a.created_at, a.action_ref"
    ))?;
    let actions = statement
        .query_map([live_page_id], map_action)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(actions)
}

/// The latest launch of every row that has run, so a Page can show each
/// button's state and reopen the run behind it. One entry per binding, which
/// bounds the answer by the rows on the page rather than by its history. A
/// decline is not a run and never marks a button.
pub fn latest_launches_for_live_page(
    mode: kronn_action_engine::Reconcile,
    conn: &Connection,
    live_page_id: &str,
) -> Result<Vec<LivePageAction>> {
    let mut statement = conn.prepare(&format!(
        "{SELECT_LAUNCH} WHERE l.id IN (
             SELECT id FROM (
                 SELECT l2.id, ROW_NUMBER() OVER (
                     PARTITION BY l2.action_id, l2.binding_key
                     ORDER BY l2.created_at DESC, l2.id DESC
                 ) AS position
                 FROM live_page_action_launches l2
                 JOIN live_page_actions a2 ON a2.id = l2.action_id
                 WHERE a2.live_page_id = ?1 AND l2.state NOT IN ('proposed', 'cancelled')
             ) WHERE position = 1
         )
         ORDER BY a.action_ref, l.binding_key"
    ))?;
    let mut launches = statement
        .query_map([live_page_id], map_action)?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for launch in &mut launches {
        reconcile(mode, conn, launch)?;
    }
    Ok(launches)
}

/// Per (Page, action_ref), across all row bindings and revisions. Active
/// launches are never evicted; their result/discussion rows have their own life.
const MAX_RETAINED_TERMINAL_LAUNCHES: i64 = 1_000;

fn declaration_for_launch(conn: &Connection, id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT action_id FROM live_page_action_launches WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Called only inside a write transaction/savepoint, never from Page polling.
fn retain_launch_history(conn: &Connection, action_id: &str, keep_id: &str) -> Result<()> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM live_page_action_launches WHERE action_id=?1",
        [action_id],
        |row| row.get(0),
    )?;
    if count <= MAX_RETAINED_TERMINAL_LAUNCHES {
        return Ok(());
    }
    // Projected GETs intentionally do not persist terminal states. Reconcile
    // those rows here, or completed asynchronous runs would evade retention.
    // Only metadata is read: no snapshots, step outputs or stderr payloads.
    let mut statement = conn.prepare("SELECT id,state,shared_run_id,result_discussion_id,diagnostic,launched_at,finished_at,updated_at FROM live_page_action_launches WHERE action_id=?1 AND state IN ('launching','running')")?;
    let active = statement
        .query_map([action_id], |row| {
            Ok(kronn_action_engine::ActionCore {
                id: row.get(0)?,
                state: if row.get::<_, String>(1)? == "launching" {
                    DiscussionActionState::Launching
                } else {
                    DiscussionActionState::Running
                },
                values: Vec::new(),
                shared_run_id: row.get(2)?,
                result_discussion_id: row.get(3)?,
                diagnostic: row.get(4)?,
                launched_at: row.get(5)?,
                finished_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    for mut core in active {
        kronn_action_engine::reconcile(
            conn,
            ActionTable::LivePageLaunch,
            &mut core,
            kronn_action_engine::Reconcile::Persisted,
        )?;
    }
    conn.execute(
        "DELETE FROM live_page_action_launches WHERE id IN (
            SELECT id FROM live_page_action_launches
            WHERE action_id=?1 AND state IN ('succeeded','failed','cancelled','preflight_failed')
            ORDER BY (id=?2) DESC, julianday(COALESCE(finished_at,updated_at,created_at)) DESC, created_at DESC, id DESC
            LIMIT -1 OFFSET ?3
        )",
        params![action_id,keep_id,MAX_RETAINED_TERMINAL_LAUNCHES],
    )?;
    Ok(())
}

/// Declining an offer is an act like launching it, and just as much a
/// per-click one: it is recorded as its own cancelled launch. The declaration
/// is left alone, so the other rows — and this one, on the next click — stay
/// armed. Cancelling a launch handle is the engine's idempotent no-op.
pub fn cancel(conn: &Connection, id: &str) -> Result<Option<LivePageAction>> {
    let transaction = conn.unchecked_transaction()?;
    if let Some(action_id) = declaration_for_launch(&transaction, id)? {
        kronn_action_engine::cancel(&transaction, ActionTable::LivePageLaunch, id)?;
        retain_launch_history(&transaction, &action_id, id)?;
        let action = launch_by_id(kronn_action_engine::Reconcile::Persisted, &transaction, id)?;
        transaction.commit()?;
        return Ok(action);
    }
    let Some(declaration) = declaration_by_id(&transaction, id)? else {
        return Ok(None);
    };
    let now = Utc::now().to_rfc3339();
    let launch_id = format!("page-launch:{}", uuid::Uuid::new_v4());
    // Unknown binding: a decline carries no selector and is never in flight.
    insert_launch(
        &transaction,
        &launch_id,
        &declaration,
        "",
        DiscussionActionState::Cancelled,
        &now,
    )?;
    retain_launch_history(&transaction, &declaration.id, &launch_id)?;
    let action = launch_by_id(
        kronn_action_engine::Reconcile::Persisted,
        &transaction,
        &launch_id,
    )?;
    transaction.commit()?;
    Ok(action)
}

/// Open a launch row from its declaration, copying what it runs against so
/// that record stays true whatever the page becomes afterwards.
fn insert_launch(
    conn: &Connection,
    launch_id: &str,
    declaration: &LivePageAction,
    binding_key: &str,
    state: DiscussionActionState,
    now: &str,
) -> Result<()> {
    let finished_at = (state == DiscussionActionState::Cancelled).then_some(now);
    conn.execute(
        "INSERT INTO live_page_action_launches (
             id, action_id, binding_key, live_page_revision_id, kind, target_id,
             target_name, project_id, state, values_json, finished_at,
             created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12)",
        params![
            launch_id,
            declaration.id,
            binding_key,
            declaration.live_page_revision_id,
            declaration.kind.as_db_str(),
            declaration.target_id,
            declaration.target_name,
            declaration.project_id,
            kronn_action_engine::state_db_str(state),
            serde_json::to_string(&declaration.values)?,
            finished_at,
            now,
        ],
    )?;
    Ok(())
}

/// The identity of one click: the row it bound to, as sorted `name=selector`
/// pairs. Empty for a CTA with no dynamic binding, which is one logical
/// instance and, with the guard scoped here, relaunchable once it is done.
fn binding_key(bindings: &HashMap<String, String>) -> String {
    let mut pairs: Vec<String> = bindings
        .iter()
        .map(|(name, selector)| format!("{name}={selector}"))
        .collect();
    pairs.sort();
    pairs.join("\u{1f}")
}

/// The launch still running for this exact binding, if there is one, brought
/// up to date with the run behind it first — a launch that has since finished
/// is not in flight and must not stand in the way of the next click.
fn in_flight_launch(
    conn: &Connection,
    action_id: &str,
    binding_key: &str,
) -> Result<Option<LivePageAction>> {
    let candidate = conn
        .query_row(
            &format!(
                "{SELECT_LAUNCH} WHERE l.action_id = ?1 AND l.binding_key = ?2
                 AND l.state IN ('launching','running')
                 ORDER BY l.created_at DESC, l.id DESC LIMIT 1"
            ),
            params![action_id, binding_key],
            map_action,
        )
        .optional()?;
    let Some(mut candidate) = candidate else {
        return Ok(None);
    };
    reconcile(
        kronn_action_engine::Reconcile::Persisted,
        conn,
        &mut candidate,
    )?;
    Ok(matches!(
        candidate.state,
        DiscussionActionState::Launching | DiscussionActionState::Running
    )
    .then_some(candidate))
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

/// Launch one instance of a Page action — the one the click bound to.
///
/// `id` is normally a declaration id. A launch id is accepted too and answered
/// with that launch as it stands, so a second click on a card that already
/// launched reports its run instead of starting another.
pub fn claim_launch(
    conn: &Connection,
    id: &str,
    supplied: &HashMap<String, String>,
    bindings: &HashMap<String, String>,
) -> Result<Option<LivePageActionClaimOutcome>> {
    let transaction = conn.unchecked_transaction()?;
    let Some(mut action) = declaration_by_id(&transaction, id)? else {
        let existing = launch_by_id(kronn_action_engine::Reconcile::Persisted, &transaction, id)?;
        transaction.commit()?;
        return Ok(existing.map(LivePageActionClaimOutcome::Existing));
    };
    if action.state != DiscussionActionState::Proposed {
        // Kronn could not make sense of the block; there is nothing to launch
        // and the card shows why.
        transaction.commit()?;
        return Ok(Some(LivePageActionClaimOutcome::Existing(action)));
    }
    if action.stale_source {
        anyhow::bail!(
            "this action is no longer present in the current Page revision; reload the Page and use a current CTA"
        );
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
    // The idempotency guard, scoped to the binding rather than to the block:
    // clicking a row that is still running shows that run, clicking any other
    // row launches it.
    let binding_key = binding_key(bindings);
    if let Some(running) = in_flight_launch(&transaction, &action.id, &binding_key)? {
        transaction.commit()?;
        return Ok(Some(LivePageActionClaimOutcome::Existing(running)));
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

    // The launch starts as a copy of the offer's contract; the engine then
    // claims it exactly as it claims a discussion proposal. The declaration
    // itself is never written to.
    let now = Utc::now().to_rfc3339();
    let launch_id = format!("page-launch:{}", uuid::Uuid::new_v4());
    insert_launch(
        &transaction,
        &launch_id,
        &action,
        &binding_key,
        DiscussionActionState::Proposed,
        &now,
    )?;
    let mut core = kronn_action_engine::ActionCore {
        id: launch_id.clone(),
        state: DiscussionActionState::Proposed,
        values: std::mem::take(&mut action.values),
        shared_run_id: None,
        result_discussion_id: None,
        diagnostic: None,
        launched_at: None,
        finished_at: None,
        updated_at: now.clone(),
    };
    let target_still_exists =
        discussion_actions::target_contract(&transaction, action.kind, &action.target_id)?
            .is_some();
    let claimed_variables = kronn_action_engine::claim_launch(
        &transaction,
        ActionTable::LivePageLaunch,
        &mut core,
        &resolved_supplied,
        target_still_exists,
    )?;
    retain_launch_history(&transaction, &action.id, &launch_id)?;
    action.id = launch_id;
    action.binding_key = Some(binding_key);
    action.created_at = now;
    action.state = core.state;
    action.values = core.values;
    action.diagnostic = core.diagnostic;
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
    // complete_quick_prompt already owns a transaction. A savepoint composes
    // with it and also makes standalone completion + retention atomic.
    conn.execute_batch("SAVEPOINT complete_page_action")?;
    let result = (|| -> Result<()> {
        kronn_action_engine::complete(conn, ActionTable::LivePageLaunch, id, completion)?;
        if let Some(action_id) = declaration_for_launch(conn, id)? {
            retain_launch_history(conn, &action_id, id)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("RELEASE complete_page_action")?;
            Ok(())
        }
        Err(error) => {
            let _ = conn
                .execute_batch("ROLLBACK TO complete_page_action; RELEASE complete_page_action");
            Err(error)
        }
    }
}

/// Record a Page-authored QP's result discussion and register it on the Page,
/// atomically. The launch stays `running`: it succeeds when the agent answers.
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
            // The discussion exists; the answer does not yet.
            state: DiscussionActionState::Running,
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
#[path = "live_page_action_retention_tests.rs"]
mod retention_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AgentType, CollectQuickExecOutputFormat, ModelTier, PromptVariable, PromptVariableSource,
        QuickApi, QuickExec, QuickPrompt,
    };

    pub(super) fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    /// Reproduces the outage exactly: a real read-only handle, not a pragma.
    #[test]
    fn listing_a_launched_action_never_writes_and_still_tells_the_truth() {
        // "Unable to list Page actions: attempt to write a readonly database".
        // Before any CTA was launched every row was `proposed`, the
        // reconciliation returned early, nothing was written and the endpoint
        // answered — which is why it looked healthy. From the first launch the
        // listing attempted an UPDATE on the read-only companion and the whole
        // page fell over, for everyone, self-sustainingly: the reconciliation
        // that would have moved the state on is exactly what was refused.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kronn.db");
        {
            let conn = Connection::open(&path).unwrap();
            crate::db::migrations::run(&conn).unwrap();
            insert_page(&conn, "page-1", "rev-1", "<p>x</p>");
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO shared_runs (id, kind, source_id, status, created_at, updated_at)
                 VALUES ('run-1', 'quick_prompt', 'qp-1', 'success', ?1, ?1)",
                params![now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO live_page_actions (
                     id, live_page_id, live_page_revision_id, action_ref, kind,
                     target_id, target_name, project_id, state, values_json,
                     created_at, updated_at
                 ) VALUES ('act-1','page-1','rev-1','cta-1','quick_prompt',
                           'qp-1','Framer',NULL,'proposed','[]',?1,?1)",
                params![now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO live_page_action_launches (
                     id, action_id, binding_key, live_page_revision_id, kind,
                     target_id, target_name, state, values_json, shared_run_id,
                     launched_at, created_at, updated_at
                 ) VALUES ('launch-1','act-1','','rev-1','quick_prompt','qp-1',
                           'Framer','running','[]','run-1',?1,?1,?1)",
                params![now],
            )
            .unwrap();
        }

        // The companion connection as production opens it (ADR-001 O2): the
        // guarantee is in the file handle, not in a pragma a closure could flip.
        let read_only = Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .unwrap();

        let actions =
            list_for_live_page(&read_only, "page-1").expect("a listing must never need to write");
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].state, DiscussionActionState::Proposed);

        // The card polls its launch through the same companion. Projected, not
        // merely unwritten: the reader still sees the truth, or the page would
        // show a run that finished long ago as still running.
        let polled = get(
            kronn_action_engine::Reconcile::Projected,
            &read_only,
            "launch-1",
        )
        .expect("a poll must never need to write")
        .unwrap();
        assert_eq!(polled.state, DiscussionActionState::Succeeded);

        // The row itself is untouched — this connection cannot write, and the
        // projection did not pretend otherwise.
        let stored: String = read_only
            .query_row(
                "SELECT state FROM live_page_action_launches WHERE id = 'launch-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, "running");

        // And the guard that broke the page is real: asking the same read-only
        // connection to persist still fails. The fix is the caller's choice,
        // not a weakened connection.
        assert!(
            get(
                kronn_action_engine::Reconcile::Persisted,
                &read_only,
                "launch-1"
            )
            .is_err(),
            "a read-only connection must still refuse a write"
        );
    }

    pub(super) fn insert_page(conn: &Connection, page_id: &str, revision_id: &str, html: &str) {
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

    pub(super) fn action_block(action_ref: &str, json: &str) -> String {
        format!(
            r#"<div><button data-kronn-action="{action_ref}">Go</button>
<script type="application/kronn-action" data-action-id="{action_ref}">{json}</script></div>"#
        )
    }

    pub(super) fn insert_target(conn: &Connection) {
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

        let action = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-2:missing",
        )
        .unwrap()
        .unwrap();
        assert_eq!(action.state, DiscussionActionState::PreflightFailed);
        assert!(action.diagnostic.unwrap().contains("n’existe plus"));
    }

    #[test]
    /// KT-582 — the twin of the discussion case, flipped for the same reason.
    /// The two surfaces must agree: the same gesture cannot mean two things
    /// depending on where the card is drawn.
    fn a_cross_project_page_action_is_launchable() {
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

        let action = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-project:cross-project",
        )
        .unwrap()
        .unwrap();
        assert_eq!(action.state, DiscussionActionState::Proposed);
        assert_eq!(action.diagnostic, None);
        assert_eq!(action.project_id.as_deref(), Some("project-other"));
        // With the variable the target's contract requires: that check must keep
        // refusing, and does.
        let variables = HashMap::from([("service".to_string(), "api".to_string())]);
        let outcome = claim_launch(&conn, &action.id, &variables, &HashMap::new()).unwrap();
        assert!(matches!(
            outcome,
            Some(LivePageActionClaimOutcome::Claimed { .. })
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
        let LivePageActionClaimOutcome::Existing(second) = second else {
            panic!("a second click on a binding still in flight must not launch again");
        };
        assert_eq!(second.id, action.id, "it shows the launch already running");
        assert_eq!(second.state, DiscussionActionState::Launching);
        let offer = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-3:collect-logs",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            offer.state,
            DiscussionActionState::Proposed,
            "the declaration is never consumed by a launch"
        );
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

        let proposed = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-suggested:suggested",
        )
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
                "SELECT values_json FROM live_page_action_launches WHERE id = ?1",
                [&action.id],
                |row| row.get(0),
            )
            .unwrap();
        let stored: Vec<DiscussionActionValue> = serde_json::from_str(&stored).unwrap();
        assert!(stored[0].value.is_none());
        assert_eq!(stored[0].suggested_value.as_deref(), Some("api"));
        // The next click on the same button opens prefilled again.
        let offer = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            &proposed.id,
        )
        .unwrap()
        .unwrap();
        assert_eq!(offer.values[0].value.as_deref(), Some("api"));
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

        let proposed = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-env:push",
        )
        .unwrap()
        .unwrap();
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

        let action = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-bad:bad",
        )
        .unwrap()
        .unwrap();
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

        let action = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-empty:empty",
        )
        .unwrap()
        .unwrap();
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
        let Some(LivePageActionClaimOutcome::Claimed { action: launch, .. }) = claim_launch(
            &conn,
            "page-action:page-stale:collect-logs",
            &supplied,
            &HashMap::new(),
        )
        .unwrap() else {
            panic!("expected a fresh claim");
        };
        conn.execute(
            "UPDATE live_page_action_launches SET launched_at = ?2 WHERE id = ?1",
            params![
                launch.id,
                (Utc::now() - chrono::Duration::minutes(6)).to_rfc3339()
            ],
        )
        .unwrap();

        let action = get(kronn_action_engine::Reconcile::Persisted, &conn, &launch.id)
            .unwrap()
            .unwrap();
        assert_eq!(action.state, DiscussionActionState::Failed);
        assert!(action
            .diagnostic
            .unwrap()
            .contains("not retried automatically"));

        // Failing closed must not wedge the button: the dead launch is no
        // longer in flight, so the next click on that row is a new launch.
        let retry = claim_launch(
            &conn,
            "page-action:page-stale:collect-logs",
            &supplied,
            &HashMap::new(),
        )
        .unwrap();
        assert!(matches!(
            retry,
            Some(LivePageActionClaimOutcome::Claimed { action, .. }) if action.id != launch.id
        ));
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

        let proposed = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-kpi:show-kpi",
        )
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

    /// The reported shape: one "Framer" block over a list of tickets, one
    /// button per row. Clicking a second ticket after the first succeeded used
    /// to answer with the first ticket's success, and nothing ran.
    #[test]
    fn each_row_of_a_listed_action_launches_its_own_run() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "frame",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).key>"}]}"#,
        );
        insert_page(&conn, "page-team", "rev-1", &html);
        insert_dataset(
            &conn,
            "page-team",
            "tickets",
            "collection",
            r#"[{"key":"EW-7706"},{"key":"EW-7704"},{"key":"EW-7701"}]"#,
        );
        ingest_page_actions(&conn, "page-team", "rev-1", &html).unwrap();
        let declaration = "page-action:page-team:frame";
        let click = |ticket: &str| {
            claim_launch(
                &conn,
                declaration,
                &HashMap::new(),
                &HashMap::from([("service".into(), ticket.into())]),
            )
            .unwrap()
            .unwrap()
        };

        let LivePageActionClaimOutcome::Claimed {
            action: first,
            variables,
        } = click("EW-7706")
        else {
            panic!("the first ticket launches");
        };
        assert_eq!(variables["service"], "EW-7706");
        complete(
            &conn,
            &first.id,
            kronn_action_engine::ActionCompletion {
                state: DiscussionActionState::Succeeded,
                shared_run_id: None,
                result_discussion_id: None,
                deep_link: Some("automation:quick_exec:run-7706".into()),
                diagnostic: None,
            },
        )
        .unwrap();

        let LivePageActionClaimOutcome::Claimed {
            action: second,
            variables,
        } = click("EW-7704")
        else {
            panic!("another ticket must launch, not replay the first one's result");
        };
        assert_eq!(variables["service"], "EW-7704");
        assert_ne!(second.id, first.id);
        assert_eq!(second.state, DiscussionActionState::Launching);
        assert_eq!(
            second.deep_link, None,
            "it does not carry the first run's link"
        );

        // A ticket still in flight is not launched twice, and the others are
        // unaffected by it.
        assert!(matches!(
            click("EW-7704"),
            LivePageActionClaimOutcome::Existing(ref running) if running.id == second.id
        ));
        assert!(matches!(
            click("EW-7701"),
            LivePageActionClaimOutcome::Claimed { .. }
        ));

        let first_now = get(kronn_action_engine::Reconcile::Persisted, &conn, &first.id)
            .unwrap()
            .unwrap();
        assert_eq!(first_now.state, DiscussionActionState::Succeeded);
        let offers = list_for_live_page(&conn, "page-team").unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(
            offers[0].state,
            DiscussionActionState::Proposed,
            "every button stays armed, however many rows have run"
        );
    }

    #[test]
    fn the_page_reads_one_latest_launch_per_row_and_never_a_decline() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block(
            "frame",
            r#"{"kind":"quick_exec","target_id":"qe-1","values":[{"name":"service","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).key>"}]}"#,
        );
        insert_page(&conn, "page-states", "rev-1", &html);
        insert_dataset(
            &conn,
            "page-states",
            "tickets",
            "collection",
            r#"[{"key":"EW-1"},{"key":"EW-2"}]"#,
        );
        ingest_page_actions(&conn, "page-states", "rev-1", &html).unwrap();
        let click = |ticket: &str| {
            let Some(LivePageActionClaimOutcome::Claimed { action, .. }) = claim_launch(
                &conn,
                "page-action:page-states:frame",
                &HashMap::new(),
                &HashMap::from([("service".into(), ticket.into())]),
            )
            .unwrap() else {
                panic!("expected a fresh claim for {ticket}");
            };
            action
        };
        let finish = |id: &str| {
            complete(
                &conn,
                id,
                kronn_action_engine::ActionCompletion {
                    state: DiscussionActionState::Failed,
                    shared_run_id: None,
                    result_discussion_id: None,
                    deep_link: None,
                    diagnostic: Some("boom".into()),
                },
            )
            .unwrap();
        };

        let first_try = click("EW-1");
        assert_eq!(first_try.binding_key.as_deref(), Some("service=EW-1"));
        finish(&first_try.id);
        let second_try = click("EW-1");
        let other_row = click("EW-2");
        cancel(&conn, "page-action:page-states:frame").unwrap();

        let latest = latest_launches_for_live_page(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-states",
        )
        .unwrap();
        let seen: Vec<(Option<&str>, &str, DiscussionActionState)> = latest
            .iter()
            .map(|launch| {
                (
                    launch.binding_key.as_deref(),
                    launch.id.as_str(),
                    launch.state,
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                (
                    Some("service=EW-1"),
                    second_try.id.as_str(),
                    DiscussionActionState::Launching
                ),
                (
                    Some("service=EW-2"),
                    other_row.id.as_str(),
                    DiscussionActionState::Launching
                ),
            ],
            "one entry per row, the newest one, and the decline is not a run"
        );
    }

    #[test]
    fn a_cta_without_binding_can_be_launched_again_once_its_run_is_over() {
        let conn = connection();
        insert_target(&conn);
        let html = action_block("refresh", r#"{"kind":"quick_exec","target_id":"qe-1"}"#);
        insert_page(&conn, "page-once", "rev-1", &html);
        ingest_page_actions(&conn, "page-once", "rev-1", &html).unwrap();
        let supplied = HashMap::from([("service".into(), "api".into())]);
        let launch = || {
            claim_launch(
                &conn,
                "page-action:page-once:refresh",
                &supplied,
                &HashMap::new(),
            )
            .unwrap()
            .unwrap()
        };

        let LivePageActionClaimOutcome::Claimed { action: first, .. } = launch() else {
            panic!("expected a fresh claim");
        };
        complete(
            &conn,
            &first.id,
            kronn_action_engine::ActionCompletion {
                state: DiscussionActionState::Failed,
                shared_run_id: None,
                result_discussion_id: None,
                deep_link: None,
                diagnostic: Some("boom".into()),
            },
        )
        .unwrap();

        let LivePageActionClaimOutcome::Claimed { action: again, .. } = launch() else {
            panic!("a finished run must not turn the button into a single-use one");
        };
        assert_ne!(again.id, first.id);
        assert_eq!(again.diagnostic, None);
    }

    #[test]
    fn upgrading_keeps_every_launch_already_recorded_on_a_declaration() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run_through(&conn, "180_agent_resume_release_attempts").unwrap();
        insert_page(&conn, "page-old", "rev-1", "<p>x</p>");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO live_page_actions (
                 id, live_page_id, live_page_revision_id, action_ref, kind,
                 target_id, target_name, state, values_json, deep_link,
                 launched_at, finished_at, created_at, updated_at
             ) VALUES
             ('act-ran','page-old','rev-1','ran','quick_exec','qe-1','Run',
              'succeeded','[]','automation:quick_exec:run-9',?1,?1,?1,?1),
             ('act-fresh','page-old','rev-1','fresh','quick_exec','qe-1','Run',
              'proposed','[]',NULL,NULL,NULL,?1,?1),
             ('act-broken','page-old','rev-1','broken','invalid','','(bloc invalide)',
              'preflight_failed','[]',NULL,NULL,NULL,?1,?1)",
            params![now],
        )
        .unwrap();

        crate::db::migrations::run(&conn).unwrap();

        let states: Vec<(String, String)> = conn
            .prepare("SELECT id, state FROM live_page_actions ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            states,
            vec![
                ("act-broken".into(), "preflight_failed".into()),
                ("act-fresh".into(), "proposed".into()),
                ("act-ran".into(), "proposed".into()),
            ],
            "the CTA that had run is armed again, the unreadable one still says why"
        );
        let carried = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-launch:act-ran",
        )
        .unwrap()
        .expect("the launch that ran survives the upgrade");
        assert_eq!(carried.state, DiscussionActionState::Succeeded);
        assert_eq!(
            carried.deep_link.as_deref(),
            Some("automation:quick_exec:run-9")
        );
        let launches: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM live_page_action_launches",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(launches, 1, "only what actually launched becomes a launch");
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

        let action = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-removed:removed",
        )
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
            get(kronn_action_engine::Reconcile::Persisted, &conn, &action.id)
                .unwrap()
                .unwrap()
                .state,
            DiscussionActionState::Proposed,
            "a rejected stale launch must not claim the old proposal"
        );
    }

    #[test]
    fn a_target_deleted_after_the_page_was_published_cannot_still_be_launched() {
        // Parity with the discussion side. A published page outlives the Quick
        // Exec it names: preflight passed when the block was ingested, and the
        // click can come weeks later. Claiming then would launch against
        // nothing — the shared engine refuses, so neither origin can drift.
        let conn = connection();
        insert_target(&conn);
        let html = action_block("gone", r#"{"kind":"quick_exec","target_id":"qe-1"}"#);
        insert_page(&conn, "page-gone", "rev-1", &html);
        ingest_page_actions(&conn, "page-gone", "rev-1", &html).unwrap();
        let proposed = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-gone:gone",
        )
        .unwrap()
        .unwrap();
        assert_eq!(proposed.state, DiscussionActionState::Proposed);

        conn.execute("DELETE FROM quick_execs WHERE id = 'qe-1'", [])
            .unwrap();

        let outcome = claim_launch(
            &conn,
            &proposed.id,
            &HashMap::from([("service".into(), "api".into())]),
            &HashMap::new(),
        )
        .unwrap();
        let Some(LivePageActionClaimOutcome::Existing(refused)) = outcome else {
            panic!("a deleted target must not be launchable from a page either");
        };
        let reloaded = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            &refused.id,
        )
        .unwrap()
        .unwrap();
        assert_ne!(
            reloaded.id, proposed.id,
            "the refusal is recorded on the click"
        );
        assert_eq!(reloaded.state, DiscussionActionState::PreflightFailed);
        assert!(reloaded
            .diagnostic
            .as_deref()
            .is_some_and(|text| text.contains("n’existe plus")));
    }

    #[test]
    fn a_launched_action_is_frozen_and_becomes_stale_source_after_a_later_revision() {
        let conn = connection();
        insert_target(&conn);
        let html_v1 = action_block("cta", r#"{"kind":"quick_exec","target_id":"qe-1"}"#);
        insert_page(&conn, "page-frozen", "rev-1", &html_v1);
        ingest_page_actions(&conn, "page-frozen", "rev-1", &html_v1).unwrap();
        let supplied = HashMap::from([("service".into(), "api".into())]);
        let Some(LivePageActionClaimOutcome::Claimed { action: launch, .. }) = claim_launch(
            &conn,
            "page-action:page-frozen:cta",
            &supplied,
            &HashMap::new(),
        )
        .unwrap() else {
            panic!("expected a fresh claim");
        };
        complete(
            &conn,
            &launch.id,
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

        let action = get(kronn_action_engine::Reconcile::Persisted, &conn, &launch.id)
            .unwrap()
            .unwrap();
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

        let a = cancel(&conn, "page-action:page-multi:cta-a")
            .unwrap()
            .unwrap();
        let b = get(
            kronn_action_engine::Reconcile::Persisted,
            &conn,
            "page-action:page-multi:cta-b",
        )
        .unwrap()
        .unwrap();
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
        let Some(LivePageActionClaimOutcome::Claimed { action: launch, .. }) = claim_launch(
            &conn,
            "page-action:page-trace:frame",
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap() else {
            panic!("expected a fresh claim");
        };
        let action_id = launch.id.as_str();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
             VALUES ('disc-page-result', 'Page result', 'ClaudeCode', 'fr', '[]', ?1, ?1)",
            [&now],
        )
        .unwrap();

        complete_quick_prompt(&conn, action_id, "page-trace", "disc-page-result").unwrap();
        let state = || {
            get(kronn_action_engine::Reconcile::Persisted, &conn, action_id)
                .unwrap()
                .unwrap()
        };

        // The discussion exists; the answer does not yet.
        let action = state();
        assert_eq!(action.state, DiscussionActionState::Running);
        assert_eq!(
            action.result_discussion_id.as_deref(),
            Some("disc-page-result")
        );
        insert_agent_turn(
            &conn,
            "turn-1",
            "disc-page-result",
            "Pending",
            None,
            "2026-09-18T10:00:00+00:00",
        );
        assert_eq!(state().state, DiscussionActionState::Running);

        conn.execute(
            "UPDATE agent_dispatch_jobs SET status = 'Completed',
             completed_at = '2026-09-18T10:05:00+00:00' WHERE id = 'turn-1'",
            [],
        )
        .unwrap();
        let answered = state();
        assert_eq!(answered.state, DiscussionActionState::Succeeded);
        assert_eq!(
            answered.finished_at.as_deref(),
            Some("2026-09-18T10:05:00+00:00")
        );

        // A follow-up the human asks in that discussion is not this launch.
        insert_agent_turn(
            &conn,
            "turn-2",
            "disc-page-result",
            "Pending",
            None,
            "2026-09-18T11:00:00+00:00",
        );
        assert_eq!(state().state, DiscussionActionState::Succeeded);
        let link: (String, String) = conn
            .query_row(
                "SELECT page_id, relation FROM live_page_discussion_links WHERE discussion_id = 'disc-page-result'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(link, ("page-trace".into(), "attached".into()));
    }

    fn insert_agent_turn(
        conn: &Connection,
        id: &str,
        discussion_id: &str,
        status: &str,
        last_error: Option<&str>,
        created_at: &str,
    ) {
        // Each turn answers a message of its own, as a real dispatch does.
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order)
             VALUES (?1, ?2, 'User', 'Frame this ticket', ?3,
                     (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM messages WHERE discussion_id = ?2))",
            params![id, discussion_id, created_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_dispatch_jobs (
                 id, discussion_id, trigger_message_id, trigger_sort_order, dedupe_key,
                 status, last_error, available_at, created_at, updated_at
             ) VALUES (?1, ?2, ?1, 1, ?1, ?3, ?4, ?5, ?5, ?5)",
            params![id, discussion_id, status, last_error, created_at],
        )
        .unwrap();
    }

    fn launched_quick_prompt(conn: &Connection, page_id: &str) -> String {
        insert_all_target_kinds(conn);
        let html = action_block("frame", r#"{"kind":"quick_prompt","target_id":"qp-1"}"#);
        insert_page(conn, page_id, &format!("rev-{page_id}"), &html);
        ingest_page_actions(conn, page_id, &format!("rev-{page_id}"), &html).unwrap();
        let Some(LivePageActionClaimOutcome::Claimed { action, .. }) = claim_launch(
            conn,
            &format!("page-action:{page_id}:frame"),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap() else {
            panic!("expected a fresh claim");
        };
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
             VALUES (?1, 'Result', 'ClaudeCode', 'fr', '[]', ?2, ?2)",
            params![format!("disc-{page_id}"), now],
        )
        .unwrap();
        complete_quick_prompt(conn, &action.id, page_id, &format!("disc-{page_id}")).unwrap();
        action.id
    }

    #[test]
    fn a_quick_prompt_launch_fails_with_its_agent_and_says_why() {
        let conn = connection();
        let launch = launched_quick_prompt(&conn, "page-qp-fail");
        insert_agent_turn(
            &conn,
            "turn-fail",
            "disc-page-qp-fail",
            "Failed",
            Some("provider refused"),
            "2026-09-18T10:00:00+00:00",
        );

        // The companion connection projects the verdict without writing it.
        let projected = get(kronn_action_engine::Reconcile::Projected, &conn, &launch)
            .unwrap()
            .unwrap();
        assert_eq!(projected.state, DiscussionActionState::Failed);
        let stored: String = conn
            .query_row(
                "SELECT state FROM live_page_action_launches WHERE id = ?1",
                [&launch],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, "running");

        let failed = get(kronn_action_engine::Reconcile::Persisted, &conn, &launch)
            .unwrap()
            .unwrap();
        assert_eq!(failed.state, DiscussionActionState::Failed);
        assert_eq!(failed.diagnostic.as_deref(), Some("provider refused"));
        assert!(failed.finished_at.is_some());
    }

    #[test]
    fn a_quick_prompt_launch_whose_discussion_is_gone_does_not_run_for_ever() {
        let conn = connection();
        let launch = launched_quick_prompt(&conn, "page-qp-gone");
        // What `ON DELETE SET NULL` leaves behind when the discussion goes.
        conn.execute(
            "UPDATE live_page_action_launches SET result_discussion_id = NULL WHERE id = ?1",
            [&launch],
        )
        .unwrap();

        let settled = get(kronn_action_engine::Reconcile::Persisted, &conn, &launch)
            .unwrap()
            .unwrap();
        assert_eq!(settled.state, DiscussionActionState::Failed);
        assert!(settled.diagnostic.unwrap().contains("n’existe plus"));
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
