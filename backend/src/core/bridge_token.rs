//! Scoped bridge tokens (KT-1006 layer B, bridge part).
//!
//! Each agent launch gets its own random 256-bit token instead of Kronn's admin
//! bearer. The token lives in this process's memory only, is bound to the
//! launch's discussion (or task execution, or workflow run) and its project,
//! and is revoked when the launch's process handle is dropped. A backend
//! restart forgets every token.
//!
//! A request bearing one is accepted only on [`BRIDGE_ROUTES`] — the routes the
//! `kronn-internal` bridge calls — and only for resources in the token's scope.
//! Ids are read from the path, the query string and the top-level JSON body.
//! A request without any token keeps the loopback trust it has today: the
//! per-action human proof that replaces it is deferred (design note §4).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use aes_gcm::aead::{rand_core::RngCore, OsRng};
use sha2::{Digest, Sha256};

/// Every bridge token starts with this, so a dead one is recognised and
/// refused instead of falling back to loopback trust.
pub const TOKEN_PREFIX: &str = "kbt_";

/// What a launch is bound to. The project is resolved from these ids on first
/// use, by the auth middleware, which has the database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeScope {
    /// The launch's own discussions: its discussion and its room / step /
    /// worker contexts' discussions.
    pub discussion_ids: Vec<String>,
    pub task_execution_id: Option<String>,
    pub workflow_run_id: Option<String>,
}

impl BridgeScope {
    pub fn is_empty(&self) -> bool {
        self.discussion_ids.is_empty()
            && self.task_execution_id.is_none()
            && self.workflow_run_id.is_none()
    }

    pub fn owns_discussion(&self, id: &str) -> bool {
        self.discussion_ids.iter().any(|own| own == id)
    }
}

/// A live token's registry entry.
#[derive(Debug)]
pub struct BridgeGrant {
    /// Short, non-secret id for logs: the first 12 hex digits of the hash.
    pub id: String,
    pub scope: BridgeScope,
    /// `None` = not resolved yet; `Some(None)` = the scope has no project.
    project: Mutex<Option<Option<String>>>,
    /// Discussions this launch created through the bridge: its own too.
    adopted: Mutex<Vec<String>>,
}

impl BridgeGrant {
    /// The launch's own discussions, plus the ones it created.
    pub fn owns_discussion(&self, id: &str) -> bool {
        self.scope.owns_discussion(id)
            || self
                .adopted
                .lock()
                .is_ok_and(|adopted| adopted.iter().any(|own| own == id))
    }

    /// Record a discussion this launch just created.
    pub fn adopt_discussion(&self, id: &str) {
        if let Ok(mut adopted) = self.adopted.lock() {
            if !adopted.iter().any(|own| own == id) {
                adopted.push(id.to_owned());
            }
        }
    }

    pub fn cached_project(&self) -> Option<Option<String>> {
        self.project.lock().ok().and_then(|cached| cached.clone())
    }

    pub fn cache_project(&self, project: Option<String>) {
        if let Ok(mut cached) = self.project.lock() {
            *cached = Some(project);
        }
    }
}

type Registry = Mutex<HashMap<[u8; 32], Arc<BridgeGrant>>>;

static REGISTRY: LazyLock<Registry> = LazyLock::new(Default::default);

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Holds a live token; dropping it revokes the token.
pub struct BridgeTokenGuard {
    value: String,
    hash: [u8; 32],
    id: String,
}

impl BridgeTokenGuard {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

impl std::fmt::Debug for BridgeTokenGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeTokenGuard")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Drop for BridgeTokenGuard {
    fn drop(&mut self) {
        if let Ok(mut registry) = REGISTRY.lock() {
            registry.remove(&self.hash);
        }
    }
}

/// Mint a token for one launch. `None` when the scope names nothing: such a
/// launch has no room tools to authorise.
pub fn mint(scope: BridgeScope) -> Option<BridgeTokenGuard> {
    if scope.is_empty() {
        return None;
    }
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let value = format!("{TOKEN_PREFIX}{}", hex(&bytes));
    let hash = digest(&value);
    let id = hex(&hash[..6]);
    let grant = Arc::new(BridgeGrant {
        id: id.clone(),
        scope,
        project: Mutex::new(None),
        adopted: Mutex::new(Vec::new()),
    });
    REGISTRY.lock().ok()?.insert(hash, grant);
    Some(BridgeTokenGuard { value, hash, id })
}

/// The live grant behind `token`, if any.
pub fn lookup(token: &str) -> Option<Arc<BridgeGrant>> {
    if !token.starts_with(TOKEN_PREFIX) {
        return None;
    }
    REGISTRY.lock().ok()?.get(&digest(token)).cloned()
}

/// How a route checks the ids a request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Every resource must be in the token's project (project-less resources
    /// are shared and pass); a discussion may also be one of the launch's own.
    Project,
    /// As `Project`, but a discussion must be one of the launch's own.
    Own,
    /// An effect (run, trigger, call, launch, generate): every resource must
    /// belong to the token's project, and the call is logged with the token id.
    Effect,
}

/// What an id names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Discussion,
    Project,
    Task,
    Workflow,
    Run,
    Execution,
    QuickPrompt,
    QuickApi,
    QuickExec,
    Page,
}

/// One route a bridge token may call.
#[derive(Debug)]
pub struct BridgeRoute {
    pub method: &'static str,
    /// The axum route pattern, exactly as registered in `lib.rs`.
    pub pattern: &'static str,
    pub rule: Rule,
    /// Path parameters that name a scoped resource.
    pub params: &'static [(&'static str, Kind)],
}

const fn r(
    method: &'static str,
    pattern: &'static str,
    rule: Rule,
    params: &'static [(&'static str, Kind)],
) -> BridgeRoute {
    BridgeRoute {
        method,
        pattern,
        rule,
        params,
    }
}

use Kind::{Discussion as D, Execution as X, Page as G, Project as P, QuickApi as QA};
use Kind::{QuickExec as QE, QuickPrompt as QP, Run as R, Task as T, Workflow as W};
use Rule::{Effect, Own, Project};

/// Every route `backend/scripts/disc-introspection-mcp.py` calls, and nothing
/// else. `backend/scripts/test_bridge_routes.py` parses the script and fails
/// when the two drift apart in either direction.
pub const BRIDGE_ROUTES: &[BridgeRoute] = &[
    // Discussions and rooms.
    r("GET", "/api/discussions", Project, &[]),
    r("PATCH", "/api/discussions/{id}", Own, &[("id", D)]),
    r("GET", "/api/discussions/{id}/meta", Project, &[("id", D)]),
    r(
        "GET",
        "/api/discussions/{id}/message/{idx}",
        Project,
        &[("id", D)],
    ),
    r("GET", "/api/discussions/{id}/notes", Project, &[("id", D)]),
    r(
        "GET",
        "/api/discussions/{id}/participants",
        Project,
        &[("id", D)],
    ),
    r(
        "GET",
        "/api/discussions/{id}/questions",
        Project,
        &[("id", D)],
    ),
    r("GET", "/api/discussions/{id}/plan", Project, &[("id", D)]),
    r(
        "GET",
        "/api/discussions/{id}/plan/changes",
        Project,
        &[("id", D)],
    ),
    r("GET", "/api/discussions/{id}/wait", Own, &[("id", D)]),
    r("POST", "/api/discussions/{id}/summarize", Own, &[("id", D)]),
    r("POST", "/api/discussions/{id}/telemetry", Own, &[("id", D)]),
    r(
        "POST",
        "/api/discussions/{id}/invite-peer",
        Own,
        &[("id", D)],
    ),
    r(
        "POST",
        "/api/discussions/{id}/context-files",
        Own,
        &[("id", D)],
    ),
    r(
        "POST",
        "/api/discussions/{id}/context-files/link-pending",
        Own,
        &[("id", D)],
    ),
    r(
        "DELETE",
        "/api/discussions/{id}/context-files/{file_id}",
        Own,
        &[("id", D)],
    ),
    r("POST", "/api/discussions/peer-join", Project, &[]),
    r("POST", "/api/discussions/peer-leave", Project, &[]),
    r("POST", "/api/discussions/peer-resume", Project, &[]),
    r("POST", "/api/discussions/workflow-step-join", Project, &[]),
    r(
        "POST",
        "/api/discussions/orchestrator-return-resume",
        Project,
        &[],
    ),
    r("POST", "/api/disc/create", Project, &[]),
    r("POST", "/api/disc/append", Own, &[]),
    r("POST", "/api/disc/link", Own, &[]),
    r("POST", "/api/disc/unlink", Own, &[]),
    r("POST", "/api/disc/transfer-session", Own, &[]),
    r("GET", "/api/disc/find_by_session", Project, &[]),
    r("GET", "/api/disc/load_other", Project, &[]),
    r("GET", "/api/disc/search", Project, &[]),
    r("GET", "/api/disc/session-status", Project, &[]),
    r("GET", "/api/disc/workspace", Project, &[]),
    r("POST", "/api/disc/workspace", Own, &[]),
    r("POST", "/api/disc/workspace/history-lease", Own, &[]),
    r("POST", "/api/human-credentials/proof", Project, &[]),
    r("GET", "/api/resolve/{id}", Project, &[]),
    // Planning.
    r("GET", "/api/planning/tasks", Project, &[]),
    r("POST", "/api/planning/tasks", Project, &[]),
    r("GET", "/api/planning/tasks/{id}", Project, &[("id", T)]),
    r("PATCH", "/api/planning/tasks/{id}", Project, &[("id", T)]),
    r(
        "PATCH",
        "/api/planning/tasks/{id}/dod/{dod_id}",
        Project,
        &[("id", T)],
    ),
    r(
        "POST",
        "/api/planning/tasks/{id}/discussions",
        Project,
        &[("id", T)],
    ),
    r(
        "DELETE",
        "/api/planning/tasks/{id}/discussions",
        Project,
        &[("id", T)],
    ),
    r(
        "POST",
        "/api/planning/tasks/{id}/blockers",
        Project,
        &[("id", T)],
    ),
    r(
        "DELETE",
        "/api/planning/tasks/{id}/blockers/{blocker_id}",
        Project,
        &[("id", T), ("blocker_id", T)],
    ),
    r("GET", "/api/planning/proposals", Project, &[]),
    r("GET", "/api/planning/proposals/{id}", Project, &[]),
    r("POST", "/api/learnings/propose", Project, &[]),
    // Task orchestration.
    r("POST", "/api/orchestration/tool/workers", Project, &[]),
    r("POST", "/api/orchestration/tool/prepare", Project, &[]),
    r("POST", "/api/orchestration/tool/launch", Effect, &[]),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/status",
        Project,
        &[("id", X)],
    ),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/resume",
        Project,
        &[("id", X)],
    ),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/cancel",
        Project,
        &[("id", X)],
    ),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/reassign",
        Project,
        &[("id", X)],
    ),
    r("POST", "/api/orchestration/accept-offer", Project, &[]),
    r("POST", "/api/orchestration/deliver", Project, &[]),
    r("POST", "/api/orchestration/worker-commit", Project, &[]),
    r("POST", "/api/orchestration/review", Project, &[]),
    // Workflows.
    r("GET", "/api/workflows", Project, &[]),
    r("POST", "/api/workflows", Project, &[]),
    r("POST", "/api/workflows/import", Project, &[]),
    r("GET", "/api/workflows/step-schema", Project, &[]),
    r("GET", "/api/workflows/{id}", Project, &[("id", W)]),
    r("PUT", "/api/workflows/{id}", Project, &[("id", W)]),
    r("GET", "/api/workflows/{id}/export", Project, &[("id", W)]),
    r("GET", "/api/workflows/{id}/runs", Project, &[("id", W)]),
    r(
        "GET",
        "/api/workflows/{id}/runs/{run_id}",
        Project,
        &[("id", W), ("run_id", R)],
    ),
    r(
        "POST",
        "/api/workflows/{id}/runs/{run_id}/cancel",
        Project,
        &[("id", W), ("run_id", R)],
    ),
    r(
        "POST",
        "/api/workflow-runs/{run_id}/resume",
        Effect,
        &[("run_id", R)],
    ),
    r("POST", "/api/mcp/workflow-trigger", Effect, &[]),
    r(
        "GET",
        "/api/mcp/workflow-run-status/{run_id}",
        Project,
        &[("run_id", R)],
    ),
    r(
        "GET",
        "/api/mcp/workflow-run-discussions/{run_id}",
        Project,
        &[("run_id", R)],
    ),
    r(
        "POST",
        "/api/mcp/workflow-wait-for-completion",
        Project,
        &[],
    ),
    // Quick prompts, APIs and execs.
    r("GET", "/api/quick-prompts", Project, &[]),
    r("POST", "/api/quick-prompts", Project, &[]),
    r("PUT", "/api/quick-prompts/{id}", Project, &[("id", QP)]),
    r("DELETE", "/api/quick-prompts/{id}", Project, &[("id", QP)]),
    r("POST", "/api/mcp/qp-run", Effect, &[]),
    r("POST", "/api/mcp/qp-batch-run", Effect, &[]),
    r("GET", "/api/quick-apis", Project, &[]),
    r("POST", "/api/quick-apis", Project, &[]),
    r("PUT", "/api/quick-apis/{id}", Project, &[("id", QA)]),
    r("POST", "/api/quick-apis/{id}/run", Effect, &[("id", QA)]),
    r("GET", "/api/quick-execs", Project, &[]),
    r("POST", "/api/quick-execs", Project, &[]),
    r("PUT", "/api/quick-execs/{id}", Project, &[("id", QE)]),
    r("POST", "/api/quick-execs/{id}/run", Effect, &[("id", QE)]),
    r("POST", "/api/agent-api/call", Effect, &[]),
    // Pages.
    r("POST", "/api/pages", Project, &[]),
    r("GET", "/api/pages/{id}", Project, &[("id", G)]),
    r("PUT", "/api/pages/{id}/html", Project, &[("id", G)]),
    r("POST", "/api/pages/{id}/datasets", Project, &[("id", G)]),
    r("GET", "/api/pages/{id}/workflows", Project, &[("id", G)]),
    r("GET", "/api/pages/{id}/discussions", Project, &[("id", G)]),
    // Media.
    r("POST", "/api/media/generate", Effect, &[]),
    r("GET", "/api/media/jobs/{id}", Project, &[]),
    // Projects and audits.
    r("GET", "/api/projects/{id}", Project, &[("id", P)]),
    r(
        "GET",
        "/api/projects/{id}/audit-info",
        Project,
        &[("id", P)],
    ),
    r(
        "GET",
        "/api/projects/{id}/audit-status",
        Project,
        &[("id", P)],
    ),
    r(
        "GET",
        "/api/projects/{id}/audit-latest",
        Project,
        &[("id", P)],
    ),
    r(
        "GET",
        "/api/projects/{id}/audit-resumable",
        Project,
        &[("id", P)],
    ),
    r(
        "POST",
        "/api/projects/{id}/install-template",
        Project,
        &[("id", P)],
    ),
    r(
        "POST",
        "/api/projects/{id}/full-audit",
        Effect,
        &[("id", P)],
    ),
    r(
        "POST",
        "/api/projects/{id}/partial-audit",
        Effect,
        &[("id", P)],
    ),
    // Agent library, MCP overview, catalogues (global, not project-bound).
    r("GET", "/api/skills", Project, &[]),
    r("POST", "/api/skills", Project, &[]),
    r("PUT", "/api/skills/{id}", Project, &[]),
    r("DELETE", "/api/skills/{id}", Project, &[]),
    r("GET", "/api/profiles", Project, &[]),
    r("POST", "/api/profiles", Project, &[]),
    r("PUT", "/api/profiles/{id}", Project, &[]),
    r("DELETE", "/api/profiles/{id}", Project, &[]),
    r("GET", "/api/directives", Project, &[]),
    r("POST", "/api/directives", Project, &[]),
    r("PUT", "/api/directives/{id}", Project, &[]),
    r("DELETE", "/api/directives/{id}", Project, &[]),
    r("GET", "/api/mcps", Project, &[]),
    r("GET", "/api/signals/catalog", Project, &[]),
    r("GET", "/api/conventions/agents-md-format-v1", Project, &[]),
];

/// Routes whose run lands on a project the body may leave unnamed.
const RUNS_FOR_A_PROJECT: &[&str] = &["/api/mcp/workflow-trigger"];

/// The body with the token's project added when a run-launching route names
/// none, so a shared or multi-project workflow resolves to that project like a
/// launch from its discussion (KT-851). `None` = the body stays as sent.
pub fn with_bound_project(
    route: &BridgeRoute,
    bound_project: Option<&str>,
    body: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let project = bound_project?;
    if !RUNS_FOR_A_PROJECT.contains(&route.pattern) {
        return None;
    }
    let mut body = body?;
    let fields = body.as_object_mut()?;
    if fields
        .get("project_id")
        .is_some_and(|value| !value.is_null())
    {
        return None;
    }
    fields.insert(
        "project_id".into(),
        serde_json::Value::String(project.into()),
    );
    Some(body)
}

/// The route a bridge token may call for this method and matched pattern.
pub fn route_for(method: &str, pattern: &str) -> Option<&'static BridgeRoute> {
    BRIDGE_ROUTES
        .iter()
        .find(|route| route.method.eq_ignore_ascii_case(method) && route.pattern == pattern)
}

/// Body and query keys that name a scoped resource, wherever they appear.
const ID_KEYS: &[(&str, Kind)] = &[
    ("discussion_id", Kind::Discussion),
    ("disc_id", Kind::Discussion),
    ("discussion_ids", Kind::Discussion),
    ("parent_discussion_id", Kind::Discussion),
    ("source_discussion_id", Kind::Discussion),
    ("sub_discussion_id", Kind::Discussion),
    ("child_discussion_id", Kind::Discussion),
    ("from_disc_id", Kind::Discussion),
    ("to_disc_id", Kind::Discussion),
    ("runtime_disc_id", Kind::Discussion),
    ("resume_disc_id", Kind::Discussion),
    ("expected_disc_id", Kind::Discussion),
    ("expected_child_disc_id", Kind::Discussion),
    ("project_id", Kind::Project),
    ("project_ids", Kind::Project),
    ("target_project_id", Kind::Project),
    ("task_id", Kind::Task),
    ("blocker_task_id", Kind::Task),
    ("workflow_id", Kind::Workflow),
    ("parent_workflow_id", Kind::Workflow),
    ("sub_workflow_id", Kind::Workflow),
    ("run_id", Kind::Run),
    ("workflow_run_id", Kind::Run),
    ("parent_run_id", Kind::Run),
    ("resume_run_id", Kind::Run),
    ("execution_id", Kind::Execution),
    ("task_execution_id", Kind::Execution),
    ("qp_id", Kind::QuickPrompt),
    ("quick_prompt_id", Kind::QuickPrompt),
    ("qa_id", Kind::QuickApi),
    ("quick_api_id", Kind::QuickApi),
    ("qe_id", Kind::QuickExec),
    ("page_id", Kind::Page),
];

fn kind_of_key(key: &str) -> Option<Kind> {
    ID_KEYS
        .iter()
        .find_map(|(name, kind)| (*name == key).then_some(*kind))
}

/// The ids a request names: path parameters the route declares, query
/// parameters and top-level JSON body fields with a known key.
pub fn collect_ids(
    route: &BridgeRoute,
    path_params: &[(String, String)],
    query: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Vec<(Kind, String)> {
    let mut ids = Vec::new();
    for (name, value) in path_params {
        if let Some((_, kind)) = route.params.iter().find(|(param, _)| param == name) {
            ids.push((*kind, value.clone()));
        }
    }
    if let Some(query) = query {
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            if let Some(kind) = kind_of_key(&percent_decode(key)) {
                let value = percent_decode(value);
                if !value.is_empty() {
                    ids.push((kind, value));
                }
            }
        }
    }
    if let Some(serde_json::Value::Object(fields)) = body {
        for (key, value) in fields {
            let Some(kind) = kind_of_key(key) else {
                continue;
            };
            match value {
                serde_json::Value::String(id) if !id.is_empty() => {
                    ids.push((kind, id.clone()));
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        if let serde_json::Value::String(id) = item {
                            if !id.is_empty() {
                                ids.push((kind, id.clone()));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    ids
}

/// `application/x-www-form-urlencoded` decoding: `+` and `%XX`. Invalid
/// escapes stay literal; invalid UTF-8 is replaced.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => out.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let pair = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match pair.and_then(|pair| u8::from_str_radix(pair, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a resource lives. `None` from the resolver = the resource does not
/// exist (the handler answers 404 on its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Residence {
    /// Project-less: shared by every scope.
    Global,
    /// Every project the resource serves.
    Projects(HashSet<String>),
    /// Serves every project (a workflow scoped to all of them).
    AllProjects,
}

impl Residence {
    fn holds(&self, project: &str) -> bool {
        match self {
            Self::Global => false,
            Self::AllProjects => true,
            Self::Projects(projects) => projects.contains(project),
        }
    }
}

/// Why a request was refused. Never echoes a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

/// Decide one request against the grant. `residence` looks a resource up;
/// `bound_project` is the grant's resolved project.
pub fn authorize(
    grant: &BridgeGrant,
    route: &BridgeRoute,
    bound_project: Option<&str>,
    ids: &[(Kind, String)],
    mut residence: impl FnMut(Kind, &str) -> Result<Option<Residence>, Refusal>,
) -> Result<(), Refusal> {
    // An explicit project in the body is where an effect lands: it lets a
    // shared (project-less) resource act on the bound project only.
    let targets_bound_project = ids
        .iter()
        .any(|(kind, id)| *kind == Kind::Project && Some(id.as_str()) == bound_project);
    for (kind, id) in ids {
        let own = match kind {
            Kind::Discussion => grant.owns_discussion(id),
            Kind::Execution => grant.scope.task_execution_id.as_deref() == Some(id.as_str()),
            Kind::Run => grant.scope.workflow_run_id.as_deref() == Some(id.as_str()),
            Kind::Project => bound_project == Some(id.as_str()),
            _ => false,
        };
        if own {
            continue;
        }
        if *kind == Kind::Discussion && route.rule == Rule::Own {
            return Err(Refusal(format!(
                "a bridge token may only write to its own discussion, not {id}"
            )));
        }
        if *kind == Kind::Project {
            return Err(Refusal(format!(
                "project {id} is outside this bridge token's project"
            )));
        }
        let Some(place) = residence(*kind, id)? else {
            continue;
        };
        let allowed = match (&place, bound_project) {
            (Residence::Global, _) => {
                route.rule != Rule::Effect || targets_bound_project || bound_project.is_none()
            }
            (_, Some(project)) => place.holds(project),
            (Residence::AllProjects, None) => route.rule != Rule::Effect,
            (Residence::Projects(_), None) => false,
        };
        if !allowed {
            return Err(Refusal(format!(
                "{kind:?} {id} is outside this bridge token's scope"
            )));
        }
    }
    Ok(())
}

fn single_or_global(project: Option<String>) -> Residence {
    match project {
        Some(project) => Residence::Projects(HashSet::from([project])),
        None => Residence::Global,
    }
}

fn workflow_residence(home: Option<String>, scope_json: Option<String>) -> Residence {
    let scope = scope_json.and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
    match scope
        .as_ref()
        .and_then(|scope| scope.get("type"))
        .and_then(|kind| kind.as_str())
    {
        Some("All") => Residence::AllProjects,
        Some("Projects") => {
            let mut projects: HashSet<String> = scope
                .as_ref()
                .and_then(|scope| scope.get("project_ids"))
                .and_then(|ids| ids.as_array())
                .map(|ids| {
                    ids.iter()
                        .filter_map(|id| id.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            projects.extend(home);
            if projects.is_empty() {
                Residence::Global
            } else {
                Residence::Projects(projects)
            }
        }
        _ => single_or_global(home),
    }
}

/// Where the resource `id` of `kind` lives, `None` when it does not exist.
pub fn residence(
    conn: &rusqlite::Connection,
    kind: Kind,
    id: &str,
) -> rusqlite::Result<Option<Residence>> {
    use rusqlite::OptionalExtension;
    let project_of = |sql: &str| -> rusqlite::Result<Option<Residence>> {
        conn.query_row(sql, [id], |row| row.get::<_, Option<String>>(0))
            .optional()
            .map(|found| found.map(single_or_global))
    };
    match kind {
        Kind::Discussion => project_of("SELECT project_id FROM discussions WHERE id = ?1"),
        Kind::QuickPrompt => project_of("SELECT project_id FROM quick_prompts WHERE id = ?1"),
        Kind::QuickApi => project_of("SELECT project_id FROM quick_apis WHERE id = ?1"),
        Kind::QuickExec => project_of("SELECT project_id FROM quick_execs WHERE id = ?1"),
        Kind::Page => project_of("SELECT project_id FROM live_pages WHERE id = ?1"),
        Kind::Execution => project_of(
            "SELECT d.project_id FROM task_executions e \
             JOIN discussions d ON d.id = e.parent_discussion_id WHERE e.id = ?1",
        ),
        Kind::Project => Ok(Some(single_or_global(Some(id.to_owned())))),
        Kind::Task => {
            let exists = conn
                .query_row("SELECT 1 FROM planning_tasks WHERE id = ?1", [id], |_| {
                    Ok(())
                })
                .optional()?
                .is_some();
            if !exists {
                return Ok(None);
            }
            let mut statement =
                conn.prepare("SELECT project_id FROM planning_task_projects WHERE task_id = ?1")?;
            let projects: HashSet<String> = statement
                .query_map([id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some(if projects.is_empty() {
                Residence::Global
            } else {
                Residence::Projects(projects)
            }))
        }
        Kind::Workflow => conn
            .query_row(
                "SELECT project_id, project_scope_json FROM workflows WHERE id = ?1",
                [id],
                |row| Ok(workflow_residence(row.get(0)?, row.get(1)?)),
            )
            .optional(),
        Kind::Run => conn
            .query_row(
                "SELECT r.project_id, w.project_id, w.project_scope_json FROM workflow_runs r \
                 LEFT JOIN workflows w ON w.id = r.workflow_id WHERE r.id = ?1",
                [id],
                |row| {
                    let run_project: Option<String> = row.get(0)?;
                    Ok(match run_project {
                        Some(project) => single_or_global(Some(project)),
                        None => workflow_residence(row.get(1)?, row.get(2)?),
                    })
                },
            )
            .optional(),
    }
}

/// The project a scope is bound to. `Err(None)` = every resource the scope
/// names is gone (its discussion or run was deleted): the token is dead.
pub fn resolve_scope_project(
    conn: &rusqlite::Connection,
    scope: &BridgeScope,
) -> rusqlite::Result<Result<Option<String>, ()>> {
    use rusqlite::OptionalExtension;
    for discussion in &scope.discussion_ids {
        let found = conn
            .query_row(
                "SELECT project_id FROM discussions WHERE id = ?1",
                [discussion],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        if let Some(project) = found {
            return Ok(Ok(project));
        }
    }
    for (kind, id) in [
        (Kind::Execution, scope.task_execution_id.as_deref()),
        (Kind::Run, scope.workflow_run_id.as_deref()),
    ] {
        let Some(id) = id else { continue };
        match residence(conn, kind, id)? {
            Some(Residence::Projects(projects)) if projects.len() == 1 => {
                return Ok(Ok(projects.into_iter().next()));
            }
            Some(_) => return Ok(Ok(None)),
            None => {}
        }
    }
    Ok(Err(()))
}

#[cfg(test)]
#[path = "bridge_token_test.rs"]
mod bridge_token_test;
