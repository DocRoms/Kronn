# Workflow Agent read-only repositories

An Agent step can declare `read_only_repos`, an array of absolute local Git
repository paths. The wizard exposes one path per line in the step's advanced
settings. An empty or absent list retains the existing launch policy. This is
an explicit per-step declaration; listing a companion project in documentation
does not automatically grant access.
[src: file: backend/src/models/workflows.rs:693]
[src: file: frontend/src/components/workflows/WorkflowWizard.tsx:4338]

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
[src: file: backend/src/agents/read_only_repos.rs:12]
[src: file: backend/src/api/workflow_step_schema.json:29]

Claude receives `--add-dir` together with `Edit` deny rules and sandbox
`denyWrite` entries. Its read block remains enabled, the OS sandbox is
mandatory, unsandboxed commands are disabled, and inherited user/project
settings are excluded. Codex receives an invocation-local permissions profile
extending `:workspace`, with explicit `read` entries and approval policy `never`.
It keeps the standard broad filesystem read access; the declared directories
are never added as writable roots, including when they live under a temporary
directory. `--strict-config` rejects unsupported Codex settings.
[src: file: backend/src/agents/read_only_repos.rs:96]
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
[src: file: backend/src/agents/runner.rs:3747]
[src: file: backend/src/agents/runner.rs:11065]
[src: file: backend/src/acp/codex_adapter.rs:404]

All Codex `exec` launches, with or without `--strict-config`, receive one
`-c projects={...}` inline table containing the working directory and, when
applicable, its `KRONN_HOST_HOME` alias for `/host-home`. The same launcher
handles direct CLI, adapter, worker and resumed turns. Path keys are serialized
as TOML; identical aliases are deduplicated.
[src: file: backend/src/agents/runner.rs:11007]
[src: file: backend/src/agents/runner.rs:11076]

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
[src: file: backend/src/workflows/steps.rs:111]

I recommend replacing the stored autoCode assertion that the main checkout is
readable with this wording when that template is available:

> A project's `path` identifies its main checkout; it does not grant filesystem
> access. Read linked repositories only when the Agent step explicitly declares
> them in `read_only_repos`. Those repositories, including their Git metadata,
> are read-only. If a read is denied, report the limitation; do not claim to have
> inspected the checkout or attempt to bypass the restriction.

## Run artifacts directory

Each run gets its own directory, created by Kronn outside every checkout and
exposed as `{{run.artifacts_dir}}` (KT-910). Exec steps write files there. A
Claude Code or Codex Agent step whose rendered prompt contains that path gets
it as a read-only directory under the policy above, without the Git checkout
requirement: `--add-dir`, `Edit` deny rule and sandbox `denyWrite` for Claude,
an explicit `read` entry for Codex. Other agents receive no grant.
[src: file: backend/src/workflows/run_artifacts.rs:1]
[src: file: backend/src/workflows/steps.rs:89]

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
for a run that ended, was deleted, or stayed interrupted past that TTL. It
lives under the Kronn data directory (`run-artifacts/<run id>`, owner-only).
A gate rejection ends the run without executing it again: its directory goes
at the next startup.

## Qualification

The automated policy and dispatch regressions use temporary repositories and
fake CLI processes. They verify declarations and arguments; they do not prove
vendor sandbox enforcement.
[src: file: backend/src/agents/read_only_repos.rs:208]
[src: file: backend/tests/adapter_worker_policy.rs:1]

The dispatch regression checks initial and resumed Codex adapter arguments:
options occur once, repeated `-c` flags have distinct keys, the project MCP
override remains present once, and the read-only permissions profile is selected
without a conflicting `--sandbox` flag. Every Codex dispatch case also checks
for exactly one inline `projects` override and rejects dotted trust keys,
including ordinary non-strict and direct launches. Unit coverage checks host
aliases, duplicate aliases and path escaping.
[src: file: backend/tests/adapter_worker_policy.rs:188]
[src: file: backend/src/agents/runner_test.rs:7933]
[src: file: backend/src/acp/codex_adapter.rs:374]

Run the opt-in host probe with authenticated Claude Code and Codex installations
and OS sandbox privileges:

```sh
cargo test --manifest-path backend/Cargo.toml --test workflow_read_only_repos_probe -- --ignored --nocapture
```

It canonicalizes the probe root before creating disposable repositories,
checks externally read values from Read (or the Codex equivalent), Git history,
head and grep, records the OS rejection of a subprocess write, checks a protected
file is unchanged, and verifies a worktree write. Review the printed receipt
for the built-in Edit/Write denial.
[src: file: backend/tests/workflow_read_only_repos_probe.rs:1]

The KT-806 worker could not run a nested macOS sandbox: invoking
`codex sandbox macos --help` returned `sandbox-exec: sandbox_apply: Operation not
permitted`. The principal subsequently reported a successful Claude host probe;
Codex failed first on a duplicate `--skip-git-repo-check`, then on the dotted
trust override. Both argument regressions are now covered. The principal still
needs to rerun the Codex host probe after the inline-table fix and update the
stored autoCode prompt.
[src: user: 2026-09-28: KT-806 request_changes reviews of f474ecd2 and bf00da66]
