# Repository profile — `kronn/project.toml`

ADR-005 slice 7 (KT-920). The profile holds what a generic workflow needs to
know about one repository, so the workflow itself names nothing specific to
a repository: validation targets, forge labels and rules, tracker statuses and
transitions, delivery workflows.

## Where it is read

- File: `kronn/project.toml`, versioned in the repository.
- Kronn reads the file **at the commit of the selected ref of the main
  checkout**, with git, never from a working tree
  [src: file: backend/src/core/project_profile.rs:1]. The ref is selected in
  this order:
  1. the branch name `origin/HEAD` points to;
  2. else `main`, else `master`.
  For that name, the local branch `refs/heads/<name>` is used when it exists,
  else the remote-tracking `refs/remotes/origin/<name>`. `HEAD`, the branch the
  checkout sits on and uncommitted changes are never read.
- Provenance: a local branch holds whatever was committed or pulled into it on
  this machine; its name says nothing about review. Kronn guarantees only that
  a run's worktree, its pull request branch or the checkout's current branch
  cannot change what the run reads. Review happens where the default branch is
  protected (a pull request merged on the forge, then pulled).
- The run's project decides which profile is read: for a workflow shared by
  several projects, each run reads its target project's profile.
- It is read before the run is pinned, outside any database connection, for
  every project the run and its sub-workflows will execute in, and **pinned
  with the run** (`workflow_run_pins`, kind `project_profile`): the ref, the
  commit and the values, or the absence of a profile. A Gate or quota resume,
  a restart and the run's sub-workflow or fan-out children read the pin,
  never the repository again, so editing, deleting or adding the profile
  during a pause changes nothing for that run tree.
- One snapshot per project for the whole run tree: a child that runs in a
  project the root did not pin records its read on the root run, first
  writer wins, so every later child of that project reads the same values.
- The pinned revision fingerprint covers the profile values (not the
  commit): a changed `validation.targets.*.command` changes the fingerprint
  that approvals compare. Callers needing that identity use
  `run_pins::revision_fingerprint_with_profiles` with snapshots resolved
  beforehand; `revision_fingerprint` alone does not cover profiles.
- Trusted Page actions (KT-1029) approve that identity. The approval, the
  approvals panel and a trusted run's admission read the profiles fresh,
  before their database transaction; the approval stores its snapshot, and
  the admitted run pins exactly the snapshot its admission compared. A
  changed profile value therefore invalidates the approval (`changed`); a
  new commit with the same values does not. Checks that run inside a Page
  write or a claim, which cannot read git, compare the approved snapshot;
  the admission re-checks fresh before anything runs.

## How a workflow reads it

- Any templated step field: `{{project.<path>}}`, e.g.
  `{{project.tracker.statuses.review}}` or
  `{{project.validation.targets.lint.command}}`.
- A table or an array is also readable whole, as JSON
  (`{{project.forge.labels.to-test.triggers}}` →
  `["env-branch","ci-test"]`); an array item by index (`.0`, `.1`).
- `project.*` is a reserved name: no variable, trigger field or launch value
  can stand in for it.
- A step that reads a key without `??` needs it: when the project has no
  profile, the profile is invalid, or the key is missing, the run fails before
  its first step with a `__project_profile__` result naming the key and the
  ref. With a fallback (`{{project.forge.base_branch ?? "main"}}`), the run
  goes on.
- A project without a profile runs every workflow that does not read one.
- Values are data, never trusted code: inside an interpreter's inline code
  (`bash -c`, `node -e`…) they are refused like any other value; pass them as
  a later argument.

## Schema, version 1

Every table refuses unknown keys. Keys you choose (target, label, status,
transition, workflow and input names) use `a-z`, `0-9`, `_` and `-`.

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | integer, required | `1` |
| `validation.targets.<name>.command` | string, required | command a worktree runs to validate a change |
| `validation.targets.<name>.description` | string | |
| `forge.base_branch` | string | branch pull requests target |
| `forge.merge_method` | `merge` \| `squash` \| `rebase` | |
| `forge.required_approvals` | integer | |
| `forge.pr_template` | relative path | pull request template |
| `forge.labels.<name>.name` | string, required | label as the forge spells it |
| `forge.labels.<name>.description` | string | |
| `forge.labels.<name>.triggers` | list of strings | CI jobs or workflows the label starts |
| `tracker.kind` | `jira` \| `github` \| `gitlab` \| `linear`, required | |
| `tracker.project_key` | string | |
| `tracker.ticket_template` | relative path | ticket template |
| `tracker.statuses.<name>` | string | status as the tracker spells it |
| `tracker.transitions.<name>.from` / `.to` | string, required | |
| `delivery.workflows.<name>.workflow` | string, required | CI/CD workflow as the forge names it |
| `delivery.workflows.<name>.description` | string | |
| `delivery.workflows.<name>.inputs.<input>` | string | inputs the workflow is dispatched with |

Refused, with a message naming the key and its line and column but never
quoting the file (a refused file may hold a secret in the wrong place); a
refused key is shown as `<invalid key #n>`, its position in its table: a
malformed file, an unknown key, a wrong type or value, a wrong
`schema_version`, a file over 64 KiB or that is not a regular file
(symlink), a template placeholder in a value, a path leaving the repository,
and **anything that looks like a secret** — a key such as `token`,
`password`, `api_key`, `client_secret`, or a value shaped like a known
credential. The profile holds no secret: keep it in Kronn and let the
workflow name it.

## Example: front_euronews

The values the autoCode workflows hard-code today (read on the live instance,
2026-10-09):

```toml
schema_version = 1

[validation.targets.deps]
command = "make iso-deps"
description = "Install dependencies inside the worktree's isolated container"

[validation.targets.lint]
command = "make iso-lint"

[validation.targets.test]
command = "make iso-test"

[forge]
base_branch = "main"
merge_method = "rebase"
required_approvals = 1
pr_template = ".github/pull_request_template.md"

# `env-branch` and `ci-test` only start on the `labeled` event.
[forge.labels.to-test]
name = "to-test"
triggers = ["env-branch", "ci-test"]

[forge.labels.to-deploy]
name = "to-deploy"

[tracker]
kind = "jira"
project_key = "EW"

[tracker.statuses]
review = "To Review"
test = "To Test"
deploy = "To Deploy"
deployed = "Deployed"

[tracker.transitions.deliver]
from = "To Deploy"
to = "Deployed"

[delivery.workflows.staging]
workflow = "CD_Deploy-NONPROD.yaml"
inputs = { environment = "staging" }

[delivery.workflows.production]
workflow = "CD_Deploy-PRODUCTION.yaml"
```

`required_approvals`, the PR template path and the `environment` input are to
be confirmed against the repository's ruleset and workflow files when the
profile is merged.

## Drafted by the audit

A Full audit that completes drafts a starter profile when the default branch
has none: `schema_version`, the validation commands the repository declares
(`package.json` scripts `lint`, `typecheck`, `test`, `build`, `check`, else
Cargo, else Makefile targets), the default branch and a PR template when one
exists, the rest commented. The draft is stored with the audit run
(`project_profile_draft` on the audit run returned by the audit API) and
shown read-only in the project's audit view, with a Copy button; the audit
never writes into the checkout. Publishing it is a human action: paste it
into `kronn/project.toml` in a pull request. A one-click "publish to the
repository" action does not exist yet.
