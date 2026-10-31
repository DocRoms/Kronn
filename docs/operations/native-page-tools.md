# Native Page tools (KT-705)

HTTP discussion agents expose `page_list`, `page_get`, `page_create`,
`page_update_html` and `page_add_dataset`. They call the same Page handlers as
the MCP bridge, including transactional dataset validation, immutable HTML
revisions and inert action ingestion. Dataset deletion (`page_delete_dataset`, KT-1104)
exists on the MCP bridge only; native discussion agents do not get it. See
[Dataset lifecycle](../architecture/live-pages.md#dataset-lifecycle-kt-1104). The dispatcher returns real stored
objects, not an HTML preview.
[src: file: backend/src/api/agent_tools.rs:1906-1908#execute_page_tool]
[src: file: backend/src/api/agent_page_tools.rs:171-384#execute_page_tool]
[src: file: backend/scripts/disc-introspection-mcp.py:8018-8024#call_page_delete_dataset]

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
[src: file: backend/src/api/agent_page_tools.rs:84-116#page_in_scope]
[src: file: backend/src/api/agent_page_tools.rs:118-169#scoped_links]
[src: file: backend/src/api/agent_tools.rs:4708-4710#project_is_in_scope]
[src: file: backend/src/api/agent_page_tests.rs:18-99#native_pages_follow_mcp_project_scope_inheritance_and_selector_contracts]
[src: file: backend/src/api/agent_page_tests.rs:86-88#standalone]
[src: file: backend/src/api/agent_page_tests.rs:110-173#native_page_validation_failures_leave_no_partial_page]
[src: file: backend/src/db/sql/123_live_pages.sql:3-5#project_id]
[src: file: backend/src/db/sql/124_live_page_library.sql:10-17#live_page_discussion_links]

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
[src: file: backend/src/api/agent_tools.rs:358-363#TOOL_FAMILIES]
[src: file: backend/src/api/agent_tools.rs:2583-2592#declarations_for_family]
[src: file: backend/src/api/agent_tools.rs:1518-1542#workflow_tool_catalogue]
[src: file: backend/src/api/agent_tools.rs:1906-1908#execute_page_tool]
[src: file: backend/src/api/agent_tools.rs:5795-5853#workflow_catalogue_is_bounded_and_contains_no_planning_mutation]
[src: file: backend/src/api/agent_page_tests.rs:300-343#native_page_html_uses_shared_inert_action_ingestion]
[src: file: backend/src/api/agent_page_tests.rs:407-538#native_workflow_page_tools_inherit_run_scope_and_execute_all_operations]
[src: file: backend/src/api/agent_page_bench.rs:83-132#native_page_pipeline_publishes_through_the_real_workflow_runner]
[src: file: frontend/src/lib/__tests__/live-page-sandbox.test.ts:31-36#LIVE_PAGE_CSP]
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
[src: file: backend/src/api/agent_page_tests.rs:18-99#native_pages_follow_mcp_project_scope_inheritance_and_selector_contracts]
[src: file: backend/src/api/agent_page_tests.rs:345-378#native_page_family_loads_and_dispatches_all_five_operations]
[src: file: backend/src/api/agent_page_bench.rs:83-132#native_page_pipeline_publishes_through_the_real_workflow_runner]

The ignored local-model trial uses an in-memory database and a seeded
`mon-suivi` Page. It retains the prompt, all attempted calls and outcomes,
catalogue size, provider token telemetry, saved workflow, execution outcome
and final Page in `KRONN_BENCH_REPORT`, including failed attempts. It exposes
the actual catalogue but only executes Page tools and disabled workflow
authoring against this fixture. The harness validates the authored data
pipeline before executing it; it does not add a workflow-launch tool.
[src: file: backend/src/api/agent_page_bench.rs:6-25#fixture]
[src: file: backend/src/api/agent_page_bench.rs:28-81#publish_draft]
[src: file: backend/src/api/agent_page_bench.rs:134-172#RecordingTools]
[src: file: backend/src/api/agent_page_bench.rs:174-282#bench_native_page_workflow]

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
[src: file: backend/src/api/agent_tools.rs:426-434#tiered_tools_enabled]
[src: file: backend/src/api/agent_page_bench.rs:174-282#bench_native_page_workflow]

## Catalogue size

The native surface measures 2,786 additional serialized bytes: the full
discussion catalogue is 64 tools / 48,141 bytes, versus 45,355 bytes with the
five Page declarations removed. The opt-in core is 15,968 bytes; the
separately serialized Page family is 2,787 bytes (including its array
envelope), of which KT-1098's optional `slug` on `page_update_html` costs
25 bytes. The test prints these figures (`--nocapture`) and only asserts the
Page family stays at or under 4,000 bytes; the absolute sizes move whenever any
other declaration changes, the Page delta does not. These are byte counts, not
provider token measurements. The bridge
declarations keep their own byte ceiling (84,396 bytes, `mcp_surface_budget.py`).
[src: file: backend/src/api/agent_page_tests.rs:380-405#native_page_surface_has_a_bounded_measured_cost]
[src: file: backend/scripts/ci/mcp_surface_budget.py:166#CATALOGUE_MAX_BYTES]
