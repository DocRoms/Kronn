//! Agent access policy for API plugins (KT-1026).
//!
//! A plugin without a policy behaves as before. With one, the broker calls only
//! declared endpoints (strict mode) and only for the callers the rule admits.
//! The caller's identity comes from what Kronn issued at launch (bridge token,
//! native executor, workflow run), never from the request body.

use crate::models::{
    AgentType, ApiAccessPolicy, ApiAccessRule, ApiAccessSubject, ApiEndpoint, McpServer, ModelTier,
    StepType, WorkflowStep,
};

/// An agent as Kronn launched it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentIdentity {
    pub agent_type: AgentType,
    /// The concrete model, when Kronn resolved one at launch.
    pub model: Option<String>,
}

// `AgentType` has unit variants only, so equality is total.
impl Eq for AgentIdentity {}

/// Who asks the broker for a call.
#[derive(Debug, Clone)]
pub enum ApiCaller {
    /// A person in the UI (Quick API launcher, wizard test).
    Human,
    /// An agent; `identity: None` means no Kronn-issued identity (no token),
    /// which is treated as a remote agent.
    Agent {
        identity: Option<AgentIdentity>,
        discussion_ids: Vec<String>,
        workflow_run_id: Option<String>,
    },
    /// A non-agent step of a human-enabled workflow; its data reaches the
    /// workflow's agents, listed here (`None` = an agent Kronn cannot name).
    Workflow { agents: Vec<Option<AgentIdentity>> },
}

impl ApiCaller {
    /// An agent without any Kronn-issued identity.
    pub fn unidentified_agent() -> Self {
        Self::Agent {
            identity: None,
            discussion_ids: Vec::new(),
            workflow_run_id: None,
        }
    }
}

/// An agent the data may reach, with its locality already decided.
#[derive(Debug, Clone)]
pub struct Member {
    pub identity: Option<AgentIdentity>,
    pub local: bool,
}

/// The caller as the rules see it.
#[derive(Debug, Clone)]
pub enum Principal {
    Human,
    Agent {
        caller: Member,
        /// Every other agent in its discussions or workflow.
        audience: Vec<Member>,
    },
    Workflow {
        audience: Vec<Member>,
    },
}

/// The call being decided: an uppercase method and the path relative to the
/// plugin's base URL, as the request will go out.
#[derive(Debug, Clone)]
pub struct Target {
    pub method: String,
    pub path: String,
}

/// The agent and model a workflow Agent step runs, resolved as the runner
/// does: explicit model, else the step connection's tier model, else the tier.
pub async fn step_identity(
    state: &crate::AppState,
    step: &WorkflowStep,
    model_tiers: Option<&crate::models::ModelTiersConfig>,
) -> AgentIdentity {
    // An unresolvable connection leaves the model unknown, hence not local.
    let connection = crate::workflows::steps::resolve_step_connection(step, Some(&state.db))
        .await
        .ok()
        .flatten();
    let model_override = crate::workflows::steps::step_model_override(step, connection.as_ref());
    AgentIdentity {
        agent_type: step.agent.clone(),
        model: crate::agents::runner::effective_model_flag(
            model_override.as_deref(),
            &step.agent,
            step.agent_settings
                .as_ref()
                .and_then(|s| s.tier)
                .unwrap_or_default(),
            model_tiers,
        ),
    }
}

/// The steps of a workflow that run an agent, main steps and rollback alike
/// (a rollback reads the main steps' results). `None` = an agent Kronn cannot
/// name from the step alone.
pub fn agent_steps(workflow: &crate::models::Workflow) -> Vec<Option<&WorkflowStep>> {
    workflow
        .steps
        .iter()
        .chain(workflow.on_failure.iter())
        .filter_map(audience_slot)
        .collect()
}

/// Whether a step hands data to an agent: `Some(Some(step))` a named Agent
/// step, `Some(None)` one Kronn cannot name, `None` no agent at all.
pub fn audience_slot(step: &WorkflowStep) -> Option<Option<&WorkflowStep>> {
    match step.step_type {
        StepType::Agent => Some(Some(step)),
        // Steps known to run no agent.
        StepType::ApiCall
        | StepType::BatchApiCall
        | StepType::CollectApiData
        | StepType::Notify
        | StepType::Gate
        | StepType::Exec
        | StepType::JsonData
        | StepType::TransformData
        | StepType::PublishPageData
        | StepType::TaskBoard => None,
        // Any other step (sub-workflows, Quick Prompt batches, and every
        // step type added later) may hand the data to an agent Kronn
        // cannot name: unknown, hence remote.
        #[allow(unreachable_patterns)]
        _ => Some(None),
    }
}

/// The agents of a workflow that receive what its steps fetch.
pub async fn workflow_agents(
    state: &crate::AppState,
    workflow: &crate::models::Workflow,
    model_tiers: Option<&crate::models::ModelTiersConfig>,
) -> Vec<Option<AgentIdentity>> {
    let mut out = Vec::new();
    for step in agent_steps(workflow) {
        out.push(match step {
            Some(step) => Some(step_identity(state, step, model_tiers).await),
            None => None,
        });
    }
    out
}

/// The caller for a workflow's own API steps.
pub async fn workflow_caller(
    state: &crate::AppState,
    workflow: &crate::models::Workflow,
) -> ApiCaller {
    let model_tiers = state.config.read().await.agents.model_tiers.clone();
    ApiCaller::Workflow {
        agents: workflow_agents(state, workflow, Some(&model_tiers)).await,
    }
}

/// A model served on this machine: Ollama on a loopback host, never one of its
/// cloud-relayed models. An unknown model is not local.
pub fn is_local(identity: &AgentIdentity, ollama_base_url: &str) -> bool {
    if identity.agent_type != AgentType::Ollama {
        return false;
    }
    let Some(model) = identity.model.as_deref().map(str::trim) else {
        return false;
    };
    let lower = model.to_ascii_lowercase();
    if model.is_empty() || lower.ends_with("-cloud") || lower.contains(":cloud") {
        return false;
    }
    is_loopback_url(ollama_base_url)
}

fn is_loopback_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || host == "host.docker.internal"
        }
    }
}

fn subject_matches(subject: &ApiAccessSubject, identity: &AgentIdentity) -> bool {
    subject.agent == identity.agent_type
        && match subject.model.as_deref().map(str::trim) {
            None | Some("") => true,
            Some(model) => identity
                .model
                .as_deref()
                .is_some_and(|own| own.trim().eq_ignore_ascii_case(model)),
        }
}

fn admitted(subjects: &[ApiAccessSubject], member: &Member) -> bool {
    member
        .identity
        .as_ref()
        .is_some_and(|identity| subjects.iter().any(|s| subject_matches(s, identity)))
}

/// Whether one rule admits the principal; `Err` carries the reason.
pub fn rule_allows(rule: &ApiAccessRule, principal: &Principal) -> Result<(), String> {
    match rule {
        ApiAccessRule::All => Ok(()),
        ApiAccessRule::Blocked => Err("it is blocked for everyone".into()),
        ApiAccessRule::Agents { agents } => match principal {
            Principal::Human => Ok(()),
            Principal::Agent { caller, .. } if admitted(agents, caller) => Ok(()),
            Principal::Agent { caller, .. } => Err(match &caller.identity {
                None => "it is reserved to named agents, and this caller has no Kronn-issued identity".into(),
                Some(_) => format!("it is reserved to {}", describe_subjects(agents)),
            }),
            Principal::Workflow { audience } if audience.iter().all(|m| admitted(agents, m)) => {
                Ok(())
            }
            Principal::Workflow { .. } => Err(format!(
                "it is reserved to {}, and this workflow runs another agent",
                describe_subjects(agents)
            )),
        },
        ApiAccessRule::LocalOnly => match principal {
            Principal::Human => Ok(()),
            Principal::Agent { caller, .. } if !caller.local => {
                Err("it is reserved to local models, and this agent is not one".into())
            }
            Principal::Agent { audience, .. } | Principal::Workflow { audience }
                if audience.iter().any(|m| !m.local) =>
            {
                Err("it is reserved to local models, and a remote agent takes part in this conversation or workflow".into())
            }
            _ => Ok(()),
        },
    }
}

fn describe_subjects(subjects: &[ApiAccessSubject]) -> String {
    if subjects.is_empty() {
        return "no agent".into();
    }
    subjects
        .iter()
        .map(|s| match s.model.as_deref() {
            Some(model) if !model.trim().is_empty() => format!("{:?} ({model})", s.agent),
            _ => format!("{:?}", s.agent),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A path's segments, or `None` when the path is ambiguous: an empty segment,
/// a dot segment, or an encoded slash, backslash or dot that a server could
/// read differently from this matcher.
fn request_segments(path: &str) -> Option<Vec<String>> {
    let trimmed = path.strip_prefix('/').unwrap_or(path);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    let segments = trimmed
        .split('/')
        .map(canonical_segment)
        .collect::<Option<Vec<String>>>()?;
    segments
        .iter()
        .all(|s| !s.is_empty() && s != "." && s != "..")
        .then_some(segments)
}

/// A segment in the one form requests and rules are compared in: an encoded
/// unreserved character (`%6D` for `m`) decoded, every other escape kept with
/// uppercase hex, an ASCII character the URL escapes (space, quote, angle
/// brackets, backtick, braces, controls) escaped, and a non-ASCII character
/// written as its UTF-8 escapes: the form the request URL is sent in. So `/users/%6De` is `/users/me` and `%3a` is `%3A`.
/// `None` for an escape that could change the path's shape (slash, backslash,
/// dot), a raw backslash, or a malformed escape.
fn canonical_segment(segment: &str) -> Option<String> {
    let mut out = String::with_capacity(segment.len());
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => return None,
            '%' => {
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() != 2 || !hex.chars().all(|h| h.is_ascii_hexdigit()) {
                    return None;
                }
                let byte = u8::from_str_radix(&hex, 16).ok()?;
                match byte {
                    b'/' | b'\\' | b'.' => return None,
                    b if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'~' => {
                        out.push(b as char)
                    }
                    _ => out.push_str(&format!("%{byte:02X}")),
                }
            }
            // The URL parser drops TAB, CR and LF and escapes the other
            // controls: a rule holding one would name another destination.
            c if c.is_ascii_control() => return None,
            c if url_path_encodes(c) => out.push_str(&format!("%{:02X}", c as u8)),
            c if c.is_ascii() => out.push(c),
            c => {
                let mut buf = [0u8; 4];
                for byte in c.encode_utf8(&mut buf).bytes() {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    Some(out)
}

/// ASCII characters the client's URL serializer (`url`, WHATWG path set)
/// escapes in a path: a rule must spell them escaped too to be compared.
fn url_path_encodes(c: char) -> bool {
    matches!(c, ' ' | '"' | '#' | '<' | '>' | '?' | '`' | '{' | '}')
}

/// Whether every segment of a rule's path has a canonical form.
fn rule_is_comparable(path: &str) -> bool {
    template_segments(path)
        .into_iter()
        .all(|segment| canonical_template_segment(segment).is_some())
}

/// A declared segment in canonical form: its literal text canonicalised, its
/// placeholder kept as written. `None` = malformed.
fn canonical_template_segment(segment: &str) -> Option<String> {
    match segment_pattern(segment)? {
        SegmentPattern::Literal(literal) => canonical_segment(literal),
        SegmentPattern::Placeholder { prefix, suffix } => {
            let placeholder = &segment[prefix.len()..segment.len() - suffix.len()];
            Some(format!(
                "{}{placeholder}{}",
                canonical_segment(prefix)?,
                canonical_segment(suffix)?
            ))
        }
    }
}

/// A declared path template's segments; a segment holding a placeholder
/// (`{id}`, `{{var}}`, `${ENV.X}`) matches any one non-empty segment.
fn template_segments(template: &str) -> Vec<&str> {
    // ASCII spaces only, as the URL parser trims them; any other character
    // is part of the destination.
    let path = template.split('?').next().unwrap_or("").trim_matches(' ');
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    if path.is_empty() {
        return Vec::new();
    }
    path.split('/').collect()
}

/// One declared segment: a literal, or a prefix and suffix around exactly
/// one placeholder (`{id}`, `{{var}}`, `${ENV.X}`). `None` = malformed.
enum SegmentPattern<'a> {
    Literal(&'a str),
    Placeholder { prefix: &'a str, suffix: &'a str },
}

fn segment_pattern(segment: &str) -> Option<SegmentPattern<'_>> {
    let Some(open) = segment
        .find(['{', '$'])
        .filter(|&i| segment[i..].starts_with('{') || segment[i..].starts_with("${"))
    else {
        return (!segment.contains(['{', '}'])).then_some(SegmentPattern::Literal(segment));
    };
    let rest = &segment[open..];
    let (inner_start, close) = if let Some(body) = rest.strip_prefix("${") {
        (2, body.find('}').map(|i| i + 2))
    } else if let Some(body) = rest.strip_prefix("{{") {
        (2, body.find("}}").map(|i| i + 2))
    } else {
        (1, rest[1..].find('}').map(|i| i + 1))
    };
    let close = close?;
    let name = &rest[inner_start..close];
    let width = if rest.starts_with("{{") { 2 } else { 1 };
    let prefix = &segment[..open];
    let suffix = &rest[close + width..];
    let clean = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    (clean && !prefix.contains(['{', '}']) && !suffix.contains(['{', '}', '$']))
        .then_some(SegmentPattern::Placeholder { prefix, suffix })
}

fn segment_matches(want: &str, got: &str) -> bool {
    // `got` is already canonical; the declaration is brought to the same form.
    match segment_pattern(want) {
        Some(SegmentPattern::Literal(literal)) => {
            canonical_segment(literal).is_some_and(|literal| literal == got)
        }
        Some(SegmentPattern::Placeholder { prefix, suffix }) => {
            match (canonical_segment(prefix), canonical_segment(suffix)) {
                (Some(prefix), Some(suffix)) => {
                    got.len() > prefix.len() + suffix.len()
                        && got.starts_with(&prefix)
                        && got.ends_with(&suffix)
                }
                _ => false,
            }
        }
        // A malformed declaration never matches: it cannot open a path.
        None => false,
    }
}

fn template_matches<S: AsRef<str>>(template: &str, segments: &[S]) -> bool {
    let declared = template_segments(template);
    declared.len() == segments.len()
        && declared
            .iter()
            .zip(segments)
            .all(|(want, got)| segment_matches(want, got.as_ref()))
}

/// The endpoint form a policy stores: uppercase method, one leading slash,
/// no query, no trailing slash. `Err` names what is wrong.
pub fn normalize_endpoint(method: &str, path: &str) -> Result<(String, String), String> {
    let method = method.trim().to_ascii_uppercase();
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err(format!("unsupported method `{method}`"));
    }
    let path = path.trim_matches(' ');
    if path.is_empty() || path.contains('?') || path.contains('#') || path.contains("://") {
        return Err(format!(
            "`{path}` is not an endpoint path (no query, fragment or host)"
        ));
    }
    let segments = template_segments(path);
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err(format!("`{path}` has an empty or dot segment"));
    }
    let mut canonical = Vec::with_capacity(segments.len());
    for segment in &segments {
        let Some(form) = canonical_template_segment(segment) else {
            return Err(format!(
                "`{segment}` in `{path}` is not a literal or one placeholder with literal text around it, or holds an encoded slash, backslash or dot"
            ));
        };
        if form == "." || form == ".." {
            return Err(format!("`{path}` has an empty or dot segment"));
        }
        canonical.push(form);
    }
    Ok((method, format!("/{}", canonical.join("/"))))
}

/// Normalise and check a policy before storing it.
pub fn normalize_policy(mut policy: ApiAccessPolicy) -> Result<ApiAccessPolicy, String> {
    let mut seen = std::collections::HashSet::new();
    for endpoint in &mut policy.endpoints {
        let (method, path) = normalize_endpoint(&endpoint.method, &endpoint.path)?;
        if !seen.insert((method.clone(), path.clone())) {
            return Err(format!("`{method} {path}` has two rules"));
        }
        endpoint.method = method;
        endpoint.path = path;
    }
    Ok(policy)
}

/// Decide one call under a plugin's policy. `requested` is the path as the
/// caller wrote it, the only one a refusal may quote (the resolved path can
/// carry configuration values).
pub fn decide(
    plugin_id: &str,
    policy: &ApiAccessPolicy,
    declared: &[ApiEndpoint],
    target: &Target,
    requested: &str,
    principal: &Principal,
) -> Result<(), String> {
    let refuse = |why: String| {
        Err(format!(
            "Access policy: `{} {requested}` on plugin `{plugin_id}` is refused: {why}.",
            target.method
        ))
    };
    // A stored rule Kronn cannot compare (a control character, a malformed
    // escape) could hide a block: the whole plugin fails closed until it is
    // fixed in the plugin form.
    if let Some(bad) = policy
        .endpoints
        .iter()
        .find(|e| !rule_is_comparable(&e.path))
    {
        return refuse(format!(
            "its access policy holds a rule Kronn cannot compare ({} {:?}); fix it in the plugin form",
            bad.method, bad.path
        ));
    }
    let Some(segments) = request_segments(&target.path) else {
        return refuse("the path is ambiguous (empty, dot or encoded separator segment)".into());
    };
    let method_is = |method: &str| method.trim().eq_ignore_ascii_case(&target.method);
    let declared_match = declared
        .iter()
        .any(|e| method_is(&e.method) && template_matches(&e.path, &segments))
        || policy
            .endpoints
            .iter()
            .any(|e| method_is(&e.method) && template_matches(&e.path, &segments));
    if !declared_match {
        return refuse(
            "it is not a declared endpoint, and this plugin's access policy allows declared endpoints only".into(),
        );
    }
    let overrides: Vec<&ApiAccessRule> = policy
        .endpoints
        .iter()
        .filter(|e| method_is(&e.method) && template_matches(&e.path, &segments))
        .map(|e| &e.access)
        .collect();
    let rules: Vec<&ApiAccessRule> = if overrides.is_empty() {
        vec![&policy.access]
    } else {
        overrides
    };
    for rule in rules {
        if let Err(why) = rule_allows(rule, principal) {
            return refuse(why);
        }
    }
    Ok(())
}

/// Resolve the caller into a principal: its own locality and every agent its
/// data may reach (its discussions' agents, its workflow's agent steps).
pub async fn principal_for(
    state: &crate::AppState,
    caller: &ApiCaller,
) -> Result<Principal, String> {
    let base_url = crate::api::ollama::resolve_base_url_pub(None);
    let member = |identity: Option<AgentIdentity>| Member {
        local: identity.as_ref().is_some_and(|i| is_local(i, &base_url)),
        identity,
    };
    match caller {
        ApiCaller::Human => Ok(Principal::Human),
        ApiCaller::Workflow { agents } => Ok(Principal::Workflow {
            audience: agents.iter().cloned().map(member).collect(),
        }),
        ApiCaller::Agent {
            identity,
            discussion_ids,
            workflow_run_id,
        } => {
            let model_tiers = state.config.read().await.agents.model_tiers.clone();
            let discussion_ids = discussion_ids.clone();
            let run_id = workflow_run_id.clone();
            let tiers = model_tiers.clone();
            let (mut audience, workflow) = state
                .db
                .with_read_conn(move |conn| {
                    let mut audience: Vec<Option<AgentIdentity>> = Vec::new();
                    for id in &discussion_ids {
                        let Some(disc) = crate::db::discussions::get_discussion(conn, id)? else {
                            continue;
                        };
                        audience.extend(discussion_agents(&disc, Some(&tiers)));
                        // A joined CLI reads the room; its identity is only
                        // declared, so it counts as unknown.
                        let joined =
                            crate::db::discussion_sessions::list_sessions(conn, id, false)?;
                        audience.extend(joined.iter().map(|_| None));
                    }
                    // The definition the run executes is its pin, never the
                    // editable one; a run without a readable pin is unknown.
                    let workflow = match run_id {
                        Some(run_id) => {
                            Some(crate::workflows::run_pins::pinned_workflow(conn, &run_id)?)
                        }
                        None => None,
                    };
                    Ok((audience, workflow))
                })
                .await
                .map_err(|e| format!("Access policy: could not read the caller's context: {e}"))?;
            match workflow {
                Some(Some(workflow)) => {
                    audience.extend(workflow_agents(state, &workflow, Some(&model_tiers)).await)
                }
                // A run Kronn cannot read back is an unknown audience.
                Some(None) => audience.push(None),
                None => {}
            }
            Ok(Principal::Agent {
                caller: member(identity.clone()),
                audience: audience.into_iter().map(member).collect(),
            })
        }
    }
}

/// The agents of a discussion: its own agent with its model, and each
/// participant at the discussion's tier.
pub fn discussion_agents(
    disc: &crate::models::Discussion,
    model_tiers: Option<&crate::models::ModelTiersConfig>,
) -> Vec<Option<AgentIdentity>> {
    let resolve = |agent: &AgentType, model: Option<&str>, tier: ModelTier| {
        Some(AgentIdentity {
            agent_type: agent.clone(),
            model: crate::agents::runner::effective_model_flag(model, agent, tier, model_tiers),
        })
    };
    let mut out = vec![resolve(&disc.agent, disc.model.as_deref(), disc.tier)];
    for participant in &disc.participants {
        if *participant != disc.agent {
            out.push(resolve(participant, None, disc.tier));
        }
    }
    out
}

/// A plugin's policy bound to one call's caller, re-checked on every request
/// the call sends: each page and each redirect hop, not only the entry URL.
#[derive(Debug, Clone)]
pub struct EndpointGate {
    plugin_id: String,
    policy: ApiAccessPolicy,
    declared: Vec<ApiEndpoint>,
    base_path: String,
    principal: Principal,
}

impl EndpointGate {
    /// Decide one outbound request by its method and absolute URL path.
    pub fn check(&self, method: &str, url_path: &str) -> Result<(), String> {
        let label = "(a redirect or next page)";
        let path = match url_path.strip_prefix(self.base_path.as_str()) {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => rest.to_string(),
            _ => {
                return Err(format!(
                    "Access policy: {label} on plugin `{}` leaves the base URL, refused.",
                    self.plugin_id
                ))
            }
        };
        let target = Target {
            method: method.to_ascii_uppercase(),
            path,
        };
        decide(
            &self.plugin_id,
            &self.policy,
            &self.declared,
            &target,
            label,
            &self.principal,
        )
    }
}

/// Enforce the plugin's policy for one call, before any outbound request.
/// `Ok(Some(gate))` must then check every request the call sends.
pub async fn enforce(
    state: &crate::AppState,
    plugin: &McpServer,
    step: &WorkflowStep,
    env: &std::collections::HashMap<String, String>,
    ctx: &crate::workflows::template::TemplateContext,
    caller: &ApiCaller,
) -> Result<Option<EndpointGate>, String> {
    let server_id = plugin.id.clone();
    let policy = state
        .db
        .with_read_conn(move |conn| crate::db::api_access_policies::get(conn, &server_id))
        .await
        .map_err(|e| format!("Access policy: could not be read, call refused: {e}"))?;
    let Some(policy) = policy else {
        return Ok(None);
    };
    let Some(spec) = plugin.api_spec.as_ref() else {
        return Err("Plugin has no `api_spec` — not an API plugin".into());
    };
    let target = crate::workflows::api_call_executor::request_target(step, spec, env, ctx)?;
    let principal = principal_for(state, caller).await?;
    let requested = step.api_endpoint_path.as_deref().unwrap_or("");
    decide(
        &plugin.id,
        &policy,
        &spec.endpoints,
        &target,
        requested,
        &principal,
    )?;
    Ok(Some(EndpointGate {
        plugin_id: plugin.id.clone(),
        policy,
        declared: spec.endpoints.clone(),
        base_path: crate::workflows::api_call_executor::base_path(spec, env)?,
        principal,
    }))
}

/// One line telling an agent what a rule allows, for prompts and tool output.
pub fn describe_rule(rule: &ApiAccessRule) -> String {
    match rule {
        ApiAccessRule::All => "all agents".into(),
        ApiAccessRule::Agents { agents } => format!("only {}", describe_subjects(agents)),
        ApiAccessRule::LocalOnly => "local models only".into(),
        ApiAccessRule::Blocked => "blocked".into(),
    }
}

/// The rule that governs one declared endpoint under a policy.
pub fn endpoint_rule<'a>(
    policy: &'a ApiAccessPolicy,
    method: &str,
    path: &str,
) -> &'a ApiAccessRule {
    let canonical = |path: &str| {
        template_segments(path)
            .into_iter()
            .map(canonical_template_segment)
            .collect::<Vec<_>>()
    };
    let segments = canonical(path);
    policy
        .endpoints
        .iter()
        .find(|e| e.method.eq_ignore_ascii_case(method.trim()) && canonical(&e.path) == segments)
        .map(|e| &e.access)
        .unwrap_or(&policy.access)
}

/// Prompt lines describing a plugin's policy, for the API context block.
pub fn describe_policy(policy: &ApiAccessPolicy) -> String {
    let mut out = format!(
        "Access policy: {}; strict, only the endpoints listed here can be called.\n",
        describe_rule(&policy.access)
    );
    for endpoint in &policy.endpoints {
        out.push_str(&format!(
            "  - {} {}: {}\n",
            endpoint.method,
            endpoint.path,
            describe_rule(&endpoint.access)
        ));
    }
    out
}

#[cfg(test)]
#[path = "api_access_test.rs"]
mod tests;
