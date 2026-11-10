# autoCode Issue: a workflow every Kronn user can run (KT-1643)

The contribution board's « autoCode Issue » workflow lets anyone with Kronn
pick an issue and contribute. A workflow is instance data (it lives in the
database), so what makes it shareable is that Kronn **ships it**: the two
workflows below are built into the binary and installed on any instance with one
call. Nothing instance-specific is written in them.

## Install

```text
POST /api/workflows/templates/autocode-issue        body: {} or { "project_id": "<id>" }
```

It creates two manual workflows (refused with the existing id when they are
already installed, so a second click never duplicates them):

| Workflow | Steps |
| --- | --- |
| `autoCode Issue - start the campaign (contribution board)` | `preflight` → `take` → `orchestrate` |
| `autoCode Issue - publish the pull request (direct or from a fork)` | `preflight` → `publish` |

The steps are deterministic scripts (no token spent), kept as
`backend/src/workflows/autocode_issue/*.py` and built by
`backend/src/workflows/autocode_issue_template.rs`. `project_id` is optional:
the workflows find their project by the repository at run time either way.

## What the workflows read, and where it comes from

| Instead of a literal… | The workflow uses | Source |
| --- | --- | --- |
| `http://localhost:3140` | `{{run.kronn_url}}` | `KRONN_BACKEND_URL` when pinned, else `http://127.0.0.1:<listening port>`; a built-in, so no variable replaces it |
| a fixed project id | the project whose remote is the repository | `GET /api/contribution/preflight` returns it as `project_id` |
| `DocRoms/Kronn` | the variable `repo` (`owner/name`) | launch form; empty = `DocRoms/Kronn` |
| the PR base | the variable `targetBranch` | launch form; empty = **main** |
| review rounds | the variable `maxReviewRounds` | launch form; empty = **10** |
| the issue | the variable `issueNumber` | required |

The defaults are the placeholders of the launch form and are applied by the
scripts when a variable is left empty. The project is found by remote: a
project's `repo_url`, else a remote of its checkout, compared as a case-blind
`owner/name`. Zero matches and several matches are both refusals with the fix
named; Kronn never guesses.

## The first step: prerequisites

```text
GET {{run.kronn_url}}/api/contribution/preflight?repo=<repo>&target_branch=<target_branch>&max_review_rounds=<max_review_rounds>&branch=<branch>
```

The `preflight` step prints one line per check (`[ok]` or `[FAIL]`, with the
fix) and stops the run when one fails. `ok` is true only when all hold:

| Check | Fails when | The message says |
| --- | --- | --- |
| `gh_installed` | `gh` does not run | install it from cli.github.com |
| `gh_auth` | `gh auth status` fails | `gh auth login` |
| `gh_scopes` | the token lists scopes and has neither `repo` nor `public_repo` | `gh auth refresh -s repo` (a token whose scopes `gh` does not list is not refused) |
| `repo_access` | `gh api repos/<repo>` fails | check the name and `gh repo view` |
| `project` | no, several, or a moved clone | clone it and add the folder in Projects; or keep one |
| `worker` | no installed, enabled CLI agent | install one in Settings > Agents |

The report also carries `workers` (the agent the campaign launches the task
on: Claude Code when available, else the first installed), `suggested_reviewer`
(below) and the `publication` plan.

## Maintainer or fork

`gh api repos/<repo> --jq .permissions.push` decides. The report's
`publication.steps` is the ordered plan the `publish` step carries out, so the
plan read in the preflight is the plan executed:

| | `mode` | steps |
| --- | --- | --- |
| can push | `maintainer` | `push` the issue branch to `<repo>`, `open_pull_request` (`--head <branch>`) |
| cannot push | `fork` | `fork_repo` (`gh repo fork`, only when the fork does not exist), `push` to `<login>/<name>`, `open_pull_request` (`--head <login>:<branch>`) |

In both modes the pull request targets `<repo>` on `<targetBranch>`. The
`publish` workflow is run once the campaign has delivered: it refuses with a
message while `autocode/issue-<N>` holds no commit beyond the target, and an
already open pull request for the branch is reported instead of duplicated.
`take` assigns the issue to the `gh` login when it can; a contributor without
triage rights works the issue anyway (the refusal is a note, not a stop).

## Who reviews a delivery

A delivery's review request is addressed to the campaign room's agent. A
campaign records who that is when it is created
(`POST /api/orchestration/campaigns`, optional field `reviewer`) and announces it
in the room with the target branch and the review rounds:

| Request | The room | The recorded reviewer |
| --- | --- | --- |
| `reviewer` passed, `remote_model` | that HTTP provider and model (LiteLLM, NVIDIA, Ollama, or `Custom` with its `connection_id`) answer in it | the remote model |
| `reviewer` passed, `agent` | that agent and the model it reviews with answer in it | the agent |
| `reviewer` passed, `human` | stays human-only: approve or request changes from the review card | the human |
| no `reviewer` | **left exactly as it is** (agent, model, participants, `no_agent`) | the room's own agent and model, or the human when the room is human-only |

A room is reconfigured only for a reviewer passed explicitly; a campaign started
from a room the owner already set up keeps that setup. A reviewer that cannot
review (an agent without a model, a CLI agent declared as a remote model) is
refused with nothing created.

The shipped `orchestrate` step creates its room human-only and passes the
report's `suggested_reviewer`: the first remote model this instance has
configured (LiteLLM once its proxy and key are set, else NVIDIA once its key is
set; its model is the reasoning-tier model, else the default-tier one). With no
remote model configured it passes none, so the human reviews from the UI.

The choice is stored on the run (`reviewer`, migration 292), announced in the
room and shown on the campaign panel. The human can always decide from the UI,
whoever the reviewer is.

## Limits

- The workflow stored on the maintainer's instance (`61f7a323`) is instance
  data and is not rewritten by Kronn: install the shipped workflows and retire
  that copy.
- The board workflow's refresh after a launch is not part of the shipped
  workflow (it would need that instance's board id); the board refreshes on its
  own trigger.
- `gh` is the only forge tool read; a non-GitHub remote resolves to no project.
