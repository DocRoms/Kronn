//! KT-1980 — a lane whose validations start no build is not warmed.
//!
//! Warming a worker lane (seed alignment, seed clone, first build) costs tens of
//! minutes and tens of GB; a task whose gates are front-end commands never uses
//! what it produced. The execution's validations are its principal's statement of
//! what the delivery will be checked with, and the worker cannot change them, so
//! they decide whether the lane is warmed:
//!
//! 1. no validation persisted: the intent is unknown, the lane is warmed;
//! 2. otherwise the lane is not warmed when no validation can start a build of a
//!    warm unit. One can when, once an `rtk` / `rtk proxy` wrapper is removed,
//!    its binary is `cargo` or `make`, or the binary of the project's declared
//!    first-build command — or when that binary is one the rule does not know
//!    (when in doubt, warm). A build binary named further on the command line
//!    (`pnpm exec cargo …`) counts too.
//!
//! Generic: nothing here names a project, a path or a command of its own.
//! See `docs/operations/worktree-build-artifacts.md`.

use serde::{Deserialize, Serialize};

/// The rule's name, as the journal and the worker read it.
pub const RULE: &str = "warm-skip rule";

/// Binaries that build a warm unit in any project.
const BUILD_BINARIES: [&str; 2] = ["cargo", "make"];

/// Binaries the rule knows: they never start a build of a warm unit on their
/// own (front-end tooling, version control, probes). Bare names only — a path
/// is a repository executable, which may do anything. A shell or a script
/// interpreter (`sh`, `python3`) is deliberately absent: it runs whatever the
/// script says.
const NON_BUILDING_BINARIES: &[&str] = &[
    "pnpm",
    "npm",
    "npx",
    "yarn",
    "bun",
    "node",
    "tsc",
    "vitest",
    "jest",
    "eslint",
    "oxlint",
    "prettier",
    "biome",
    "playwright",
    "git",
    "gh",
    "echo",
    "true",
    "false",
    "sleep",
];

/// Why a lane was not warmed: the validations read, and the binaries that would
/// have needed a warm lane. Kept on the warm-start record so a later change of
/// the gates is checked against the same rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WarmSkip {
    /// The sentence the journal and the worker read: the rule and the
    /// validations it read.
    pub reason: String,
    /// The validation commands the decision read, in order.
    pub validations: Vec<String>,
    /// The binaries that start a build of a warm unit for this project.
    pub build_binaries: Vec<String>,
}

impl WarmSkip {
    /// The first of `validations` that would need the warm lane this one is
    /// not, with why; `None` when every one of them runs on a cold lane.
    pub fn refusal_for(&self, validations: &[String]) -> Option<String> {
        validations
            .iter()
            .find_map(|command| warm_reason(command, &self.build_binaries))
    }

    /// What the worker reads about its Rust target in its toolchain record (and
    /// so in its brief): it is cold, why, and what a build would cost.
    pub fn cold_target_note(&self) -> String {
        format!(
            "THE LANE'S RUST TARGET IS COLD — {}. A cargo build or test here compiles \
             everything from nothing (tens of minutes): start one only if the task \
             really needs it",
            self.reason
        )
    }
}

/// What the rule decided for a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarmDecision {
    /// Warm it as before, and why.
    Warm(String),
    /// Do not warm it.
    Skip(WarmSkip),
}

/// The rule, over the execution's validation commands and the project's declared
/// first-build command (a shell command line, when there is one).
pub fn decide(validations: &[String], declared_first_build: Option<&str>) -> WarmDecision {
    if validations.is_empty() {
        return WarmDecision::Warm(
            "the execution has no validation: what it will build is unknown".into(),
        );
    }
    let build_binaries = build_binaries(declared_first_build);
    if let Some(reason) = validations
        .iter()
        .find_map(|command| warm_reason(command, &build_binaries))
    {
        return WarmDecision::Warm(reason);
    }
    let listed = validations
        .iter()
        .map(|command| format!("`{}`", command.trim()))
        .collect::<Vec<_>>()
        .join(", ");
    let binaries = build_binaries
        .iter()
        .map(|binary| format!("`{binary}`"))
        .collect::<Vec<_>>()
        .join(", ");
    WarmDecision::Skip(WarmSkip {
        reason: format!(
            "not warmed by the {RULE}: none of the execution's validations ({listed}) can \
             start a build of a warm unit (none runs {binaries}, once an rtk wrapper is \
             removed), so the lane's Rust target is cold"
        ),
        validations: validations.to_vec(),
        build_binaries,
    })
}

/// `cargo`, `make` and the binaries of the project's declared first-build
/// command, in that order and without repeats.
pub fn build_binaries(declared_first_build: Option<&str>) -> Vec<String> {
    let mut binaries: Vec<String> = BUILD_BINARIES.iter().map(|b| b.to_string()).collect();
    for binary in declared_command_binaries(declared_first_build.unwrap_or_default()) {
        if !binaries.contains(&binary) {
            binaries.push(binary);
        }
    }
    binaries
}

/// Why `command` needs a warm lane, or `None` when the rule knows it does not.
pub fn warm_reason(command: &str, build_binaries: &[String]) -> Option<String> {
    let words = unwrapped(command.split_whitespace().collect());
    let Some(&first) = words.first() else {
        return Some(format!(
            "the validation `{}` names no binary: when in doubt, the lane is warmed",
            command.trim()
        ));
    };
    let binary = base_name(first);
    if build_binaries.iter().any(|build| build == binary) {
        return Some(format!(
            "the validation `{}` runs `{binary}`, which can start a build of a warm unit",
            command.trim()
        ));
    }
    if let Some(named) = words[1..]
        .iter()
        .map(|word| base_name(word))
        .find(|word| build_binaries.iter().any(|build| build == word))
    {
        return Some(format!(
            "the validation `{}` names `{named}`, which can start a build of a warm unit",
            command.trim()
        ));
    }
    let known =
        !first.contains('/') && !first.contains('\\') && NON_BUILDING_BINARIES.contains(&first);
    if !known {
        return Some(format!(
            "the validation `{}` runs `{first}`, a binary the {RULE} does not know: when in \
             doubt, the lane is warmed",
            command.trim()
        ));
    }
    None
}

/// The words of a command line once an `rtk` or `rtk proxy` wrapper is removed.
fn unwrapped(mut words: Vec<&str>) -> Vec<&str> {
    if words.first().map(|word| base_name(word)) == Some("rtk") {
        words.remove(0);
        if words.first() == Some(&"proxy") {
            words.remove(0);
        }
    }
    words
}

fn base_name(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

/// The binary of each simple command of a shell command line: its first word
/// after `VAR=value` prefixes and an rtk wrapper. `cd` changes directory only.
fn declared_command_binaries(command: &str) -> Vec<String> {
    let mut binaries = Vec::new();
    for segment in command.split(['&', '|', ';', '\n']) {
        let words: Vec<&str> = segment
            .split_whitespace()
            .skip_while(|word| is_assignment(word))
            .collect();
        let words = unwrapped(words);
        let Some(first) = words.first() else {
            continue;
        };
        let binary = base_name(first);
        if binary != "cd" && !binaries.iter().any(|known: &String| known == binary) {
            binaries.push(binary.to_string());
        }
    }
    binaries
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

#[cfg(test)]
#[path = "task_worker_warm_skip_test.rs"]
mod task_worker_warm_skip_test;
