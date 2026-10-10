//! Scoped bridge tokens (KT-1006 layer B, bridge part).
//!
//! Every agent launch gets its own random 256-bit token instead of Kronn's
//! admin bearer. The token lives in this process's memory only, is bound to the
//! launch's discussions (or task execution, or workflow run, or bare project)
//! and to one project frozen on first use, and dies when the launch's process
//! handle is dropped or killed, when its maximum lifetime passes, when its
//! discussion or run is deleted, or when the backend restarts.
//!
//! A request bearing one is accepted only on [`BRIDGE_ROUTES`] — the routes the
//! `kronn-internal` bridge calls — and only for resources in the token's scope,
//! wherever the request names them: path, query, any depth of the JSON body
//! (an imported workflow's `content` included). Shared resources (project-less,
//! or serving every project) may be read, never written; effects on them run
//! only for the bound project. Every response is scoped too. A request without
//! any token keeps the loopback trust it has today: the per-action human proof
//! that replaces it is deferred (design note §4, §9).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use aes_gcm::aead::{rand_core::RngCore, OsRng};
use sha2::{Digest, Sha256};

/// Every bridge token starts with this, so a dead one is recognised and
/// refused instead of falling back to loopback trust.
pub const TOKEN_PREFIX: &str = "kbt_";

/// Longest a token lives, whatever its launch does: a wedged agent does not
/// keep a live credential forever.
pub const MAX_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

/// What a launch is bound to. The project is resolved from these on first use
/// (the auth middleware has the database) and frozen there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeScope {
    /// The launch's own discussions: its discussion and its room / step /
    /// worker contexts' discussions.
    pub discussion_ids: Vec<String>,
    pub task_execution_id: Option<String>,
    pub workflow_run_id: Option<String>,
    /// The launch's project when it names no discussion, execution or run
    /// (audits, document agents). Ignored otherwise.
    pub project_id: Option<String>,
}

impl BridgeScope {
    /// No discussion, execution or run: nothing the launch owns.
    pub fn owns_nothing(&self) -> bool {
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
    /// Discussions this launch created through the bridge: its own too.
    adopted: Mutex<Vec<String>>,
    /// The project frozen on first use: `Some(None)` = bound to no project.
    bound: Mutex<Option<Option<String>>>,
    expires_at: Instant,
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

    /// Every discussion the launch owns, for handlers that query by it.
    pub fn own_discussions(&self) -> Vec<String> {
        let mut own = self.scope.discussion_ids.clone();
        if let Ok(adopted) = self.adopted.lock() {
            own.extend(adopted.iter().cloned());
        }
        own
    }

    /// Record a discussion this launch just created.
    pub fn adopt_discussion(&self, id: &str) {
        if let Ok(mut adopted) = self.adopted.lock() {
            if !adopted.iter().any(|own| own == id) {
                adopted.push(id.to_owned());
            }
        }
    }

    /// Freeze `resolved` as the token's project on first use; later it must
    /// match, or the binding changed under the launch and the token is dead.
    pub fn freeze(&self, resolved: Option<String>) -> Result<Option<String>, Refusal> {
        let Ok(mut bound) = self.bound.lock() else {
            return Err(Refusal("bridge token state unavailable".into()));
        };
        match bound.as_ref() {
            Some(frozen) if *frozen != resolved => Err(Refusal(
                "the bridge token's project changed since its launch".into(),
            )),
            Some(frozen) => Ok(frozen.clone()),
            None => {
                *bound = Some(resolved.clone());
                Ok(resolved)
            }
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

/// Whether `presented` is the operator token. Compares SHA-256 digests in
/// constant time, so the comparison leaks neither content nor length.
pub fn operator_token_matches(expected: Option<&str>, presented: &str) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    let (left, right) = (digest(expected), digest(presented));
    left.iter()
        .zip(right.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// The scope of a bridge-token request, attached to the request for handlers
/// that pick a project or a list themselves.
#[derive(Debug, Clone)]
pub struct BridgeCaller {
    pub token_id: String,
    /// The token's frozen project; `None` when it has none.
    pub project: Option<String>,
    /// The launch's own discussions (its scope and the ones it created).
    pub own_discussions: Vec<String>,
    /// The launch's own workflow run, when it is one.
    pub own_run: Option<String>,
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

/// Mint a token for one launch, whatever its scope: a launch that owns
/// nothing gets a token refused almost everywhere rather than no token (which
/// would leave its bridge on loopback trust). An error must fail the launch.
pub fn mint(scope: BridgeScope) -> Result<BridgeTokenGuard, String> {
    mint_with_lifetime(scope, MAX_LIFETIME)
}

/// [`mint`] with an explicit lifetime.
pub fn mint_with_lifetime(
    scope: BridgeScope,
    lifetime: Duration,
) -> Result<BridgeTokenGuard, String> {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let value = format!("{TOKEN_PREFIX}{}", hex(&bytes));
    let hash = digest(&value);
    let id = hex(&hash[..6]);
    let grant = Arc::new(BridgeGrant {
        id: id.clone(),
        scope,
        adopted: Mutex::new(Vec::new()),
        bound: Mutex::new(None),
        expires_at: Instant::now() + lifetime,
    });
    REGISTRY
        .lock()
        .map_err(|_| "the bridge token registry is unavailable".to_string())?
        .insert(hash, grant);
    Ok(BridgeTokenGuard { value, hash, id })
}

/// The live grant behind `token`, if any. An expired grant is removed.
pub fn lookup(token: &str) -> Option<Arc<BridgeGrant>> {
    if !token.starts_with(TOKEN_PREFIX) {
        return None;
    }
    let mut registry = REGISTRY.lock().ok()?;
    let key = digest(token);
    let grant = registry.get(&key)?.clone();
    if Instant::now() >= grant.expires_at {
        registry.remove(&key);
        return None;
    }
    Some(grant)
}

/// How a route treats the resources a request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Reads: shared resources and the token's project are visible.
    Read,
    /// Writes to the launch's own discussions; other ids as for `Write`.
    Own,
    /// Mutations: the target (path ids, subject keys) must belong to the
    /// token's project and to no other; referenced ids must be visible.
    Write,
    /// Runs, triggers, calls, launches: every resource must belong to the
    /// token's project, or be shared on a route whose handler runs it for that
    /// project. Logged with the token id.
    Effect,
    /// Called by the bridge, never granted to a token (the shared agent
    /// library is written by humans only).
    Denied,
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
    /// An MCP / API plugin configuration (`api_config_id`, `mcp_config_ids`).
    McpConfig,
    /// A model-provider connection; shared by design.
    Connection,
    /// A task-execution worker offer.
    Offer,
    // Resources that live inside a discussion or a project: checked as that
    // discussion or project (see [`canonical`]).
    Message,
    ContextFile,
    Dispatch,
    /// A joined CLI session (`cli_session_id`).
    Session,
    Workspace,
    OrchestrationRun,
    Proposal,
    MediaJob,
    AuditRun,
}

impl Kind {
    /// A resource checked as the discussion or project it lives in.
    fn derived(self) -> bool {
        matches!(
            self,
            Kind::Message
                | Kind::ContextFile
                | Kind::Dispatch
                | Kind::Session
                | Kind::Workspace
                | Kind::OrchestrationRun
                | Kind::Proposal
                | Kind::MediaJob
                | Kind::AuditRun
        )
    }
}

/// Whether an id is what the request acts on, or something it references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Target,
    Ref,
}

/// One id a request names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NamedId {
    pub kind: Kind,
    pub id: String,
    pub role: Role,
}

impl NamedId {
    fn new(kind: Kind, id: impl Into<String>, role: Role) -> Self {
        Self {
            kind,
            id: id.into(),
            role,
        }
    }
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
use Rule::{Denied, Effect, Own, Read, Write};

/// Every route `backend/scripts/disc-introspection-mcp.py` calls, and nothing
/// else. `backend/scripts/test_bridge_routes.py` parses the script and fails
/// when the two drift apart in either direction.
pub const BRIDGE_ROUTES: &[BridgeRoute] = &[
    r("GET", "/api/discussions", Read, &[]),
    r("PATCH", "/api/discussions/{id}", Own, &[("id", D)]),
    r("GET", "/api/discussions/{id}/meta", Read, &[("id", D)]),
    r(
        "GET",
        "/api/discussions/{id}/message/{idx}",
        Read,
        &[("id", D)],
    ),
    r("GET", "/api/discussions/{id}/notes", Read, &[("id", D)]),
    r(
        "GET",
        "/api/discussions/{id}/participants",
        Read,
        &[("id", D)],
    ),
    r("GET", "/api/discussions/{id}/questions", Read, &[("id", D)]),
    r("GET", "/api/discussions/{id}/plan", Read, &[("id", D)]),
    r(
        "GET",
        "/api/discussions/{id}/plan/changes",
        Read,
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
        &[("id", D), ("file_id", Kind::ContextFile)],
    ),
    r("POST", "/api/discussions/peer-join", Write, &[]),
    r("POST", "/api/discussions/peer-leave", Write, &[]),
    r("POST", "/api/discussions/peer-resume", Write, &[]),
    r("POST", "/api/discussions/workflow-step-join", Write, &[]),
    r(
        "POST",
        "/api/discussions/orchestrator-return-resume",
        Write,
        &[],
    ),
    r("POST", "/api/disc/create", Write, &[]),
    r("POST", "/api/disc/append", Own, &[]),
    r("POST", "/api/disc/link", Own, &[]),
    r("POST", "/api/disc/unlink", Own, &[]),
    r("POST", "/api/disc/transfer-session", Own, &[]),
    r("GET", "/api/disc/find_by_session", Read, &[]),
    r("GET", "/api/disc/load_other", Read, &[]),
    r("GET", "/api/disc/search", Read, &[]),
    r("GET", "/api/disc/session-status", Read, &[]),
    r("GET", "/api/disc/workspace", Read, &[]),
    r("POST", "/api/disc/workspace", Own, &[]),
    r("POST", "/api/disc/workspace/history-lease", Own, &[]),
    r("POST", "/api/human-credentials/proof", Read, &[]),
    r("GET", "/api/resolve/{id}", Read, &[]),
    r("GET", "/api/planning/tasks", Read, &[]),
    r("POST", "/api/planning/tasks", Write, &[]),
    r("GET", "/api/planning/tasks/{id}", Read, &[("id", T)]),
    r("PATCH", "/api/planning/tasks/{id}", Write, &[("id", T)]),
    r(
        "PATCH",
        "/api/planning/tasks/{id}/dod/{dod_id}",
        Write,
        &[("id", T)],
    ),
    r(
        "POST",
        "/api/planning/tasks/{id}/discussions",
        Write,
        &[("id", T)],
    ),
    r(
        "DELETE",
        "/api/planning/tasks/{id}/discussions",
        Write,
        &[("id", T)],
    ),
    r(
        "POST",
        "/api/planning/tasks/{id}/blockers",
        Write,
        &[("id", T)],
    ),
    r(
        "DELETE",
        "/api/planning/tasks/{id}/blockers/{blocker_id}",
        Write,
        &[("id", T), ("blocker_id", T)],
    ),
    r("GET", "/api/planning/proposals", Read, &[]),
    r(
        "GET",
        "/api/planning/proposals/{id}",
        Read,
        &[("id", Kind::Proposal)],
    ),
    r("POST", "/api/learnings/propose", Write, &[]),
    r("POST", "/api/orchestration/tool/workers", Read, &[]),
    r("POST", "/api/orchestration/tool/prepare", Read, &[]),
    r("POST", "/api/orchestration/tool/launch", Effect, &[]),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/status",
        Read,
        &[("id", X)],
    ),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/resume",
        Write,
        &[("id", X)],
    ),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/cancel",
        Write,
        &[("id", X)],
    ),
    r(
        "POST",
        "/api/orchestration/tool/executions/{id}/reassign",
        Write,
        &[("id", X)],
    ),
    r("POST", "/api/orchestration/accept-offer", Write, &[]),
    r("POST", "/api/orchestration/deliver", Write, &[]),
    r("POST", "/api/orchestration/worker-commit", Write, &[]),
    r("POST", "/api/orchestration/review", Write, &[]),
    r("GET", "/api/workflows", Read, &[]),
    r("POST", "/api/workflows", Write, &[]),
    r("POST", "/api/workflows/import", Write, &[]),
    r("GET", "/api/workflows/step-schema", Read, &[]),
    r("GET", "/api/workflows/{id}", Read, &[("id", W)]),
    r("PUT", "/api/workflows/{id}", Write, &[("id", W)]),
    r("GET", "/api/workflows/{id}/export", Read, &[("id", W)]),
    r("GET", "/api/workflows/{id}/runs", Read, &[("id", W)]),
    r(
        "GET",
        "/api/workflows/{id}/runs/{run_id}",
        Read,
        &[("id", W), ("run_id", R)],
    ),
    r(
        "POST",
        "/api/workflows/{id}/runs/{run_id}/cancel",
        Write,
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
        Read,
        &[("run_id", R)],
    ),
    r(
        "GET",
        "/api/mcp/workflow-run-discussions/{run_id}",
        Read,
        &[("run_id", R)],
    ),
    r("POST", "/api/mcp/workflow-wait-for-completion", Read, &[]),
    r("GET", "/api/quick-prompts", Read, &[]),
    r("POST", "/api/quick-prompts", Write, &[]),
    r("PUT", "/api/quick-prompts/{id}", Write, &[("id", QP)]),
    r("DELETE", "/api/quick-prompts/{id}", Write, &[("id", QP)]),
    r("POST", "/api/mcp/qp-run", Effect, &[]),
    r("POST", "/api/mcp/qp-batch-run", Effect, &[]),
    r("GET", "/api/quick-apis", Read, &[]),
    r("POST", "/api/quick-apis", Write, &[]),
    r("PUT", "/api/quick-apis/{id}", Write, &[("id", QA)]),
    r("POST", "/api/quick-apis/{id}/run", Effect, &[("id", QA)]),
    r("GET", "/api/quick-execs", Read, &[]),
    r("POST", "/api/quick-execs", Write, &[]),
    r("PUT", "/api/quick-execs/{id}", Write, &[("id", QE)]),
    r("POST", "/api/quick-execs/{id}/run", Effect, &[("id", QE)]),
    r("POST", "/api/agent-api/call", Effect, &[]),
    r("POST", "/api/pages", Write, &[]),
    r("GET", "/api/pages/{id}", Read, &[("id", G)]),
    r("PATCH", "/api/pages/{id}", Write, &[("id", G)]),
    r("PUT", "/api/pages/{id}/html", Write, &[("id", G)]),
    r("POST", "/api/pages/{id}/datasets", Write, &[("id", G)]),
    r("GET", "/api/pages/{id}/workflows", Read, &[("id", G)]),
    r("GET", "/api/pages/{id}/discussions", Read, &[("id", G)]),
    r("POST", "/api/media/generate", Effect, &[]),
    r(
        "GET",
        "/api/media/jobs/{id}",
        Read,
        &[("id", Kind::MediaJob)],
    ),
    r("GET", "/api/projects/{id}", Read, &[("id", P)]),
    r("GET", "/api/projects/{id}/audit-info", Read, &[("id", P)]),
    r("GET", "/api/projects/{id}/audit-status", Read, &[("id", P)]),
    r("GET", "/api/projects/{id}/audit-latest", Read, &[("id", P)]),
    r(
        "GET",
        "/api/projects/{id}/audit-resumable",
        Read,
        &[("id", P)],
    ),
    r(
        "POST",
        "/api/projects/{id}/install-template",
        Write,
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
    r("GET", "/api/skills", Read, &[]),
    r("POST", "/api/skills", Denied, &[]),
    r("PUT", "/api/skills/{id}", Denied, &[]),
    r("DELETE", "/api/skills/{id}", Denied, &[]),
    r("GET", "/api/profiles", Read, &[]),
    r("POST", "/api/profiles", Denied, &[]),
    r("PUT", "/api/profiles/{id}", Denied, &[]),
    r("DELETE", "/api/profiles/{id}", Denied, &[]),
    r("GET", "/api/directives", Read, &[]),
    r("POST", "/api/directives", Denied, &[]),
    r("PUT", "/api/directives/{id}", Denied, &[]),
    r("DELETE", "/api/directives/{id}", Denied, &[]),
    r("GET", "/api/mcps", Read, &[]),
    r("GET", "/api/signals/catalog", Read, &[]),
    r("GET", "/api/conventions/agents-md-format-v1", Read, &[]),
];
/// The route a bridge token may call for this method and matched pattern.
pub fn route_for(method: &str, pattern: &str) -> Option<&'static BridgeRoute> {
    BRIDGE_ROUTES
        .iter()
        .find(|route| route.method.eq_ignore_ascii_case(method) && route.pattern == pattern)
}

/// Routes a token that owns nothing and has no project may still call: global
/// catalogues, read only.
const UNBOUND_ROUTES: &[&str] = &[
    "/api/conventions/agents-md-format-v1",
    "/api/workflows/step-schema",
    "/api/signals/catalog",
    "/api/skills",
    "/api/profiles",
    "/api/directives",
];

/// Whether a token whose scope owns nothing and names no project may call
/// this route.
pub fn unbound_route_allowed(route: &BridgeRoute) -> bool {
    route.method == "GET" && UNBOUND_ROUTES.contains(&route.pattern)
}

/// Routes whose run lands on a project the body may leave unnamed: the token's
/// project is added, so a shared or multi-project resource runs for it.
const RUNS_FOR_A_PROJECT: &[&str] = &[
    "/api/mcp/workflow-trigger",
    "/api/mcp/qp-run",
    "/api/mcp/qp-batch-run",
];

/// Effect routes whose handler runs a shared resource for the token's project
/// (body injection above, or the handler reads [`BridgeCaller`]).
const SHARED_EFFECT_ROUTES: &[&str] = &[
    "/api/mcp/workflow-trigger",
    "/api/mcp/qp-run",
    "/api/mcp/qp-batch-run",
    "/api/quick-apis/{id}/run",
    "/api/quick-execs/{id}/run",
    "/api/agent-api/call",
    "/api/media/generate",
    "/api/orchestration/tool/launch",
];

/// Create routes and the field holding the new resource's project: forced to
/// the token's project.
const CREATE_ROUTES: &[(&str, &str)] = &[
    ("/api/workflows", "project_id"),
    ("/api/workflows/import", "project_id"),
    ("/api/quick-prompts", "project_id"),
    ("/api/quick-apis", "project_id"),
    ("/api/quick-execs", "project_id"),
    ("/api/pages", "project_id"),
    ("/api/planning/tasks", "project_ids"),
    ("/api/disc/create", "project_id"),
    ("/api/learnings/propose", "project_id"),
];

/// Update routes whose handler resets or may drop the project when the body
/// omits it: the token's project is sent explicitly.
const KEEPS_PROJECT_ON_UPDATE: &[&str] = &[
    "/api/workflows/{id}",
    "/api/quick-prompts/{id}",
    "/api/quick-apis/{id}",
    "/api/quick-execs/{id}",
];

/// The JSON body the handler will receive: the token's project forced on
/// creation, kept on update, added to a project-less run; a project removed,
/// changed or widened, or a workflow scope beyond the token's project, is
/// refused. `Ok(None)` = the body stays as sent.
pub fn prepare_body(
    route: &BridgeRoute,
    bound: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Result<Option<serde_json::Value>, Refusal> {
    if route.method == "GET" {
        return Ok(None);
    }
    let Some(body) = body else {
        return Ok(None);
    };
    let mut body = body.clone();
    let mut changed = false;
    if let Some(fields) = body
        .as_object()
        .filter(|_| route.pattern == "/api/learnings/propose")
    {
        // A preference applies to every project: a human records it.
        if fields.get("kind").and_then(|kind| kind.as_str()) == Some("preference") {
            return Err(Refusal(
                "a bridge token cannot propose a preference for every project".into(),
            ));
        }
    }
    if route.pattern == "/api/disc/link"
        && body.get("force_reassign") == Some(&serde_json::json!(true))
    {
        return Err(Refusal(
            "a bridge token cannot take a session over from another discussion".into(),
        ));
    }
    // A token's planning write is an agent's, whatever actor it names: the
    // event log never records it as a human's (or the backend's).
    if route.pattern.starts_with("/api/planning/tasks") {
        if let Some(fields) = body.as_object_mut() {
            let actor = fields
                .entry("actor")
                .or_insert_with(|| serde_json::json!({}));
            if !actor.is_object() {
                *actor = serde_json::json!({});
            }
            if let Some(actor) = actor.as_object_mut() {
                actor.insert("kind".into(), serde_json::json!("agent"));
                // An agent actor must name itself; a token that did not is
                // recorded as a bridge agent.
                let named = actor
                    .get("id")
                    .and_then(|id| id.as_str())
                    .is_some_and(|id| !id.trim().is_empty());
                if !named {
                    actor.insert("id".into(), serde_json::json!("bridge-agent"));
                }
            }
            changed = true;
        }
    }
    if let Some(fields) = body.as_object_mut() {
        let create = CREATE_ROUTES
            .iter()
            .find(|(pattern, _)| route.method == "POST" && *pattern == route.pattern);
        if let Some((_, field)) = create {
            let unset = match fields.get(*field) {
                None | Some(serde_json::Value::Null) => true,
                Some(serde_json::Value::Array(items)) => items.is_empty(),
                Some(_) => false,
            };
            if unset {
                match (bound, *field) {
                    (Some(project), "project_ids") => {
                        fields.insert(field.to_string(), serde_json::json!([project]));
                        changed = true;
                    }
                    (Some(project), _) => {
                        fields.insert(field.to_string(), serde_json::json!(project));
                        changed = true;
                    }
                    // A project-less launch may create its own discussions only.
                    (None, _) if route.pattern == "/api/disc/create" => {}
                    (None, _) => {
                        return Err(Refusal(
                            "a bridge token without a project cannot create shared resources"
                                .into(),
                        ))
                    }
                }
            }
        } else if let Some(project) = bound {
            let keeps = route.method == "PUT" && KEEPS_PROJECT_ON_UPDATE.contains(&route.pattern);
            if keeps && !fields.contains_key("project_id") {
                fields.insert("project_id".into(), serde_json::json!(project));
                changed = true;
            }
            if RUNS_FOR_A_PROJECT.contains(&route.pattern)
                && fields.get("project_id").is_none_or(|value| value.is_null())
            {
                fields.insert("project_id".into(), serde_json::json!(project));
                changed = true;
            }
        }
        // The resource's own project may be named only as the token's.
        match fields.get("project_id") {
            Some(serde_json::Value::Null) => {
                return Err(Refusal(
                    "a bridge token cannot move a resource out of its project".into(),
                ))
            }
            Some(serde_json::Value::String(project)) if Some(project.as_str()) != bound => {
                return Err(Refusal(
                    "the project named is outside this bridge token's project".into(),
                ))
            }
            _ => {}
        }
        match fields.get("project_ids") {
            Some(serde_json::Value::Null) => {
                return Err(Refusal(
                    "a bridge token cannot move a resource out of its project".into(),
                ))
            }
            Some(serde_json::Value::Array(items)) if items.is_empty() => {
                return Err(Refusal(
                    "a bridge token cannot move a resource out of its project".into(),
                ))
            }
            _ => {}
        }
    }
    check_project_scopes(&body, bound)?;
    if route.pattern == "/api/workflows/import" {
        let content = import_content(&body)?;
        check_project_scopes(&content, bound)?;
    }
    Ok(changed.then_some(body))
}

/// A workflow may be scoped to the token's project only: `null`, or
/// `{"type":"Projects","project_ids":[bound]}`.
fn check_project_scopes(value: &serde_json::Value, bound: Option<&str>) -> Result<(), Refusal> {
    match value {
        serde_json::Value::Object(fields) => {
            if let Some(scope) = fields.get("project_scope").filter(|scope| !scope.is_null()) {
                let kind = scope.get("type").and_then(|kind| kind.as_str());
                let ids: Vec<&str> = scope
                    .get("project_ids")
                    .and_then(|ids| ids.as_array())
                    .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
                    .unwrap_or_default();
                let only_bound =
                    kind == Some("Projects") && bound.is_some_and(|project| ids == [project]);
                if !only_bound {
                    return Err(Refusal(
                        "a bridge token may scope a workflow to its own project only".into(),
                    ));
                }
            }
            fields
                .values()
                .try_for_each(|child| check_project_scopes(child, bound))
        }
        serde_json::Value::Array(items) => items
            .iter()
            .try_for_each(|child| check_project_scopes(child, bound)),
        _ => Ok(()),
    }
}

/// The workflow envelope an import carries as a JSON string.
fn import_content(body: &serde_json::Value) -> Result<serde_json::Value, Refusal> {
    body.get("content")
        .and_then(|content| content.as_str())
        .and_then(|content| serde_json::from_str(content).ok())
        .ok_or_else(|| Refusal("an imported workflow must be a JSON envelope".into()))
}

/// Body and query keys that name a scoped resource, at any depth.
const ID_KEYS: &[(&str, Kind)] = &[
    ("discussion_id", Kind::Discussion),
    ("disc_id", Kind::Discussion),
    ("discussion_ids", Kind::Discussion),
    ("disc_ids", Kind::Discussion),
    ("parent_discussion_id", Kind::Discussion),
    ("source_discussion_id", Kind::Discussion),
    ("sub_discussion_id", Kind::Discussion),
    ("child_discussion_id", Kind::Discussion),
    ("origin_discussion_id", Kind::Discussion),
    ("judge_discussion_id", Kind::Discussion),
    ("review_discussion_id", Kind::Discussion),
    ("validation_discussion_id", Kind::Discussion),
    ("from_disc_id", Kind::Discussion),
    ("to_disc_id", Kind::Discussion),
    ("runtime_disc_id", Kind::Discussion),
    ("resume_disc_id", Kind::Discussion),
    ("return_disc_id", Kind::Discussion),
    ("previous_disc_id", Kind::Discussion),
    ("expected_disc_id", Kind::Discussion),
    ("expected_child_disc_id", Kind::Discussion),
    ("bound_disc_id", Kind::Discussion),
    ("connected_disc_id", Kind::Discussion),
    ("room_id", Kind::Discussion),
    ("project_id", Kind::Project),
    ("project_ids", Kind::Project),
    ("target_project_id", Kind::Project),
    ("task_id", Kind::Task),
    ("blocker_task_id", Kind::Task),
    ("task_reference", Kind::Task),
    ("task_ref", Kind::Task),
    ("parent_reference", Kind::Task),
    ("workflow_id", Kind::Workflow),
    ("parent_workflow_id", Kind::Workflow),
    ("sub_workflow_id", Kind::Workflow),
    ("run_id", Kind::Run),
    ("workflow_run_id", Kind::Run),
    ("parent_run_id", Kind::Run),
    ("resume_run_id", Kind::Run),
    ("child_run_id", Kind::Run),
    ("judge_run_id", Kind::Run),
    ("carried_from_run_id", Kind::Run),
    ("triggered_by_run_id", Kind::Run),
    ("execution_id", Kind::Execution),
    ("task_execution_id", Kind::Execution),
    ("qp_id", Kind::QuickPrompt),
    ("quick_prompt_id", Kind::QuickPrompt),
    ("batch_quick_prompt_id", Kind::QuickPrompt),
    ("batch_chain_prompt_ids", Kind::QuickPrompt),
    ("originating_qp_id", Kind::QuickPrompt),
    ("qa_id", Kind::QuickApi),
    ("quick_api_id", Kind::QuickApi),
    ("qe_id", Kind::QuickExec),
    ("quick_exec_id", Kind::QuickExec),
    ("approved_quick_exec_ids", Kind::QuickExec),
    ("page_id", Kind::Page),
    ("api_config_id", Kind::McpConfig),
    ("mcp_config_ids", Kind::McpConfig),
    ("config_id", Kind::McpConfig),
    ("config_ids", Kind::McpConfig),
    ("source_config_id", Kind::McpConfig),
    ("imported_config_ids", Kind::McpConfig),
    ("connection_id", Kind::Connection),
    ("worker_connection_id", Kind::Connection),
    ("offer_id", Kind::Offer),
    ("message_id", Kind::Message),
    ("message_ref", Kind::Message),
    ("last_message_id", Kind::Message),
    ("reply_to_message_id", Kind::Message),
    ("source_message_id", Kind::Message),
    ("target_message_id", Kind::Message),
    ("trigger_message_id", Kind::Message),
    ("offer_message_id", Kind::Message),
    ("delivered_message_ids", Kind::Message),
    ("file_id", Kind::ContextFile),
    ("file_ids", Kind::ContextFile),
    ("context_file_id", Kind::ContextFile),
    ("asset_id", Kind::ContextFile),
    ("reference_asset_id", Kind::ContextFile),
    ("reference_asset_ids", Kind::ContextFile),
    ("extracted_from_asset_id", Kind::ContextFile),
    ("dispatch_id", Kind::Dispatch),
    ("dispatch_job_id", Kind::Dispatch),
    ("source_dispatch_job_id", Kind::Dispatch),
    ("completion_dispatch_id", Kind::Dispatch),
    ("cli_session_id", Kind::Session),
    ("target_cli_session_id", Kind::Session),
    ("worker_cli_session_id", Kind::Session),
    ("workspace_id", Kind::Workspace),
    ("target_workspace_id", Kind::Workspace),
    ("orchestration_run_id", Kind::OrchestrationRun),
    ("proposal_id", Kind::Proposal),
    ("job_id", Kind::MediaJob),
    ("audit_run_id", Kind::AuditRun),
];

/// Id-shaped keys known to carry no scoped resource id. Reviewed one by one:
/// anything id-shaped that is neither here nor in [`ID_KEYS`] is refused.
const PLAIN_ID_KEYS: &[&str] = &[
    // The caller's own CLI session identity; the routes keyed by it resolve
    // its room before the check (see `credential_targets`).
    "session_id",
    "actor_session_id",
    "source_session_id",
    "source_binding_session_id",
    "conversation_id",
    // Idempotency keys the caller chooses for a message it is writing.
    "source_msg_id",
    "client_message_id",
    // Definition-of-done items: scoped by the task in the path or by the
    // execution the handler revalidates.
    "dod_id",
    "worker_dod_ids",
    // A dataset of the page named in the path.
    "dataset_id",
    // Model catalogue entries (no project, no secret).
    "model_id",
    "voice_id",
    "generation_id",
    "runtime_target_id",
    // MCP server catalogue entries (global).
    "server_id",
    "replacement_server_id",
    // The shared agent library: read-only for a bridge token.
    "skill_id",
    "skill_ids",
    "default_skill_ids",
    "profile_id",
    "profile_ids",
    "default_profile_id",
    "worker_profile_id",
    "directive_id",
    "directive_ids",
    // Git refs and tracker keys, not Kronn ids.
    "backup_ref",
    "base_ref",
    "source_ref",
    "test_mode_stash_ref",
    "ticket_ref",
    // Option ids of a question card.
    "recommended_option_ids",
];

/// Containers whose objects carry their own `id` (a step, a DoD item, an
/// option, the planning actor) rather than a reference to a resource, or a
/// `ref` Kronn never resolves (a learning's evidence: a path, URL or note).
const OWN_CONTAINERS: &[&str] = &[
    "evidence",
    "steps",
    "on_failure",
    "definition_of_done",
    "dod",
    "items",
    "variables",
    "options",
    "sources",
    "actor",
    "links",
    "exec_script_files",
    "messages",
];

/// Fields holding a caller's own data (an HTTP body for an external API, a
/// page dataset, a JSON schema, a sample): Kronn never reads a reference out
/// of them, so their contents are not checked.
const OPAQUE_KEYS: &[&str] = &[
    "api_body",
    "body",
    "json_data_payload",
    "data",
    "envelope",
    "sample",
    "payload",
    "current",
    "initial",
    "value",
    "schema",
    "json_schema",
    "fallback",
    "details",
];

/// Maps whose keys the caller chooses (template variables, an external API's
/// path, query and headers, per-step choices): a key there is a name, not a
/// Kronn field. Their values reach Kronn through templates, and a rendered
/// page or room is checked at run time (`workflows::run_scope`).
const USER_KEYED_KEYS: &[&str] = &[
    "vars",
    "variables",
    "path_params",
    "query",
    "headers",
    "api_query",
    "api_path_params",
    "api_headers",
    "quick_prompt_variables",
    "sub_workflow_variables",
    "step_agents",
    "state",
];

/// Keys naming what a write acts on rather than what it references.
const TARGET_KEYS: &[&str] = &["task_execution_id", "execution_id", "offer_id", "parent_id"];

/// Whether a key looks like it holds an id.
pub fn looks_like_id(key: &str) -> bool {
    matches!(key, "id" | "ref")
        || ["_id", "_ids", "_ref", "_refs", "_reference", "_references"]
            .iter()
            .any(|suffix| key.ends_with(suffix))
}

fn kind_of_key(route: &BridgeRoute, key: &str) -> Option<Kind> {
    // `parent_id` is a task only on the planning routes; elsewhere it is an
    // unknown id field.
    if key == "parent_id" {
        return route
            .pattern
            .starts_with("/api/planning/tasks")
            .then_some(Kind::Task);
    }
    ID_KEYS
        .iter()
        .find_map(|(name, kind)| (*name == key).then_some(*kind))
}

fn key_role(key: &str) -> Role {
    if TARGET_KEYS.contains(&key) {
        Role::Target
    } else {
        Role::Ref
    }
}

/// Path parameters a route declares that name what it references, not what
/// it acts on.
fn param_role(route: &BridgeRoute, name: &str) -> Role {
    let reference =
        (name == "id" && route.pattern.contains("/runs/{run_id}")) || name == "blocker_id";
    if reference {
        Role::Ref
    } else {
        Role::Target
    }
}

/// A page or room a saved workflow will write into: on a token's save, update
/// or import it must be the token's project's, since a literal one is used at
/// run time whatever its project (`workflows::run_scope`).
fn publishes_into(route: &BridgeRoute, key: &str, parent: Option<&str>) -> bool {
    let saves_workflow = matches!(
        (route.method, route.pattern),
        ("POST", "/api/workflows")
            | ("PUT", "/api/workflows/{id}")
            | ("POST", "/api/workflows/import")
    );
    saves_workflow && (key == "room_id" || (key == "page_id" && parent == Some("page_publish")))
}

fn unknown_id_field(key: &str) -> Refusal {
    Refusal(format!(
        "`{key}` is an id field a bridge token cannot use on this route"
    ))
}

/// The resources an import bundles, per kind: refs to them are remapped by
/// the import and not looked up. Discussions, configs and connections are
/// never bundled.
#[derive(Default)]
struct Bundled(HashSet<(Kind, String)>);

impl Bundled {
    fn of_import(content: &serde_json::Value) -> Self {
        let mut bundled = HashSet::new();
        let mut add = |kind: Kind, value: Option<&serde_json::Value>| {
            if let Some(id) = value.and_then(|value| value.as_str()) {
                bundled.insert((kind, id.to_owned()));
            }
        };
        add(Kind::Workflow, content.pointer("/workflow/id"));
        for (list, kind) in [
            ("referenced_workflows", Kind::Workflow),
            ("referenced_quick_prompts", Kind::QuickPrompt),
            ("referenced_quick_apis", Kind::QuickApi),
            ("referenced_quick_execs", Kind::QuickExec),
            ("referenced_pages", Kind::Page),
        ] {
            for item in content
                .get(list)
                .and_then(|items| items.as_array())
                .into_iter()
                .flatten()
            {
                add(kind, item.get("id"));
                if kind == Kind::Page {
                    add(kind, item.get("slug"));
                }
            }
        }
        Self(bundled)
    }

    fn holds(&self, kind: Kind, id: &str) -> bool {
        self.0.contains(&(kind, id.to_owned()))
    }
}

struct Walk<'a> {
    route: &'a BridgeRoute,
    /// Inside an import's `content`: its own project fields are replaced at
    /// import, its `id`s are definitions, bundled refs are remapped.
    bundled: Option<&'a Bundled>,
}

impl Walk<'_> {
    /// Collect every id under a known key, at any depth; refuse an id-shaped
    /// key that is neither known nor reviewed as plain.
    fn walk(
        &self,
        value: &serde_json::Value,
        parent: Option<&str>,
        depth: usize,
        out: &mut Vec<NamedId>,
    ) -> Result<(), Refusal> {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, child) in fields {
                    if OPAQUE_KEYS.contains(&key.as_str()) && !looks_like_id(key) {
                        continue;
                    }
                    // A map whose keys the caller names: each key is a name,
                    // each value is walked as it is.
                    if let (true, Some(entries)) =
                        (USER_KEYED_KEYS.contains(&key.as_str()), child.as_object())
                    {
                        for value in entries.values() {
                            self.walk(value, None, depth + 2, out)?;
                        }
                        continue;
                    }
                    self.key(key, child, parent, depth, out)?;
                    self.walk(child, Some(key), depth + 1, out)?;
                }
                Ok(())
            }
            serde_json::Value::Array(items) => items
                .iter()
                .try_for_each(|item| self.walk(item, parent, depth + 1, out)),
            _ => Ok(()),
        }
    }

    fn key(
        &self,
        key: &str,
        value: &serde_json::Value,
        parent: Option<&str>,
        depth: usize,
        out: &mut Vec<NamedId>,
    ) -> Result<(), Refusal> {
        if !looks_like_id(key) {
            return Ok(());
        }
        if let Some(kind) = kind_of_key(self.route, key) {
            if self.bundled.is_some() && kind == Kind::Project {
                return Ok(());
            }
            let role = if publishes_into(self.route, key, parent) {
                Role::Target
            } else {
                key_role(key)
            };
            let mut push = |raw: &serde_json::Value| {
                let id = match raw {
                    serde_json::Value::String(id) => id.clone(),
                    serde_json::Value::Number(number) => number.to_string(),
                    _ => return,
                };
                let bundled = self.bundled.is_some_and(|b| b.holds(kind, &id));
                if !id.is_empty() && !bundled {
                    out.push(NamedId::new(kind, id, role));
                }
            };
            match value {
                serde_json::Value::Array(items) => items.iter().for_each(&mut push),
                other => push(other),
            }
            return Ok(());
        }
        if PLAIN_ID_KEYS.contains(&key) {
            return Ok(());
        }
        // The resource's own id, or a sub-object's own id.
        let own_id = matches!(key, "id" | "ref")
            && (depth == 0
                || self.bundled.is_some()
                || parent.is_some_and(|parent| OWN_CONTAINERS.contains(&parent)));
        if own_id {
            return Ok(());
        }
        Err(unknown_id_field(key))
    }
}

/// The ids a request names: declared path parameters, query parameters, and
/// id-shaped keys at any depth of the JSON body, plus the workflow an import
/// carries. An id-shaped key the gate cannot resolve is refused.
pub fn collect_ids(
    route: &BridgeRoute,
    path_params: &[(String, String)],
    query: Option<&str>,
    body: Option<&serde_json::Value>,
) -> Result<Vec<NamedId>, Refusal> {
    let mut ids = Vec::new();
    for (name, value) in path_params {
        if let Some((_, kind)) = route.params.iter().find(|(param, _)| param == name) {
            ids.push(NamedId::new(*kind, value.clone(), param_role(route, name)));
        }
    }
    if let Some(query) = query {
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = percent_decode(key);
            if !looks_like_id(&key) {
                continue;
            }
            match kind_of_key(route, &key) {
                Some(kind) => {
                    let value = percent_decode(value);
                    if !value.is_empty() {
                        ids.push(NamedId::new(kind, value, key_role(&key)));
                    }
                }
                None if PLAIN_ID_KEYS.contains(&key.as_str()) => {}
                None => return Err(unknown_id_field(&key)),
            }
        }
    }
    if let Some(body) = body {
        let walk = Walk {
            route,
            bundled: None,
        };
        walk.walk(body, None, 0, &mut ids)?;
        if route.pattern == "/api/workflows/import" {
            let content = import_content(body)?;
            let bundled = Bundled::of_import(&content);
            let walk = Walk {
                route,
                bundled: Some(&bundled),
            };
            walk.walk(&content, None, 0, &mut ids)?;
        }
    }
    ids.sort_by(|a, b| (a.id.as_str(), a.role as u8).cmp(&(b.id.as_str(), b.role as u8)));
    ids.dedup();
    Ok(ids)
}

/// `application/x-www-form-urlencoded` decoding: `+` and `%XX`. Invalid
/// escapes stay literal; invalid UTF-8 is replaced.
pub fn percent_decode(raw: &str) -> String {
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

/// Where a resource lives. `None` from the resolver = it does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Residence {
    /// Project-less: shared by every scope (a project-less discussion is
    /// private to its own launches instead).
    Global,
    /// Every project the resource serves.
    Projects(HashSet<String>),
    /// Serves every project (a global config, a workflow scoped to all).
    AllProjects,
    /// Visible to project-less callers only (an MCP config opted into General
    /// discussions and linked to no project).
    General,
    /// These projects, and project-less callers too (an MCP config linked to
    /// projects and opted into General discussions).
    ProjectsAndGeneral(HashSet<String>),
}

impl Residence {
    fn holds(&self, project: &str) -> bool {
        match self {
            Self::AllProjects => true,
            Self::Projects(projects) | Self::ProjectsAndGeneral(projects) => {
                projects.contains(project)
            }
            Self::Global | Self::General => false,
        }
    }

    fn only(&self, project: &str) -> bool {
        matches!(self, Self::Projects(projects) if projects.len() == 1 && projects.contains(project))
    }

    fn shared(&self) -> bool {
        !matches!(self, Self::Projects(_))
    }
}

/// Why a request was refused. Never echoes a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal(pub String);

fn owns(grant: &BridgeGrant, kind: Kind, id: &str) -> bool {
    match kind {
        Kind::Discussion => grant.owns_discussion(id),
        Kind::Execution => grant.scope.task_execution_id.as_deref() == Some(id),
        Kind::Run => grant.scope.workflow_run_id.as_deref() == Some(id),
        _ => false,
    }
}

/// Whether a token may see a resource: its own, its project's, or shared
/// (project-less discussions excepted: they are private to their launches).
pub fn visible_for_read(
    grant: &BridgeGrant,
    kind: Kind,
    id: &str,
    place: &Residence,
    bound: Option<&str>,
) -> bool {
    if owns(grant, kind, id) {
        return true;
    }
    match (kind, place) {
        (Kind::Project, _) => bound == Some(id),
        // A discussion or a run without a project is private to its launch.
        (Kind::Discussion | Kind::Run, Residence::Projects(_)) => {
            bound.is_some_and(|p| place.holds(p))
        }
        (Kind::Discussion | Kind::Run, _) => false,
        (_, Residence::Projects(_)) => bound.is_some_and(|p| place.holds(p)),
        (_, Residence::ProjectsAndGeneral(_)) => bound.is_none_or(|p| place.holds(p)),
        (_, Residence::General) => bound.is_none(),
        (_, Residence::Global | Residence::AllProjects) => true,
    }
}

fn writable(
    grant: &BridgeGrant,
    kind: Kind,
    id: &str,
    place: &Residence,
    bound: Option<&str>,
) -> bool {
    owns(grant, kind, id) || bound.is_some_and(|project| place.only(project))
}

fn effect_allowed(
    grant: &BridgeGrant,
    route: &BridgeRoute,
    kind: Kind,
    id: &str,
    place: &Residence,
    bound: Option<&str>,
) -> bool {
    if owns(grant, kind, id) {
        return true;
    }
    if kind == Kind::Discussion {
        // Discussions are never shared resources: own, or the bound project's.
        return matches!(place, Residence::Projects(_)) && bound.is_some_and(|p| place.holds(p));
    }
    if let Some(project) = bound {
        if matches!(
            place,
            Residence::Projects(_) | Residence::ProjectsAndGeneral(_)
        ) {
            return place.holds(project);
        }
    }
    let shared_effect = SHARED_EFFECT_ROUTES.contains(&route.pattern);
    match (place, bound) {
        (Residence::Projects(_), _) => false,
        (_, Some(_)) => shared_effect && !matches!(place, Residence::General),
        // A project-less token runs only what is itself project-less.
        (Residence::Global | Residence::General | Residence::ProjectsAndGeneral(_), None) => {
            shared_effect
        }
        (Residence::AllProjects, None) => false,
    }
}

/// Decide one request against the grant. `residence` looks a resource up;
/// `bound` is the grant's frozen project.
pub fn authorize(
    grant: &BridgeGrant,
    route: &BridgeRoute,
    bound: Option<&str>,
    ids: &[NamedId],
    mut residence: impl FnMut(Kind, &str) -> Result<Option<Residence>, Refusal>,
) -> Result<(), Refusal> {
    if route.rule == Rule::Denied {
        return Err(Refusal(
            "this route is not available to a Kronn-launched agent's bridge token".into(),
        ));
    }
    for named in ids {
        let (kind, id) = (named.kind, named.id.as_str());
        if kind == Kind::Project {
            if bound == Some(id) {
                continue;
            }
            return Err(Refusal(
                "a project the request names is outside this bridge token's project".into(),
            ));
        }
        if kind == Kind::Discussion && route.rule == Rule::Own && !grant.owns_discussion(id) {
            return Err(Refusal(
                "a bridge token may only write to its own discussion".into(),
            ));
        }
        // An id the operation would not resolve either is refused, never let
        // through: authorizing must see what the handler will act on.
        let place = match residence(kind, id)? {
            Some(place) => place,
            None if owns(grant, kind, id) => continue,
            None => {
                return Err(Refusal(format!(
                    "a {kind:?} the request names does not exist"
                )))
            }
        };
        let allowed = match (route.rule, named.role) {
            (Rule::Effect, _) => effect_allowed(grant, route, kind, id, &place, bound),
            (Rule::Write | Rule::Own, Role::Target) => writable(grant, kind, id, &place, bound),
            _ => visible_for_read(grant, kind, id, &place, bound),
        };
        if !allowed {
            let why = if place.shared() && named.role == Role::Target {
                "is shared: a bridge token may read it, not change it"
            } else {
                "is outside this bridge token's scope"
            };
            // Never the id: it may belong to another project.
            return Err(Refusal(format!("a {kind:?} the request names {why}")));
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
) -> anyhow::Result<Option<Residence>> {
    use rusqlite::OptionalExtension;
    let project_of = |sql: &str| -> anyhow::Result<Option<Residence>> {
        Ok(conn
            .query_row(sql, [id], |row| row.get::<_, Option<String>>(0))
            .optional()?
            .map(single_or_global))
    };
    let execution_residence = |execution_id: &str| -> anyhow::Result<Option<Residence>> {
        // Same resolution as the orchestration tools: an execution id, or a
        // task reference standing for its active or latest execution.
        let Some(execution) =
            crate::api::orchestration::resolve_task_execution_reference(conn, execution_id)?
        else {
            return Ok(None);
        };
        Ok(conn
            .query_row(
                "SELECT project_id FROM discussions WHERE id = ?1",
                [&execution.parent_discussion_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .map(single_or_global))
    };
    match kind {
        Kind::Discussion => project_of("SELECT project_id FROM discussions WHERE id = ?1"),
        Kind::QuickPrompt => project_of("SELECT project_id FROM quick_prompts WHERE id = ?1"),
        Kind::QuickApi => project_of("SELECT project_id FROM quick_apis WHERE id = ?1"),
        Kind::QuickExec => project_of("SELECT project_id FROM quick_execs WHERE id = ?1"),
        Kind::Page => project_of("SELECT project_id FROM live_pages WHERE id = ?1"),
        Kind::Execution => execution_residence(id),
        // Checked as the discussion or project they live in: see `canonical`.
        Kind::Message
        | Kind::ContextFile
        | Kind::Dispatch
        | Kind::Session
        | Kind::Workspace
        | Kind::OrchestrationRun
        | Kind::Proposal
        | Kind::MediaJob
        | Kind::AuditRun => Ok(None),
        Kind::Offer => {
            let execution: Option<String> = conn
                .query_row(
                    "SELECT task_execution_id FROM task_execution_worker_offers WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            match execution {
                Some(execution) => execution_residence(&execution),
                None => Ok(None),
            }
        }
        Kind::Project => Ok(conn
            .query_row("SELECT 1 FROM projects WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .map(|()| single_or_global(Some(id.to_owned())))),
        Kind::Connection => Ok(conn
            .query_row(
                "SELECT 1 FROM external_api_connections WHERE id = ?1",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .map(|()| Residence::Global)),
        Kind::McpConfig => {
            let flags: Option<(bool, bool)> = conn
                .query_row(
                    "SELECT is_global, include_general FROM mcp_configs WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((is_global, include_general)) = flags else {
                return Ok(None);
            };
            if is_global {
                return Ok(Some(Residence::AllProjects));
            }
            let mut statement =
                conn.prepare("SELECT project_id FROM mcp_config_projects WHERE config_id = ?1")?;
            let projects: HashSet<String> = statement
                .query_map([id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some(if !projects.is_empty() && include_general {
                Residence::ProjectsAndGeneral(projects)
            } else if !projects.is_empty() {
                Residence::Projects(projects)
            } else if include_general {
                Residence::General
            } else {
                // Linked nowhere: usable by no agent.
                Residence::Projects(HashSet::new())
            }))
        }
        Kind::Task => {
            // Same resolution as the planning handlers: `KT-12` or an id.
            let Some(task_id) = crate::db::planning::lookup_task_id(conn, id)? else {
                return Ok(None);
            };
            let mut statement =
                conn.prepare("SELECT project_id FROM planning_task_projects WHERE task_id = ?1")?;
            let projects: HashSet<String> = statement
                .query_map([&task_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Some(if projects.is_empty() {
                Residence::Global
            } else {
                Residence::Projects(projects)
            }))
        }
        Kind::Workflow => Ok(conn
            .query_row(
                "SELECT project_id, project_scope_json FROM workflows WHERE id = ?1",
                [id],
                |row| Ok(workflow_residence(row.get(0)?, row.get(1)?)),
            )
            .optional()?),
        Kind::Run => {
            // A run's own project, never its workflow's current home: a run
            // without one stays project-less (private to its launch) even
            // after its workflow is moved into a project.
            let workflow_run = conn
                .query_row(
                    "SELECT project_id FROM workflow_runs WHERE id = ?1",
                    [id],
                    |row| Ok(single_or_global(row.get::<_, Option<String>>(0)?)),
                )
                .optional()?;
            match workflow_run {
                Some(place) => Ok(Some(place)),
                // A Quick Prompt / API / Exec run is a run too.
                None => project_of("SELECT project_id FROM shared_runs WHERE id = ?1"),
            }
        }
    }
}

/// The resource a request's id is really checked as: a message, file,
/// dispatch, session, workspace, orchestration run or proposal stands for its
/// discussion; a media job for its discussion, else its project; an audit run
/// for its project. `None` when it resolves to nothing.
pub fn canonical(
    conn: &rusqlite::Connection,
    kind: Kind,
    id: &str,
) -> anyhow::Result<Option<(Kind, String)>> {
    use rusqlite::OptionalExtension;
    let discussion_of = |sql: &str| -> anyhow::Result<Option<(Kind, String)>> {
        Ok(conn
            .query_row(sql, [id], |row| row.get::<_, String>(0))
            .optional()?
            .map(|disc| (Kind::Discussion, disc)))
    };
    match kind {
        Kind::Message => discussion_of("SELECT discussion_id FROM messages WHERE id = ?1"),
        Kind::ContextFile => discussion_of("SELECT discussion_id FROM context_files WHERE id = ?1"),
        Kind::Dispatch => {
            discussion_of("SELECT discussion_id FROM agent_dispatch_jobs WHERE id = ?1")
        }
        Kind::Session => match id.parse::<i64>() {
            Ok(pk) => Ok(conn
                .query_row(
                    "SELECT disc_id FROM discussion_sessions WHERE id = ?1",
                    [pk],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|disc| (Kind::Discussion, disc))),
            Err(_) => Ok(None),
        },
        Kind::Workspace => discussion_of("SELECT disc_id FROM discussion_workspaces WHERE id = ?1"),
        Kind::OrchestrationRun => {
            discussion_of("SELECT discussion_id FROM orchestration_runs WHERE id = ?1")
        }
        Kind::Proposal => {
            discussion_of("SELECT discussion_id FROM planning_proposals WHERE id = ?1")
        }
        Kind::MediaJob => {
            let found: Option<(Option<String>, Option<String>)> = conn
                .query_row(
                    "SELECT discussion_id, project_id FROM media_jobs WHERE id = ?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            Ok(match found {
                Some((Some(disc), _)) => Some((Kind::Discussion, disc)),
                Some((None, Some(project))) => Some((Kind::Project, project)),
                _ => None,
            })
        }
        Kind::AuditRun => Ok(conn
            .query_row(
                "SELECT project_id FROM audit_runs WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|project| (Kind::Project, project))),
        // As the page handlers resolve it, so a slug or a renamed page's old
        // slug is checked as the page it opens.
        Kind::Page => {
            Ok(crate::db::live_pages::resolve_live_page_id(conn, id)?
                .map(|page| (Kind::Page, page)))
        }
        _ => Ok(Some((kind, id.to_owned()))),
    }
}

/// The ids of a request as the gate checks them ([`canonical`]); an id that
/// resolves to nothing stays as named and is refused by `authorize`.
pub fn canonical_ids(
    conn: &rusqlite::Connection,
    ids: Vec<NamedId>,
) -> anyhow::Result<Vec<NamedId>> {
    ids.into_iter()
        .map(|named| {
            Ok(match canonical(conn, named.kind, &named.id)? {
                Some((kind, id)) => NamedId::new(kind, id, named.role),
                None => named,
            })
        })
        .collect()
}

/// What a scope is bound to now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeBinding {
    Bound(Option<String>),
    /// Every resource the scope names is gone: the token is dead.
    Dead,
    /// The scope's resources sit in different projects: no single binding.
    Conflict,
}

/// The project a scope is bound to: every existing discussion, execution and
/// run it names must agree. A scope naming none is bound to its declared
/// project (when it still exists), or to none.
/// What the grant is bound to now. Every discussion it owns (its launch's and
/// the ones it created) and its execution and run must still exist and sit in
/// one project: one deleted kills the token, one moved is a conflict.
pub fn resolve_grant_project(
    conn: &rusqlite::Connection,
    grant: &BridgeGrant,
) -> anyhow::Result<ScopeBinding> {
    let adopted = grant
        .adopted
        .lock()
        .map(|adopted| adopted.clone())
        .unwrap_or_default();
    resolve_scope_project(conn, &grant.scope, &adopted)
}

pub fn resolve_scope_project(
    conn: &rusqlite::Connection,
    scope: &BridgeScope,
    adopted: &[String],
) -> anyhow::Result<ScopeBinding> {
    use rusqlite::OptionalExtension;
    let mut seen: Vec<Option<String>> = Vec::new();
    if scope.owns_nothing() {
        match &scope.project_id {
            None => seen.push(None),
            Some(project) => match residence(conn, Kind::Project, project)? {
                Some(_) => seen.push(Some(project.clone())),
                None => return Ok(ScopeBinding::Dead),
            },
        }
    }
    for discussion in scope.discussion_ids.iter().chain(adopted) {
        let found = conn
            .query_row(
                "SELECT project_id FROM discussions WHERE id = ?1",
                [discussion],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        match found {
            Some(project) => seen.push(project),
            None => return Ok(ScopeBinding::Dead),
        }
    }
    for (kind, id) in [
        (Kind::Execution, scope.task_execution_id.as_deref()),
        (Kind::Run, scope.workflow_run_id.as_deref()),
    ] {
        let Some(id) = id else { continue };
        match residence(conn, kind, id)? {
            Some(Residence::Projects(projects)) if projects.len() == 1 => {
                seen.push(projects.into_iter().next());
            }
            Some(_) => seen.push(None),
            None => return Ok(ScopeBinding::Dead),
        }
    }
    let Some(first) = seen.first().cloned() else {
        return Ok(ScopeBinding::Dead);
    };
    if seen.iter().any(|project| *project != first) {
        return Ok(ScopeBinding::Conflict);
    }
    Ok(ScopeBinding::Bound(first))
}

/// Discussions some routes reach through a credential instead of an id (an
/// invite token, a resume credential, a session): resolved read-only before
/// the handler acts, so a token never crosses into another project's room
/// through them. An invite that resolves to nothing here (another instance's
/// room) is refused.
pub fn credential_targets(
    conn: &rusqlite::Connection,
    route: &BridgeRoute,
    query: Option<&str>,
    body: Option<&serde_json::Value>,
) -> anyhow::Result<Vec<NamedId>> {
    use rusqlite::OptionalExtension;
    let field = |name: &str| {
        let from_body = body
            .and_then(|body| body.get(name))
            .and_then(|value| value.as_str())
            .map(str::to_owned);
        from_body.or_else(|| {
            query.and_then(|query| {
                query.split('&').find_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    (percent_decode(key) == name).then(|| percent_decode(value))
                })
            })
        })
    };
    let role = if route.method == "GET" {
        Role::Ref
    } else {
        Role::Target
    };
    // The room a caller-supplied (agent, session) is joined to.
    let joined_room = || -> anyhow::Result<Option<String>> {
        match (field("source_agent"), field("source_session_id")) {
            (Some(agent), Some(session)) => Ok(
                crate::db::discussion_sessions::find_active_session(conn, &agent, &session)?
                    .map(|session| session.disc_id),
            ),
            _ => Ok(None),
        }
    };
    // Every room where a caller-supplied (agent_type, session_id) is active.
    let session_rooms = || -> anyhow::Result<Vec<String>> {
        let (Some(agent), Some(session)) = (field("agent_type"), field("session_id")) else {
            return Ok(Vec::new());
        };
        let mut statement = conn.prepare(
            "SELECT DISTINCT disc_id FROM discussion_sessions \
             WHERE agent_type = ?1 AND session_id = ?2 AND status != 'left'",
        )?;
        let rooms = statement
            .query_map([agent, session], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rooms)
    };
    // The room a caller-supplied (agent, session) is currently bound to.
    let bound_room = || -> anyhow::Result<Option<String>> {
        match (field("source_agent"), field("source_session_id")) {
            (Some(agent), Some(session)) => {
                crate::db::disc_source::find_disc_by_source_session(conn, &agent, &session)
            }
            _ => Ok(None),
        }
    };
    let hash = |raw: &str| hex(&Sha256::digest(raw.as_bytes()));
    let unresolved = || NamedId::new(Kind::Discussion, "<unresolved credential>", Role::Target);
    let mut out = Vec::new();
    match route.pattern {
        "/api/discussions/peer-join" => {
            let found: Option<String> = match field("token") {
                Some(token) => conn
                    .query_row(
                        "SELECT disc_id FROM discussion_invite_tokens WHERE token_hash = ?1",
                        [hash(token.trim())],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => None,
            };
            out.push(found.map_or_else(unresolved, |disc| {
                NamedId::new(Kind::Discussion, disc, Role::Target)
            }));
            // Joining ends the named session in every other room: each of
            // them must be one the token may write.
            out.extend(
                session_rooms()?
                    .into_iter()
                    .map(|disc| NamedId::new(Kind::Discussion, disc, Role::Target)),
            );
        }
        "/api/discussions/peer-resume" | "/api/discussions/orchestrator-return-resume" => {
            let found: Option<String> = match field("resume_token") {
                Some(token) => conn
                    .query_row(
                        "SELECT disc_id FROM discussion_sessions WHERE resume_token_hash = ?1",
                        [hash(token.trim())],
                        |row| row.get(0),
                    )
                    .optional()?,
                None => None,
            };
            out.push(found.map_or_else(unresolved, |disc| {
                NamedId::new(Kind::Discussion, disc, Role::Target)
            }));
        }
        "/api/discussions/peer-leave" => {
            out.extend(
                session_rooms()?
                    .into_iter()
                    .map(|disc| NamedId::new(Kind::Discussion, disc, Role::Target)),
            );
        }
        // Acting on the room of a joined session: that room must be in
        // scope, and a session that resolves to no room is refused.
        "/api/disc/workspace/history-lease" => {
            out.push(joined_room()?.map_or_else(unresolved, |disc| {
                NamedId::new(Kind::Discussion, disc, role)
            }));
        }
        "/api/disc/workspace" => match joined_room()? {
            Some(disc) => out.push(NamedId::new(Kind::Discussion, disc, role)),
            None if route.method != "GET" => out.push(unresolved()),
            None => {}
        },
        // Moving a session's binding: the room it leaves must be writable too.
        "/api/disc/link" | "/api/disc/unlink" | "/api/disc/transfer-session" => {
            if let Some(disc) = bound_room()? {
                out.push(NamedId::new(Kind::Discussion, disc, Role::Target));
            }
        }
        // Reads keyed by a session: the room they would reveal must be visible.
        "/api/disc/find_by_session" | "/api/disc/session-status" => {
            if let Some(disc) = bound_room()? {
                out.push(NamedId::new(Kind::Discussion, disc, Role::Ref));
            }
        }
        _ => {}
    }
    // Any other route naming a joined session acts as that session: its room
    // must be visible to the token. An `Own` route already requires its own
    // room, which is the only one the session acts in there.
    let explicit = SESSION_KEYED_ROUTES.iter().any(|(_, pattern)| {
        *pattern == route.pattern && *pattern != "/api/orchestration/accept-offer"
    });
    if !explicit && route.rule != Rule::Own {
        if let Some(disc) = joined_room()? {
            out.push(NamedId::new(Kind::Discussion, disc, Role::Ref));
        }
        if let (Some(agent), Some(session)) = (field("agent_type"), field("session_id")) {
            if let Some(found) =
                crate::db::discussion_sessions::find_active_session(conn, &agent, &session)?
            {
                out.push(NamedId::new(Kind::Discussion, found.disc_id, Role::Ref));
            }
        }
    }
    Ok(out)
}

/// Routes that act on a discussion reached through a caller-supplied
/// session, invite or resume credential rather than an id.
pub const SESSION_KEYED_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/discussions/peer-join"),
    ("POST", "/api/discussions/peer-leave"),
    ("POST", "/api/discussions/peer-resume"),
    ("POST", "/api/discussions/orchestrator-return-resume"),
    ("POST", "/api/disc/workspace"),
    ("GET", "/api/disc/workspace"),
    ("POST", "/api/disc/workspace/history-lease"),
    ("POST", "/api/disc/link"),
    ("POST", "/api/disc/unlink"),
    ("POST", "/api/disc/transfer-session"),
    ("POST", "/api/orchestration/accept-offer"),
    ("GET", "/api/disc/find_by_session"),
    ("GET", "/api/disc/session-status"),
];

// ─── Response scoping ───────────────────────────────────────────────────────

/// Whether every bridge-token response is scoped after the handler ran: yes,
/// whatever the verb.
pub fn scopes_response(_route: &BridgeRoute) -> bool {
    true
}

/// The kind of the `id` field of a response's root object and list entries,
/// for routes that return resources without naming them under a typed key.
fn returned_kind(pattern: &str) -> Option<Kind> {
    Some(match pattern {
        "/api/discussions" | "/api/discussions/{id}" | "/api/discussions/{id}/meta" => {
            Kind::Discussion
        }
        // A page's feeding workflows may belong to other projects.
        "/api/workflows" | "/api/workflows/{id}" | "/api/pages/{id}/workflows" => Kind::Workflow,
        "/api/quick-prompts" | "/api/quick-prompts/{id}" => Kind::QuickPrompt,
        "/api/quick-apis" | "/api/quick-apis/{id}" => Kind::QuickApi,
        "/api/quick-execs" | "/api/quick-execs/{id}" => Kind::QuickExec,
        "/api/planning/tasks" | "/api/planning/tasks/{id}" => Kind::Task,
        "/api/workflows/{id}/runs" | "/api/workflows/{id}/runs/{run_id}" => Kind::Run,
        "/api/pages" | "/api/pages/{id}" => Kind::Page,
        _ => return None,
    })
}

fn is_project_key(key: &str) -> bool {
    matches!(key, "project_id" | "project_ids" | "target_project_id")
}

/// The scoped resource an id-resolver answer names (`/api/resolve/{id}`).
pub fn resolved_resource(data: &serde_json::Value) -> Option<(Kind, String)> {
    let kind = data.get("kind")?.as_str()?;
    let id = data.get("id")?.as_str()?.to_owned();
    let parent = || {
        data.get("parent")
            .and_then(|parent| parent.get("id"))
            .and_then(|id| id.as_str())
            .map(str::to_owned)
    };
    Some(match kind {
        "discussion" => (Kind::Discussion, id),
        "message" | "planning_proposal" => (Kind::Discussion, parent()?),
        "project" => (Kind::Project, id),
        "workflow" => (Kind::Workflow, id),
        "workflow_run" => (Kind::Run, id),
        "task" => (Kind::Task, id),
        "task_execution" => (Kind::Execution, id),
        "quick_prompt" => (Kind::QuickPrompt, id),
        "quick_api" => (Kind::QuickApi, id),
        "quick_exec" => (Kind::QuickExec, id),
        "page" => (Kind::Page, id),
        "mcp_config" => (Kind::McpConfig, id),
        _ => return None,
    })
}

/// Every (kind, id) a response names that scoping must resolve: known keys at
/// any depth, the typed `id` of the root object and of list entries, and the
/// resource an id-resolver answer names.
pub fn response_ids(route: &BridgeRoute, data: &serde_json::Value) -> Vec<(Kind, String)> {
    fn visit(route: &BridgeRoute, value: &serde_json::Value, out: &mut Vec<(Kind, String)>) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, child) in fields {
                    if is_project_key(key) {
                        continue;
                    }
                    if let Some(kind) = kind_of_key(route, key) {
                        match child {
                            serde_json::Value::String(id) if !id.is_empty() => {
                                out.push((kind, id.clone()))
                            }
                            serde_json::Value::Array(items) => out.extend(
                                items
                                    .iter()
                                    .filter_map(|item| item.as_str())
                                    .filter(|id| !id.is_empty())
                                    .map(|id| (kind, id.to_owned())),
                            ),
                            _ => {}
                        }
                    }
                    visit(route, child, out);
                }
            }
            serde_json::Value::Array(items) => {
                items.iter().for_each(|item| visit(route, item, out))
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    visit(route, data, &mut out);
    if let Some(kind) = returned_kind(route.pattern) {
        for entry in typed_entries(data) {
            if let Some(id) = entry.get("id").and_then(|id| id.as_str()) {
                out.push((kind, id.to_owned()));
            }
        }
    }
    if route.pattern == "/api/resolve/{id}" {
        out.extend(resolved_resource(data));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out.dedup();
    out
}

/// Where a typed `id` names the route's kind: the root object when it has an
/// `id` (the resource itself), else the entries of the root array or of the
/// arrays directly under a root wrapper object.
fn typed_entries(data: &serde_json::Value) -> Vec<&serde_json::Map<String, serde_json::Value>> {
    let mut entries = Vec::new();
    match data {
        serde_json::Value::Object(fields) if fields.contains_key("id") => entries.push(fields),
        serde_json::Value::Object(fields) => {
            for child in fields.values() {
                if let serde_json::Value::Array(items) = child {
                    entries.extend(items.iter().filter_map(|item| item.as_object()));
                }
            }
        }
        serde_json::Value::Array(items) => {
            entries.extend(items.iter().filter_map(|item| item.as_object()))
        }
        _ => {}
    }
    entries
}

/// Resolved residences for [`scope_response`]: `None` = the id resolves to
/// nothing (hidden by default).
pub type Residences = HashMap<(Kind, String), Option<(Kind, String, Residence)>>;

/// Resolve a response's ids for [`scope_response`]: each to the resource it is
/// checked as, with that resource's residence.
pub fn resolve_residences(
    conn: &rusqlite::Connection,
    wanted: Vec<(Kind, String)>,
) -> anyhow::Result<Residences> {
    let mut residences = Residences::new();
    for (kind, id) in wanted {
        let resolved = match canonical(conn, kind, &id)? {
            Some((as_kind, as_id)) => {
                residence(conn, as_kind, &as_id)?.map(|place| (as_kind, as_id, place))
            }
            None => None,
        };
        residences.insert((kind, id), resolved);
    }
    Ok(residences)
}

struct Scoper<'a> {
    route: &'a BridgeRoute,
    grant: &'a BridgeGrant,
    bound: Option<&'a str>,
    residences: &'a Residences,
}

impl Scoper<'_> {
    fn id_visible(&self, kind: Kind, id: &str) -> bool {
        match self.residences.get(&(kind, id.to_owned())) {
            Some(Some((as_kind, as_id, place))) => {
                visible_for_read(self.grant, *as_kind, as_id, place, self.bound)
            }
            // A response may name a message or job since deleted, or a key
            // shared with another table: it hides nothing it cannot place.
            Some(None) if kind.derived() => true,
            _ => owns(self.grant, kind, id),
        }
    }

    /// The object's own fields: its project, its workflow scope, every typed
    /// id it names. Nested objects are judged by [`Self::keep`].
    fn own_fields_visible(
        &self,
        fields: &serde_json::Map<String, serde_json::Value>,
        typed: Option<Kind>,
    ) -> bool {
        let holds = |project: &str| self.bound == Some(project);
        if let Some(scope) = fields.get("project_scope").filter(|scope| !scope.is_null()) {
            let home = fields
                .get("project_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let place = workflow_residence(home, Some(scope.to_string()));
            let visible = match place {
                Residence::Projects(projects) => self.bound.is_some_and(|p| projects.contains(p)),
                Residence::General => self.bound.is_none(),
                Residence::ProjectsAndGeneral(projects) => {
                    self.bound.is_none_or(|p| projects.contains(p))
                }
                Residence::Global | Residence::AllProjects => true,
            };
            if !visible {
                return false;
            }
        } else if let Some(serde_json::Value::String(project)) = fields.get("project_id") {
            let also_listed = fields
                .get("project_ids")
                .and_then(|v| v.as_array())
                .is_some_and(|ids| ids.iter().filter_map(|id| id.as_str()).any(holds));
            if !holds(project) && !also_listed {
                return false;
            }
        }
        if let Some(listed) = fields.get("project_ids").and_then(|v| v.as_array()) {
            let ids: Vec<&str> = listed.iter().filter_map(|id| id.as_str()).collect();
            if !ids.is_empty() && !ids.iter().any(|p| holds(p)) {
                return false;
            }
        }
        if let (Some(kind), Some(id)) = (typed, fields.get("id").and_then(|id| id.as_str())) {
            if !self.id_visible(kind, id) {
                return false;
            }
        }
        for (key, child) in fields {
            if is_project_key(key) {
                continue;
            }
            let Some(kind) = kind_of_key(self.route, key) else {
                continue;
            };
            let named: Vec<&str> = match child {
                serde_json::Value::String(id) if !id.is_empty() => vec![id.as_str()],
                serde_json::Value::Array(items) => {
                    items.iter().filter_map(|i| i.as_str()).collect()
                }
                _ => Vec::new(),
            };
            if named.iter().any(|id| !self.id_visible(kind, id)) {
                return false;
            }
        }
        true
    }

    /// Whether `value` stays, filtering its lists in place. An object hidden
    /// anywhere through its object fields hides its parent; a hidden list
    /// entry is dropped, and the counts beside the list follow.
    fn keep(&self, value: &mut serde_json::Value, typed: Option<Kind>) -> bool {
        match value {
            serde_json::Value::Object(fields) => {
                if !self.own_fields_visible(fields, typed) {
                    return false;
                }
                let mut filtered: Vec<(String, usize)> = Vec::new();
                for (key, child) in fields.iter_mut() {
                    if child.is_array() {
                        if let Some(left) = self.filter_list(child, None) {
                            filtered.push((key.clone(), left));
                        }
                    } else if child.is_object() && !self.keep(child, None) {
                        return false;
                    }
                }
                if !filtered.is_empty() {
                    fix_counts(fields, &filtered);
                }
                true
            }
            serde_json::Value::Array(_) => {
                self.filter_list(value, typed);
                true
            }
            _ => true,
        }
    }

    /// Drop hidden entries; `Some(len)` when any was dropped.
    fn filter_list(&self, value: &mut serde_json::Value, typed: Option<Kind>) -> Option<usize> {
        let serde_json::Value::Array(items) = value else {
            return None;
        };
        let before = items.len();
        items.retain_mut(|item| self.keep(item, typed));
        (items.len() != before).then_some(items.len())
    }
}

/// Responses a token gets whole or not at all.
pub fn scoped_whole_or_refused(route: &BridgeRoute) -> bool {
    route.pattern == "/api/workflows/{id}/export"
}

/// Counts known to pair with a list, besides the `<list>_count`,
/// `<list>_total` and `total_<list>` conventions.
const PAIRED_COUNTS: &[(&str, &str)] = &[
    ("discussions", "disc_count"),
    ("disc_ids", "disc_count"),
    ("items", "total"),
    ("discs", "disc_count"),
];

/// After entries were dropped from a list, the count paired with that list
/// follows its new length; every other field stays as the handler wrote it.
fn fix_counts(
    fields: &mut serde_json::Map<String, serde_json::Value>,
    filtered: &[(String, usize)],
) {
    for (list, left) in filtered {
        let mut paired: Vec<String> = vec![
            format!("{list}_count"),
            format!("{list}_total"),
            format!("total_{list}"),
        ];
        paired.extend(
            PAIRED_COUNTS
                .iter()
                .filter(|(name, _)| name == list)
                .map(|(_, count)| count.to_string()),
        );
        for count in paired {
            if fields.get(&count).is_some_and(|value| value.is_number()) {
                fields.insert(count, serde_json::json!(left));
            }
        }
    }
}

/// Scope a response's `data` to the token, at any depth: an object naming
/// anything outside the scope is dropped from its list, or refused when it is
/// the response itself; an id that resolves to nothing hides its object.
pub fn scope_response(
    route: &BridgeRoute,
    data: &mut serde_json::Value,
    grant: &BridgeGrant,
    bound: Option<&str>,
    residences: &Residences,
) -> Result<(), Refusal> {
    let scoper = Scoper {
        route,
        grant,
        bound,
        residences,
    };
    if route.pattern == "/api/resolve/{id}" {
        if let Some((kind, id)) = resolved_resource(data) {
            if !scoper.id_visible(kind, &id) {
                return Err(Refusal(
                    "this resource is outside the bridge token's project".into(),
                ));
            }
        }
    }
    let typed = returned_kind(route.pattern);
    let hidden = || {
        Err(Refusal(
            "this resource is outside the bridge token's project".into(),
        ))
    };
    match data {
        serde_json::Value::Array(_) => {
            scoper.filter_list(data, typed);
            Ok(())
        }
        serde_json::Value::Object(fields) if fields.contains_key("id") => {
            if scoper.keep(data, typed) {
                Ok(())
            } else {
                hidden()
            }
        }
        serde_json::Value::Object(fields) => {
            // A wrapper: its own fields, then its lists of typed entries.
            if !scoper.own_fields_visible(fields, None) {
                return hidden();
            }
            let mut filtered = Vec::new();
            for (key, child) in fields.iter_mut() {
                if child.is_array() {
                    if let Some(left) = scoper.filter_list(child, typed) {
                        filtered.push((key.clone(), left));
                    }
                } else if child.is_object() && !scoper.keep(child, None) {
                    return hidden();
                }
            }
            if !filtered.is_empty() {
                fix_counts(fields, &filtered);
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "bridge_token_test.rs"]
mod bridge_token_test;
