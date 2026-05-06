- **ID**: TD-20260417-projects-monolith
- **Area**: Backend
- **Severity**: Medium (most tractable of the three big api/*.rs files)

## Problem (fact)
`backend/src/api/projects.rs` is ~1819 lines mixing project CRUD, bootstrap pipeline, clone-from-URL with auth injection, git operations (status/diff/branch/commit/push/exec), PR creation, template install, and path remapping. The file's concerns are cleanly separable but currently live together.

## Impact
- Same as the other two monoliths: review friction, merge conflicts, cognitive load.
- Specifically, the bootstrap flow (a feature in active iteration — Bootstrap++, 3-stage gated validation) shares the file with unrelated git plumbing, making each commit's blast radius unnecessarily large.

## Why we can't fix now (constraint)
Less coupled than audit.rs, but still has some shared helpers (`resolve_project_path`, `resolve_github_token_from_state`, `inject_clone_auth`) used across CRUD, clone, and git handlers. A clean split needs those helpers in a `projects_helpers.rs` first so every sub-module can import them.

## Where (pointers)
- `backend/src/api/projects.rs` (~1819L)
- CRUD: `list` (32), `get` (51), `scan` (185), `create` (233), `add_folder` (287), `delete` (1083)
- Discovery: `discover_wsl_homes` (77)
- Bootstrap: `bootstrap` (425), `build_bootstrap_plus_prompt` (660), `build_bootstrap_prompt` (698)
- Clone: `inject_token_into_url` (831), `https_to_ssh` (851), `inject_clone_auth` (862), `clone_project` (948)
- Template: `install_template` (1154)
- Defaults: `set_default_skills` (1366), `set_default_profile` (1397)
- Git ops: `git_status` (1457), `git_diff` (1477), `git_branch` (1504), `git_commit` (1542), `git_push` (1579), `project_exec` (1601), `create_pr` (1646), `pr_template` (1671)
- Path: `remap_path` (1703)
- Shared helpers: `determine_parent_dir` (408), `find_common_parent` (1342), `resolve_github_token_from_state` (1432), `resolve_project_path` (1443)

## Suggested direction (non-binding)
Split into a sub-directory mirroring the existing `git_ops.rs` precedent:

```
backend/src/api/projects/
├── mod.rs          # route dispatcher, re-exports
├── crud.rs         # list, get, create, add_folder, delete, scan, discover_wsl_homes
├── bootstrap.rs    # bootstrap, build_bootstrap_prompt, build_bootstrap_plus_prompt
├── clone.rs        # clone_project, inject_clone_auth, inject_token_into_url, https_to_ssh
├── template.rs     # install_template
├── git.rs          # git_status, git_diff, git_branch, git_commit, git_push,
│                   # project_exec, create_pr, pr_template
├── defaults.rs     # set_default_skills, set_default_profile
└── helpers.rs      # determine_parent_dir, find_common_parent,
                    # resolve_project_path, resolve_github_token_from_state,
                    # remap_path
```

### Pure-helper quick win
`https_to_ssh`, `inject_token_into_url`, `find_common_parent`, `remap_path`, `build_bootstrap_plus_prompt`, `build_bootstrap_prompt` — all pure. Extract to `projects_helpers.rs` with unit tests in ~45 min. Mirrors the `disc_helpers.rs` pattern from 2026-04-17.

### Full split safety
- All handlers take `State(state): State<AppState>` — no shared file-private state.
- Route registration lives in `backend/src/lib.rs` (`build_router`); moving handlers requires updating a few `projects::fn_name` references there.
- No `#[derive(TS)]` models live in this file, so ts-rs is not affected.

## Next step
1. **Quick win (~45 min)**: extract the 6 pure helpers to `projects_helpers.rs` with unit tests. Focus on `inject_token_into_url` and `https_to_ssh` — both have tricky URL-munging logic that deserves explicit test coverage (regression targets).
2. **Full split (~1-2h)**: straightforward after the helpers are out. Lower risk than audit.rs because no shared engine/state.
