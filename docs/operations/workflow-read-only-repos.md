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

Both the default [ACP adapters](acp-adapters.md) and the direct CLI route use
this policy, overriding `full_access`. The Claude process cannot inherit the
container sandbox-bypass marker when a mandatory sandbox policy is present.
[src: file: backend/src/agents/runner.rs:3747]
[src: file: backend/src/agents/runner.rs:11042]
[src: file: backend/src/acp/codex_adapter.rs:400]

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

## Qualification

The automated policy and dispatch regressions use temporary repositories and
fake CLI processes. They verify declarations and arguments; they do not prove
vendor sandbox enforcement.
[src: file: backend/src/agents/read_only_repos.rs:208]
[src: file: backend/tests/adapter_worker_policy.rs:1]

Run the opt-in host probe with authenticated Claude Code and Codex installations
and OS sandbox privileges:

```sh
cargo test --manifest-path backend/Cargo.toml --test workflow_read_only_repos_probe -- --ignored --nocapture
```

It creates disposable repositories, checks externally read values from Read
(or the Codex equivalent), Git history, head and grep, records the OS rejection
of a subprocess write, checks a protected file is unchanged, and verifies a
worktree write. Review the printed receipt for the built-in Edit/Write denial.
[src: file: backend/tests/workflow_read_only_repos_probe.rs:1]

The KT-806 worker could not run a nested macOS sandbox: invoking
`codex sandbox macos --help` returned `sandbox-exec: sandbox_apply: Operation not
permitted`. The host probe and the stored autoCode prompt update still require
principal qualification; neither is claimed as completed by the unit tests.
