# Native Page tools (KT-705)

HTTP discussion agents expose `page_list`, `page_get`, `page_create`,
`page_update_html` and `page_add_dataset`. They call the same Page handlers as
the MCP bridge, including transactional dataset validation, immutable HTML
revisions and inert action ingestion. The dispatcher returns real stored
objects, not an HTML preview.
[src: file: backend/src/api/agent_tools.rs:1675]
[src: file: backend/src/api/agent_page_tools.rs:133]
[src: file: backend/scripts/disc-introspection-mcp.py:7594]

Native and bridge access are two policies, not one:

- **Native executor:** lists, reads and writes project-less Pages and its own
  project's Pages, never another project's. Writes on project-less Pages are
  allowed today; that rule awaits Romu's decision and is unchanged here.
- **MCP bridge token (0.14.3 layer B):** reads project-less Pages and its own
  project's Pages, but writes (`PATCH`, HTML, datasets) only Pages that sit in
  its bound project or that it created. A project-less Page is read-only to a
  token.

Both resolve a selector the same way, through
`db::live_pages::resolve_live_page_id`: id, then live slug, then a slug the
Page was renamed from. A renamed Page is therefore in scope under its old slug,
and another project's executor or token is refused under any of its slugs.
`page_update_html` takes `slug` alone or with `html`, like the MCP tool: every
argument (slug type, non-blank HTML, 1,000,000-byte UTF-8 limit) is checked
before the first write, the rename runs first, and if the HTML revision then
fails the error names the Page id and the slug it was already renamed to.
`page_get` hides workflow and discussion links outside that scope.
Creation inherits missing bindings from the current executor, refuses a
project or origin discussion outside its scope, and derives revision
authorship from the executor instead of model arguments. Standalone Pages accept an empty dataset array.
An unknown project or discussion fails through the shared foreign-key checks;
a failed creation does not leave a partial Page.
[src: file: backend/src/api/agent_page_tools.rs:153]
[src: file: backend/src/api/agent_page_tests.rs:19]
[src: file: backend/src/api/agent_page_tests.rs:87]
[src: file: backend/src/db/sql/123_live_pages.sql:3]

The default catalogue remains complete. `KRONN_TIERED_TOOLS=1` moves these five
declarations into the `pages` family; `tools_load({family:"pages"})` returns
their real declarations to the runner. Both discussion paths execute the same
handlers. By Romu's decision on 2026-09-27, workflow Agent steps expose all five
Page operations and `tool_manual`, within the run's project scope as above.
Planning mutations remain forbidden in workflow Agent steps. The bounded
catalogue permits only `page_create`, `page_update_html` and `page_add_dataset`
among tools whose names contain `create`, `update`, `remove` or `link`.
Workflows also publish data through the `PublishPageData` step.
Rendering still uses the existing Page viewer and its network-denying CSP;
persisting an action block never launches its automation.
[src: file: backend/src/api/agent_tools.rs:327]
[src: file: backend/src/api/agent_tools.rs:391]
[src: file: backend/src/api/agent_tools.rs:1344]
[src: file: backend/src/api/agent_tools.rs:1577]
[src: file: backend/src/api/agent_tools.rs:5236]
[src: file: backend/src/api/agent_page_tests.rs:232]
[src: file: backend/src/api/agent_page_tests.rs:339]
[src: file: backend/src/api/agent_page_bench.rs:84]
[src: file: frontend/src/lib/__tests__/live-page-sandbox.test.ts:29]
[src: user: 2026-09-27: KT-705 review 2 relays Romu's decision to allow workflow Agent Page creation and modification]

## Reproducible validation

From `backend/`, always with a throwaway data directory so no test touches the
real Kronn data (operator secret, config):

```sh
KRONN_DATA_DIR="$(mktemp -d)" cargo test --offline --lib page_tools -- --nocapture
```

It checks selectors, scope inheritance and refusals, malformed requests and rollback,
revision preservation, dataset conflicts, inert actions, workflow Agent reads
and writes with and without a project, progressive loading and the serialized
catalogue cost. The pipeline fixture saves a disabled workflow through the
native dispatcher and executes
`JsonData` followed by `PublishPageData` through the real workflow runner, then
checks the Page and publication ledger.
[src: file: backend/src/api/agent_page_tests.rs:19]
[src: file: backend/src/api/agent_page_tests.rs:312]
[src: file: backend/src/api/agent_page_bench.rs:84]

The ignored local-model trial uses an in-memory database and a seeded
`mon-suivi` Page. It retains the prompt, all attempted calls and outcomes,
catalogue size, provider token telemetry, saved workflow, execution outcome
and final Page in `KRONN_BENCH_REPORT`, including failed attempts. It exposes
the actual catalogue but only executes Page tools and disabled workflow
authoring against this fixture. The harness validates the authored data
pipeline before executing it; it does not add a workflow-launch tool.
[src: file: backend/src/api/agent_page_bench.rs:6]
[src: file: backend/src/api/agent_page_bench.rs:33]
[src: file: backend/src/api/agent_page_bench.rs:135]
[src: file: backend/src/api/agent_page_bench.rs:180]

For each attempt, choose an installed local model and a new report filename:

```sh
KRONN_DATA_DIR="$(mktemp -d)" \
KRONN_PAGE_BENCH=1 KRONN_BENCH_MODEL='<installed model>' \
KRONN_BENCH_REPORT='<new full-mode report.json>' \
cargo test --offline --lib bench_native_page_workflow -- --ignored --nocapture

KRONN_DATA_DIR="$(mktemp -d)" \
KRONN_TIERED_TOOLS=1 KRONN_PAGE_BENCH=1 \
KRONN_BENCH_MODEL='<same installed model>' \
KRONN_BENCH_REPORT='<new progressive-mode report.json>' \
cargo test --offline --lib bench_native_page_workflow -- --ignored --nocapture
```

Unset `KRONN_TIERED_TOOLS` for the first command. If the environment points
`CARGO_TARGET_DIR` outside the writable worktree, override it with a local
build directory. A transport failure with zero calls does not establish model
quality or satisfy the real-model acceptance criterion.
[src: file: backend/src/api/agent_tools.rs:391]
[src: file: backend/src/api/agent_page_bench.rs:180]

## Catalogue size

The native surface measures 2,786 additional serialized bytes: the full
discussion catalogue is 64 tools / 47,520 bytes, versus 44,734 bytes with the
five Page declarations removed. The opt-in core is 15,850 bytes; the
separately serialized Page family is 2,787 bytes (including its array
envelope), of which KT-1098's optional `slug` on `page_update_html` costs
25 bytes. These are byte counts, not provider token measurements. The bridge
declarations keep their own byte ceiling (85,735 bytes, `mcp_surface_budget.py`).
[src: file: backend/src/api/agent_page_tests.rs:312]
[src: file: backend/scripts/ci/mcp_surface_budget.py:65]
