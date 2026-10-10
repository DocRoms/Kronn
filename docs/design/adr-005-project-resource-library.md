# ADR-005 — Kronn resources versioned in the project repository

- **Status:** Accepted, revision 2 (KT-845, child of KT-535), validated by
  Romuald on 2026-09-27.
- **Date:** 2026-09-27.
- **Scope:** how prompts and skills, Quick Execs (QE), Quick APIs (QA),
  workflows, artifacts (pages), directives, profiles, plugin declarations and a
  repository profile live in a project repository, so they can be reviewed in
  a PR, linked to the project, recovered by cloning it, and **used without
  Kronn whenever that is possible**.
- **Decided with Romuald, 2026-09-26/27** (room « Kronn 0.14.2 »):
  - versioned folder `kronn/`; `.kronn/` stays local and ignored;
  - the repository must stay useful without Kronn: Kronn automates, secures
    and orchestrates, but a human or a bare CLI can read, and often run, what
    the repository carries;
  - a Quick Prompt is a skill with variables; without variables it is a skill;
  - one entry point, not one catalogue entry per resource;
  - database and file each carry an update date and are compared, never
    silently overwritten;
  - phase 1 marks every workflow and every QA as requiring Kronn.

## Context

Every Kronn resource lives in the instance only. A project cloned on another
machine arrives without its workflows, prompts or artifacts. The autoCode
workflows had to be cloned per project (`p1a`, `p2a`, `p3a`) and kept in sync
by script.

What the code does today:

- Workflows, QP, QA, QE and pages are SQLite rows with instance UUIDs and one
  optional `project_id` [src: file: backend/src/models/workflows.rs:19].
- Skills, directives and profiles are Markdown files under the config
  directory, with an id derived from the name (`custom-<slug>`). Updating one
  deletes it and saves it again, so renaming changes its id and any run that
  resolves it later drops it silently
  [src: file: backend/src/api/skills.rs:50]
  [src: file: backend/src/core/skills.rs:409-414].
- Exports exist per type (workflow bundle v3, `kronn.artifact`,
  `kronn.plugins`). Imports create new UUIDs on every run. Only the artifact
  import remembers `(kind, source_id) -> target_id`
  [src: file: backend/src/db/sql/188_artifact_import_origins.sql:4].
- `.kronn/` is the runtime directory (worktrees, MCP backups, ownership
  ledgers, temporary files), added to `.gitignore`
  [src: file: backend/src/core/mcp_scanner.rs:361]
  [src: file: backend/src/core/native_files.rs:20].
- Secrets are masked on export by `core/export_secrets.rs`.

## Principles

1. **Readable first.** Every file makes sense on its own, in Markdown or
   commented YAML, without opaque Kronn identifiers.
2. **Runnable without Kronn when nothing requires it.** Kronn adds forms,
   typed variables, batch, versioning, approvals, secrets brokering and
   scheduling; it is not needed to read or to follow a resource.
3. **Declared, not guessed.** Each file states what it needs (`requires`).
4. **One entry point.** Agents discover resources through one index, not
   through one catalogue entry per resource.

## Portability levels

Every resource declares a `requires` list. Its level follows from it:

| Level | Meaning | Examples |
|---|---|---|
| **N0 — readable** | A human or an agent understands it without Kronn | every resource |
| **N1 — runnable by a bare CLI** | An agent or a human can execute it with the CLIs installed on the machine | prompts and skills, QE, CLI-only plugin access, workflows made only of Agent, Exec and Gate steps |
| **N2 — Kronn required** | Needs the Kronn engine, secret broker or data store | QA, workflows with API steps, cron or tracker triggers, artifacts that read Kronn datasets |

Phase 1: every workflow and every QA declares `requires: [kronn]`. Portable
workflows (N1) come in a later phase.

## Resources

### Prompts and skills: one format

A Quick Prompt is a skill with variables. Both use **one file format,
compatible with Agent Skills `SKILL.md`**:

```markdown
---
name: review-pr
description: Review a pull request against the repository rules.
metadata:
  kronn:
    kind: prompt            # prompt = launched by a human; skill = loaded by the agent
    variables:
      - { name: pr_url, type: string, source: user_input }
    agent: claude-code
    tier: reasoning
    updated_at: 2026-09-27T10:00:00Z
requires: []
---
Review {{pr_url}} …
```

- Without a `metadata.kronn` block, the file is an ordinary skill that any CLI
  can read.
- With variables, Kronn renders a typed form, runs batches and keeps versions.
  Without Kronn, an agent asks for the values.
- The difference between a skill and a prompt is **who triggers it**: the agent
  (context it loads when relevant) or the human (a task launched explicitly).
  Screens may stay separate at first; the storage format is shared.

### Quick Exec

A QE is a declared command: one allowed binary and its arguments, no shell.
A human or an agent can run it as is (N1). Kronn adds the allowlist, the
approval, the typed output and its use as a task_exec validation. It lives in
`kronn/quick-execs/<slug>.yaml`.

### Plugins: a service with up to three accesses

A plugin is a service identity that offers up to three accesses:

- **MCP**: Kronn writes the server configuration into the CLIs (Claude, Codex,
  Vibe…); credentials are stored in Kronn.
- **API**: Kronn calls the service as a broker, reusing the credentials of the
  MCP configuration.
- **CLI**: a binary on the machine (`fastly`, `glab`…) with its own
  authentication. No token in Kronn nor in an MCP. This is what makes Exec and
  QE deterministic.

`kronn/plugins.toml` declares, per plugin:

- the MCP server by name, with the **names** of the keys it needs (each machine
  provides the values);
- the API access as `requires: [kronn]`;
- the **CLI prerequisites**: binary, minimum version, and the command that
  checks the authentication (`glab auth status`, `fastly whoami`).

The index shows the CLI prerequisites, so an agent without Kronn knows it
should use `glab` and how to check it is logged in.

### Workflows

A workflow stored in `kronn/workflows/<slug>.yaml` belongs to the repository
that carries it; its project is implicit. Phase 1 marks all of them
`requires: [kronn]`. Later, a workflow made only of Agent, Exec and Gate steps
is written as a runbook whose steps an agent can follow without Kronn, while
the Kronn engine automates the same file.

A workflow meant for several projects resolves its project at trigger time and
isolates the run in that project's worktree (KT-851):

- `project_scope` declares it: `{"type":"All"}` or
  `{"type":"Projects","project_ids":[…]}`; the home project (`project_id`)
  is always served. Without it, nothing changes
  [src: file: backend/src/models/workflows.rs:92].
- Manual launch: the request's `project_id`, refused when not served; else
  the launching discussion's or page's project when served; else the home
  project. Cron: one run per served project. Tracker: the served project
  whose `repo_url` is the tracked GitHub repository (home project first when
  several are linked); none linked skips the poll with a warning
  [src: file: backend/src/workflows/project_scope.rs:1].
- The run records its project (`workflow_runs.project_id`, KT-1015) and runs
  in that project's checkout or worktree, with its environment. A
  multi-project sub-workflow follows its parent run's project when it serves
  it.
- `concurrency_limit` (and `concurrency_key`) count each project's runs
  apart for a multi-project workflow.
- Quick Prompt and Quick API references written `ref:<kind>:<slug>` stay
  symbolic in a multi-project workflow and resolve in each run's project;
  a literal id is the same resource for every project. Quick Exec and
  workflow targets are always stored as literal ids (allowlist and graph
  checks need them).
- Project ids are instance-local: importing a listed scope from `kronn/`
  keeps only the projects this instance has, and an empty list makes the
  workflow single-project.

**Carrying repository vs target project (decided for KT-918 and KT-920).**
The repository that carries a multi-project workflow is its home project's:
`kronn/workflows/<slug>.yaml` is published there and its approval (per
content hash) covers the definition for every target. The target project is
where the run executes: its worktree, its environment and secrets, and its
own repository profile. Approved scripts (KT-918) are therefore read from the
carrying repository at the approved content and copied into the run's
artifacts directory before execution, never read from the target worktree;
the profile (KT-920) is the target project's, read from its base branch, not
from the run's worktree a pull request can modify.

**Implementation note (KT-918, 0.14.3).** An Exec step lists its entry
script and the modules it loads in `exec_script_files` (`{path, sha256}`,
relative to the home project's checkout). The hashes live in the step, so
they enter the workflow's approval hash and a published file carries them as
references, never the content. Saving validates each path (relative, inside
the repository, no symlink leading out, regular file under 4 MiB) and pins an
empty hash to the current content when a human saves; an agent's save (bridge
token or Kronn agent tool) leaves it empty, and the step refuses to run. Before each execution the runner reads the
home project's checkout (the stored definition's `project_id`, not the run's
project), checks every hash, and writes the verified bytes to
`{{run.artifacts_dir}}/approved-scripts/<step>/`; a mismatch fails the step
before anything runs, naming the file. The main command runs with that copy
as its working directory and receives `KRONN_WORKTREE` (the run's checkout)
and `KRONN_APPROVED_SCRIPTS_DIR` through the child environment builder.
Paths built from the cwd resolve in the copy; absolute imports, paths built
from `KRONN_WORKTREE` and interpreter search paths outside the copy (Node's
parent `node_modules`, Python's site-packages) are not covered
[src: file: backend/src/core/approved_scripts.rs:1]
[src: file: backend/src/workflows/exec_step.rs:102].

**Implementation note (KT-920, 0.15.0).** Templated step fields read the
target project's profile as `{{project.<path>}}` (`project.*` is reserved).
The runner reads `kronn/project.toml` once, before the first step, with git,
at the commit of the selected ref of the main checkout: the branch
`origin/HEAD` names, else `main`, else `master`, as the local branch when it
exists, else `origin/<name>`; never `HEAD` nor a working tree. That ref's
name proves nothing about review; what is guaranteed is that the run's
worktree or branch cannot change it. What was read (ref, commit, values, or
the absence of a profile) is resolved before the pin, outside any connection,
for every project the run tree executes in, and pinned with the run
(`workflow_run_pins`, kind `project_profile`); a child in another project
records its read on the root run, first writer wins. Resumes and children
never read the repository again, and the pinned fingerprint covers the
values (`revision_fingerprint_with_profiles`). A trusted Page action's
approval (KT-1029) covers them too: the approval and the admission read the
profiles fresh, and the admitted run pins the snapshot its admission
compared.
The schema refuses unknown keys, placeholders and secret-looking keys or
values, and its refusals never quote the file. A step reading a key without
`??` fails the run before it starts when the profile is absent, invalid or
lacks the key; a workflow that reads none is unaffected. A completed Full
audit drafts a starter profile and stores it with the audit run, shown
read-only with a Copy button in the audit view; it never writes into the
checkout, and only a human publishes it
[src: file: backend/src/core/project_profile.rs:1]
[src: file: docs/guides/project-profile.md:1].

### Other resources

QA, artifacts, directives and profiles live in `kronn/` with the same identity,
reference, secret and alignment rules.

## Layout

```
kronn/
  INDEX.md               # generated: what the repository offers, how to use it, what it needs
  kronn.toml             # schema version, project identity, required secret names
  kronn.lock             # sha256 per resource, written by Kronn
  project.toml           # repository profile
  prompts/<slug>.md      # prompts and Kronn skills, SKILL.md-compatible
  quick-execs/<slug>.yaml
  quick-apis/<slug>.yaml
  workflows/<slug>.yaml
  artifacts/<slug>/artifact.yaml
  artifacts/<slug>/index.html
  directives/<slug>.md
  profiles/<slug>.md
  plugins.toml
.agents/skills/kronn/SKILL.md    # the single router skill
.agents/skills/<slug>/SKILL.md   # the repository's own skills, and prompts promoted on purpose
```

- `.kronn/` keeps everything that is local, generated or machine-specific.
- The repository profile holds what a generic workflow needs to know about the
  repository: validation commands that run from a worktree, commit and PR
  conventions, forge and tracker settings, language policy. The audit (KT-819)
  proposes it; a human validates it in a PR.

## Discovery: one entry point

CLIs load the name and description of every skill at the start of every
session. Thirty prompts exposed as skills would cost two to three thousand
tokens in every session and every sub-agent, and compete to trigger.

- **One router skill** `.agents/skills/kronn/SKILL.md`: what Kronn resources
  this repository has, and how to list them (`kronn/INDEX.md`).
- **One line in the T0 of `docs/AGENTS.md`**: « Kronn resources → `kronn/INDEX.md` ».
  Vercel's evaluation found a skill not invoked in 56 % of cases while an
  always-loaded index did better, so both entry points exist until the A/B
  bench shows which one works.
- **`kronn/INDEX.md`** is generated by Kronn: one line per resource with its
  description, level (N0/N1/N2) and prerequisites.
- **No resource is exposed as a native skill by default.** A prompt used every
  day can be **promoted**, one by one and on purpose, into `.agents/skills/`
  (so `/review-pr` works in the CLI). Kronn projects skills to `.claude/skills`
  where needed.

### Project skills kept in Kronn (KT-1128)

A skill can belong to one project before it is published into that project's
repository. It stays a custom skill of the library carrying
`metadata.kronn-project`, with the same file and the same `custom-<slug>` id,
so run pins (KT-1096) and references treat it like any custom skill. It is
offered only in its project, listed on its AI & automation tab as a project
skill without being a default of its discussions, and refused by name when an
agent launch of another project names it.
`[src: file: backend/src/core/skills.rs:624]`
A workflow Agent step also loads the project's repository skills: the run pin
reads each one as committed on the default branch and pins it, so neither the
checkout's branch nor the run's worktree changes it; one the pin lacks stops
the step by name. A discussion still reads the checkout as it is.
`[src: file: backend/src/api/projects/used_skills.rs:410]`

## Identity

- Every resource declares `kind` and an immutable `slug` in the file; the file
  name matches the slug. Renaming the display name never changes the slug.
- A local table maps `(project, kind, slug)` to the instance id, generalising
  `artifact_import_origins`. Re-importing the same slug updates the same row.
- The project identity is the normalised `repo_url` when present, otherwise the
  project path, never the local UUID.

## References

- Files reference other resources symbolically: `prompt:review-pr`,
  `workflow:triage`, `skill:block-migration`, `plugin:github`.
- Instance-bound references (API config, MCP config, connection, agent or
  model) are requirements resolved locally on import, as `rebind_api_configs`
  already does for API configs [src: file: backend/src/api/workflows.rs:1977].
- Ids embedded in artifact HTML are rewritten on import, as `remap_html`
  already does [src: file: backend/src/api/artifact_portability/import.rs:251].
- Workflows (KT-917). One resolver serves every kind (`workflow`, `qe`, `qa`,
  `prompt`, `skill`, `plugin`, `artifact`): the run's project first, then the
  global scope; in each scope the identity table, then the slug publication
  would give the resource's name (a page's own slug). `skill:` also finds a
  `SKILL.md` in the project repository (`repository:<project>:<slug>`). Two
  matches in one scope are refused as ambiguous
  [src: file: backend/src/db/resource_identities.rs:146].
  - Templated step fields write `{{ref:<kind>:<slug>}}`. The run resolves
    every reference its steps, Quick Prompts and Quick APIs read before the
    first step; an unknown one fails its step before launch, naming it.
  - Structured id fields (`sub_workflow_id`, `quick_prompt_id`,
    `quick_api_id`, batch and CollectApiData ids) are published as
    `ref:<kind>:<slug>` and stored as local ids at save and import, because
    graph validation needs literal ids. Importing a workflow whose target is
    not imported yet is refused with the reference; import the target first.
    A literal Page id is published as `{{ref:artifact:<slug>}}`.
  - Scripts resolve one reference with
    `GET /api/resources/resolve?ref=<kind>:<slug>&project=<id>`
    [src: file: backend/src/core/resource_refs.rs:1].

## Secrets

- Never in the repository. A file names what it needs (`secret://FASTLY_TOKEN`);
  Kronn resolves the name against its encrypted store.
- `kronn.toml` lists the required names so a clone can tell the user what to
  provide. Export reuses `core/export_secrets.rs`; notification and gate URLs
  are masked as well.
- A CI check refuses a secret-looking value in `kronn/`.

## Database and file alignment

- Each resource carries `updated_at` **inside the file** (git does not keep file
  dates across checkout or pull) and in the database.
- Kronn also stores the content hash of the last alignment. Comparing both
  current hashes with it tells which side moved:
  - file only: the repository changed (a pull); Kronn proposes to update the
    database;
  - database only: the resource was edited in Kronn; Kronn proposes to write
    the file;
  - both: conflict; the human chooses, with a diff.
- Nothing is overwritten silently. Dates help the choice; hashes decide whether
  there is a difference.

## Trust on clone

- Nothing imported from a repository is active in Kronn until approved:
  workflows are imported disabled; QE and Exec steps, worktree hooks and
  triggers need an explicit approval.
- Approval is recorded **per content hash**, outside the repository. A changed
  hash needs a new approval; approval by name only is refused.
- Paths are validated (relative, inside the folder, no symlink escape), and
  Kronn never overwrites a git-tracked file it did not write (KT-818).
- Outside Kronn, running an N1 resource is the user's own action with the
  user's own CLIs, as with any script in the repository.

## Prerequisites

- Skills, directives, profiles and prompts are updated in place, atomically,
  with a stable id. Without this, a pull that renames a resource would
  reproduce the data loss of KT-818.
- A running workflow keeps a snapshot of the resources it loaded.

## Options rejected

- **One global skill with sub-skills.** CLIs only discover skills one level
  deep, and one description means one chance to trigger. The router skill of
  this ADR only points to the index; it does not contain the resources.
- **Every prompt as a native skill.** Rejected as a default because of the
  catalogue cost; kept as an opt-in promotion.
- **Everything under `.kronn/`.** Would require migrating every repository's
  `.gitignore` and moving runtime content without a single mistake.
- **A plugin package** (Claude Code `.claude-plugin/` or Agent Plugins 1.0) as
  the source of truth. Kept as a later export target.

## KT-498 prototype inventory

Branch `archive/feat-0.13.0-portable-prototype-20260830`.

| Component | Decision |
|---|---|
| Lock file with sha256 per resource | Keep, as `kronn/kronn.lock` |
| Deletion limited to a managed manifest | Keep (already in KT-818's ledger) |
| Trust on first use bound to the lock hash, stored outside the repository | Keep |
| Relative path and symlink validation | Keep |
| Project scope overriding global scope, duplicates refused | Keep |
| `.env.example` generated from required secret names | Keep, derived from `kronn.toml` |
| Generated router skill | Keep, reduced to a pointer to `kronn/INDEX.md`, measured on the A/B bench |
| QP rendered as SKILL.md with a JSON sidecar | Replace by the single SKILL.md-compatible format with `metadata.kronn` |
| `.agents/` as the library root | Drop |
| Separate portable workflow format and parallel runner (`kronn run`) | Drop: the Kronn workflow engine stays the only runner; portable workflows are runbooks readable by an agent |

## Implementation slices

1. In-place update with stable ids for skills, directives, profiles and
   prompts, plus a run snapshot.
2. Single prompt/skill file format with `metadata.kronn`, readable and
   writable by Kronn.
3. Identity table `(project, kind, slug)` and symbolic reference resolution.
4. Read `kronn/` into the database: import, disabled by default, trust per
   hash, alignment status.
5. Write `kronn/` from the database, lock update, secret check, generated
   `INDEX.md`, router skill and T0 line.
6. Plugin declarations: accesses and CLI prerequisites in `plugins.toml`,
   shared with the Plugins page health probes (KT-829).
7. Repository profile `project.toml`, proposed by the audit and consumed by
   task_exec validations and generic workflows. **Status (KT-920, 0.15.0):**
   schema v1, `{{project.<path>}}` in workflows and the audit proposal are
   done (see the implementation note in §Workflows); task_exec validations,
   the `INDEX.md` line and moving the autoCode onto the profile remain.
8. Workflows shared across projects, project resolved at trigger time
   (KT-851, see §Workflows).
9. Later: portable workflows (N1 runbooks), prompt promotion to native skills,
   Agent Plugin export.

## User experience (validated 2026-09-27)

- **`kronn/` is created lazily.** No folder while nothing is linked to the
  project in the database. It appears the first time a user chooses to publish
  a project resource to the repository (« Publier dans le dépôt »), and only
  published resources are written.
- **Project page**: one **Resources** tab has three remembered sections:
  Skills, Automation and Artifacts. Skills separates resources already present
  in the repository/project from the remaining Kronn catalogue and reports
  repository/Kronn provenance. Automation groups workflows, Quick Prompts, QE
  and QA by type; Artifacts lists the project's pages. Existing portability
  levels, repository alignment states, selection and `kronn/` preview remain
  visible [src: file: frontend/src/components/ProjectRepositoryResourcesPanel.tsx:124-155].
- The Overview shows the linked MCP count and companion repositories, and owns
  the project deletion controls [src: file: frontend/src/components/ProjectCard.tsx:2395]
  [src: file: frontend/src/components/ProjectCard.tsx:2411].
- **Automations and Artifacts pages**: grouped by project; everything not
  linked to a project goes into « Général ».
- Simple and fast first: the listing reads existing rows and a cheap hash
  comparison, no background scan.

## Open questions

- Planning tasks stay out of scope (live state, instance numbering). Confirm.
- Do the current Kronn Skills screens and the Quick Prompts screens merge, or
  only share the storage format?
