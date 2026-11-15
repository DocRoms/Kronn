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
        // `#[cfg(test)]` then `mod`, with a `#[path = ".."]` line allowed between.
        let next_is_mod = lines.peek().is_some_and(|next| {
            next.starts_with("mod ") || next.starts_with("pub mod ") || next.starts_with("#[path")
        });
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

/// What starts a git process that is not the lane builder.
const SPAWNS: [&str; 6] = [
    "git_cmd()",
    "async_git_cmd()",
    "project_git_cmd()",
    "Command::new(\"git\")",
    "sync_cmd(\"git\"",
    "async_cmd(\"git\"",
];

/// The lines of `text` that start a plain git command without saying why.
fn plain_git(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") {
            continue;
        }
        let spawns = SPAWNS.iter().any(|spawn| code.contains(spawn));
        if !spawns || code.starts_with("pub fn ") || code.starts_with("fn ") {
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
                "run_git_in_index",
                "uncommitted_content_over",
                "write_capped_git_patch",
                "worktree_index_path",
                "review_worktree_holds_something",
                "write_uncommitted_patches",
                "committed_file_changes",
                "file_at_revision",
                "committer_identity",
                // Called with a worker's worktree (`resolve_commit(child, "HEAD")`),
                // and with a submodule of it by the re-provisioning.
                "resolve_commit",
                "is_ancestor",
                "task_submodule_head",
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

/// Every production `.rs` file under `src`, by path relative to it.
fn production_files() -> Vec<String> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    walk(&path, root, found);
                }
            } else if name.ends_with(".rs")
                && !name.ends_with("_test.rs")
                && !name.ends_with("_tests.rs")
                && name != "tests.rs"
            {
                let relative = path.strip_prefix(root).unwrap_or(&path);
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    walk(&root, &root, &mut found);
    found.sort();
    found
}

/// Files with plain git commands that do not run on a worker's worktree, and how
/// many each may keep. This ratchet counts DIRECT spawns only: a plain helper that
/// is handed a worktree path (`resolve_commit(child, "HEAD")` was one) is not seen
/// here, so a helper that can be called with a worker's path belongs in the
/// function list of `the_functions_that_run_git_on_a_worktree_...` above. A file that is not listed keeps none; a listed file may
/// not gain one. Each entry claims that its commands run in the REGISTERED
/// project checkout, the owner's own checkout, a repository Kronn creates or a
/// throw-away one, never in a directory a worker writes. A command that does run
/// in a worker's worktree goes through `lane_git`, or says why with
/// `lane-git-allow:` on its line.
const PLAIN_GIT_ALLOWED: &[(&str, usize)] = &[
    ("agents/read_only_repos.rs", 3),
    ("agents/runner.rs", 4),
    ("api/ai_docs.rs", 1),
    ("api/audit/step_remit.rs", 1),
    ("api/orchestration.rs", 9),
    ("api/projects/bootstrap.rs", 1),
    ("api/projects/clone.rs", 3),
    ("api/projects/crud.rs", 2),
    ("api/projects/git.rs", 2),
    ("api/projects/template_merge.rs", 1),
    ("api/projects/used_skills.rs", 1),
    ("core/anti_halluc.rs", 2),
    ("core/audit_baseline.rs", 1),
    ("core/audit_detectors.rs", 1),
    ("core/audit_run_state.rs", 1),
    ("core/briefing_docs.rs", 1),
    ("core/checksums.rs", 2),
    ("core/commit_facts.rs", 1),
    ("core/doc_relevance.rs", 2),
    ("core/docs_migration.rs", 3),
    ("core/kronn_state.rs", 1),
    ("core/native_files.rs", 1),
    ("core/repo_facts.rs", 1),
    ("core/repository_resources.rs", 4),
    ("core/scanner.rs", 2),
    ("core/task_worker_lane_seeds.rs", 1),
    ("core/upstream_mirror.rs", 5),
    ("core/worktree.rs", 45),
    ("workflows/batch_step.rs", 1),
    ("workflows/workspace.rs", 8),
];

#[test]
fn every_production_git_command_is_the_lane_builder_or_explained() {
    let mut surprises = Vec::new();
    let mut seen = Vec::new();
    for file in production_files() {
        if file == "core/cmd.rs" {
            continue; // defines the plain builders
        }
        let text = source(&file);
        let count = plain_git(production(&text)).len();
        let allowed = PLAIN_GIT_ALLOWED
            .iter()
            .find(|(name, _)| *name == file)
            .map_or(0, |(_, allowed)| *allowed);
        if count == 0 && allowed == 0 {
            continue;
        }
        seen.push(format!("    (\"{file}\", {count}),"));
        // A ratchet: a command that left (moved to the lane builder) lowers the
        // allowance, so a later plain one cannot hide in the slack.
        if count != allowed {
            surprises.push(format!(
                "{file}: {count} plain git command(s), {allowed} allowed"
            ));
        }
    }
    assert!(
        surprises.is_empty(),
        "a plain git command was added where none was expected. If it runs in a worker's \
         worktree, build it with crate::core::lane_git::git; if it runs in the registered or \
         the owner's checkout, say so with `// lane-git-allow: <why>` on its line.\n\
         {surprises:#?}\ncurrent counts:\n{}",
        seen.join("\n")
    );
}

#[test]
fn the_guard_sees_a_plain_command_and_an_explained_one() {
    let sample = "fn a() {\n    let c = git_cmd();\n}\nfn b() {\n    // lane-git-allow: the owner's checkout\n    let c = git_cmd();\n}\n";
    assert_eq!(plain_git(function(sample, "a")).len(), 1);
    assert!(plain_git(function(sample, "b")).is_empty());
}
