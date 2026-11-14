//! KT-1745 — a guard, not a behaviour test: every function that runs `git` on a
//! worker's worktree builds its command through `lane_git`, never `git_cmd()`.
//!
//! A `git_cmd()` that is not the lane builder lets git discover the repository
//! from the worktree (so from the `.git` file the worker writes) and run the
//! hooks, filters and programs that repository names, as the host user. A line
//! that is legitimate (the registered checkout, the user's own checkout) carries
//! a `lane-git-allow:` comment, on its line or the one above, saying why.

use std::path::Path;

fn source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The production part of a file: what precedes its first test module (a
/// `#[cfg(..test..)]` line directly followed by `mod`; a `#[cfg(test)]` on a
/// constant or a statement does not end it).
fn production(text: &str) -> &str {
    let mut offset = 0;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let is_test_cfg = line.starts_with("#[cfg(") && line.contains("test");
        let next_is_mod = lines
            .peek()
            .is_some_and(|next| next.starts_with("mod ") || next.starts_with("pub mod "));
        if is_test_cfg && next_is_mod {
            return &text[..offset];
        }
        offset += line.len() + 1;
    }
    text
}

/// The text of function `name`, from its signature to the closing brace at the
/// start of a line.
fn function<'a>(text: &'a str, name: &str) -> &'a str {
    let start = [
        "pub fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "fn ",
        "async fn ",
        "pub async fn ",
    ]
    .iter()
    .flat_map(|prefix| {
        let signature = format!("{prefix}{name}(");
        text.match_indices(&signature)
            .filter(|(at, _)| *at == 0 || text.as_bytes()[at - 1] == b'\n')
            .map(|(at, _)| at)
            .collect::<Vec<_>>()
    })
    .min()
    .unwrap_or_else(|| panic!("function `{name}` not found: the guard list is stale"));
    let rest = &text[start..];
    let end = rest.find("\n}\n").map_or(rest.len(), |end| end + 3);
    &rest[..end]
}

/// The lines of `text` that start a plain git command without saying why.
fn plain_git(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let spawns = ["git_cmd()", "async_git_cmd()", "project_git_cmd()"]
            .iter()
            .any(|spawn| code.contains(spawn));
        if !spawns {
            continue;
        }
        let allowed = line.contains("lane-git-allow:")
            || (index > 0 && lines[index - 1].contains("lane-git-allow:"));
        if !allowed {
            found.push(format!("{}: {}", index + 1, code));
        }
    }
    found
}

#[test]
fn files_that_only_run_git_on_a_worktree_never_use_a_plain_git_command() {
    for file in [
        "core/lane_git.rs",
        "core/commit_hooks.rs",
        "core/docs_citations/merge.rs",
        "core/docs_citations/diff_map.rs",
        "core/project_profile.rs",
        "api/worker_time_limit.rs",
        "api/agent_workspace_tools.rs",
    ] {
        let text = source(file);
        let offenders = plain_git(production(&text));
        assert!(
            offenders.is_empty(),
            "{file} runs a plain git command on what may be a worker's worktree; build it with \
             crate::core::lane_git::git (or say why with `lane-git-allow:`): {offenders:#?}"
        );
    }
}

#[test]
fn the_functions_that_run_git_on_a_worktree_never_use_a_plain_git_command() {
    let lists: [(&str, &[&str]); 4] = [
        ("core/docs_write_filter.rs", &["document_paths"]),
        ("workflows/delegate_subtasks_step.rs", &["git_diff"]),
        (
            "core/worktree.rs",
            &[
                "build_candidate_with_hooks",
                "settle_branch_docs",
                "abort_merge",
                "merge_subject",
                "git_head",
                "verify_worktree_head",
                "worktree_dirty_files",
                "committed_file_changes",
                "file_at_revision",
                "committer_identity",
            ],
        ),
        (
            "api/git_ops.rs",
            &[
                "stage_explicit_paths",
                "git_in",
                "ensure_git_identity",
                "run_commit_command",
                "commit_explicit_paths",
                "merge_in_progress",
                "git_path_list",
                "paths_changed_by_merged_side",
                "run_git_merge_commit_for_worker",
                "authenticated_push_command",
                "run_git_push",
            ],
        ),
    ];
    for (file, functions) in lists {
        let text = source(file);
        let text = production(&text);
        for name in functions {
            let offenders = plain_git(function(text, name));
            assert!(
                offenders.is_empty(),
                "{file}: `{name}` runs git on a worker's worktree with a plain git command; use \
                 crate::core::lane_git::git: {offenders:#?}"
            );
        }
    }
}

#[test]
fn the_guard_sees_a_plain_command_and_an_explained_one() {
    let sample = "fn a() {\n    let c = git_cmd();\n}\nfn b() {\n    // lane-git-allow: the owner's checkout\n    let c = git_cmd();\n}\n";
    assert_eq!(plain_git(function(sample, "a")).len(), 1);
    assert!(plain_git(function(sample, "b")).is_empty());
}
