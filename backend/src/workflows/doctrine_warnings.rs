//! KT-1216 — advisory signs that a workflow drifts from the authoring doctrine
//! (`workflows/authoring_doctrine.md`): a script copied into several Exec
//! steps, and an Exec that reaches with `gh`/`curl` a service Kronn already has
//! a configured plugin for. They ride in `kronn_readiness.warnings`; they never
//! block a run and never change `ready`.
use std::collections::BTreeMap;

use regex_lite::Regex;

use crate::models::{StepType, Workflow, WorkflowStep, WorkflowWarning, WorkflowWarningKind};

/// An argument this long is a script or inline code, not a flag or an operand.
const SCRIPT_ARG_BYTES: usize = 200;

/// A configured API plugin an Exec could bypass.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguredApi {
    /// The plugin's display name, for the message.
    pub name: String,
    /// Host its base URL serves; `*` stands for a placeholder segment.
    pub host: String,
}

impl ConfiguredApi {
    /// The host of a plugin base URL (`https://${ENV.DOMAIN}.example.com/api`).
    pub fn from_base_url(name: &str, base_url: &str) -> Option<Self> {
        const WILD: &str = "kronnwildcard";
        let placeholder = Regex::new(r"\$\{[^}]*\}").ok()?;
        let url = reqwest::Url::parse(&placeholder.replace_all(base_url, WILD)).ok()?;
        let host = url.host_str()?.replace(WILD, "*");
        Some(Self {
            name: name.to_string(),
            host: host.to_ascii_lowercase(),
        })
    }

    fn serves(&self, host: &str) -> bool {
        let pattern = self
            .host
            .split('*')
            .map(regex_lite::escape)
            .collect::<Vec<_>>()
            .join(".*");
        Regex::new(&format!("^{pattern}$")).is_ok_and(|re| re.is_match(&host.to_ascii_lowercase()))
    }
}

/// Every warning of one workflow, over its steps and its rollback chain.
pub fn warnings(wf: &Workflow, apis: &[ConfiguredApi]) -> Vec<WorkflowWarning> {
    let execs: Vec<&WorkflowStep> = wf
        .steps
        .iter()
        .chain(wf.on_failure.iter())
        .filter(|step| step.step_type == StepType::Exec)
        .collect();
    let mut found = duplicated_scripts(wf, &execs);
    found.extend(plugin_bypasses(wf, &execs, apis));
    found
}

fn warning(
    wf: &Workflow,
    steps: Vec<String>,
    kind: WorkflowWarningKind,
    message: String,
    action: &str,
) -> WorkflowWarning {
    WorkflowWarning {
        workflow_id: wf.id.clone(),
        workflow_name: wf.name.clone(),
        steps,
        kind,
        message,
        action: action.to_string(),
    }
}

/// What a step runs that another step may repeat: each long argument with its
/// command, and a long argument list as a whole.
fn script_keys(step: &WorkflowStep) -> Vec<(String, String)> {
    let command = step.exec_command.clone().unwrap_or_default();
    let mut keys: Vec<(String, String)> = step
        .exec_args
        .iter()
        .filter(|arg| arg.len() >= SCRIPT_ARG_BYTES)
        .map(|arg| (command.clone(), arg.clone()))
        .collect();
    let joined = step.exec_args.join("\u{1f}");
    if joined.len() >= SCRIPT_ARG_BYTES && !keys.iter().any(|(_, arg)| *arg == joined) {
        keys.push((command, joined));
    }
    keys
}

fn duplicated_scripts(wf: &Workflow, execs: &[&WorkflowStep]) -> Vec<WorkflowWarning> {
    let mut by_script: BTreeMap<(String, String), Vec<&str>> = BTreeMap::new();
    for step in execs {
        for key in script_keys(step) {
            let steps = by_script.entry(key).or_default();
            if !steps.contains(&step.name.as_str()) {
                steps.push(step.name.as_str());
            }
        }
    }
    // One warning per set of steps, whatever number of arguments they share.
    let mut by_steps: BTreeMap<Vec<&str>, usize> = BTreeMap::new();
    for ((_, script), steps) in &by_script {
        if steps.len() > 1 {
            let size = by_steps.entry(steps.clone()).or_default();
            *size = (*size).max(script.len());
        }
    }
    by_steps
        .into_iter()
        .map(|(steps, bytes)| {
            let names = steps
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            warning(
                wf,
                steps.iter().map(|name| name.to_string()).collect(),
                WorkflowWarningKind::DuplicatedExecScript,
                format!(
                    "Steps {names} carry the same {bytes}-byte script or inline code: a fix \
                     has to be made in each copy."
                ),
                "Keep the script in one place: a saved Quick Exec (a CollectApiData source's \
                 `quick_exec_id`) or `exec_script_files` (a file in the project's repository), \
                 and pass what differs between the steps as an argument. Read \
                 `authoring_doctrine` in workflow_step_schema.",
            )
        })
        .collect()
}

/// HTTP clients and interpreters whose arguments may name a URL to call.
const HTTP_CLIENTS: &[&str] = &["curl", "wget", "http", "https", "xh"];
const INTERPRETERS: &[&str] = &[
    "bash", "sh", "zsh", "python", "python3", "node", "deno", "ruby", "perl", "php",
];

fn plugin_bypasses(
    wf: &Workflow,
    execs: &[&WorkflowStep],
    apis: &[ConfiguredApi],
) -> Vec<WorkflowWarning> {
    if apis.is_empty() {
        return Vec::new();
    }
    let (Ok(gh), Ok(url)) = (
        Regex::new(
            r#"(?:^|[^\w./-])gh[\s"',\]]+(?:api|pr|issue|repo|run|search|release|workflow|gist|project)\b"#,
        ),
        Regex::new(r"https?://([A-Za-z0-9._*-]+)"),
    ) else {
        return Vec::new();
    };
    let github = apis.iter().find(|api| api.host == "api.github.com");
    let mut found = Vec::new();
    for step in execs {
        let command = step
            .exec_command
            .as_deref()
            .map(|c| c.rsplit('/').next().unwrap_or(c))
            .unwrap_or_default();
        let text = std::iter::once(command.to_string())
            .chain(step.exec_args.iter().cloned())
            .chain(step.exec_stdin.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let mut reached: Vec<(&str, &str)> = Vec::new();
        if let Some(api) = github {
            if command == "gh" || gh.is_match(&text) {
                reached.push((&api.name, "gh"));
            }
        }
        if HTTP_CLIENTS.contains(&command) || INTERPRETERS.contains(&command) {
            for captured in url.captures_iter(&text) {
                let host = captured.get(1).map_or("", |m| m.as_str());
                if let Some(api) = apis.iter().find(|api| api.serves(host)) {
                    if !reached.iter().any(|(name, _)| *name == api.name) {
                        reached.push((&api.name, "an HTTP call"));
                    }
                }
            }
        }
        for (plugin, how) in reached {
            found.push(warning(
                wf,
                vec![step.name.clone()],
                WorkflowWarningKind::ExecBypassesPlugin,
                format!(
                    "Step `{}` reaches {plugin} with {how} in an Exec, but {plugin} has a \
                     configured API plugin: the plugin's credentials, the API call log and \
                     the inferred no-change classification are bypassed.",
                    step.name
                ),
                "Use ApiCall, BatchApiCall or CollectApiData (a saved Quick API first: qa_list, \
                 plugin and config ids from mcp_list). If a need has no native answer (a \
                 paginated GraphQL query), tell the user and keep one targeted Exec for it. \
                 Read `authoring_doctrine` in workflow_step_schema.",
            ));
        }
    }
    found
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "doctrine_warnings_test.rs"]
mod tests;
