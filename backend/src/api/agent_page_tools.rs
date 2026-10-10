//! Native Page tools. Like the MCP bridge, they reach only project-less Pages
//! and the current project's Pages.
use super::*;
use crate::api::live_pages;
use crate::models::{
    CreateLivePageDataset, CreateLivePageRequest, UpdateLivePageHtmlRequest, UpdateLivePageRequest,
};
use axum::{extract::Path, extract::Query, extract::State, Json};

#[cfg(test)]
#[path = "agent_page_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "agent_page_bench.rs"]
mod bench;

pub(super) fn declarations() -> Vec<Value> {
    let dataset = json!({
        "name": {"type":"string"},
        "kind": {"type":"string","enum":["snapshot","time_series","collection"]},
        "initial": {}, "schema": {},
        "max_points": {"type":"integer"}, "max_age_days": {"type":"integer"}
    });
    let selector = json!({"type":"string","description":"Page id or slug from page_list."});
    let mut add_dataset = dataset.clone();
    add_dataset["page_id"] = selector.clone();
    [
        ("page_list", "List shared Live Pages (id, title, slug, project, data revision and timestamps). Reuse an existing destination before authoring PublishPageData.", json!({}), json!([])),
        ("page_get", "Read a shared Live Page by id or slug: current HTML revision, datasets, retained points, workflow and discussion links. Read before editing.", json!({"page_id":selector}), json!(["page_id"])),
        ("page_create", "Create a sandboxed Live Page and its first immutable HTML revision. Call page_list first; use the returned id in PublishPageData. Read tool_manual for data and action buttons.", json!({
            "title":{"type":"string","description":"Title, 1-200 chars."},
            "slug":{"type":"string","description":"Optional ASCII slug."},
            "project_id":{"type":"string","description":"Optional project binding; otherwise inherits the current scope."},
            "discussion_id":{"type":"string","description":"Optional origin discussion; otherwise inherits the current room."},
            "html":{"type":"string","description":"Complete self-contained HTML, max 1 MB."},
            "datasets":{"type":"array","description":"Named PublishPageData contracts; may be empty.","items":{"type":"object","properties":dataset,"required":["name","kind"]}}
        }), json!(["title","html","datasets"])),
        ("page_update_html", "New immutable HTML revision of a Live Page; keeps datasets and publication history. Read page_get first; send the complete HTML. `slug` renames it. Read tool_manual for action buttons.", json!({"page_id":selector,"html":{"type":"string","description":"Complete HTML document (1 MB max)."},"slug":{"type":"string"}}), json!(["page_id"])),
        ("page_add_dataset", "Attach a dataset so PublishPageData can write to it. Idempotent on name+kind; reusing a name with another kind conflicts.", add_dataset, json!(["page_id","name","kind"]))
    ].into_iter().map(|(name,description,properties,required)| json!({"type":"function","function":{"name":name,"description":description,"parameters":{"type":"object","properties":properties,"required":required}}})).collect()
}

fn page_selector(arguments: &Value) -> Result<String, &'static str> {
    arguments
        .get("page_id")
        .filter(|value| !value.is_null() && **value != "")
        .or_else(|| arguments.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or("page_id is required; use page_list")
}

const PAGE_HTML_MAX_BYTES: usize = 1_000_000;

/// The HTML and new slug of a `page_update_html` call, refused as a whole when
/// one of them is invalid so a rename never lands alone.
fn page_edit(arguments: &Value) -> Result<(Option<String>, Option<String>), &'static str> {
    let slug = match arguments.get("slug") {
        None | Some(Value::Null) => None,
        Some(Value::String(slug)) => Some(slug.clone()),
        Some(_) => return Err("page_update_html: 'slug' must be a string"),
    };
    let html = match arguments.get("html") {
        None | Some(Value::Null) => None,
        Some(Value::String(html)) if html.trim().is_empty() => {
            return Err("page_update_html: missing required 'html'")
        }
        Some(Value::String(html)) if html.len() > PAGE_HTML_MAX_BYTES => {
            return Err("page_update_html: 'html' exceeds 1 MB")
        }
        Some(Value::String(html)) => Some(html.clone()),
        Some(_) => return Err("page_update_html: missing required 'html'"),
    };
    if html.is_none() && slug.is_none() {
        return Err("page_update_html: missing required 'html' (or 'slug' to rename)");
    }
    Ok((html, slug))
}

const OUT_OF_SCOPE: &str = "Page is not available in this scope; use page_list";

impl KronnToolExecutor {
    /// The canonical id of a Page this executor may read and write.
    async fn page_in_scope(
        &self,
        selector: String,
        scope: Option<String>,
    ) -> Result<String, String> {
        use rusqlite::OptionalExtension;
        let found = self
            .state
            .db
            .with_read_conn(move |conn| {
                // Same resolution as the page handlers, so a renamed page's old
                // slug is checked as the page it opens.
                let Some(id) = crate::db::live_pages::resolve_live_page_id(conn, &selector)? else {
                    return Ok(None);
                };
                Ok(conn
                    .query_row(
                        "SELECT id, project_id FROM live_pages WHERE id = ?1",
                        [&id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                    )
                    .optional()?)
            })
            .await
            .map_err(|error| format!("Unable to load Page: {error}"))?;
        match found {
            Some((id, project)) if project_is_in_scope(project.as_deref(), scope.as_deref()) => {
                Ok(id)
            }
            _ => Err(OUT_OF_SCOPE.into()),
        }
    }

    /// Workflow and discussion links of other projects stay hidden.
    async fn scoped_links(&self, detail: &mut Value, scope: Option<String>) {
        let workflows: Vec<String> = detail["workflows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|link| link["id"].as_str().map(str::to_owned))
            .collect();
        let discussions: Vec<String> = detail["discussions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|link| link["discussion_id"].as_str().map(str::to_owned))
            .collect();
        let own = self.disc_id.clone();
        let visible = self
            .state
            .db
            .with_read_conn(move |conn| {
                let project_of = |sql: &str, id: &str| -> rusqlite::Result<Option<String>> {
                    conn.query_row(sql, [id], |row| row.get(0))
                };
                let mut keep = std::collections::HashSet::new();
                for id in workflows {
                    let project = project_of("SELECT project_id FROM workflows WHERE id = ?1", &id)
                        .unwrap_or(Some(String::new()));
                    if project_is_in_scope(project.as_deref(), scope.as_deref()) {
                        keep.insert(id);
                    }
                }
                for id in discussions {
                    let project =
                        project_of("SELECT project_id FROM discussions WHERE id = ?1", &id)
                            .ok()
                            .flatten();
                    // A project-less discussion stays private to itself.
                    if own.as_deref() == Some(id.as_str())
                        || (project.is_some() && project == scope)
                    {
                        keep.insert(id);
                    }
                }
                Ok(keep)
            })
            .await
            .unwrap_or_default();
        for (field, key) in [("workflows", "id"), ("discussions", "discussion_id")] {
            if let Some(links) = detail[field].as_array_mut() {
                links.retain(|link| link[key].as_str().is_some_and(|id| visible.contains(id)));
            }
        }
    }

    pub(super) async fn execute_page_tool(&self, call: &ToolCall) -> ToolOutcome {
        let state = State(self.state.clone());
        let scope = self.effective_project_id().await;
        match call.name.as_str() {
            "page_list" => {
                let Json(response) =
                    live_pages::list(state, Query(live_pages::ListLivePagesQuery::default())).await;
                let in_scope = |page: &crate::models::LivePage| {
                    project_is_in_scope(page.project_id.as_deref(), scope.as_deref())
                };
                let data = response.data.map(|pages| pages.into_iter().filter(in_scope).map(|page| json!({
                    "id":page.id,"title":page.title,"slug":page.slug,"project_id":page.project_id,
                    "data_revision":page.data_revision,"updated_at":page.updated_at,
                    "last_published_at":page.last_published_at,"pinned":page.pinned,"archived":page.archived
                })).collect::<Vec<_>>());
                unwrap_api(call, response.success, data, response.error)
            }
            "page_create" => {
                // Match the bridge's closed body: source_message_id and actor
                // overrides are not part of this authoring contract.
                if !call.arguments["datasets"].is_array() {
                    return fail(call, "page_create: datasets must be an array");
                }
                let mut body = json!({});
                for field in [
                    "title",
                    "html",
                    "datasets",
                    "slug",
                    "project_id",
                    "discussion_id",
                ] {
                    if let Some(value) = call.arguments.get(field) {
                        body[field] = value.clone();
                    }
                }
                if body["discussion_id"].is_null() || body["discussion_id"] == "" {
                    body["discussion_id"] = json!(self.disc_id);
                } else if body["discussion_id"].as_str() != self.disc_id.as_deref() {
                    return fail(
                        call,
                        "page_create: discussion_id must be the current discussion",
                    );
                }
                match body.get("project_id") {
                    None => body["project_id"] = json!(scope),
                    Some(Value::Null) => {}
                    Some(project) => {
                        if !project.as_str().is_some_and(|project| {
                            project_is_in_scope(Some(project), scope.as_deref())
                        }) {
                            return fail(call, "page_create: project_id is outside this scope");
                        }
                    }
                }
                body["created_by_agent"] = json!(self
                    .actor_type
                    .as_ref()
                    .map(crate::db::discussions::format_agent_type));
                let request: CreateLivePageRequest = match serde_json::from_value(body) {
                    Ok(request) => request,
                    Err(error) => return fail(call, format!("Invalid Page: {error}")),
                };
                let Json(response) = live_pages::create(state, None, Json(request)).await;
                unwrap_api(call, response.success, response.data, response.error)
            }
            "page_get" | "page_update_html" | "page_add_dataset" => {
                let id = match page_selector(&call.arguments) {
                    Ok(id) => id,
                    Err(error) => return fail(call, error),
                };
                // Every argument is checked before the first write.
                let edit = if call.name == "page_update_html" {
                    match page_edit(&call.arguments) {
                        Ok(edit) => Some(edit),
                        Err(error) => return fail(call, error),
                    }
                } else {
                    None
                };
                let id = match self.page_in_scope(id, scope.clone()).await {
                    Ok(id) => id,
                    Err(error) => return fail(call, error),
                };
                match call.name.as_str() {
                    "page_get" => {
                        let Json(response) = live_pages::get(state.clone(), Path(id.clone())).await;
                        let mut detail = match (response.success, response.data) {
                            (true, Some(detail)) => json!(detail),
                            _ => {
                                return fail(
                                    call,
                                    response.error.unwrap_or_else(|| "Page not found".into()),
                                )
                            }
                        };
                        let Json(workflows) =
                            live_pages::workflows(state.clone(), Path(id.clone())).await;
                        if !workflows.success {
                            return fail(
                                call,
                                workflows
                                    .error
                                    .unwrap_or_else(|| "Unable to load Page workflows".into()),
                            );
                        }
                        let Json(discussions) = live_pages::discussions(state, Path(id)).await;
                        if !discussions.success {
                            return fail(
                                call,
                                discussions
                                    .error
                                    .unwrap_or_else(|| "Unable to load Page discussions".into()),
                            );
                        }
                        detail["workflows"] = json!(workflows.data.unwrap_or_default());
                        detail["discussions"] = json!(discussions.data.unwrap_or_default());
                        self.scoped_links(&mut detail, scope).await;
                        ok(call, detail)
                    }
                    "page_update_html" => {
                        let (html, slug) = edit.unwrap_or_default();
                        let mut renamed = None;
                        if let Some(slug) = slug {
                            let Json(response) = live_pages::update(
                                state.clone(),
                                Path(id.clone()),
                                Json(UpdateLivePageRequest {
                                    title: None,
                                    slug: Some(slug),
                                    pinned: None,
                                    archived: None,
                                }),
                            )
                            .await;
                            match (response.success, response.data) {
                                (true, Some(detail)) => renamed = Some(detail),
                                _ => {
                                    return fail(
                                        call,
                                        response.error.unwrap_or_else(|| "Page not found".into()),
                                    )
                                }
                            }
                        }
                        let Some(html) = html else {
                            return ok(
                                call,
                                json!(renamed.map(|detail| json!({
                                    "id":detail.page.id,"title":detail.page.title,
                                    "slug":detail.page.slug,"slug_aliases":detail.slug_aliases,
                                    "project_id":detail.page.project_id,
                                    "updated_at":detail.page.updated_at,
                                }))),
                            );
                        };
                        let Json(response) = live_pages::update_html(
                            state,
                            Path(id.clone()),
                            Json(UpdateLivePageHtmlRequest {
                                html,
                                created_by_agent: self
                                    .actor_type
                                    .as_ref()
                                    .map(crate::db::discussions::format_agent_type),
                            }),
                        )
                        .await;
                        match (&renamed, response.success) {
                            (Some(detail), false) => fail(
                                call,
                                format!(
                                    "page_update_html: page '{}' was already renamed to slug '{}', but the HTML revision failed: {}",
                                    detail.page.id,
                                    detail.page.slug,
                                    response.error.unwrap_or_default()
                                ),
                            ),
                            _ => unwrap_api(call, response.success, response.data, response.error),
                        }
                    }
                    _ => {
                        let dataset: CreateLivePageDataset =
                            match serde_json::from_value(call.arguments.clone()) {
                                Ok(dataset) => dataset,
                                Err(error) => {
                                    return fail(call, format!("Invalid Page dataset: {error}"))
                                }
                            };
                        let Json(response) =
                            live_pages::add_dataset(state, Path(id), Json(dataset)).await;
                        unwrap_api(call, response.success, response.data, response.error)
                    }
                }
            }
            _ => fail(call, "Unknown Page tool"),
        }
    }
}

pub(super) const CREATE_MANUAL: &str = "Live Page HTML is a complete self-contained document rendered without network access. Declare snapshot, time_series or collection datasets; an empty array creates standalone HTML. initial values support mock design; max_points and max_age_days bound retention. Missing project/discussion bindings inherit the current scope; explicit bindings follow the MCP contract. Pages are shared destinations. Use the returned id in PublishPageData.page_publish.page_id. Read data from window.KronnPageData and the kronn:page-data CustomEvent. Inline action buttons follow the page_update_html manual.";

pub(super) const UPDATE_MANUAL: &str = "Replace the whole HTML document; datasets and publication history survive. Keep stable data-action-id and data-kronn-action references across revisions to preserve launch history. An inline action pairs a visible data-kronn-action=\"stable-ref\" element with an inert <script type=\"application/kronn-action\" data-action-id=\"stable-ref\"> JSON block. References are 1–256 URL-safe [A-Za-z0-9._~-] characters. JSON: {kind,target_id,project_id?,values?}; kind is quick_prompt, quick_api, quick_exec or workflow. Discover real target ids first. Page values may use provenance dynamic_binding and source_ref <page.title>, <page.dataset.summary.owner> or <page.dataset.tickets.find(key).id>. data-kronn-bindings is a JSON selector map keyed by variable name, one button per row sharing one block. A user_input value with source_ref starts from the row and stays editable. Never embed secrets or resolved environment values. The sandbox emits intentions; only the native card's explicit human launch executes them. Style data-kronn-action-state (launching, running, succeeded, failed, preflight_failed); data-kronn-action-launch identifies the latest launch. A running row cannot launch twice; clicking it reopens its run. Other clicks offer a fresh attempt with prior history available. A Quick Prompt succeeds once its agent answers.";
