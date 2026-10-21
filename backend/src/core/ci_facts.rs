//! KT-1404 — what the CI files say about WHEN they run and on WHICH runtime.
//!
//! The audit wrote "CI runs ESLint and Jest on every PR and blocks merge" for
//! a repository whose workflow runs only on the `ci-test` label, called that
//! label-only CI "an intentional budget guard" (a purpose written nowhere),
//! and stated Node >=22 in two documents next to a debt sheet saying CI runs
//! Node 20. Each document was written by a different step from a different
//! reading of the workflow. This reads it once: the events that trigger each
//! workflow, the condition on each job, and the runtime versions each job runs,
//! so every step describes the same thing, as the file states it.
//!
//! Files are listed from the tracked paths and read through [`rooted_io`]:
//! nothing outside the project reaches a prompt. Every text taken from a CI
//! file is flattened to one bounded line, and every value the secret scan would
//! flag in it (a token compared in an `if:`, a webhook URL) is replaced by a
//! marker first: the gate keeps its meaning, never its secret.
//!
//! A file the reader could not follow completely says so: a job whose lines it
//! had to skip is rendered "not fully read", never as "no condition".

use std::path::Path;

use crate::core::rooted_io;
use crate::core::secret_facts::{self, Literals};
use crate::core::yaml_lite::{self, Node};

const MAX_FILE: u64 = 512 * 1024;
const MAX_WORKFLOWS: usize = 12;
const MAX_JOBS: usize = 12;
/// The most the block may weigh: it rides in four step prompts.
const BUDGET: usize = 4_200;
const CONDITION_CAP: usize = 150;

/// `<key>: <value>` of a setup step, and the runtime it picks.
const RUNTIME_KEYS: &[(&str, &str)] = &[
    ("node-version", "node"),
    ("php-version", "php"),
    ("python-version", "python"),
    ("go-version", "go"),
    ("java-version", "java"),
    ("ruby-version", "ruby"),
    ("dotnet-version", "dotnet"),
];

/// One job of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub line: usize,
    /// The job's `if:` / `rules:` / `only:` / `except:`, flattened.
    pub condition: Option<String>,
    /// `node 20, 22` and where each was written.
    pub runtimes: Vec<(String, String, usize)>,
    /// Lines of the job the reader skipped: its condition and runtimes may be incomplete.
    pub unread: Vec<usize>,
}

/// One CI file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub file: String,
    /// What triggers it, flattened: `pull_request (types: [labeled]); push (branches: [main])`.
    pub triggers: Option<(String, usize)>,
    pub jobs: Vec<Job>,
    /// Lines of the trigger section the reader skipped.
    pub triggers_unread: Vec<usize>,
    /// Lines skipped outside any job and the trigger section.
    pub unread_elsewhere: Vec<usize>,
}

/// A text from a CI file as one line that may sit in a prompt.
fn flat(text: &str, cap: usize) -> String {
    let text = secret_facts::redact(text);
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let out: String = out
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == '`' { '\'' } else { c })
        .collect();
    if out.chars().count() > cap {
        let cut: String = out.chars().take(cap).collect();
        format!("{cut}…")
    } else {
        out
    }
}

fn trigger_summary(on: &Node) -> String {
    let event = |name: &str, config: &Node| -> String {
        let mut parts = Vec::new();
        for field in ["types", "branches", "tags", "workflows"] {
            if let Some(value) = config.get(field) {
                parts.push(format!("{field}: {}", value.flow()));
            }
        }
        if config.get("paths").is_some() || config.get("paths-ignore").is_some() {
            parts.push("path-filtered".to_string());
        }
        if config.get("branches-ignore").is_some() {
            parts.push("branches-ignore set".to_string());
        }
        if parts.is_empty() {
            name.to_string()
        } else {
            format!("{name} ({})", parts.join("; "))
        }
    };
    match on {
        Node::Scalar(name) => name.clone(),
        Node::Seq(names) => names
            .iter()
            .filter_map(|n| n.scalar().map(str::to_string))
            .collect::<Vec<_>>()
            .join("; "),
        Node::Map(entries) => entries
            .iter()
            .map(|entry| {
                if entry.key == "schedule" {
                    let crons: Vec<&str> = entry
                        .value
                        .items()
                        .iter()
                        .filter_map(|item| item.get("cron").and_then(Node::scalar))
                        .collect();
                    format!("schedule (cron: {})", crons.join(", "))
                } else {
                    event(&entry.key, &entry.value)
                }
            })
            .collect::<Vec<_>>()
            .join("; "),
    }
}

/// `${{ matrix.node }}` → the values of `strategy.matrix.node`.
fn resolve(raw: &str, matrix: Option<&Node>) -> String {
    let inner = raw
        .trim()
        .trim_start_matches("${{")
        .trim_end_matches("}}")
        .trim();
    if let Some(axis) = inner.strip_prefix("matrix.") {
        if let Some(values) = matrix.and_then(|m| m.get(axis)) {
            return match values {
                Node::Seq(items) => flat(
                    &items
                        .iter()
                        .filter_map(|n| n.scalar().map(str::to_string))
                        .collect::<Vec<_>>()
                        .join(", "),
                    80,
                ),
                other => other
                    .scalar()
                    .map_or_else(|| flat(raw, 40), |value| flat(value, 80)),
            };
        }
    }
    // A flow list written in the version itself: `[18, 20]`.
    flat(raw.trim_matches(|c| c == '[' || c == ']'), 40)
}

fn runtimes_of(job: &Node, job_line: usize) -> Vec<(String, String, usize)> {
    let matrix = job.get("strategy").and_then(|s| s.get("matrix"));
    let mut out: Vec<(String, String, usize)> = Vec::new();
    let mut add = |runtime: &str, version: String, line: usize| {
        let version = flat(&version, 80);
        if version.is_empty() || out.iter().any(|(r, v, _)| r == runtime && v == &version) {
            return;
        }
        out.push((runtime.to_string(), version, line.max(job_line)));
    };
    for step in job.get("steps").map(Node::items).unwrap_or_default() {
        let Some(with) = step.get("with") else {
            continue;
        };
        for (field, runtime) in RUNTIME_KEYS {
            if let Some(entry) = with.entry(field) {
                let raw = match &entry.value {
                    Node::Seq(_) => entry.value.flow(),
                    other => other.scalar().unwrap_or_default().to_string(),
                };
                add(runtime, resolve(&raw, matrix), entry.line);
            }
        }
        for (field, runtime) in [
            ("node-version-file", "node"),
            ("python-version-file", "python"),
            ("php-version-file", "php"),
        ] {
            if let Some(entry) = with.entry(field) {
                let file = entry.value.scalar().unwrap_or_default();
                add(runtime, format!("from {}", flat(file, 60)), entry.line);
            }
        }
    }
    // `container: node:20` / `container: {image: php:8.3-cli}`.
    let image = job.entry("container").and_then(|c| {
        c.value
            .scalar()
            .or_else(|| c.value.get("image").and_then(Node::scalar))
            .map(|image| (image.to_string(), c.line))
    });
    if let Some((image, line)) = image {
        let name = image.rsplit('/').next().unwrap_or(&image);
        if let Some((runtime, tag)) = name.split_once(':') {
            if matches!(runtime, "node" | "php" | "python") && !tag.contains("${{") {
                let version = tag.split('-').next().unwrap_or(tag);
                add(runtime, flat(version, 40), line);
            }
        }
    }
    out
}

fn job_condition(job: &Node) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(value) = job.get("if") {
        parts.push(format!("if: {}", flat(&value.flow(), CONDITION_CAP)));
    }
    for field in ["rules", "only", "except"] {
        if let Some(value) = job.get(field) {
            parts.push(format!("{field}: {}", flat(&value.flow(), CONDITION_CAP)));
        }
    }
    // A step gated on a label is a gate too.
    let label_steps = job
        .get("steps")
        .map(Node::items)
        .unwrap_or_default()
        .iter()
        .filter_map(|step| step.get("if").and_then(Node::scalar))
        .filter(|condition| condition.contains("label"))
        .count();
    if label_steps > 0 {
        parts.push(format!("{label_steps} step(s) gated on a label"));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// The unread lines in `[from, to)`.
fn within(unread: &[usize], from: usize, to: usize) -> Vec<usize> {
    unread
        .iter()
        .copied()
        .filter(|line| (from..to).contains(line))
        .collect()
}

/// The line where the entry at `line` ends: the next sibling's line, or `parent_end`.
fn section_end(siblings: &[yaml_lite::Entry], line: usize, parent_end: usize) -> usize {
    siblings
        .iter()
        .map(|entry| entry.line)
        .filter(|other| *other > line)
        .min()
        .unwrap_or(parent_end)
        .min(parent_end)
}

fn github_job(
    entry: &yaml_lite::Entry,
    siblings: &[yaml_lite::Entry],
    parent_end: usize,
    unread: &[usize],
) -> Job {
    let end = section_end(siblings, entry.line, parent_end);
    Job {
        line: entry.line,
        condition: job_condition(&entry.value),
        runtimes: runtimes_of(&entry.value, entry.line),
        id: flat(&entry.key, 60),
        unread: within(unread, entry.line, end),
    }
}

fn github_workflow(file: &str, text: &str) -> Option<Workflow> {
    let (root, unread) = yaml_lite::parse_checked(text)?;
    let top = root.entries();
    let on = root.entry("on");
    let triggers = on
        .map(|entry| (flat(&trigger_summary(&entry.value), 300), entry.line))
        .filter(|(summary, _)| !summary.is_empty());
    let triggers_unread = on
        .map(|entry| {
            within(
                &unread,
                entry.line,
                section_end(top, entry.line, usize::MAX),
            )
        })
        .unwrap_or_default();
    let mut jobs = Vec::new();
    let mut covered: Vec<usize> = triggers_unread.clone();
    if let Some(jobs_entry) = root.entry("jobs") {
        let jobs_end = section_end(top, jobs_entry.line, usize::MAX);
        let siblings = jobs_entry.value.entries();
        for entry in siblings.iter().take(MAX_JOBS) {
            let job = github_job(entry, siblings, jobs_end, &unread);
            covered.extend(job.unread.iter().copied());
            jobs.push(job);
        }
        // Lines in the `jobs:` section before its first job, or in a job past the cap.
        let first = siblings.iter().map(|e| e.line).min().unwrap_or(jobs_end);
        covered.extend(within(&unread, jobs_entry.line, first));
        if siblings.len() > MAX_JOBS {
            covered.extend(within(&unread, siblings[MAX_JOBS].line, jobs_end));
        }
    }
    let unread_elsewhere = unread
        .iter()
        .copied()
        .filter(|line| !covered.contains(line))
        .collect();
    Some(Workflow {
        file: file.to_string(),
        triggers,
        jobs,
        triggers_unread,
        unread_elsewhere,
    })
}

const GITLAB_RESERVED: &[&str] = &[
    "stages",
    "variables",
    "default",
    "include",
    "workflow",
    "image",
    "services",
    "cache",
    "before_script",
    "after_script",
];

fn image_of(node: &Node) -> Option<String> {
    node.get("image").and_then(|image| {
        image
            .scalar()
            .or_else(|| image.get("name").and_then(Node::scalar))
            .map(str::to_string)
    })
}

fn gitlab_workflow(file: &str, text: &str) -> Option<Workflow> {
    let (root, unread) = yaml_lite::parse_checked(text)?;
    let top = root.entries();
    let triggers = root
        .get("workflow")
        .and_then(|workflow| workflow.entry("rules"))
        .map(|rules| {
            (
                format!("workflow rules: {}", flat(&rules.value.flow(), 300)),
                rules.line,
            )
        });
    let default_image = image_of(&root);
    let mut jobs = Vec::new();
    for entry in root.entries() {
        if entry.key.starts_with('.') || GITLAB_RESERVED.contains(&entry.key.as_str()) {
            continue;
        }
        let job = &entry.value;
        if job.get("script").is_none() && job.get("trigger").is_none() {
            continue;
        }
        let mut runtimes = Vec::new();
        if let Some(image) = image_of(job).or_else(|| default_image.clone()) {
            let name = image.rsplit('/').next().unwrap_or(&image).to_string();
            if let Some((runtime, tag)) = name.split_once(':') {
                if matches!(runtime, "node" | "php" | "python") {
                    let version = tag.split('-').next().unwrap_or(tag);
                    runtimes.push((runtime.to_string(), flat(version, 40), entry.line));
                }
            }
        }
        jobs.push(Job {
            line: entry.line,
            condition: job_condition(job),
            runtimes,
            id: flat(&entry.key, 60),
            unread: within(
                &unread,
                entry.line,
                section_end(top, entry.line, usize::MAX),
            ),
        });
        if jobs.len() >= MAX_JOBS {
            break;
        }
    }
    let covered: Vec<usize> = jobs.iter().flat_map(|job| job.unread.clone()).collect();
    let unread_elsewhere = unread
        .iter()
        .copied()
        .filter(|line| !covered.contains(line))
        .collect();
    Some(Workflow {
        file: file.to_string(),
        triggers,
        jobs,
        triggers_unread: Vec::new(),
        unread_elsewhere,
    })
}

/// A Jenkins pipeline file: its trigger and gate lines and the Node it asks for.
/// A line with a URL is never read: a webhook has no business in a CI fact.
fn jenkins_workflow(file: &str, text: &str) -> Option<Workflow> {
    let mut triggers: Vec<String> = Vec::new();
    let mut first = None;
    let mut runtimes = Vec::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") || line.contains("://") {
            continue;
        }
        let gate = [
            "cron(",
            "pollSCM(",
            "githubPush",
            "upstream(",
            "changeRequest",
            "buildingTag",
        ]
        .iter()
        .any(|marker| line.contains(marker))
            || line.starts_with("branch ")
            || line.starts_with("triggers");
        if gate && triggers.len() < 6 {
            first.get_or_insert(n + 1);
            triggers.push(flat(line, 100));
        }
        if let Some(at) = line.find("nodejs") {
            let rest = &line[at + 6..];
            if let Some(quote) = rest.find(['\'', '"']) {
                let version: String = rest[quote + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                    .collect();
                if !version.is_empty() {
                    runtimes.push(("node".to_string(), flat(&version, 30), n + 1));
                }
            }
        }
    }
    let triggers = (!triggers.is_empty()).then(|| (triggers.join(" | "), first.unwrap_or(1)));
    if triggers.is_none() && runtimes.is_empty() {
        return Some(Workflow {
            file: file.to_string(),
            triggers: None,
            jobs: Vec::new(),
            triggers_unread: Vec::new(),
            unread_elsewhere: Vec::new(),
        });
    }
    Some(Workflow {
        file: file.to_string(),
        triggers,
        jobs: vec![Job {
            id: "pipeline".to_string(),
            line: 1,
            condition: None,
            runtimes,
            unread: Vec::new(),
        }],
        triggers_unread: Vec::new(),
        unread_elsewhere: Vec::new(),
    })
}

fn is_workflow_path(path: &str) -> bool {
    let in_workflows = path.starts_with(".github/workflows/")
        && path.matches('/').count() == 2
        && (path.ends_with(".yml") || path.ends_with(".yaml"));
    in_workflows || path == ".gitlab-ci.yml" || is_jenkins_path(path)
}

fn is_jenkins_path(path: &str) -> bool {
    !path.contains('/') && path.to_ascii_lowercase().starts_with("jenkins")
}

/// A file name that may be shown in a prompt.
fn printable(path: &str) -> bool {
    path.len() <= 120
        && path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
}

/// Reads every CI file among `tracked` (a git listing) or, with no listing,
/// among the standard locations.
pub fn detect(project: &Path, tracked: Option<&[String]>) -> Vec<Workflow> {
    detect_scrubbed(project, tracked, &Literals::default())
}

/// [`detect`], with every value the secret scan flagged anywhere in the
/// repository removed from the CI text before it is read: a gate that compares
/// a detected token (`contains(env.X, '<token>')`, `'<token>' == env.X`) keeps
/// its shape and loses the token, whatever the form of the condition.
pub fn detect_scrubbed(
    project: &Path,
    tracked: Option<&[String]>,
    literals: &Literals,
) -> Vec<Workflow> {
    let mut files: Vec<String> = match tracked {
        Some(tracked) => tracked
            .iter()
            .filter(|path| is_workflow_path(path))
            .cloned()
            .collect(),
        None => {
            let mut found = Vec::new();
            for rel in [".gitlab-ci.yml", "Jenkinsfile"] {
                if std::fs::symlink_metadata(project.join(rel)).is_ok_and(|m| m.is_file()) {
                    found.push(rel.to_string());
                }
            }
            let workflows = project.join(".github/workflows");
            if crate::core::fs_guard::assert_contained_no_symlink(project, &workflows).is_ok() {
                for entry in std::fs::read_dir(&workflows)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let rel = format!(".github/workflows/{name}");
                    if is_workflow_path(&rel) {
                        found.push(rel);
                    }
                }
            }
            found
        }
    };
    files.retain(|file| printable(file));
    files.sort();
    files.truncate(MAX_WORKFLOWS);
    files
        .into_iter()
        .filter_map(|file| {
            let bytes = rooted_io::read_capped(project, Path::new(&file), MAX_FILE).ok()?;
            let text = String::from_utf8(bytes).ok()?;
            let text = literals.scrub(&text);
            if file == ".gitlab-ci.yml" {
                gitlab_workflow(&file, &text)
            } else if is_jenkins_path(&file) {
                jenkins_workflow(&file, &text)
            } else {
                github_workflow(&file, &text)
            }
        })
        .collect()
}

/// `3, 8, 12`, and `… (+N)` past five.
fn lines_list(lines: &[usize]) -> String {
    let shown: Vec<String> = lines.iter().take(5).map(usize::to_string).collect();
    if lines.len() > 5 {
        format!("{}, … (+{})", shown.join(", "), lines.len() - 5)
    } else {
        shown.join(", ")
    }
}

/// The prompt block; `None` when the repository has no CI file.
pub fn render_block(workflows: &[Workflow]) -> Option<String> {
    if workflows.is_empty() {
        return None;
    }
    let mut out = String::from(
        "## CI facts — computed by Kronn from the workflow files (ground truth: describe a trigger, a condition or a runtime exactly as stated here)\n\n",
    );
    let mut dropped = 0;
    for workflow in workflows {
        let mut entry = match &workflow.triggers {
            Some((triggers, line)) => {
                format!("- `{}:{line}` runs on: {triggers}\n", workflow.file)
            }
            None => format!("- `{}`: no trigger found\n", workflow.file),
        };
        if !workflow.triggers_unread.is_empty() {
            entry.push_str(&format!(
                "  - the trigger section is not fully read (lines {} use YAML Kronn's reader does not follow): read it in the file before describing when the workflow runs\n",
                lines_list(&workflow.triggers_unread)
            ));
        }
        for job in &workflow.jobs {
            let gate = match (&job.condition, job.unread.is_empty()) {
                (Some(condition), true) => format!("only when `{condition}`"),
                (Some(condition), false) => format!(
                    "reads `{condition}` but is not fully read (lines {} use YAML Kronn's reader does not follow): read the job in the file before stating its condition or its runtime",
                    lines_list(&job.unread)
                ),
                (None, true) => "no condition of its own".to_string(),
                (None, false) => format!(
                    "not fully read (lines {} use YAML Kronn's reader does not follow): Kronn cannot say whether it has a condition or which runtime it runs; read the job in the file",
                    lines_list(&job.unread)
                ),
            };
            let runtimes = if job.runtimes.is_empty() {
                String::new()
            } else {
                let list: Vec<String> = job
                    .runtimes
                    .iter()
                    .map(|(runtime, version, line)| {
                        format!("{runtime} {version} ({}:{line})", workflow.file)
                    })
                    .collect();
                format!("; runs {}", list.join(", "))
            };
            entry.push_str(&format!(
                "  - job `{}` ({}:{}): {gate}{runtimes}\n",
                job.id, workflow.file, job.line
            ));
        }
        if !workflow.unread_elsewhere.is_empty() {
            entry.push_str(&format!(
                "  - `{}` is not fully read (lines {} use YAML Kronn's reader does not follow): the facts above may be incomplete\n",
                workflow.file,
                lines_list(&workflow.unread_elsewhere)
            ));
        }
        if out.len() + entry.len() > BUDGET {
            dropped += 1;
            continue;
        }
        out.push_str(&entry);
    }
    if dropped > 0 {
        out.push_str(&format!(
            "- … {dropped} more CI file(s) not shown; read them before describing the CI\n"
        ));
    }
    out.push_str(
        "\nRules for every statement about CI:\n\
         1. A trigger or a gate is described only as the file states it: `runs on` lists the events, a job `only when` runs only if the condition holds. A job that runs on a label, a branch or a schedule is never described as running \"on every PR\", \"on every push\" or \"always\".\n\
         2. Never state a purpose or an intent the repository does not write (\"budget guard\", \"to save minutes\", \"by design\"): say what the file does, and, for why, cite a document that says it or write `intent not documented`.\n\
         3. That a check \"blocks merge\" or is \"required\" is a repository setting (branch protection) no workflow file shows: write `not verified: branch protection is not in the repository`.\n\
         4. A job or a section marked \"not fully read\" is NOT a job without condition or runtime: write that Kronn could not read it completely and read the file yourself; never assert a missing condition, trigger or runtime from it.\n\
         5. The runtime a job runs is the version listed here, with its file and line; use it for every statement about what CI runs, and give the same one in every document. A CI version is not the runtime the application ships on.\n",
    );
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    const LABELLED: &str = "name: CI\n\
on:\n  pull_request:\n    types: [labeled]\n    branches: [main]\n  workflow_dispatch:\n\
jobs:\n  test:\n    if: contains(github.event.pull_request.labels.*.name, 'ci-test')\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n      - run: npm test\n  lint:\n    runs-on: ubuntu-latest\n    strategy:\n      matrix:\n        node: [18, 20]\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: ${{ matrix.node }}\n";

    fn tracked(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn a_label_gated_workflow_is_stated_as_it_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".github/workflows/ci.yml", LABELLED);
        let found = detect(tmp.path(), Some(&tracked(&[".github/workflows/ci.yml"])));
        let block = render_block(&found).expect("a block");
        assert!(
            block.contains(
                "runs on: pull_request (types: [labeled]; branches: [main]); workflow_dispatch"
            ),
            "{block}"
        );
        assert!(
            block.contains("job `test`")
                && block.contains(
                    "only when `if: contains(github.event.pull_request.labels.*.name, 'ci-test')`"
                ),
            "{block}"
        );
        assert!(
            block.contains("node 20 (.github/workflows/ci.yml:14)"),
            "{block}"
        );
        // The matrix axis is resolved, and a job without condition says so.
        assert!(block.contains("node 18, 20"), "{block}");
        assert!(block.contains("no condition of its own"), "{block}");
        // The prompt rules against invented guarantees and intents.
        assert!(
            block.contains("never described as running \"on every PR\""),
            "{block}"
        );
        assert!(
            block.contains("Never state a purpose or an intent"),
            "{block}"
        );
        assert!(
            block.contains("not verified: branch protection is not in the repository"),
            "{block}"
        );
    }

    #[test]
    fn a_credential_in_a_job_condition_never_reaches_the_block() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = ["a1B2c3D4", "e5F6g7H8", "i9J0k1L2"].concat();
        let hook = format!("https://hooks.slack.com/services/T0A1B2C3D/B4E5F6G7H/{secret}");
        let workflow = [
            "on:\n  pull_request:\n    types: [labeled]\njobs:\n  deploy:\n",
            "    if: env.DEPLOY_TOKEN == 'SECRET' && contains(github.event.pull_request.labels.*.name, 'ci-test')\n",
            "    runs-on: x\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 'SECRET'\n",
            "  notify:\n    if: github.event.label.name == 'HOOK'\n    runs-on: x\n    steps: []\n",
        ]
        .concat()
        .replace("SECRET", &secret)
        .replace("HOOK", &hook);
        write(tmp.path(), ".github/workflows/ci.yml", &workflow);
        let found = detect(tmp.path(), Some(&tracked(&[".github/workflows/ci.yml"])));
        let block = render_block(&found).unwrap();
        assert!(!block.contains(&secret), "{block}");
        assert!(
            !block.contains("A1B2C3D4") && !block.contains("T0A1B2C3D"),
            "{block}"
        );
        // The gate keeps its meaning: the credential name and the label.
        assert!(block.contains("env.DEPLOY_TOKEN =="), "{block}");
        assert!(block.contains("'ci-test'"), "{block}");
        assert!(block.contains("redacted"), "{block}");
    }

    #[test]
    fn an_anchor_with_a_nested_mapping_keeps_the_job_condition_and_runtime() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".github/workflows/ci.yml",
            "on:\n  pull_request:\n    types: [labeled]\njobs:\n  test:\n    env: &defaults\n      NODE_ENV: test\n      CI: true\n    if: contains(github.event.pull_request.labels.*.name, 'ci-test')\n    runs-on: x\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n  lint:\n    env: *defaults\n    runs-on: x\n    steps: []\n",
        );
        let found = detect(tmp.path(), Some(&tracked(&[".github/workflows/ci.yml"])));
        let block = render_block(&found).unwrap();
        assert!(
            block.contains(
                "only when `if: contains(github.event.pull_request.labels.*.name, 'ci-test')`"
            ),
            "{block}"
        );
        assert!(block.contains("node 20"), "{block}");
        assert!(!block.contains("use YAML Kronn's reader"), "{block}");
    }

    #[test]
    fn a_job_the_reader_could_not_follow_is_never_stated_as_having_no_condition() {
        let tmp = tempfile::tempdir().unwrap();
        // `stray text` sits in the job where no key can own it: that job is partial.
        write(
            tmp.path(),
            ".github/workflows/ci.yml",
            "on: push\njobs:\n  a:\n    steps:\n      - run: x\n      stray text\n    runs-on: x\n  b:\n    runs-on: x\n    steps: []\n",
        );
        let found = detect(tmp.path(), Some(&tracked(&[".github/workflows/ci.yml"])));
        let block = render_block(&found).unwrap();
        assert!(
            block.contains("job `a` (.github/workflows/ci.yml:3): not fully read"),
            "{block}"
        );
        assert!(
            !block.contains("job `a` (.github/workflows/ci.yml:3): no condition"),
            "{block}"
        );
        // The other job was read whole.
        assert!(
            block.contains("job `b` (.github/workflows/ci.yml:8): no condition of its own"),
            "{block}"
        );
        assert!(
            block.contains("never assert a missing condition"),
            "{block}"
        );
    }

    #[test]
    fn push_and_schedule_triggers_and_a_single_event_string() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".github/workflows/nightly.yaml",
            "on:\n  push:\n    branches: [main]\n    paths: ['src/**']\n  schedule:\n    - cron: '0 3 * * *'\njobs:\n  b:\n    runs-on: x\n    container: node:22-alpine\n    steps: []\n",
        );
        write(
            tmp.path(),
            ".github/workflows/rel.yml",
            "on: release\njobs:\n  p:\n    runs-on: x\n    steps: []\n",
        );
        let found = detect(
            tmp.path(),
            Some(&tracked(&[
                ".github/workflows/nightly.yaml",
                ".github/workflows/rel.yml",
            ])),
        );
        let block = render_block(&found).unwrap();
        assert!(
            block.contains("push (branches: [main]; path-filtered); schedule (cron: 0 3 * * *)"),
            "{block}"
        );
        assert!(block.contains("runs on: release"), "{block}");
        assert!(block.contains("node 22"), "{block}");
    }

    #[test]
    fn a_gitlab_job_reports_its_rules_and_image() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".gitlab-ci.yml",
            "image: php:8.3-cli\nstages: [test]\nunit:\n  stage: test\n  script: [vendor/bin/phpunit]\n  rules:\n    - if: '$CI_MERGE_REQUEST_ID'\n",
        );
        let found = detect(tmp.path(), Some(&tracked(&[".gitlab-ci.yml"])));
        let block = render_block(&found).unwrap();
        assert!(block.contains("job `unit`"), "{block}");
        assert!(block.contains("CI_MERGE_REQUEST_ID"), "{block}");
        assert!(block.contains("php 8.3"), "{block}");
    }

    #[test]
    fn a_jenkins_file_gives_its_triggers_and_node_without_urls() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "Jenkinsfile.staging",
            "pipeline {\n  triggers { cron('H 2 * * *') }\n  tools { nodejs 'node-20' }\n  post { always { office365ConnectorSend webhookUrl: 'https://contoso.webhook.office.com/webhookb2/x' } }\n}\n",
        );
        let found = detect(tmp.path(), Some(&tracked(&["Jenkinsfile.staging"])));
        let block = render_block(&found).unwrap();
        assert!(block.contains("cron('H 2 * * *')"), "{block}");
        assert!(block.contains("node node-20"), "{block}");
        assert!(!block.contains("webhook"), "{block}");
    }

    #[test]
    fn no_ci_file_no_block_and_a_hostile_name_is_not_listed() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(render_block(&detect(tmp.path(), Some(&[]))), None);
        write(
            tmp.path(),
            ".github/workflows/a`b.yml",
            "on: push\njobs: {}\n",
        );
        let found = detect(tmp.path(), Some(&tracked(&[".github/workflows/a`b.yml"])));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn an_untracked_listing_falls_back_to_the_standard_locations() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".github/workflows/ci.yml", LABELLED);
        let found = detect(tmp.path(), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, ".github/workflows/ci.yml");
    }

    #[test]
    fn the_block_stays_within_its_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for n in 0..12 {
            let rel = format!(".github/workflows/w{n:02}.yml");
            write(tmp.path(), &rel, LABELLED);
            paths.push(rel);
        }
        let block = render_block(&detect(tmp.path(), Some(&paths))).unwrap();
        assert!(block.contains("more CI file(s) not shown"), "{block}");
        // The rules follow the budgeted part: bounded as a whole.
        assert!(block.len() < BUDGET + 1_800, "{} bytes", block.len());
    }

    #[test]
    fn a_link_out_of_the_project_is_never_read() {
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            write(
                outside.path(),
                "evil.yml",
                "on: push\njobs:\n  OUTSIDE_MARKER:\n    steps: []\n",
            );
            std::fs::create_dir_all(project.path().join(".github/workflows")).unwrap();
            std::os::unix::fs::symlink(
                outside.path().join("evil.yml"),
                project.path().join(".github/workflows/ci.yml"),
            )
            .unwrap();
            let found = detect(
                project.path(),
                Some(&tracked(&[".github/workflows/ci.yml"])),
            );
            assert!(found.is_empty(), "{found:?}");
        }
    }
}
