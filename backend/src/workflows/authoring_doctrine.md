## Authoring doctrine

This schema says what is POSSIBLE. This doctrine says what is PREFERABLE when several step types can do the same job. `workflow_step_schema` (field `authoring_doctrine`) and the `workflow-architect` guide serve this exact text; read it before composing or editing a workflow.

### Decision tree

For each step, take the first line that fits.

1. A fixed list of items used as data → `JsonData`.
2. One dataset built from several independent sources → `CollectApiData`; saved Quick APIs (`qa_list`) and Quick Execs (`qe_list`) first, inline `quick_exec` only for a one-off.
3. Selecting, renaming, aggregating or coercing typed JSON → `TransformData`, never an Agent or an Exec.
4. The result feeds a Live Page → `PublishPageData` (`page_list` first; reuse a Page).
5. Sending something to a webhook URL → `Notify`.
6. An HTTP call to a service that has a plugin (`mcp_list`) → `ApiCall`; the same call on N items → `BatchApiCall`; several calls forming one dataset → `CollectApiData`. A saved Quick API first (`quick_api_id`). No plugin yet: propose a Custom API plugin to the user before anything else.
7. Deterministic logic on data already in the run (parse, compare, compute, filter, build, test, git) → `Exec`.
8. A human must approve before the run continues → `Gate`.
9. The step must read, judge or write in natural language → `Agent`, after the cheaper types have collected and shaped its input. The same Agent task on N items → `BatchQuickPrompt`.
10. A chunk reused by several workflows → `SubWorkflow` (the parent waits for the result) or `TriggerWorkflow` (fire and continue).
11. Already-planned subtasks to implement and review → `DelegateSubtasks`; a board of tagged tasks behind a Page → `TaskBoard`.

### At equal token cost, choose for maintainability

An Exec that runs `gh api` costs zero tokens, like an ApiCall. Cost does not tell them apart; these rules do.

- **A service with a plugin is reached through the plugin.** Every HTTP call to it goes through ApiCall, BatchApiCall or CollectApiData. Never `gh`, `curl`, `wget` or an HTTP library inside an Exec for such a service. The plugin's configured credentials, the API call log, rate limiting and the "no change" classification Kronn infers for a successful GET or HEAD ApiCall all disappear with an Exec: the machine's own `gh` login serves instead, the call leaves no trace, and the run reads as a changed one.
- **Exec is for logic only.** Its inputs come from earlier steps (`exec_stdin` for a large one); it does not fetch what a plugin can fetch. An Exec that changes nothing declares it by printing a `KRONN_NOOP` line.
- **One script lives in one place.** Never paste the same script or inline code into several steps (one run observed 29 Exec steps carrying the same 59 KB script, each with another mode argument: 1.7 MB to fix 29 times). Share it through a saved Quick Exec (a CollectApiData source's `quick_exec_id`) or through `exec_script_files` (a file committed in the home project's repository, run from an approved copy), and pass the variant as an argument. If one script serves many modes, ask whether one collection step and a `TransformData` would do.
- **State passes through step outputs.** Later steps read `{{steps.<name>.data…}}`, `input_from` or `value_from`; a loop carries values with `---STATE:k=v---` and `{{state.k}}`. Never route state through a file in the artifacts directory (a `context.json`): the run view then shows only opaque steps, and no step can be re-read or re-run on its own.
- **When no native step answers a need, say so.** Tell the user which need is uncovered (see the ApiCall limits below) and confine the workaround to one targeted step. Do not move a whole collection to Exec because one request is awkward.

### ApiCall limits, as trade-offs

- **Pagination.** `api_pagination: {"type":"Auto"}` issues ONE request: it does not walk pages. To walk, name the strategy: `LinkHeader` (GitHub REST: `{"type":"LinkHeader","page_size_param":"per_page","page_size":100}`), `Offset`, `Cursor` or `Page`. The walk merges the page items back into one response, so `api_extract` is written as for a single page. It detects the array in a top-level object (the longest one) or a bare top-level array.
- **Past the cap.** The default cap is 50 pages (`max_pages`, settable per step). Reaching it is neither a failure nor silent: the items gathered so far are returned, the step stays a success, and its output carries the signal `PAGINATION_TRUNCATED`. When completeness matters, branch on it (`on_result: [{"contains":"PAGINATION_TRUNCATED","action":{"type":"Stop"}}]`) or narrow the filter; never trust an unchecked walk to be complete.
- **GraphQL.** An ApiCall can send one query: `api_method: POST`, `api_endpoint_path: /graphql` on a plugin whose base URL serves it (GitHub: `https://api.github.com`), `api_body: {"query": "...", "variables": {...}}`. Write GraphQL braces with a space (`{ a { b } }`): `{{` is the template syntax. A plugin access policy, when set, can restrict the path. Kronn does NOT paginate GraphQL: `Cursor` feeds the cursor into the query string, not into `variables`, and only a top-level array is merged, so a cursor-paged GraphQL query returns its first page. Prefer the REST endpoint with `LinkHeader`. If only GraphQL has the data and the user needs all of it, tell them, and give that one query a single targeted Exec step (`gh api graphql --paginate`, `gh` allowlisted); every other read stays an ApiCall.

### Size and shape

- A workflow holds 1 to 20 steps, and `on_failure` 0 to 20. There is no ideal count: one step, one responsibility. Past 20, move a coherent chunk into a `SubWorkflow`.
- The last step is whichever delivers the result: `PublishPageData` for a Page pipeline, `Notify` for a webhook, an Agent in FreeText for a human-readable report. Structured output is for handing data to a later step.
- Exec accepts `bash -c`, `python3 -c` and `node -e` when the binary is allowlisted: run values travel as later arguments, never inside the code (see Exec). An Exec line you write that takes run values waits for a human's approval; tell the user which steps. Reach for an Agent plus a shell only when the step needs reasoning.
