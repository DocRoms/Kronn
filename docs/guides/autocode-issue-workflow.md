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

It is refused, with nothing created, while `python3` does not resolve on the
PATH of the Kronn process (see [Prerequisite: python3](#prerequisite-python3)).
Otherwise it creates two manual workflows (refused with the existing id when
they are already installed, so a second click never duplicates them):

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
| who reviews | the variable `reviewer` | launch form; empty = the first configured remote model, else you (see [Who reviews a delivery](#who-reviews-a-delivery)) |

The defaults are the placeholders of the launch form and are applied by the
scripts when a variable is left empty. The project is found by remote: a
project's `repo_url`, else a remote of its checkout, compared as a case-blind
`owner/name`. Zero matches and several matches are both refusals with the fix
named; Kronn never guesses.

## Prerequisite: python3

Every step of the shipped workflows, the preflight included, is a script run as
`python3` (no token is spent, and the scripts are the same on every platform).
`python3` is therefore a prerequisite like `gh`, and it is the only name Kronn
runs: not `python`, not `py -3`. It is checked twice, since the preflight itself
is a `python3` script and cannot report its own absence:

- **At install.** `POST /api/workflows/templates/autocode-issue` looks for an
  executable `python3` in the PATH of the Kronn process and **refuses** (a
  `validation` error, nothing installed) when there is none. It does not install
  with a warning: workflows whose first step cannot start would only fail later,
  on the board. Install Python 3, check that `python3 --version` works in a
  terminal, restart Kronn so it sees the new PATH, then install again.
- **At run time.** If `python3` disappears afterwards (uninstalled, PATH
  changed), the step that cannot start it fails with the same message instead of
  the raw OS error (`failed to spawn …: No such file or directory`). Any other
  program that fails to start keeps the raw error.

| Platform | What to install |
| --- | --- |
| macOS | `brew install python3` |
| Debian, Ubuntu | `apt install python3` |
| Windows | Python from the Microsoft Store, whose build registers `python3`. The python.org installer provides `python` and the `py` launcher, **not** `python3`: with it, copy `python.exe` to `python3.exe` in the same folder (and keep that folder on the PATH). |
| Anywhere else | https://www.python.org/downloads/ |

Resolving `python` or `py -3` instead was considered and left out on purpose: the
workflow is shared as it is (no per-machine command written into a step), and the
guarantee « the name in the step is the name that is checked » stays simple.

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
| `reviewer` | never (informational) | who reviews each delivery, which model was picked and how to change it |

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
report's `suggested_reviewer`, which is, in this order:

1. what the `reviewer` workflow variable names, when it is set;
2. else the first remote model this instance has configured: LiteLLM once its
   proxy and key are set, else NVIDIA once its key is set, with the
   reasoning-tier model, else the default-tier one. This is an arbitrary pick
   (the first one that is configured), which is why it is shown;
3. else none: the room keeps its setup and the human reviews from the UI.

**The choice is visible before anything runs.** The preflight prints a
`[ok] reviewer: …` line naming the model it picked (or that none is configured)
and how to change it, and the campaign room opens with the same reviewer in its
rules message.

**Overriding it.** Set the `reviewer` variable on the launch form (empty keeps
the pick above). The value is checked by the preflight, before `gh` is called:
a value that cannot review is refused with the accepted forms.

| `reviewer` | Reviews with |
| --- | --- |
| *(empty)* | the first configured remote model, else you |
| `human` | you, from the review card (no agent answers in the room) |
| `litellm:<model>`, `nvidia:<model>`, `ollama:<model>` | that remote model (the model is everything after the first colon, so `ollama:llama3:8b` works) |
| `agent:<Agent>:<model>` | a local agent with the model it reviews with, `<Agent>` as Kronn names it (`ClaudeCode`, `Codex`, …) |

A named connection (a `Custom` remote model) is not expressible in the variable:
pass `reviewer` with its `connection_id` to `POST /api/orchestration/campaigns`.

The choice is stored on the run (`reviewer`, migration 292), announced in the
room and shown on the campaign panel. The human can always decide from the UI,
whoever the reviewer is.

## Limits

- The workflow stored on the maintainer's instance (`61f7a323`) is instance
  data and is not rewritten by Kronn: install the shipped workflows and retire
  that copy.
- **The board refresh is gone.** The stored workflow ended by triggering the
  contribution board's own workflow (`POST /api/workflows/<id>/trigger`, best
  effort) so that the board showed the new autoCode phase under the issue right
  away. That call named the board workflow by its id, an instance value, so it
  is not in the shipped workflow: the shipped steps never address another
  workflow of the instance. A launch and a publication work without it; only the
  board's display of the new phase waits for the board workflow to run. What the
  owner does instead, on their own instance: run the board workflow from the
  Workflows page after a launch, or keep the trigger in a private copy of the
  « start the campaign » workflow by adding, as its last step, a sub-workflow
  step on the board workflow (picked from the list, so no id is typed). Either
  way the id stays out of what Kronn ships.
- `gh` is the only forge tool read; a non-GitHub remote resolves to no project.
