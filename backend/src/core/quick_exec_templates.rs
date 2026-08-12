//! Quick Exec templates — KT-195.
//!
//! The named catalogue of mechanical operations. A caller picks an id and, at
//! most, supplies one argument; the binary, the flags and the summariser are
//! fixed here. That is what makes a Quick Exec reviewable: the interesting part
//! of the command line lives in this file, under version control, instead of
//! being assembled per call.
//!
//! It is also the security consequence of allowlisting powerful binaries. `gh`
//! and `git` can write, so the templates pin the subcommand and the argument
//! shape — a caller can name a PR number, not add `-X DELETE`.

use super::quick_exec::{QuickExecSpec, Rejection, Summariser, ALLOWED_BINARIES};
use std::path::Path;

/// What a template accepts after its fixed arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentShape {
    /// Nothing. Any extra argument is refused.
    None,
    /// Digits only — a PR number, a workflow run id.
    Numeric,
    /// A test path or filter: module separators, dots, dashes, stars.
    TestFilter,
    /// A repository-relative path.
    PathLike,
    /// A branch, tag or SHA, including the `a...b` range form.
    GitRef,
}

impl ArgumentShape {
    /// Check one argument.
    ///
    /// Every shape refuses a leading `-`. That single rule is what keeps a
    /// template from being turned into another command: without it, a caller
    /// appends `--upload-file` or `-X DELETE` and the fixed subcommand stops
    /// deciding what happens.
    fn check(self, argument: &str) -> Result<(), Rejection> {
        if self == Self::None {
            return Err(Rejection("this template takes no arguments".to_string()));
        }
        if argument.is_empty() {
            return Err(Rejection("an empty argument was given".to_string()));
        }
        if argument.starts_with('-') {
            return Err(Rejection(format!(
                "`{argument}` looks like a flag — a template argument is a value, never an option"
            )));
        }
        if argument.contains('\0') {
            return Err(Rejection("an argument contains a NUL byte".to_string()));
        }
        let ok = match self {
            Self::None => false,
            Self::Numeric => argument.chars().all(|c| c.is_ascii_digit()),
            Self::TestFilter => argument.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.' | '*' | '/')
            }),
            // No traversal: the cwd is already bounded, and a `..` argument is how
            // a command is pointed back out of it.
            Self::PathLike => !argument.contains("..") && argument.chars().all(|c| !c.is_control()),
            Self::GitRef => {
                !argument.contains("..=")
                    && argument
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
            }
        };
        if ok {
            Ok(())
        } else {
            Err(Rejection(format!(
                "`{argument}` does not match the shape this template accepts ({self:?})"
            )))
        }
    }
}

/// One mechanical operation, fully specified except for its argument.
#[derive(Debug, Clone, Copy)]
pub struct QuickExecTemplate {
    pub id: &'static str,
    /// What a passing run establishes. Written for the reader of a result, who
    /// needs to know what the green actually covers.
    pub establishes: &'static str,
    pub binary: &'static str,
    pub base_argv: &'static [&'static str],
    pub summariser: Summariser,
    pub timeout_secs: u64,
    pub argument: ArgumentShape,
    /// How many arguments may follow `base_argv`.
    pub max_arguments: usize,
}

/// The catalogue. Grouped by the families the ticket names: targeted tests, the
/// full gate, build/typecheck/lint, forge and CI collection, token diagnostics.
pub const TEMPLATES: &[QuickExecTemplate] = &[
    // ── targeted tests ──
    QuickExecTemplate {
        id: "backend-tests-filtered",
        establishes: "the named backend tests pass; says nothing about the rest of the suite",
        binary: "cargo",
        base_argv: &["test", "--lib"],
        summariser: Summariser::CargoTest,
        timeout_secs: 900,
        argument: ArgumentShape::TestFilter,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "frontend-tests-filtered",
        establishes: "the named frontend tests pass; says nothing about the rest of the suite",
        binary: "vitest",
        base_argv: &["run", "--reporter=basic"],
        summariser: Summariser::Vitest,
        timeout_secs: 900,
        argument: ArgumentShape::PathLike,
        max_arguments: 1,
    },
    // ── the full gate ──
    QuickExecTemplate {
        id: "backend-tests-full",
        establishes: "the whole backend lib suite passes",
        binary: "cargo",
        base_argv: &["test", "--lib"],
        summariser: Summariser::CargoTest,
        timeout_secs: 1_800,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "frontend-tests-full",
        establishes: "the whole frontend suite passes",
        binary: "vitest",
        base_argv: &["run", "--reporter=basic"],
        summariser: Summariser::Vitest,
        timeout_secs: 1_800,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    // ── build, typecheck, lint ──
    QuickExecTemplate {
        id: "backend-clippy",
        establishes: "clippy is clean at deny-warnings across all targets",
        binary: "cargo",
        base_argv: &["clippy", "--all-targets", "--", "-D", "warnings"],
        summariser: Summariser::Clippy,
        timeout_secs: 1_800,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "backend-build",
        establishes: "the backend compiles in debug; not that its tests or lints pass",
        binary: "cargo",
        base_argv: &["build"],
        summariser: Summariser::Clippy,
        timeout_secs: 1_800,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "frontend-typecheck",
        establishes: "tsc reports no type error",
        binary: "tsc",
        base_argv: &["--noEmit"],
        summariser: Summariser::Tsc,
        timeout_secs: 900,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "frontend-lint",
        establishes: "eslint reports no violation",
        binary: "eslint",
        base_argv: &["."],
        summariser: Summariser::Generic,
        timeout_secs: 900,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    // ── forge and CI collection ──
    QuickExecTemplate {
        id: "pr-checks",
        establishes: "the current state of a pull request's checks",
        binary: "gh",
        base_argv: &["pr", "checks"],
        summariser: Summariser::Generic,
        timeout_secs: 120,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "ci-failed-logs",
        establishes: "the failing steps of a workflow run",
        binary: "gh",
        base_argv: &["run", "view", "--log-failed"],
        summariser: Summariser::Generic,
        timeout_secs: 300,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "pr-head-sha",
        establishes: "the head SHA a review is being established against",
        binary: "gh",
        base_argv: &["pr", "view", "--json", "headRefOid", "-q", ".headRefOid"],
        summariser: Summariser::Generic,
        timeout_secs: 120,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    // ── deterministic review collection ──
    //
    // The set below fetches everything a review needs about a pull request
    // WITHOUT an agent: metadata, changed files, both comment streams, checks and
    // reactions. Each writes its full payload to an artifact and reports only how
    // much arrived, so a page of JSON never enters a context merely because
    // something had to retrieve it.
    //
    // `{owner}` and `{repo}` are resolved by `gh` from the working directory;
    // `{}` is the slot this module fills with the validated PR number.
    QuickExecTemplate {
        id: "pr-metadata",
        establishes: "a pull request's state, head, base and review decision",
        binary: "gh",
        base_argv: &[
            "pr",
            "view",
            "{}",
            "--json",
            "number,title,state,isDraft,mergeable,headRefOid,baseRefName,reviewDecision",
        ],
        summariser: Summariser::Collected,
        timeout_secs: 120,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "pr-changed-files",
        establishes: "which files a pull request touches, as a plain list",
        binary: "gh",
        base_argv: &["pr", "diff", "{}", "--name-only"],
        summariser: Summariser::Collected,
        timeout_secs: 300,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "pr-review-comments",
        establishes: "the inline review comments on a pull request, with their file and line",
        binary: "gh",
        base_argv: &[
            "api",
            "repos/{owner}/{repo}/pulls/{}/comments",
            "--paginate",
            "-q",
            ".[] | [.id, .path, (.line // .original_line // 0), .user.login, (.body | gsub(\"\\n\"; \" \"))] | @tsv",
        ],
        summariser: Summariser::Collected,
        timeout_secs: 300,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "pr-issue-comments",
        establishes: "the conversation-level comments on a pull request",
        binary: "gh",
        base_argv: &[
            "api",
            "repos/{owner}/{repo}/issues/{}/comments",
            "--paginate",
            "-q",
            ".[] | [.id, .user.login, (.body | gsub(\"\\n\"; \" \"))] | @tsv",
        ],
        summariser: Summariser::Collected,
        timeout_secs: 300,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    QuickExecTemplate {
        id: "pr-reactions",
        establishes: "the reactions on a pull request, which is how an answered comment is acknowledged",
        binary: "gh",
        base_argv: &[
            "api",
            "repos/{owner}/{repo}/issues/{}/reactions",
            "--paginate",
            "-q",
            ".[] | [.id, .user.login, .content] | @tsv",
        ],
        summariser: Summariser::Collected,
        timeout_secs: 120,
        argument: ArgumentShape::Numeric,
        max_arguments: 1,
    },
    // The delta a re-review replays. Fetching it here rather than having an agent
    // read the diff is the point of the ledger's changed-paths input.
    QuickExecTemplate {
        id: "changed-paths",
        establishes: "which files a range touched, as a plain list",
        binary: "git",
        base_argv: &["diff", "--name-only"],
        summariser: Summariser::Generic,
        timeout_secs: 120,
        argument: ArgumentShape::GitRef,
        max_arguments: 1,
    },
    // ── token diagnostics ──
    //
    // The five sources of RTK adoption (KT-197). Each is a command nobody runs
    // unprompted, which is why adoption stayed invisible; `core::rtk_state` folds
    // their outputs into one bounded panel.
    QuickExecTemplate {
        id: "rtk-gain",
        establishes: "the token savings RTK has recorded, and over how many commands",
        binary: "rtk",
        base_argv: &["gain"],
        summariser: Summariser::Generic,
        timeout_secs: 120,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "rtk-session",
        establishes: "what share of a session's commands went through RTK",
        binary: "rtk",
        base_argv: &["session"],
        summariser: Summariser::Generic,
        timeout_secs: 120,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "rtk-discover",
        establishes: "commands that could have gone through RTK and did not",
        binary: "rtk",
        // Scans recorded CLI history, so it gets more room than the others.
        base_argv: &["discover"],
        summariser: Summariser::Collected,
        timeout_secs: 600,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "rtk-hook-audit",
        establishes: "what the RTK hook rewrote, when auditing is enabled",
        binary: "rtk",
        base_argv: &["hook-audit"],
        summariser: Summariser::Generic,
        timeout_secs: 120,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
    QuickExecTemplate {
        id: "rtk-cc-economics",
        establishes: "spend paired with savings; needs a ccusage whose schema rtk still reads",
        binary: "rtk",
        base_argv: &["cc-economics"],
        // Observed at 17s on a machine where it had to fetch ccusage first.
        timeout_secs: 300,
        summariser: Summariser::Generic,
        argument: ArgumentShape::None,
        max_arguments: 0,
    },
];

pub fn template(id: &str) -> Option<&'static QuickExecTemplate> {
    TEMPLATES.iter().find(|candidate| candidate.id == id)
}

/// Build a spec from a template id and its arguments.
///
/// The result still goes through `quick_exec::validate` — this function narrows
/// what may be asked for, it does not replace the boundary.
pub fn spec_from_template(
    id: &str,
    cwd: &Path,
    arguments: &[String],
) -> Result<QuickExecSpec, Rejection> {
    let template = template(id).ok_or_else(|| Rejection(format!("no template named `{id}`")))?;
    if arguments.len() > template.max_arguments {
        return Err(Rejection(format!(
            "`{id}` accepts at most {} argument(s), {} given",
            template.max_arguments,
            arguments.len()
        )));
    }
    for argument in arguments {
        template.argument.check(argument)?;
    }

    // A `{}` in a fixed argument is a slot, for the cases where the value belongs
    // INSIDE an argument rather than after it — an API path carrying a PR number.
    // Substitution is safe only because the value passed `check` first, and it is
    // still literal: nothing re-splits the result.
    let slots = template
        .base_argv
        .iter()
        .filter(|part| part.contains("{}"))
        .count();
    if slots > 0 && arguments.len() < slots {
        return Err(Rejection(format!(
            "`{id}` has {slots} slot(s) to fill and got {} argument(s)",
            arguments.len()
        )));
    }
    let mut argv: Vec<String> = Vec::with_capacity(template.base_argv.len() + arguments.len());
    let mut next = arguments.iter();
    for part in template.base_argv {
        if part.contains("{}") {
            let value = next.next().expect("slot count checked above");
            argv.push(part.replace("{}", value));
        } else {
            argv.push(part.to_string());
        }
    }
    // Arguments not consumed by a slot are appended, still after everything fixed.
    argv.extend(next.cloned());

    Ok(QuickExecSpec {
        binary: template.binary.to_string(),
        argv,
        cwd: cwd.to_path_buf(),
        timeout_secs: Some(template.timeout_secs),
        stdin: None,
        summariser: template.summariser,
    })
}

/// Structural check: every template names an allowlisted binary.
///
/// Exposed rather than test-only so a startup path can assert it — a template
/// pointing at a refused binary is a permanent failure, not a runtime surprise.
pub fn unspawnable_templates() -> Vec<&'static str> {
    TEMPLATES
        .iter()
        .filter(|t| !ALLOWED_BINARIES.contains(&t.binary))
        .map(|t| t.id)
        .collect()
}

/// The templates a review collects from, in the order a payload wants them.
///
/// Named here rather than at the call site so "what a deterministic review
/// fetches" is one reviewable list, and so a missing collector is a compile-time
/// concern instead of a silently thinner payload.
pub const REVIEW_COLLECTORS: &[&str] = &[
    "pr-metadata",
    "pr-changed-files",
    "pr-review-comments",
    "pr-issue-comments",
    "pr-checks",
    "pr-reactions",
];

#[cfg(test)]
#[path = "quick_exec_templates_test.rs"]
mod quick_exec_templates_test;
