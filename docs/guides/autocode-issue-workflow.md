# autoCode Issue: a workflow every Kronn user can run (KT-1643)

The contribution board's « autoCode Issue » workflow lets anyone with Kronn
pick an issue and contribute. A workflow is instance data (it lives in the
database, not in the repository), so what makes it shareable is the set of
values Kronn now supplies at run time instead of the workflow hard-coding them.
Re-point a stored copy of the workflow at them once; an exported copy then runs
on any other instance.

## What the workflow reads, and where it comes from

| Instead of a literal… | The workflow uses | Source |
| --- | --- | --- |
| `http://localhost:3140` | `{{run.kronn_url}}` | `KRONN_BACKEND_URL` when pinned, else `http://127.0.0.1:<listening port>`; a built-in, so no variable replaces it |
| a fixed project id | the project whose remote is the repository | `GET /api/contribution/preflight` returns it as `project_id` |
| `DocRoms/Kronn` | a workflow variable `repo` (`owner/name`) | the workflow's own settings |
| review rounds | a workflow variable `max_review_rounds` | default **10** |
| the PR base | a workflow variable `target_branch` | default **main** |

The project is found by remote: a project's `repo_url`, else a remote of its
checkout, compared as a case-blind `owner/name`. Zero matches and several
matches are both refusals with the fix named; Kronn never guesses.

## The first step: prerequisites

```text
GET {{run.kronn_url}}/api/contribution/preflight?repo=<repo>&target_branch=<target_branch>&max_review_rounds=<max_review_rounds>&branch=<branch>
```

The report lists six checks, each with an actionable message, and `ok` is true
only when all hold:

| Check | Fails when | The message says |
| --- | --- | --- |
| `gh_installed` | `gh` does not run | install it from cli.github.com |
| `gh_auth` | `gh auth status` fails | `gh auth login` |
| `gh_scopes` | the token lists scopes and has neither `repo` nor `public_repo` | `gh auth refresh -s repo` (a token whose scopes `gh` does not list is not refused) |
| `repo_access` | `gh api repos/<repo>` fails | check the name and `gh repo view` |
| `project` | no, several, or a moved clone | clone it and add the folder in Projects; or keep one |
| `worker` | no installed, enabled CLI agent | install one in Settings > Agents |

## Maintainer or fork

`gh api repos/<repo> --jq .permissions.push` decides. The report's `publication`
field is the plan the push and the pull request follow:

| | `mode` | pushes to | `gh pr create --head` | before the push |
| --- | --- | --- | --- | --- |
| can push | `maintainer` | `<repo>` | `<branch>` | nothing: unchanged |
| cannot push | `fork` | `<login>/<name>` | `<login>:<branch>` | `gh repo fork` when `needs_fork` |

In both modes the pull request targets `<repo>` on `<target_branch>`.

## Who reviews a delivery

A delivery's review request is addressed to the campaign room's agent. A room
created with no agent (or with one that no session serves) used to leave it
orphaned. A campaign now names its reviewer when it is created
(`POST /api/orchestration/campaigns`, field `reviewer`):

| `reviewer.kind` | Needs | The room |
| --- | --- | --- |
| `remote_model` | an HTTP provider (`LiteLlm`, `Nvidia`, `Ollama`, or `Custom` with its `connection_id`) | that provider and model answer in the room |
| `agent` | an agent and the model it reviews with | that agent and model answer in the room |
| `human` | nothing | the room stays human-only; approve or request changes from the review card |

Left out, the default is the first configured remote model (LiteLLM once its
proxy and key are set, else NVIDIA once its key is set; its model is the
reasoning-tier model, else the default-tier one) and, when none is configured,
the human. A reviewer that cannot review (an agent without a model, a CLI agent
declared as a remote model) is refused with nothing created.

The choice is stored on the run (`reviewer`, migration 292), announced in the
room (target branch, review rounds, reviewer) and shown on the campaign panel.
The human can always decide from the UI, whoever the reviewer is.

## Limits

- The stored workflow `61f7a323` on the maintainer's instance still carries its
  old literals until someone edits it to use the values above: Kronn ships the
  values, not a rewrite of an instance's data.
- `gh` is the only forge tool read; a non-GitHub remote resolves to no project.
