//! Contributing to a shared repository from a Kronn instance (KT-1643).
//!
//! The "autoCode Issue" workflow must work on any Kronn, not only the
//! maintainer's. Everything it used to hard-code is decided here, from facts
//! the runtime reads:
//!
//! - the Kronn URL comes from the runtime ([`kronn_url`]), not from a literal;
//! - the Kronn project is the one whose remote is the repository
//!   ([`find_project_by_repo`]), not a fixed id;
//! - a contributor without push rights works through a fork
//!   ([`plan_publication`]);
//! - the prerequisites are checked before anything starts, each failure with
//!   the command that fixes it ([`evaluate`]);
//! - who reviews each delivery is named when the campaign is created
//!   ([`resolve_reviewer`]).
//!
//! The functions are pure: the facts (`gh`, the project list, the installed
//! agents) are collected by the caller, so the decisions run in tests on a
//! second, empty instance.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::{AgentType, CampaignReviewer, CampaignReviewerKind, Project};

/// The repository the shipped autoCode Issue workflow contributes to unless
/// its `repo` variable names another.
pub const DEFAULT_REPOSITORY: &str = "DocRoms/Kronn";
/// The review rounds a campaign allows when the workflow sets none.
pub const DEFAULT_MAX_REVIEW_ROUNDS: u32 = 10;
/// The branch a pull request targets when the workflow sets none.
pub const DEFAULT_TARGET_BRANCH: &str = "main";

/// The Kronn instance's own address, as a process it spawns reaches it:
/// `KRONN_BACKEND_URL` when the operator (or Docker) pinned it, else loopback
/// on the port the backend is listening on.
pub fn kronn_url(pinned: Option<&str>, listening_port: u16) -> String {
    match pinned.map(str::trim).filter(|url| !url.is_empty()) {
        Some(url) => url.trim_end_matches('/').to_string(),
        None => format!("http://127.0.0.1:{listening_port}"),
    }
}

/// `owner/name`, lower-cased for a comparison (GitHub slugs are case-blind).
pub fn normalize_slug(slug: &str) -> Option<String> {
    let slug = slug.trim().trim_matches('/');
    let slug = slug.strip_suffix(".git").unwrap_or(slug);
    let (owner, name) = slug.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}").to_ascii_lowercase())
}

/// What resolving a repository to a Kronn project found.
#[derive(Debug, Clone)]
pub enum RepoProject {
    Found(Box<Project>),
    /// Several projects point at the repository: the workflow cannot guess.
    Ambiguous(Vec<String>),
    Missing,
}

/// The project whose remote is `slug`: its `repo_url`, else a remote of its
/// checkout. `remotes_of` reads a checkout's remote URLs (git config), so the
/// function itself touches no disk.
pub fn find_project_by_repo(
    projects: &[Project],
    slug: &str,
    remotes_of: &dyn Fn(&Project) -> Vec<String>,
) -> RepoProject {
    let Some(wanted) = normalize_slug(slug) else {
        return RepoProject::Missing;
    };
    let matches_slug = |url: &str| {
        crate::core::github_connection::github_repo_slug(url)
            .and_then(|found| normalize_slug(&found))
            .is_some_and(|found| found == wanted)
    };
    let mut found: Vec<&Project> = projects
        .iter()
        .filter(|project| {
            project.repo_url.as_deref().is_some_and(matches_slug)
                || remotes_of(project).iter().any(|url| matches_slug(url))
        })
        .collect();
    match found.len() {
        0 => RepoProject::Missing,
        1 => RepoProject::Found(Box::new(found.remove(0).clone())),
        _ => RepoProject::Ambiguous(found.iter().map(|project| project.name.clone()).collect()),
    }
}

// ─── Publication: maintainer or fork ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PublicationMode {
    /// The user may push to the repository: the branch goes there.
    Maintainer,
    /// The user may not: the branch goes to their fork.
    Fork,
}

/// Where the branch is pushed and what the pull request is opened with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublicationPlan {
    pub mode: PublicationMode,
    /// `owner/name` the pull request is opened against.
    pub base_repo: String,
    pub base_branch: String,
    /// `owner/name` the branch is pushed to.
    pub push_repo: String,
    /// `gh pr create --head`: `branch`, or `login:branch` from a fork.
    pub pr_head: String,
    /// The fork has to be created (`gh repo fork`) before the push.
    pub needs_fork: bool,
    /// What publishing does, in order: the workflow's publish step runs
    /// exactly these, so the plan read here is the plan carried out.
    pub steps: Vec<PublicationStep>,
}

/// One action of a publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum PublicationStep {
    /// `gh repo fork <repo>`: the contributor's copy of the repository.
    ForkRepo { repo: String },
    /// Push `branch` to the repository `repo`.
    Push { repo: String, branch: String },
    /// `gh pr create --repo <repo> --base <base> --head <head>`.
    OpenPullRequest {
        repo: String,
        base: String,
        head: String,
    },
}

/// How to publish `branch`: straight to `base_repo` when the user can push
/// there, through `login`'s fork otherwise. `fork_exists` is whether
/// `login/<name>` already exists.
pub fn plan_publication(
    base_repo: &str,
    base_branch: &str,
    login: &str,
    can_push: bool,
    fork_exists: bool,
    branch: &str,
) -> PublicationPlan {
    let pull_request = |head: &str| PublicationStep::OpenPullRequest {
        repo: base_repo.to_string(),
        base: base_branch.to_string(),
        head: head.to_string(),
    };
    if can_push {
        return PublicationPlan {
            mode: PublicationMode::Maintainer,
            base_repo: base_repo.to_string(),
            base_branch: base_branch.to_string(),
            push_repo: base_repo.to_string(),
            pr_head: branch.to_string(),
            needs_fork: false,
            steps: vec![
                PublicationStep::Push {
                    repo: base_repo.to_string(),
                    branch: branch.to_string(),
                },
                pull_request(branch),
            ],
        };
    }
    let name = base_repo.rsplit('/').next().unwrap_or(base_repo);
    let push_repo = format!("{login}/{name}");
    let pr_head = format!("{login}:{branch}");
    let mut steps = Vec::new();
    if !fork_exists {
        steps.push(PublicationStep::ForkRepo {
            repo: base_repo.to_string(),
        });
    }
    steps.push(PublicationStep::Push {
        repo: push_repo.clone(),
        branch: branch.to_string(),
    });
    steps.push(pull_request(&pr_head));
    PublicationPlan {
        mode: PublicationMode::Fork,
        base_repo: base_repo.to_string(),
        base_branch: base_branch.to_string(),
        push_repo,
        pr_head,
        needs_fork: !fork_exists,
        steps,
    }
}

// ─── Prerequisites ──────────────────────────────────────────────────────────

/// What `gh` said about the user's login.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GhAuth {
    pub login: Option<String>,
    /// OAuth scopes of the token; `None` when `gh` does not show them (a
    /// fine-grained or environment token), which is not a failure.
    pub scopes: Option<Vec<String>>,
}

/// The facts the checks read. Collected by the caller.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub gh_installed: bool,
    /// `None`: `gh` is not logged in.
    pub gh_auth: Option<GhAuth>,
    /// Whether the logged-in user can push to the repository; `None` when
    /// `gh` could not answer (no such repository, no network).
    pub can_push: Option<bool>,
    pub fork_exists: bool,
    /// The project resolved from the repository.
    pub project: Option<RepoProjectFact>,
    /// Local workers: installed CLI agents Kronn can launch a task on.
    pub local_workers: Vec<AgentType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoProjectFact {
    Linked {
        id: String,
        name: String,
        path_exists: bool,
    },
    Ambiguous(Vec<String>),
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreflightCheck {
    /// `gh_installed`, `gh_auth`, `gh_scopes`, `repo_access`, `project`, `worker`.
    pub id: String,
    pub ok: bool,
    /// What was found, or what to do about it.
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreflightReport {
    pub ok: bool,
    pub repo: String,
    pub target_branch: String,
    pub max_review_rounds: u32,
    pub checks: Vec<PreflightCheck>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub publication: Option<PublicationPlan>,
    /// The remote model this instance proposes as the campaign's reviewer, when
    /// one is configured; the workflow passes it explicitly. Absent: the
    /// campaign room keeps its own setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub suggested_reviewer: Option<CampaignReviewer>,
    /// The local agents a task can be launched on, for the workflow to pick
    /// its default worker from instead of assuming one is installed.
    pub workers: Vec<AgentType>,
}

fn check(id: &str, ok: bool, message: impl Into<String>) -> PreflightCheck {
    PreflightCheck {
        id: id.to_string(),
        ok,
        message: message.into(),
    }
}

/// The scopes `gh auth status` prints (`- Token scopes: 'gist', 'repo'`).
/// `None` when it prints none (a fine-grained or environment token).
pub fn parse_token_scopes(status: &str) -> Option<Vec<String>> {
    let line = status.lines().find_map(|line| {
        line.trim()
            .trim_start_matches('-')
            .trim()
            .strip_prefix("Token scopes:")
    })?;
    let scopes: Vec<String> = line
        .split(',')
        .map(|scope| scope.trim().trim_matches(['\'', '"']).to_string())
        .filter(|scope| !scope.is_empty())
        .collect();
    (!scopes.is_empty()).then_some(scopes)
}

/// A token able to push a branch and open a pull request.
fn scopes_allow_pull_requests(scopes: &[String]) -> bool {
    scopes
        .iter()
        .any(|scope| matches!(scope.as_str(), "repo" | "public_repo"))
}

/// Every prerequisite, in the order a contributor fixes them, each failure
/// with an actionable message. `ok` is true only when all hold.
pub fn evaluate(
    repo: &str,
    target_branch: &str,
    max_review_rounds: u32,
    branch: &str,
    facts: &Facts,
) -> PreflightReport {
    let mut checks = Vec::new();
    let mut publication = None;

    if facts.gh_installed {
        checks.push(check("gh_installed", true, "GitHub CLI (gh) is installed."));
    } else {
        checks.push(check(
            "gh_installed",
            false,
            "GitHub CLI (gh) is not installed. Install it from https://cli.github.com, \
             then run `gh auth login`.",
        ));
    }

    let login = facts
        .gh_auth
        .as_ref()
        .and_then(|auth| auth.login.clone())
        .filter(|login| !login.is_empty());
    match (&facts.gh_auth, facts.gh_installed) {
        (_, false) => checks.push(check("gh_auth", false, "Install gh first.")),
        (Some(_), true) if login.is_some() => checks.push(check(
            "gh_auth",
            true,
            format!(
                "gh is logged in as {}.",
                login.as_deref().unwrap_or_default()
            ),
        )),
        _ => checks.push(check(
            "gh_auth",
            false,
            "gh is not logged in. Run `gh auth login` and choose GitHub.com.",
        )),
    }

    match facts.gh_auth.as_ref().and_then(|auth| auth.scopes.as_ref()) {
        Some(scopes) if !scopes_allow_pull_requests(scopes) => checks.push(check(
            "gh_scopes",
            false,
            "The gh token lacks the `repo` scope needed to push a branch and open a pull \
             request. Run `gh auth refresh -s repo`.",
        )),
        Some(_) => checks.push(check(
            "gh_scopes",
            true,
            "The gh token has the `repo` scope.",
        )),
        None => checks.push(check(
            "gh_scopes",
            facts.gh_auth.is_some(),
            if facts.gh_auth.is_some() {
                "gh does not list the token scopes (fine-grained or environment token): \
                 they are checked by the push itself."
            } else {
                "Log in with gh first."
            },
        )),
    }

    match (facts.can_push, login.as_deref()) {
        (Some(can_push), Some(login)) => {
            publication = Some(plan_publication(
                repo,
                target_branch,
                login,
                can_push,
                facts.fork_exists,
                branch,
            ));
            checks.push(check(
                "repo_access",
                true,
                if can_push {
                    format!("You can push to {repo}: the branch goes there.")
                } else {
                    format!(
                        "You cannot push to {repo}: the branch goes to your fork {login}/{}, \
                         and the pull request targets {repo}.",
                        repo.rsplit('/').next().unwrap_or(repo)
                    )
                },
            ));
        }
        _ => checks.push(check(
            "repo_access",
            false,
            format!(
                "Cannot read {repo} with gh: check the repository name and that `gh repo \
                 view {repo}` works."
            ),
        )),
    }

    let mut project_id = None;
    match &facts.project {
        Some(RepoProjectFact::Linked {
            id,
            name,
            path_exists: true,
        }) => {
            project_id = Some(id.clone());
            checks.push(check(
                "project",
                true,
                format!("Kronn project `{name}` is the clone of {repo}."),
            ));
        }
        Some(RepoProjectFact::Linked { name, .. }) => checks.push(check(
            "project",
            false,
            format!(
                "Kronn project `{name}` points at {repo} but its folder is missing. Remap it \
                 from the project card, or clone {repo} again."
            ),
        )),
        Some(RepoProjectFact::Ambiguous(names)) => checks.push(check(
            "project",
            false,
            format!(
                "Several Kronn projects point at {repo} ({}). Keep one, or archive the others.",
                names.join(", ")
            ),
        )),
        Some(RepoProjectFact::Missing) | None => checks.push(check(
            "project",
            false,
            format!(
                "No Kronn project is a clone of {repo}. Clone it (`git clone \
                 https://github.com/{repo}`), then add the folder in Projects."
            ),
        )),
    }

    if facts.local_workers.is_empty() {
        checks.push(check(
            "worker",
            false,
            "No local worker is available. Install a coding agent (Claude Code, Codex, …) \
             in Settings > Agents and sign in.",
        ));
    } else {
        let names: Vec<String> = facts
            .local_workers
            .iter()
            .map(|agent| format!("{agent:?}"))
            .collect();
        checks.push(check(
            "worker",
            true,
            format!("Local worker available: {}.", names.join(", ")),
        ));
    }

    PreflightReport {
        ok: checks.iter().all(|c| c.ok),
        repo: repo.to_string(),
        target_branch: target_branch.to_string(),
        max_review_rounds,
        checks,
        project_id,
        publication,
        suggested_reviewer: None,
        workers: facts.local_workers.clone(),
    }
}

// ─── Reviewer ───────────────────────────────────────────────────────────────

/// A provider reached over HTTP: no session of it is ever "joined", so it is
/// reviewed through the room's own native turn.
pub fn is_remote_agent(agent: &AgentType) -> bool {
    matches!(
        agent,
        AgentType::Ollama | AgentType::LiteLlm | AgentType::Nvidia | AgentType::Custom
    )
}

/// The remote models this instance can review with, read from its own
/// settings: LiteLLM once its proxy and key are set, NVIDIA once its key is.
/// Each with the model its reasoning tier (else default tier) names, when set.
pub fn remote_reviewer_candidates(
    config: &crate::models::AppConfig,
) -> Vec<(AgentType, Option<String>)> {
    let tiers = &config.agents.model_tiers;
    let pick = |tier: &crate::models::ModelTierConfig| {
        tier.reasoning.clone().or_else(|| tier.default.clone())
    };
    let mut candidates = Vec::new();
    if config.agents.lite_llm.base_url.is_some() && config.tokens.has_active_key_for("litellm") {
        candidates.push((AgentType::LiteLlm, pick(&tiers.lite_llm)));
    }
    if config.tokens.has_active_key_for("nvidia") {
        candidates.push((AgentType::Nvidia, pick(&tiers.nvidia)));
    }
    candidates
}

/// The reviewer a workflow proposes when the contributor names none: a remote
/// model (`remote_candidates`, the first that is configured), else `None`. A
/// proposal only: a campaign reconfigures its room for a reviewer the caller
/// passes explicitly, never for one it chose on its own.
pub fn suggested_reviewer(
    remote_candidates: &[(AgentType, Option<String>)],
) -> Option<CampaignReviewer> {
    let (agent, model) = remote_candidates.first()?;
    Some(CampaignReviewer {
        kind: CampaignReviewerKind::RemoteModel,
        agent: Some(agent.clone()),
        model: model.clone(),
        connection_id: None,
        defaulted: true,
    })
}

/// Who reviews a campaign created without a named reviewer: the room as it
/// is. The room's own agent and model when one answers in it, the human when
/// the room is `no_agent`. The room is not touched, so a campaign started from
/// a room the owner set up keeps that setup.
pub fn room_reviewer(
    room_agent: &AgentType,
    model: Option<&str>,
    connection_id: Option<&str>,
    no_agent: bool,
) -> CampaignReviewer {
    if no_agent {
        return CampaignReviewer {
            kind: CampaignReviewerKind::Human,
            agent: None,
            model: None,
            connection_id: None,
            defaulted: true,
        };
    }
    CampaignReviewer {
        kind: if is_remote_agent(room_agent) {
            CampaignReviewerKind::RemoteModel
        } else {
            CampaignReviewerKind::Agent
        },
        agent: Some(room_agent.clone()),
        model: model.map(String::from),
        connection_id: connection_id.map(String::from),
        defaulted: true,
    }
}

/// A reviewer the caller named, checked: one that cannot review is refused, so
/// no campaign is created with a delivery nobody serves.
pub fn resolve_reviewer(mut reviewer: CampaignReviewer) -> Result<CampaignReviewer, String> {
    reviewer.defaulted = false;
    let model = reviewer
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty());
    match reviewer.kind {
        CampaignReviewerKind::Human => {
            reviewer.agent = None;
            reviewer.model = None;
            reviewer.connection_id = None;
        }
        CampaignReviewerKind::Agent => {
            let agent = reviewer
                .agent
                .as_ref()
                .ok_or("an agent reviewer needs the agent that reviews")?;
            if is_remote_agent(agent) {
                return Err(
                    "a remote model is a `remote_model` reviewer, not an `agent` reviewer".into(),
                );
            }
            if model.is_none() {
                return Err("an agent reviewer needs the model it reviews with".into());
            }
        }
        CampaignReviewerKind::RemoteModel => {
            let agent = reviewer
                .agent
                .as_ref()
                .ok_or("a remote-model reviewer needs its provider")?;
            if !is_remote_agent(agent) {
                return Err("a `remote_model` reviewer needs an HTTP provider (LiteLLM, NVIDIA, Ollama or a named connection)".into());
            }
            if *agent == AgentType::Custom
                && reviewer
                    .connection_id
                    .as_deref()
                    .is_none_or(|id| id.trim().is_empty())
            {
                return Err("a custom remote model needs its connection".into());
            }
        }
    }
    Ok(reviewer)
}

/// The line the campaign room opens with: the target branch, the review
/// budget and who reviews, so a contributor reads the rules before the first
/// delivery.
pub fn room_announcement(
    target_branch: &str,
    max_review_rounds: &str,
    reviewer: &CampaignReviewer,
) -> String {
    let who = match (reviewer.kind, reviewer.agent.as_ref()) {
        (CampaignReviewerKind::Human, _) => {
            "**you** (no agent answers in this room: approve or request changes from the \
             review card)"
                .to_string()
        }
        (CampaignReviewerKind::Agent, Some(agent)) => format!(
            "the agent {agent:?} with the model `{}`",
            reviewer.model.as_deref().unwrap_or("default")
        ),
        (CampaignReviewerKind::RemoteModel, Some(agent)) => format!(
            "the remote model {agent:?} `{}`",
            reviewer.model.as_deref().unwrap_or("default")
        ),
        _ => "nobody named".to_string(),
    };
    let origin = if reviewer.defaulted {
        " (this room as it is set up; you can still decide from the review card)"
    } else {
        ""
    };
    format!(
        "**Campaign rules** — deliveries integrate into `{target_branch}`, at most \
         {max_review_rounds} review round(s) per task, reviewed by {who}{origin}."
    )
}

#[cfg(test)]
#[path = "contribution_test.rs"]
mod tests;
