# GitHub connection per project

Each project decides whether the agents Kronn launches for it receive a GitHub
token (`GH_TOKEN`, `GITHUB_TOKEN`, `COPILOT_GITHUB_TOKEN`). Design:
[`design/agent-secret-boundary.md` §4.5](../design/agent-secret-boundary.md).

## States

Projects → a project → Overview → **GitHub** row. The same state appears as a
chip in the discussion header (details) of that project.

| State | What agents of the project receive |
|---|---|
| Not connected | No GitHub token. No token was found on this machine either. |
| Available but off | No GitHub token. A token exists on this machine (backend `GH_TOKEN`/`GITHUB_TOKEN`, else `gh auth token`). |
| Connected via your gh login | The machine token, read again at each launch (cached one minute). |
| Connected via a stored token | A token pasted for this project, stored encrypted with the instance key (same format as MCP secrets). |

New projects start *Not connected*. On the upgrade that introduced this
setting, existing projects whose remote is on github.com (from `repo_url`, else
the git config) stay *Connected via your gh login* and show a one-time notice
with a "Turn off" button; dismissing it is kept in the server interface
preferences (`kronn:githubUpgradeNoticeDismissed`).

**Connect GitHub** opens a confirmation stating the risk and the scope of the
gh token. When that token reaches every repository (classic or OAuth scopes
such as `repo`, `workflow`, `admin:org`), the dialog recommends pasting a
fine-grained token restricted to the project's repositories instead.

**Turn off** applies to new launches. Agents already running keep the token
they received until they stop.

## Scope shown

Read from the GitHub API with the token, cached on the project and refreshed
with the refresh button (or on connect):
- classic (`ghp_`) and OAuth (`gho_`, the gh login) tokens: the `X-OAuth-Scopes`
  header of `GET /user`;
- fine-grained tokens (`github_pat_`): the repositories `GET /user/repos`
  returns (first 100). GitHub does not expose a fine-grained token's
  permissions, so only the repositories are listed;
- otherwise "Scope not verified" with the reason (401, unreachable, no scope
  reported).

No token value is returned by the API, written to the logs or shown again.

## What it does not cover

- **Natively**, an agent runs as you and can still use your own `gh` login in
  your home directory (`~/.config/gh`). Kronn cannot close that.
- **Under Docker**, `~/.config/gh` is masked (`scripts/docker-secret-masks.sh`)
  and the `GH_TOKEN`/`GITHUB_TOKEN` passed to the container
  (`docker-compose.yml`) are removed from every agent of a project that is not
  connected.
- Kronn's own GitHub calls (PR creation, `gh pr view` in the git status,
  tracker triggers) keep using the backend's access.
- **Workflow Exec steps** follow the workflow's project: a connected project
  passes its token, otherwise the three variables are removed and a failed
  `gh` step says the project is not connected. Natively `gh` still finds your
  own login, so Exec steps keep working there. Quick execs, `collect_api_data`
  commands and the project exec route keep the backend environment until the
  shared environment builder (KT-1013) governs them.
- Summaries and audits run without a project grant and receive no token.
- A token configured for Copilot CLI in Settings (its own `GH_TOKEN`) is still
  passed to Copilot launches; it is that agent's credential, not the project's.

## API

- `GET /api/projects/{id}/github` → `ProjectGithubConnection`
- `PUT /api/projects/{id}/github` with `{ "mode": "not_connected" | "gh_login" | "stored_token", "token"?: "…" }`
- `POST /api/projects/{id}/github/scope` → re-reads the scope (the machine
  token's when the project is off, for the connect dialog)

`KRONN_GITHUB_API_BASE` points the scope check at another API base (tests use
a local stub).
