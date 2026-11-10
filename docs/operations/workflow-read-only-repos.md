# Workflow Agent read-only repositories

An Agent step can declare `read_only_repos`, an array of absolute local Git
repository paths. The wizard exposes one path per line in the step's advanced
settings. An empty or absent list retains the existing launch policy. This is
an explicit per-step declaration; listing a companion project in documentation
does not automatically grant access.
[src: file: backend/src/models/workflows.rs:1050-1054#read_only_repos]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4577-4591#read_only_repos]

```json
{
  "name": "inspect",
  "step_type": {"type": "Agent"},
  "agent": "ClaudeCode",
  "prompt_template": "Read the linked repository's docs/AGENTS.md, then inspect its API.",
  "read_only_repos": ["/absolute/path/to/api-repository"]
}
```

The launch resolves host paths, canonicalizes symlinks, deduplicates paths and
includes external Git metadata for linked worktrees. Missing/non-Git paths,
permission-pattern characters, overlap with the working directory, unsupported
agents and native Windows launches are rejected before dispatch. The list does
not accept workflow template expressions.
[src: file: backend/src/agents/read_only_repos.rs:672-776#resolve_with_dirs]
[src: file: backend/src/agents/read_only_repos.rs:701-731#canonical_directory]
[src: file: backend/src/api/workflow_step_schema.json:16#read_only_repos]

Claude receives `--add-dir` together with `Edit` deny rules and sandbox
`denyWrite` entries. Its read block remains enabled, the OS sandbox is
mandatory, unsandboxed commands are disabled, and inherited user/project
settings are excluded. Codex receives an invocation-local permissions profile
extending `:workspace`, with explicit `read` entries and approval policy `never`.
It keeps the standard broad filesystem read access; the declared directories
are never added as writable roots, including when they live under a temporary
directory. `--strict-config` rejects unsupported Codex settings.
[src: file: backend/src/agents/read_only_repos.rs:990-1107#args]
[Claude permission rules](https://code.claude.com/docs/en/permissions)
[Claude sandbox](https://code.claude.com/docs/en/sandboxing)
[Codex permissions](https://learn.chatgpt.com/docs/permissions)

Claude's effective settings can still list a linked repository as writable due
to `--add-dir`; `denyWrite` and `Edit` deny rules take precedence. The principal's
host probe confirmed that Python, Write and shell redirection were all refused.
[src: user: 2026-09-28: KT-806 request_changes review of the host probe]

Both the default [ACP adapters](acp-adapters.md) and the direct CLI route use
this policy, overriding `full_access`. The Claude process cannot inherit the
container sandbox-bypass marker when a mandatory sandbox policy is present.
[src: file: backend/src/agents/runner.rs:4691-4695#read_only_repos]
[src: file: backend/src/agents/runner.rs:4722-4724#read_only_repos]
[src: file: backend/src/agents/read_only_repos.rs:972-979#apply]
[src: file: backend/src/agents/runner.rs:14653-14658#sandbox_required]
[src: file: backend/src/agents/runner.rs:14877-14887#CLAUDE_CODE_BUBBLEWRAP]
[src: file: backend/src/acp/codex_adapter.rs:464-478#worker_args]

All Codex `exec` launches, with or without `--strict-config`, receive one
`-c projects={...}` inline table containing the working directory and, when
applicable, its `KRONN_HOST_HOME` alias for `/host-home`. The same launcher
handles direct CLI, adapter, worker and resumed turns. Path keys are serialized
as TOML; identical aliases are deduplicated.
[src: file: backend/src/agents/runner.rs:14590-14609#codex_project_trust_override]
[src: file: backend/src/agents/runner.rs:14666-14680#is_codex]

The principal's 2026-09-28 check found that Codex 0.156.1 rejects the previous
dotted `projects."<path>".trust_level` override under `--strict-config`, even
with a canonical path. The inline table passed configuration loading in both
strict and non-strict modes; the intentionally invalid model then failed at
the API. This verifies configuration loading, not sandbox enforcement.
[src: user: 2026-09-28: KT-806 request_changes review of bf00da66]
[Codex CLI override values use TOML](https://learn.chatgpt.com/docs/config-file/config-advanced#one-off-overrides-from-the-cli)

Hypothesis (unverified): non-strict Codex may have silently ignored the old
dotted path override, so earlier launches may never have applied that trust
setting. The principal's checks do not establish this behavior.
[src: user: 2026-09-28: KT-806 request_changes review of bf00da66]

Workflow prompts state that repository paths are locations, not permission
grants, and list the repositories declared by the current step. This corrects
the runtime guidance even when companion context mentions other checkouts.
It does not rewrite separately stored autoCode Quick Prompts or workflows.
[src: file: backend/src/workflows/steps.rs:173-184#read_only_repos]

I recommend replacing the stored autoCode assertion that the main checkout is
readable with this wording when that template is available:

> A project's `path` identifies its main checkout; it does not grant filesystem
> access. Read linked repositories only when the Agent step explicitly declares
> them in `read_only_repos`. Those repositories, including their Git metadata,
> are read-only. If a read is denied, report the limitation; do not claim to have
> inspected the checkout or attempt to bypass the restriction.

## Room checkouts, read-only and writable (KT-1406, KT-1491)

A discussion's native Claude Code agent gets the projects a human granted to the
room (`discussion_project_grants`) through the same policy, built per launch by
`ReadOnlyRepos::resolve_room`: a `read` or `plan` grant opens the checkout
read-only as above; a `code` grant opens it writable (`--add-dir`, the checkout in
the sandbox's `allowWrite`, no `Edit` deny over it). Workflows never get a `code`
root: a run is given no checkout, and `code` is refused on a workflow scope. The
two launch routes (the direct CLI and the ACP adapter) build their arguments from
the same policy at every launch, so a revoke applies from the next turn; revoking
a `code` grant (or lowering the trust level that gave it) also stops the room's
running turn, which still holds the checkout open.
[src: file: backend/src/agents/read_only_repos.rs:785-841#resolve_room]
[src: file: backend/src/api/discussions/messaging.rs:1556-1585#stop_running_turn]

A writable root never opens what a later program runs or what is secret, whether
or not the path exists when the turn starts:

- **What runs later**, refused as `Edit`/`Write` rules and sandbox `denyWrite`, at
  the top and at any depth: `.git` (hooks, `core.fsmonitor`, `core.sshCommand`),
  `.envrc`, `.vscode`, the hook managers' files (`.husky`, `.githooks`,
  `lefthook*.yml`, `.lefthook`) and the agent CLIs' own files
  (`api::audit::step_remit::KRONN_OWNED`: `.claude`, `.cursor`, `.opencode`,
  `opencode.json`, `.mcp.json`, the instruction files). `core.hooksPath` is read at
  the launch (`git config`, which runs nothing): a hooks directory inside the
  checkout (husky v9's `.husky/_`, a custom path) is denied too, with the path as
  configured, where it really lies, and every symbolic link on the way (`hooks ->
  .config/hooks`, `tools -> scripts` for `tools/hooks`), so the agent cannot replace
  a link by a directory of its own. A `core.hooksPath` equal to the checkout root
  does not fail the room: the `code` grant is not opened, the checkout is opened
  read-only, and the agent is told why. The common Git directory of a linked
  worktree lies outside its checkout, so it is not in `allowWrite` and needs no
  rule, except when the granted project is a linked worktree of the room's own
  repository: its common directory is then `<work_dir>/.git`, writable with the
  working directory. Kronn's own
  `git worktree add` and `rev-parse` on a project run with
  `core.hooksPath=/dev/null` and `core.fsmonitor=false` (`core::cmd::project_git_cmd`).
  Kronn's commits, merges and pushes still run the repository's hooks, which is why
  `.git` and the hooks are closed. A consequence the agent is told: it cannot
  `git add`, `git commit` or `git stash` in a `code` root; the human commits.
- **Secrets**, refused as `Read`/`Edit`/`Write` rules by name at any depth
  (`.env*`, `*.pem`, `*.key`, `.npmrc`, `.netrc`, `credentials.json`, `.ssh/`,
  `.aws/`, `.kronn/backups/`, the Kronn vault file names) and in the sandbox's
  `denyRead` and `denyWrite`: the fixed credential paths literally, whether or not
  they exist (Seatbelt takes a path that is not there yet), the names as patterns
  (honoured where the sandbox supports patterns), and what a bounded scan finds now
  (depth 8, 20 000 entries, no link followed) as literal paths. The complete inline
  `--settings` document has one 96 KiB budget shared by every root and already
  includes the vault rules. Required tool rules and sandbox patterns come first.
  Existing secrets no pattern can name (`is_secret_file` ignores letter case:
  `PROD.PEM`, `Deploy.Key`) come next and are never dropped: their directories,
  then parents, are denied until they fit. Pattern-covered literal paths use only
  the remaining bytes. When absolute mandatory rules exceed the budget, Kronn uses
  the shorter working-directory-relative form Claude documents for permission and
  sandbox paths; if the mandatory compact form still cannot fit, the launch is
  refused before the OS can fail it with `E2BIG`. A secret with an unpatterned name
  created after the launch is not covered. The scan
  runs once at the launch, off the async executor (`spawn_blocking`). A versioned
  template such as `.env.example` is refused with them: a deny rule has no
  exception. The same read denies apply to the `read` and `plan` roots of a room,
  so a lower level never reads what `code` cannot; workflow `read_only_repos` are
  unchanged.
- **The Kronn data directory** (and the backup directory): a checkout that holds it
  (a dotfiles repository registered at `$HOME`) is never opened for editing. The
  directories are in `denyWrite` of a launch only when the vault protection is on
  (`core::vault_guard`); the overlap refusal above holds either way.

Full-access agents (Codex with full access, Gemini, OpenCode, Kiro…) ignore a
project's scope: nothing is opened or closed for them, a `code` grant is not
enforced, and their context says so. Codex with its default sandbox is told the
grant is opened for Claude Code only.

## Run artifacts directory

Each run gets its own directory, created by Kronn outside every checkout and
exposed as `{{run.artifacts_dir}}` (KT-910). Exec steps write files there. A
Claude Code or Codex Agent step whose rendered prompt contains that path gets
it as a read-only directory under the policy above, without the Git checkout
requirement: `--add-dir`, `Edit` deny rule and sandbox `denyWrite` for Claude,
an explicit `read` entry for Codex. Other agents receive no grant.
[src: file: backend/src/workflows/run_artifacts.rs:192-201#ensure]
[src: file: backend/src/workflows/steps.rs:126-147#artifacts_read_only_dirs]
[src: file: backend/src/agents/read_only_repos.rs:672-702#resolve_with_dirs]

```json
[
  {"name": "capture", "step_type": {"type": "Exec"}, "exec_command": "npx",
   "exec_args": ["playwright", "screenshot", "https://example.org", "{{run.artifacts_dir}}/S1.png"]},
  {"name": "verdict", "step_type": {"type": "Agent"}, "agent": "ClaudeCode",
   "prompt_template": "Open {{run.artifacts_dir}}/S1.png and judge the layout."}
]
```

Retention: the directory exists while its run can still execute (running,
waiting for approval, interrupted within `interrupted_worktree_ttl_days`). It
is removed when the run ends, the same moment as its worktree, and at startup
for a run that ended, was deleted, or stayed interrupted past that TTL.
A gate rejection ends the run without executing it again: its directory goes
at the next startup.
[src: file: backend/src/workflows/run_artifacts.rs:250-271#keeps_directory]

Location (KT-1585): each run directory (`<root>/<run id>`, owner-only) lives
**outside** the Kronn data directory. The vault protection (KT-1495) denies
that directory to every agent process (Claude `Read(...)` rules and
`denyRead`, Codex's Seatbelt profile, bubblewrap), and a deny wins over
`--add-dir`, so an artifacts directory inside it could never be read. The root
is always Kronn's own directory, `<base>/kronn-run-artifacts/<key>`, where
`<key>` comes from the data directory (two instances on one account never
sweep each other's runs). The base is `KRONN_RUN_ARTIFACTS_DIR` when set, else
the platform cache directory (`~/Library/Caches/com.kronn.kronn` on macOS,
`~/.cache/kronn` on Linux). A base that is relative, inside a protected
directory (the data directory, `KRONN_BACKUP_DIR`) or containing one (`$HOME`,
`~/.config`) is refused with a warning and the default is used; if the default
is refused too (an `XDG_CACHE_HOME` inside the data directory), runs get no
artifacts directory and the warning says to set `KRONN_RUN_ARTIFACTS_DIR`. The
sweep only lists that own directory and only removes names shaped like a run
id (a UUID), so it never reaches a directory a person chose. Steps write under
the root while Kronn creates and deletes there, so no operation goes by a path
checked once: the root is opened as a directory file descriptor, walking
`kronn-run-artifacts` and `<key>` with `O_NOFOLLOW | O_DIRECTORY`, and checked
by device and inode to be outside every protected directory. Run directories
are created with `mkdirat` against that fd, and deleted by an `unlinkat` walk
that opens each subdirectory with `O_NOFOLLOW` and unlinks a link as an entry:
**a deletion never follows a link**, even one swapped in between the listing
and the delete (the sweep holds the fd across its database reads). Migration
moves with `renameat` and copies through fds as well. Kronn's own writes
inside a run directory go the same way: the approved copy of repository
scripts (`approved-scripts/`) and the full output of a cut stdout
(`exec-output/`) are written from the run directory's fd with `mkdirat` and
`openat(O_CREAT | O_EXCL | O_NOFOLLOW)` then `renameat`, so a link a step
planted there is refused or replaced, never written through. Names are raw
bytes end to end (a name that is not UTF-8 is removed and copied like any
other), and every walk uses an explicit stack capped at 64 levels (a copy
holds two fds per level, well under macOS's default 256 open files): a deeper
tree is refused and logged, never followed. Kronn never deletes such a tree:
the sweep warns at every start (`run artifacts directory not removed: deeper
than 64 levels`) and the directory keeps its disk space. An operator removes it
by hand once Kronn has stopped: check that `<root>/<run id>` is a real
directory, not a link (`ls -ld`), then `rm -rf` it. If a part of the root is a link,
the root sits in or around a protected directory, or a protected directory
cannot be checked, Kronn logs an error and touches nothing (a run then gets no
artifacts directory). The path a run receives is read from the directory's fd
and checked once more. Off unix there are no directory fds: Kronn refuses to
create, delete or move run directories there. Docker sets
`KRONN_RUN_ARTIFACTS_DIR=/run-artifacts`, a named volume
(`kronn-run-artifacts`) separate from `/data`, so a run resumed after a
restart still finds its files. At startup the sweep first deals with the run
directories an older version left in `<data dir>/run-artifacts/`: a run that
will not read its directory again has it deleted; a live one is moved (a
rename, or across volumes a copy that skips links, made under a temporary name
and renamed into place, so a crash never leaves a partial run). Failures are
logged and the directory is left where it was. The file work runs off the
async executor.
[src: file: backend/src/workflows/run_artifacts.rs:74-122#resolve_root]
[src: file: backend/src/workflows/run_artifacts.rs:124-140#refusal]
[src: file: backend/src/workflows/run_artifacts.rs:161-190#open_root]
[src: file: backend/src/workflows/run_artifacts.rs:238-248#open_run_dir]
[src: file: backend/src/core/rooted_fd.rs:362-400#remove_child]
[src: file: backend/src/workflows/run_artifacts.rs:341-382#migrate_from]
[src: file: backend/src/workflows/run_artifacts.rs:407-425#move_into]
[src: file: backend/src/workflows/run_artifacts.rs:432-464#sweep_root]

Access across runs (decided for now, KT-1587 in 0.15.3 confines it): run
artifacts sit outside the data directory, so agents can read them, and an
Agent step can read **any** run's artifacts, not only its own, when its prompt
names that path. An Exec stdout kept there is scrubbed of the secrets Kronn
knows; reports, screenshots and other files a step writes are not.

### A cut Exec stdout (KT-1570)

An Exec stdout past its limit (100 KB, 2 MiB when a `PublishPageData` write
reads the step) is cut, and the cut text ends with an explicit marker, so a
template that interpolates `{{steps.<exec>.data.stdout}}` never hands an agent
amputated context silently:
`[TRUNCATED: kept 100 KB of 151 KB — full output: <path>]`. The scrubbed stdout
(same secret scrubbing as the step record, owner-only file) is kept whole, up
to 8 MiB, under `{{run.artifacts_dir}}/exec-output/` (outside the data
directory, see Location above), and its path is
`{{steps.<exec>.data.stdout_file}}` (`null` when nothing was cut or no
artifacts directory exists). Past 8 MiB the marker says
`first 8192 KB: <path>`; without a file it ends after the sizes. Since the
marker names the artifacts directory, a Claude Code or Codex step reading it
gets that directory read-only and can open the file. The step summary also
carries `WARNING: stdout cut at 100 KB, full output in data.stdout_file`, and
`data.stdout_truncated` stays `true`. The file is written through a temporary
file and a rename, so a link an Exec step planted there is replaced, never
written through. This applies to workflow Exec steps only: a cut stderr, a
setup output, a `CollectApiData` source and a Quick Exec run keep the
historical `[... output tronqué — limite N KB ...]` suffix and write no file,
so a collected value never carries a local path. The file goes with the run's
directory.
[src: file: backend/src/workflows/exec_step.rs:1130-1175#captured_text]
[src: file: backend/src/workflows/exec_step.rs:1249-1266#truncation_marker]

### Approved repository scripts (KT-918)

An Exec step that runs a script from the repository declares it, with every
module it loads, in `exec_script_files` (one path per line in the step editor,
relative to the repository of the workflow's home project). Saving refuses a
path outside the repository, a missing file and a symlink leading out of it,
then records each file's SHA-256: the person saving approves that content.
A save through an agent (a bridge token or a Kronn agent tool) checks the
paths but leaves an empty hash empty, so the step refuses to run ("no
approved hash") until a human saves the workflow from the UI.
[src: file: backend/src/core/approved_scripts.rs:88-134#read_declared]
[src: file: backend/src/core/approved_scripts.rs:181-194#prepare_copy]

```json
{"name": "framing", "step_type": {"type": "Exec"}, "exec_command": "node",
 "exec_args": [".agents/skills/ticket-framing/scripts/jira.cjs", "read"],
 "exec_script_files": [
   {"path": ".agents/skills/ticket-framing/scripts/jira.cjs", "sha256": ""},
   {"path": ".agents/skills/ticket-framing/scripts/lib/http.cjs", "sha256": ""}]}
```

At each execution Kronn reads the files from the home project's checkout,
never from the run's worktree, checks them against the approved hashes and
writes them to `{{run.artifacts_dir}}/approved-scripts/<step>/` with the same
layout. A mismatch fails the step before the setup or main command runs and
names the file; the step editor then shows it as "changed since approval" and
"Approve the current content" re-pins it at the next save. The main command
runs with the copy as its working directory, so `node <declared path>` and a
helper loaded through `process.cwd()` or the script's own directory come from
the copy. The run's checkout is in `KRONN_WORKTREE`, the copy in
`KRONN_APPROVED_SCRIPTS_DIR`. The setup command still runs in the checkout.
[src: file: backend/src/workflows/exec_step.rs:133-153#approved_script_child]
[src: file: backend/src/workflows/exec_step.rs:157-181#prepare_approved_scripts]
[src: file: backend/src/workflows/exec_step.rs:518-529#approved_copy]
[src: file: backend/src/workflows/exec_step.rs:769-775#approved_script_child]

Limits: only the declared files are verified. Absolute imports, paths built
from `KRONN_WORKTREE` and interpreter search paths outside the copy are not
covered: a bare `require('pkg')` walks up `node_modules` from the copy (the
repository's `node_modules` is not found), and Python reads its own
site-packages. `NODE_PATH` and `PYTHONPATH` are not inherited from the backend.
Arguments that name worktree files must use `KRONN_WORKTREE` or an absolute
path, since the cwd is the copy. The copy goes with the artifacts directory.

## Qualification

The automated policy and dispatch regressions use temporary repositories and
fake CLI processes. They verify declarations and arguments; they do not prove
vendor sandbox enforcement.
[src: file: backend/src/agents/read_only_repos.rs:1144-1165#fixture]
[src: file: backend/tests/adapter_worker_policy.rs:41#default_adapters_and_explicit_fallback_keep_worker_spawn_boundaries]

The dispatch regression checks initial and resumed Codex adapter arguments:
options occur once, repeated `-c` flags have distinct keys, the project MCP
override remains present once, and the read-only permissions profile is selected
without a conflicting `--sandbox` flag. Every Codex dispatch case also checks
for exactly one inline `projects` override and rejects dotted trust keys,
including ordinary non-strict and direct launches. Unit coverage checks host
aliases, duplicate aliases and path escaping.
[src: file: backend/tests/adapter_worker_policy.rs:193-223#trust_overrides]
[src: file: backend/tests/adapter_worker_policy.rs:226-316#config_keys]
[src: file: backend/src/agents/runner_test.rs:12057-12118#codex_project_trust_override_preserves_paths_and_host_alias]
[src: file: backend/src/acp/codex_adapter.rs:479-490#codex_project_mcp_override]

Run the opt-in host probe with authenticated Claude Code and Codex installations
and OS sandbox privileges:

```sh
cargo test --manifest-path backend/Cargo.toml --test it workflow_read_only_repos_probe -- --ignored --nocapture
```

It canonicalizes the probe root before creating disposable repositories,
checks externally read values from Read (or the Codex equivalent), Git history,
head and grep, records the OS rejection of a subprocess write, checks a protected
file is unchanged, and verifies a worktree write. Review the printed receipt
for the built-in Edit/Write denial.
[src: file: backend/tests/workflow_read_only_repos_probe.rs:7-158#workflow_read_only_repos_live_probe]

The KT-806 worker could not run a nested macOS sandbox: invoking
`codex sandbox macos --help` returned `sandbox-exec: sandbox_apply: Operation not
permitted`. The principal subsequently reported a successful Claude host probe;
Codex failed first on a duplicate `--skip-git-repo-check`, then on the dotted
trust override. Both argument regressions are now covered. The principal still
needs to rerun the Codex host probe after the inline-table fix and update the
stored autoCode prompt.
[src: user: 2026-09-28: KT-806 request_changes reviews of f474ecd2 and bf00da66]
