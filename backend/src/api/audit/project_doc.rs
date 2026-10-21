// `docs/project.md`: tracker, forge, environments, people and process,
// rendered deterministically from the briefing's closed answers and checked
// read-only against the tracking connections configured in Kronn (KT-1173).

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;

use crate::models::{
    BriefingCommitConvention, BriefingDefaultBranch, BriefingEnvironmentKind, BriefingFact,
    BriefingForge, BriefingMergeStrategy, BriefingPersonRole, BriefingProjectAnswers,
    BriefingReleaseCadence, BriefingReviewPolicy, BriefingTracker, BriefingYesNo,
};

/// Heading of the answers section inside `docs/briefing.md`.
pub(crate) const FACTS_HEADING: &str = "## Project facts";
/// Machine-readable copy of the answers, one line, invisible once rendered.
const ANSWERS_PREFIX: &str = "<!-- kronn:project-answers v1 ";
const ANSWERS_SUFFIX: &str = " -->";
/// First line of a `docs/project.md` Kronn may regenerate. Removing it hands
/// the file over to humans: the audit then never rewrites it.
pub(crate) const GENERATED_MARKER: &str = "<!-- kronn:generated=\"briefing\"";

const NOT_PROVIDED: &str = "Not provided — ask before assuming.";
/// Longest note attached to one project fact (KT-1196).
pub(crate) const FACT_NOTE_MAX_CHARS: usize = 300;
const JIRA_PLUGIN: &str = "mcp-atlassian";
const GITHUB_PLUGIN: &str = "mcp-github";

// ─── Validation ─────────────────────────────────────────────────────────────

pub(crate) fn check_text(field: &str, value: &str, max: usize) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{field}: at most {max} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{field}: a single line, no control characters"));
    }
    if value.contains("{{") || value.contains("}}") || value.contains("<!--") {
        return Err(format!(
            "{field}: `{{{{`, `}}}}` and `<!--` are not allowed"
        ));
    }
    Ok(())
}

fn is_jira_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_uppercase())
        && (2..=10).contains(&key.len())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn is_repo_path(repo: &str, forge: BriefingForge) -> bool {
    let segments: Vec<&str> = repo.split('/').collect();
    let count_ok = if forge == BriefingForge::Github {
        segments.len() == 2
    } else {
        (2..=5).contains(&segments.len())
    };
    count_ok
        && repo.len() <= 140
        && segments.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

fn is_http_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && url.len() <= 200
        && !url
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '|' | '<' | '>' | '"' | '`'))
}

/// Trim, normalise and validate the answers; environments and people keep the
/// first entry per kind/role.
pub(crate) fn normalize_answers(
    answers: &BriefingProjectAnswers,
) -> Result<BriefingProjectAnswers, String> {
    let mut out = answers.clone();
    out.tracker_key = out.tracker_key.trim().to_string();
    if out.tracker == BriefingTracker::Jira {
        out.tracker_key = out.tracker_key.to_ascii_uppercase();
        if !out.tracker_key.is_empty() && !is_jira_key(&out.tracker_key) {
            return Err(format!(
                "tracker_key: `{}` is not a Jira project key (2 to 10 characters, A-Z, 0-9, _; starts with a letter)",
                out.tracker_key
            ));
        }
    }
    check_text("tracker_key", &out.tracker_key, 64)?;

    out.forge_repo = out
        .forge_repo
        .trim()
        .trim_end_matches(".git")
        .trim_matches('/')
        .to_string();
    if !out.forge_repo.is_empty() && !is_repo_path(&out.forge_repo, out.forge) {
        return Err(format!(
            "forge_repo: `{}` is not an `owner/name` repository path",
            out.forge_repo
        ));
    }

    let mut seen_env = Vec::new();
    out.environments.retain(|e| {
        let fresh = !seen_env.contains(&e.kind);
        seen_env.push(e.kind);
        fresh
    });
    for env in &mut out.environments {
        env.url = env.url.trim().to_string();
        if !env.url.is_empty() && !is_http_url(&env.url) {
            return Err(format!(
                "environments.url: `{}` must be an http(s) URL",
                env.url
            ));
        }
    }

    let mut seen_role = Vec::new();
    out.people.retain(|p| {
        let fresh = !seen_role.contains(&p.role);
        seen_role.push(p.role);
        fresh
    });
    for person in &mut out.people {
        person.name = person.name.trim().to_string();
        person.contact = person.contact.trim().to_string();
        check_text("people.name", &person.name, 80)?;
        check_text("people.contact", &person.contact, 120)?;
    }
    out.people
        .retain(|p| !p.name.is_empty() || !p.contact.is_empty());

    let mut notes = std::collections::BTreeMap::new();
    for (fact, note) in &out.notes {
        let note = note.trim();
        if note.is_empty() {
            continue;
        }
        check_text(
            &format!("notes.{}", fact_key(*fact)),
            note,
            FACT_NOTE_MAX_CHARS,
        )?;
        notes.insert(*fact, note.to_string());
    }
    out.notes = notes;
    Ok(out)
}

// ─── Labels ─────────────────────────────────────────────────────────────────

fn fact_key(v: BriefingFact) -> &'static str {
    match v {
        BriefingFact::Tracker => "tracker",
        BriefingFact::TrackerKey => "tracker_key",
        BriefingFact::Forge => "forge",
        BriefingFact::ForgeRepo => "forge_repo",
        BriefingFact::DefaultBranch => "default_branch",
        BriefingFact::CommitConvention => "commit_convention",
        BriefingFact::Signoff => "signoff",
        BriefingFact::MergeStrategy => "merge_strategy",
        BriefingFact::Review => "review",
        BriefingFact::ReleaseCadence => "release_cadence",
        BriefingFact::Environments => "environments",
        BriefingFact::People => "people",
    }
}

fn fact_label(v: BriefingFact) -> &'static str {
    match v {
        BriefingFact::Tracker => "Tracker",
        BriefingFact::TrackerKey => "Project key",
        BriefingFact::Forge => "Forge",
        BriefingFact::ForgeRepo => "Repository",
        BriefingFact::DefaultBranch => "Default branch",
        BriefingFact::CommitConvention => "Commit convention",
        BriefingFact::Signoff => "Sign-off",
        BriefingFact::MergeStrategy => "Merge strategy",
        BriefingFact::Review => "Review",
        BriefingFact::ReleaseCadence => "Release cadence",
        BriefingFact::Environments => "Environments",
        BriefingFact::People => "People",
    }
}

/// `value` followed by the fact's note, when the team wrote one.
fn with_note(value: String, answers: &BriefingProjectAnswers, fact: BriefingFact) -> String {
    match answers.notes.get(&fact) {
        Some(note) => format!("{value} — note: {}", cell(note)),
        None => value,
    }
}

fn tracker_label(v: BriefingTracker) -> &'static str {
    match v {
        BriefingTracker::Unknown => NOT_PROVIDED,
        BriefingTracker::Jira => "Jira",
        BriefingTracker::GithubIssues => "GitHub Issues",
        BriefingTracker::GitlabIssues => "GitLab Issues",
        BriefingTracker::Linear => "Linear",
        BriefingTracker::Other => "Other (ask the team)",
        BriefingTracker::None => "No tracker",
    }
}

fn forge_label(v: BriefingForge) -> &'static str {
    match v {
        BriefingForge::Unknown => NOT_PROVIDED,
        BriefingForge::Github => "GitHub",
        BriefingForge::Gitlab => "GitLab",
        BriefingForge::Bitbucket => "Bitbucket",
        BriefingForge::AzureDevops => "Azure DevOps",
        BriefingForge::Other => "Other (ask the team)",
    }
}

fn branch_label(v: BriefingDefaultBranch) -> Option<&'static str> {
    match v {
        BriefingDefaultBranch::Unknown => None,
        BriefingDefaultBranch::Main => Some("main"),
        BriefingDefaultBranch::Master => Some("master"),
        BriefingDefaultBranch::Develop => Some("develop"),
        BriefingDefaultBranch::Trunk => Some("trunk"),
    }
}

fn commit_label(v: BriefingCommitConvention) -> &'static str {
    match v {
        BriefingCommitConvention::Unknown => NOT_PROVIDED,
        BriefingCommitConvention::Conventional => "Conventional Commits",
        BriefingCommitConvention::TicketPrefixed => "Ticket ID prefix",
        BriefingCommitConvention::Free => "Free-form",
    }
}

fn yes_no_label(v: BriefingYesNo) -> &'static str {
    match v {
        BriefingYesNo::Unknown => NOT_PROVIDED,
        BriefingYesNo::Yes => "Required",
        BriefingYesNo::No => "Not required",
    }
}

fn merge_label(v: BriefingMergeStrategy) -> &'static str {
    match v {
        BriefingMergeStrategy::Unknown => NOT_PROVIDED,
        BriefingMergeStrategy::Squash => "Squash",
        BriefingMergeStrategy::Rebase => "Rebase",
        BriefingMergeStrategy::MergeCommit => "Merge commit",
    }
}

fn review_label(v: BriefingReviewPolicy) -> &'static str {
    match v {
        BriefingReviewPolicy::Unknown => NOT_PROVIDED,
        BriefingReviewPolicy::NoReview => "No required review",
        BriefingReviewPolicy::OneApproval => "One approval",
        BriefingReviewPolicy::TwoApprovals => "Two approvals",
    }
}

fn cadence_label(v: BriefingReleaseCadence) -> &'static str {
    match v {
        BriefingReleaseCadence::Unknown => NOT_PROVIDED,
        BriefingReleaseCadence::Continuous => "Continuous (every merge)",
        BriefingReleaseCadence::Scheduled => "Scheduled",
        BriefingReleaseCadence::OnDemand => "On demand",
    }
}

fn env_label(v: BriefingEnvironmentKind) -> &'static str {
    match v {
        BriefingEnvironmentKind::Local => "Local",
        BriefingEnvironmentKind::Dev => "Dev",
        BriefingEnvironmentKind::Staging => "Staging",
        BriefingEnvironmentKind::Preprod => "Pre-production",
        BriefingEnvironmentKind::Production => "Production",
        BriefingEnvironmentKind::Preview => "Preview",
    }
}

fn role_label(v: BriefingPersonRole) -> &'static str {
    match v {
        BriefingPersonRole::Arbiter => "Arbiter (final call)",
        BriefingPersonRole::TechLead => "Tech lead",
        BriefingPersonRole::Devops => "DevOps lead",
        BriefingPersonRole::ProductOwner => "Product owner",
        BriefingPersonRole::Qa => "QA",
    }
}

/// Markdown-table-safe rendering of a user-entered value.
pub(crate) fn cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn or_not_provided(value: &str) -> String {
    if value.is_empty() {
        NOT_PROVIDED.to_string()
    } else {
        cell(value)
    }
}

fn code_or_not_provided(value: &str) -> String {
    if value.is_empty() {
        NOT_PROVIDED.to_string()
    } else {
        format!("`{value}`")
    }
}

// ─── Briefing section (docs/briefing.md) ───────────────────────────────────

/// The `## Project facts` section appended to `docs/briefing.md`, or `None`
/// when every card was left unanswered (the briefing then stays byte-identical
/// to the six-question format).
pub(crate) fn render_facts_section(answers: &BriefingProjectAnswers) -> Option<String> {
    if *answers == BriefingProjectAnswers::default() {
        return None;
    }
    let lower = |s: &str| {
        if s == NOT_PROVIDED {
            "not provided".to_string()
        } else {
            s.to_string()
        }
    };
    let mut out = format!("{FACTS_HEADING}\n");
    let key = if answers.tracker_key.is_empty() {
        String::new()
    } else {
        format!(" — key `{}`", answers.tracker_key)
    };
    let notes_of = |facts: &[BriefingFact]| -> String {
        facts
            .iter()
            .filter_map(|f| {
                let note = answers.notes.get(f)?;
                Some(format!("  - {} note: {}\n", fact_label(*f), cell(note)))
            })
            .collect()
    };
    out.push_str(&format!(
        "- Tracker: {}{key}\n",
        lower(tracker_label(answers.tracker))
    ));
    out.push_str(&notes_of(&[
        BriefingFact::Tracker,
        BriefingFact::TrackerKey,
    ]));
    let repo = if answers.forge_repo.is_empty() {
        String::new()
    } else {
        format!(" — `{}`", answers.forge_repo)
    };
    let branch = branch_label(answers.default_branch)
        .map(|b| format!(", default branch `{b}`"))
        .unwrap_or_default();
    out.push_str(&format!(
        "- Forge: {}{repo}{branch}\n",
        lower(forge_label(answers.forge))
    ));
    out.push_str(&notes_of(&[
        BriefingFact::Forge,
        BriefingFact::ForgeRepo,
        BriefingFact::DefaultBranch,
    ]));
    out.push_str(&format!(
        "- Commits: {}; sign-off: {}; merge: {}\n",
        lower(commit_label(answers.commit_convention)),
        lower(yes_no_label(answers.signoff_required)),
        lower(merge_label(answers.merge_strategy)),
    ));
    out.push_str(&notes_of(&[
        BriefingFact::CommitConvention,
        BriefingFact::Signoff,
        BriefingFact::MergeStrategy,
    ]));
    out.push_str(&format!(
        "- Review: {}; releases: {}\n",
        lower(review_label(answers.review_policy)),
        lower(cadence_label(answers.release_cadence)),
    ));
    out.push_str(&notes_of(&[
        BriefingFact::Review,
        BriefingFact::ReleaseCadence,
    ]));
    let env_note = notes_of(&[BriefingFact::Environments]);
    if answers.environments.is_empty() && !env_note.is_empty() {
        out.push_str("- Environments: not provided\n");
    }
    if !answers.environments.is_empty() {
        let envs: Vec<String> = answers
            .environments
            .iter()
            .map(|e| {
                if e.url.is_empty() {
                    env_label(e.kind).to_string()
                } else {
                    format!("{} ({})", env_label(e.kind), e.url)
                }
            })
            .collect();
        out.push_str(&format!("- Environments: {}\n", envs.join(", ")));
    }
    out.push_str(&env_note);
    let people_note = notes_of(&[BriefingFact::People]);
    if answers.people.is_empty() && !people_note.is_empty() {
        out.push_str("- People: not provided\n");
    }
    if !answers.people.is_empty() {
        let people: Vec<String> = answers
            .people
            .iter()
            .map(|p| {
                let who = [p.name.as_str(), p.contact.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}: {}", role_label(p.role), who)
            })
            .collect();
        out.push_str(&format!("- People: {}\n", people.join("; ")));
    }
    out.push_str(&people_note);
    // `<` and `>` are escaped so a value can never close the HTML comment.
    let json = serde_json::to_string(answers)
        .unwrap_or_else(|_| "{}".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    out.push_str(&format!("{ANSWERS_PREFIX}{json}{ANSWERS_SUFFIX}\n"));
    Some(out)
}

/// The answers recorded in a briefing, if any.
pub(crate) fn parse_answers(notes: &str) -> Option<BriefingProjectAnswers> {
    notes.lines().find_map(|line| {
        let json = line
            .trim()
            .strip_prefix(ANSWERS_PREFIX)?
            .strip_suffix(ANSWERS_SUFFIX)?;
        serde_json::from_str(json).ok()
    })
}

/// `notes` without the sections under `headings` (each from its heading to the
/// next `## ` heading or the end) and without any line starting with one of
/// `machine_prefixes`.
pub(crate) fn strip_sections(notes: &str, headings: &[&str], machine_prefixes: &[&str]) -> String {
    let mut out = String::with_capacity(notes.len());
    let mut skipping = false;
    for line in notes.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if headings.contains(&trimmed) {
            skipping = true;
            continue;
        }
        if skipping && trimmed.starts_with("## ") {
            skipping = false;
        }
        let machine = machine_prefixes
            .iter()
            .any(|p| trimmed.trim_start().starts_with(p));
        if skipping || machine {
            continue;
        }
        out.push_str(line);
    }
    out
}

/// `notes` without the form-owned sections (`## Project facts`,
/// `## Existing documentation`, `## Project rules`) and without any stray
/// machine copy.
pub(crate) fn strip_facts_section(notes: &str) -> String {
    strip_sections(
        notes,
        &[
            FACTS_HEADING,
            super::briefing_docs::DOCS_HEADING,
            super::briefing_rules::RULES_HEADING,
        ],
        &[
            ANSWERS_PREFIX,
            super::briefing_docs::DOCS_ANSWERS_PREFIX,
            super::briefing_rules::RULES_ANSWERS_PREFIX,
        ],
    )
}

/// The briefing as the audit prompts see it: the human-readable facts stay,
/// the machine copies (which repeat them) go.
pub(crate) fn strip_answers_line(notes: &str) -> String {
    strip_sections(
        notes,
        &[],
        &[
            ANSWERS_PREFIX,
            super::briefing_docs::DOCS_ANSWERS_PREFIX,
            super::briefing_rules::RULES_ANSWERS_PREFIX,
        ],
    )
}

/// After an AI review rewrote `docs/briefing.md`, put the form's sections back
/// exactly as answered: the reviewer may drop or garble them, and the answers
/// belong to the form, not to the model. They go before `## Kronn options`
/// when the review kept it, so that section stays last.
pub(crate) fn reattach_facts(previous: Option<&str>, reviewed: &str) -> String {
    let facts = previous
        .and_then(parse_answers)
        .and_then(|a| render_facts_section(&a));
    let docs = previous
        .and_then(super::briefing_docs::parse)
        .and_then(|d| super::briefing_docs::render_section(&d));
    let rules = previous
        .and_then(super::briefing_rules::parse)
        .and_then(|r| super::briefing_rules::render_section(&r));
    if facts.is_none() && docs.is_none() && rules.is_none() {
        return reviewed.to_string();
    }
    let stripped = strip_facts_section(reviewed);
    let options_at = stripped
        .split_inclusive('\n')
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len();
            Some((start, line))
        })
        .find(|(_, line)| line.trim_end() == crate::core::td_tasks::BRIEFING_OPTIONS_HEADING)
        .map(|(start, _)| start);
    let (head, tail) = match options_at {
        Some(at) => stripped.split_at(at),
        None => (stripped.as_str(), ""),
    };
    let mut body = head.trim_end().to_string();
    body.push('\n');
    for section in [facts, docs, rules].into_iter().flatten() {
        body.push('\n');
        body.push_str(&section);
    }
    if !tail.is_empty() {
        body.push('\n');
        body.push_str(tail);
    }
    body
}

// ─── Effective answers ──────────────────────────────────────────────────────

/// The answers plus what the repository itself says: an empty forge/repository
/// is taken from the project's git remote. The flag says the repository was
/// inferred, so the page cites its source.
pub(crate) fn effective_answers(
    answers: Option<BriefingProjectAnswers>,
    repo_url: Option<&str>,
) -> (BriefingProjectAnswers, bool) {
    let mut answers = answers.unwrap_or_default();
    let mut inferred = false;
    if let Some(url) = repo_url {
        if let Some(slug) = crate::core::github_connection::github_repo_slug(url) {
            if answers.forge == BriefingForge::Unknown {
                answers.forge = BriefingForge::Github;
            }
            if answers.forge == BriefingForge::Github && answers.forge_repo.is_empty() {
                answers.forge_repo = slug;
                inferred = true;
            }
        }
    }
    (answers, inferred)
}

// ─── Read-only checks ───────────────────────────────────────────────────────

/// One GET against a configured connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeRequest {
    pub plugin_id: String,
    pub config_id: String,
    pub endpoint: String,
    pub path_params: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub(crate) enum ProbeResponse {
    Found(serde_json::Value),
    NotFound,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckStatus {
    /// Nothing a connection could check.
    NotApplicable,
    Verified {
        via: String,
    },
    Mismatch {
        via: String,
        detail: String,
    },
    NotVerified {
        reason: String,
    },
}

impl CheckStatus {
    fn render(&self) -> String {
        match self {
            CheckStatus::NotApplicable => "—".into(),
            CheckStatus::Verified { via } => format!("Verified read-only ({via})"),
            CheckStatus::Mismatch { via, detail } => {
                format!("**Mismatch** ({via}): {}", cell(detail))
            }
            CheckStatus::NotVerified { reason } => format!("Not verified: {}", cell(reason)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectChecks {
    pub tracker_key: CheckStatus,
    pub forge_repo: CheckStatus,
    pub default_branch: CheckStatus,
    /// The default branch the forge reports, used when the briefing left it open.
    pub observed_default_branch: Option<String>,
}

impl ProjectChecks {
    pub(crate) fn mismatches(&self) -> Vec<String> {
        [
            ("tracker key", &self.tracker_key),
            ("repository", &self.forge_repo),
            ("default branch", &self.default_branch),
        ]
        .into_iter()
        .filter_map(|(field, status)| match status {
            CheckStatus::Mismatch { detail, .. } => Some(format!("{field}: {detail}")),
            _ => None,
        })
        .collect()
    }
}

/// Outcome of trying every connection of one plugin, first `Found` wins.
async fn probe_all<F, Fut>(
    connections: &[(String, String)],
    plugin_id: &str,
    endpoint: &str,
    path_params: &HashMap<String, String>,
    probe: &F,
) -> Option<ProbeResponse>
where
    F: Fn(ProbeRequest) -> Fut,
    Fut: Future<Output = ProbeResponse>,
{
    let mut last = None;
    for (pid, cid) in connections.iter().filter(|(p, _)| p == plugin_id) {
        let response = probe(ProbeRequest {
            plugin_id: pid.clone(),
            config_id: cid.clone(),
            endpoint: endpoint.to_string(),
            path_params: path_params.clone(),
        })
        .await;
        match response {
            ProbeResponse::Found(_) => return Some(response),
            // A failure outranks a not-found: an unreachable connection
            // proves nothing about the value.
            ProbeResponse::Failed(_) => last = Some(response),
            ProbeResponse::NotFound => {
                if last.is_none() {
                    last = Some(response)
                }
            }
        }
    }
    last
}

fn failure_reason(error: &str) -> String {
    let first = error.lines().next().unwrap_or("").trim();
    let short: String = first.chars().take(160).collect();
    format!("check failed ({short})")
}

/// Check what a configured connection can confirm, read-only: the Jira
/// project key, the GitHub repository and its default branch. No connection
/// is not an error, just "not verified".
pub(crate) async fn run_checks<F, Fut>(
    answers: &BriefingProjectAnswers,
    connections: &[(String, String)],
    probe: F,
) -> ProjectChecks
where
    F: Fn(ProbeRequest) -> Fut,
    Fut: Future<Output = ProbeResponse>,
{
    let mut checks = ProjectChecks {
        tracker_key: CheckStatus::NotApplicable,
        forge_repo: CheckStatus::NotApplicable,
        default_branch: CheckStatus::NotApplicable,
        observed_default_branch: None,
    };

    if answers.tracker == BriefingTracker::Jira && !answers.tracker_key.is_empty() {
        let via = "Jira connection".to_string();
        let params = HashMap::from([("key".to_string(), answers.tracker_key.clone())]);
        checks.tracker_key = match probe_all(
            connections,
            JIRA_PLUGIN,
            "/rest/api/3/project/{key}",
            &params,
            &probe,
        )
        .await
        {
            None => CheckStatus::NotVerified {
                reason: "no Jira connection configured for this project".into(),
            },
            Some(ProbeResponse::Found(body)) => {
                let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if key.eq_ignore_ascii_case(&answers.tracker_key) {
                    CheckStatus::Verified { via }
                } else {
                    CheckStatus::Mismatch {
                        via,
                        detail: format!(
                            "Jira answers with project key `{key}`, not `{}`",
                            answers.tracker_key
                        ),
                    }
                }
            }
            Some(ProbeResponse::NotFound) => CheckStatus::Mismatch {
                via,
                detail: format!(
                    "no Jira project `{}` is visible to the configured connection",
                    answers.tracker_key
                ),
            },
            Some(ProbeResponse::Failed(e)) => CheckStatus::NotVerified {
                reason: failure_reason(&e),
            },
        };
    }

    if answers.forge == BriefingForge::Github && !answers.forge_repo.is_empty() {
        let via = "GitHub connection".to_string();
        let (owner, name) = answers
            .forge_repo
            .split_once('/')
            .unwrap_or((answers.forge_repo.as_str(), ""));
        let params = HashMap::from([
            ("owner".to_string(), owner.to_string()),
            ("repo".to_string(), name.to_string()),
        ]);
        let response = probe_all(
            connections,
            GITHUB_PLUGIN,
            "/repos/{owner}/{repo}",
            &params,
            &probe,
        )
        .await;
        let unverified = |reason: String| CheckStatus::NotVerified { reason };
        match response {
            None => {
                let reason = "no GitHub connection configured for this project".to_string();
                checks.forge_repo = unverified(reason.clone());
                if answers.default_branch != BriefingDefaultBranch::Unknown {
                    checks.default_branch = unverified(reason);
                }
            }
            Some(ProbeResponse::Found(body)) => {
                let full_name = body.get("full_name").and_then(|v| v.as_str()).unwrap_or("");
                checks.forge_repo = if full_name.eq_ignore_ascii_case(&answers.forge_repo) {
                    CheckStatus::Verified { via: via.clone() }
                } else {
                    CheckStatus::Mismatch {
                        via: via.clone(),
                        detail: format!("GitHub resolves it to `{full_name}` (renamed or moved)"),
                    }
                };
                let observed = body
                    .get("default_branch")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let (Some(stated), Some(seen)) =
                    (branch_label(answers.default_branch), observed.as_deref())
                {
                    checks.default_branch = if stated == seen {
                        CheckStatus::Verified { via: via.clone() }
                    } else {
                        CheckStatus::Mismatch {
                            via,
                            detail: format!("GitHub reports `{seen}`, not `{stated}`"),
                        }
                    };
                }
                checks.observed_default_branch = observed;
            }
            Some(ProbeResponse::NotFound) => {
                checks.forge_repo = CheckStatus::Mismatch {
                    via,
                    detail: format!(
                        "`{}` is not found, or not visible to the configured token",
                        answers.forge_repo
                    ),
                };
            }
            Some(ProbeResponse::Failed(e)) => {
                checks.forge_repo = unverified(failure_reason(&e));
                if answers.default_branch != BriefingDefaultBranch::Unknown {
                    checks.default_branch = unverified(failure_reason(&e));
                }
            }
        }
    }
    checks
}

// ─── docs/project.md ────────────────────────────────────────────────────────

/// Render `docs/project.md`. Deterministic: same answers, checks and date give
/// the same bytes.
pub(crate) fn render_project_doc(
    answers: &BriefingProjectAnswers,
    repo_inferred: bool,
    checks: &ProjectChecks,
    today: &str,
) -> String {
    let mut out = String::new();
    out.push_str(GENERATED_MARKER);
    out.push_str(
        " — regenerated by each audit from docs/briefing.md; edit the briefing, or delete this line to own the file. -->\n",
    );
    out.push_str("# Project\n\n");
    out.push_str(
        "> Filled by Kronn from the briefing's closed answers and checked read-only against the tracking connections configured in Kronn. \
         `Not provided — ask before assuming.` means the briefing left it open: ask, never guess.\n\n",
    );
    out.push_str(&format!("Last generated: {today}.\n\n"));

    out.push_str("## Tracker\n\n| Field | Value | Check |\n|-------|-------|-------|\n");
    let note = |value: String, fact: BriefingFact| with_note(value, answers, fact);
    out.push_str(&format!(
        "| Tool | {} | — |\n",
        note(tracker_label(answers.tracker).into(), BriefingFact::Tracker)
    ));
    let key_row = !matches!(
        answers.tracker,
        BriefingTracker::None | BriefingTracker::GithubIssues | BriefingTracker::GitlabIssues
    );
    if key_row || answers.notes.contains_key(&BriefingFact::TrackerKey) {
        out.push_str(&format!(
            "| Project key | {} | {} |\n",
            note(
                code_or_not_provided(&answers.tracker_key),
                BriefingFact::TrackerKey
            ),
            checks.tracker_key.render()
        ));
    }

    out.push_str("\n## Forge\n\n| Field | Value | Check |\n|-------|-------|-------|\n");
    out.push_str(&format!(
        "| Host | {} | — |\n",
        note(forge_label(answers.forge).into(), BriefingFact::Forge)
    ));
    let repo = if repo_inferred {
        format!("`{}` (from the git remote)", answers.forge_repo)
    } else {
        code_or_not_provided(&answers.forge_repo)
    };
    out.push_str(&format!(
        "| Repository | {} | {} |\n",
        note(repo, BriefingFact::ForgeRepo),
        checks.forge_repo.render()
    ));
    let branch = match (
        branch_label(answers.default_branch),
        checks.observed_default_branch.as_deref(),
    ) {
        (Some(b), _) => format!("`{b}`"),
        (None, Some(seen)) => format!("`{}` (reported by the forge)", cell(seen)),
        (None, None) => NOT_PROVIDED.to_string(),
    };
    out.push_str(&format!(
        "| Default branch | {} | {} |\n",
        note(branch, BriefingFact::DefaultBranch),
        checks.default_branch.render()
    ));
    out.push_str(&format!(
        "| Commit convention | {} | — |\n",
        note(
            commit_label(answers.commit_convention).into(),
            BriefingFact::CommitConvention
        )
    ));
    out.push_str(&format!(
        "| Sign-off (`Signed-off-by`) | {} | — |\n",
        note(
            yes_no_label(answers.signoff_required).into(),
            BriefingFact::Signoff
        )
    ));
    out.push_str(&format!(
        "| Merge strategy | {} | — |\n",
        note(
            merge_label(answers.merge_strategy).into(),
            BriefingFact::MergeStrategy
        )
    ));

    out.push_str("\n## Environments\n\n");
    if answers.environments.is_empty() {
        out.push_str(NOT_PROVIDED);
        out.push('\n');
    } else {
        out.push_str("| Environment | URL |\n|-------------|-----|\n");
        for env in &answers.environments {
            out.push_str(&format!(
                "| {} | {} |\n",
                env_label(env.kind),
                or_not_provided(&env.url)
            ));
        }
    }
    if let Some(n) = answers.notes.get(&BriefingFact::Environments) {
        out.push_str(&format!("\nNote: {}\n", cell(n)));
    }
    out.push_str("\nNever write a secret value here; name the variable and where it comes from.\n");

    out.push_str("\n## People\n\n");
    if answers.people.is_empty() {
        out.push_str(NOT_PROVIDED);
        out.push('\n');
    } else {
        out.push_str("| Role | Name | Contact |\n|------|------|---------|\n");
        for person in &answers.people {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                role_label(person.role),
                or_not_provided(&person.name),
                or_not_provided(&person.contact)
            ));
        }
    }
    if let Some(n) = answers.notes.get(&BriefingFact::People) {
        out.push_str(&format!("\nNote: {}\n", cell(n)));
    }
    out.push_str(
        "\nRoles and contacts exactly as entered in the briefing; nothing else about anyone is recorded here.\n",
    );

    out.push_str("\n## Process\n\n| Field | Value |\n|-------|-------|\n");
    out.push_str(&format!(
        "| Review | {} |\n",
        note(
            review_label(answers.review_policy).into(),
            BriefingFact::Review
        )
    ));
    out.push_str(&format!(
        "| Release cadence | {} |\n",
        note(
            cadence_label(answers.release_cadence).into(),
            BriefingFact::ReleaseCadence
        )
    ));
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectDocWrite {
    Written,
    /// The file lost its generated marker: a human owns it.
    KeptHumanOwned,
}

/// Write `docs/project.md` unless a human took it over.
pub(crate) fn write_project_doc(
    project_root: &Path,
    docs_dir: &Path,
    body: &str,
) -> Result<ProjectDocWrite, String> {
    let target = docs_dir.join("project.md");
    crate::core::fs_guard::assert_contained_no_symlink(project_root, &target)?;
    match std::fs::read_to_string(&target) {
        Ok(existing) if !existing.trim_start().starts_with(GENERATED_MARKER) => {
            return Ok(ProjectDocWrite::KeptHumanOwned)
        }
        Ok(existing) if existing == body => return Ok(ProjectDocWrite::Written),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("read {}: {e}", target.display())),
    }
    std::fs::write(&target, body).map_err(|e| format!("write {}: {e}", target.display()))?;
    Ok(ProjectDocWrite::Written)
}

/// What the audit did with `docs/project.md`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ProjectDocReport {
    pub written: bool,
    pub kept_human_owned: bool,
    pub mismatches: Vec<String>,
    pub error: Option<String>,
}

/// The production probe: the ApiCall executor's readiness path, GET only,
/// no retry, bounded in time. Credentials stay inside the executor.
async fn api_probe(
    state: &crate::AppState,
    plugins: &[crate::core::mcp_scanner::ActiveApiPlugin],
    request: ProbeRequest,
) -> ProbeResponse {
    use crate::models::{RunStatus, StepType, WorkflowStep};
    let Some((server, config_id, env)) = plugins
        .iter()
        .find(|(s, c, _)| s.id == request.plugin_id && *c == request.config_id)
    else {
        return ProbeResponse::Failed("connection no longer configured".into());
    };
    let step = WorkflowStep {
        name: "briefing-project-check".into(),
        step_type: StepType::ApiCall,
        api_plugin_slug: Some(server.id.clone()),
        api_config_id: Some(config_id.clone()),
        api_endpoint_path: Some(request.endpoint),
        api_method: Some("GET".into()),
        api_path_params: Some(request.path_params),
        api_timeout_ms: Some(8_000),
        api_max_retries: Some(0),
        ..WorkflowStep::default()
    };
    let context = crate::workflows::template::TemplateContext::new();
    let call = crate::workflows::api_call_executor::execute_api_call_probe(
        &step,
        server,
        config_id,
        env,
        state,
        &context,
        crate::workflows::api_call_executor::SecurityPolicy::production(),
    );
    match tokio::time::timeout(std::time::Duration::from_secs(10), call).await {
        Err(_) => ProbeResponse::Failed("timed out after 10 s".into()),
        Ok(outcome) if outcome.result.status == RunStatus::Success => {
            crate::workflows::template::extract_step_envelope(&outcome.result.output)
                .and_then(|envelope| serde_json::from_str(&envelope.data_json).ok())
                .map(ProbeResponse::Found)
                .unwrap_or_else(|| ProbeResponse::Failed("unreadable response".into()))
        }
        Ok(outcome) if outcome.result.output.contains("[SIGNAL: http_404]") => {
            ProbeResponse::NotFound
        }
        Ok(outcome) => ProbeResponse::Failed(outcome.result.output),
    }
}

/// Audit hook: render `docs/project.md` from the briefing and the read-only
/// checks. Never fails the audit; the report says what happened.
pub(crate) async fn fill_project_doc_for_audit(
    state: &crate::AppState,
    project_id: &str,
    project_path: &Path,
    repo_url: Option<&str>,
    briefing_notes: Option<&str>,
) -> ProjectDocReport {
    let docs_dir = crate::core::scanner::detect_docs_dir(project_path);
    if !docs_dir.is_dir() {
        return ProjectDocReport::default();
    }
    let (answers, repo_inferred) =
        effective_answers(briefing_notes.and_then(parse_answers), repo_url);

    let secret = state.config.read().await.encryption_secret.clone();
    let plugins = match secret {
        Some(secret) => {
            let pid = project_id.to_string();
            state
                .db
                .with_conn(move |conn| {
                    crate::core::mcp_scanner::collect_active_api_plugins_for_scope(
                        conn,
                        Some(&pid),
                        &secret,
                    )
                })
                .await
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    let plugins: Vec<_> = plugins
        .into_iter()
        .filter(|(s, _, _)| s.id == JIRA_PLUGIN || s.id == GITHUB_PLUGIN)
        .collect();
    let connections: Vec<(String, String)> = plugins
        .iter()
        .map(|(s, c, _)| (s.id.clone(), c.clone()))
        .collect();
    let checks = run_checks(&answers, &connections, |request| {
        api_probe(state, &plugins, request)
    })
    .await;

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let body = render_project_doc(&answers, repo_inferred, &checks, &today);
    // Only Kronn's marker names the briefing's path (KT-1192); the answers stay as written.
    let docs_root = crate::core::docs_location::DocsRoot::of(project_path);
    let body = if docs_root.is_default() {
        body
    } else {
        body.replacen(
            "from docs/briefing.md;",
            &format!("from {};", docs_root.path("briefing.md")),
            1,
        )
    };
    let mut report = ProjectDocReport {
        mismatches: checks.mismatches(),
        ..ProjectDocReport::default()
    };
    match write_project_doc(project_path, &docs_dir, &body) {
        Ok(ProjectDocWrite::Written) => report.written = true,
        Ok(ProjectDocWrite::KeptHumanOwned) => report.kept_human_owned = true,
        Err(e) => report.error = Some(e),
    }
    report
}

#[cfg(test)]
#[path = "project_doc_tests.rs"]
mod tests;
