# Artifacts architecture (formerly Live Pages)

## Product naming and compatibility

The UI calls this library **Artifacts**. Existing `page_*` MCP tools,
`/api/pages` endpoints, `#page/…` and `#pages/mosaic?…` URLs, `PublishPageData`
workflow steps, stored IDs and sandbox event names keep their established
contracts. An Artifact is the same persisted resource; renaming its product
label does not migrate or duplicate it. Historical code and documentation use
“Page” for that domain model.
[src: file: frontend/src/lib/i18n/locales/en.ts:55]
[src: file: frontend/src/lib/live-page-navigation.ts:1]

## Portable JSON bundles

The Artifact export action downloads a `kronn.artifact` version-1 JSON bundle.
It contains the current HTML, named dataset values (including JSON `null`
distinct from an unpopulated snapshot), schemas, retention settings and every
retained time-series point with its observation time and deduplication key.
It follows parsed action targets and workflow dependencies transitively,
including saved workflows that publish into the root Artifact, their failure
steps, sub-workflows, Quick Prompts, Quick APIs, Quick Execs and secondary
Artifacts. Secondary Artifacts do not pull in unrelated incoming publishers.
Export uses one read transaction. Missing dependencies, unsupported bundles and
bundles exceeding 16 MiB or 512 resources fail explicitly; nothing is silently
truncated. Historical HTML revisions, run/publication history and discussion
links are not included.
[src: file: backend/src/models/artifact_portability.rs:1]
[src: file: backend/src/api/artifact_portability.rs:1]

Import is available in the library and in a workflow's publishing step before
the library is activated. The user selects a project and reviews each planned
creation, reuse or conflict before committing. Reuse requires a matching source
identity (or a recorded earlier import) and definition; names alone never match.
Changed definitions require an explicit local-version or new-copy choice.
The root Artifact is always new. Publishers and action-bearing dependencies
that need different destinations are copied, with typed references remapped;
literal text, CSS and unrelated JavaScript are not rewritten. An explicit reuse
that cannot target the new destination is rejected. New workflows are disabled;
existing workflows keep their state and no automation is executed by import.
For every Quick Exec that will be created, the preview shows the exact saved
command and argument array. The user must approve each command and refresh the
preview before import is enabled. The server requires these explicit source
identities in `approved_quick_exec_ids`; the reviewed digest includes approvals
and bundle content, so changing either invalidates the previous review.
Reused local Quick Execs require no new approval. Choosing another file clears
all approvals. New Quick APIs show their saved method, endpoint and plugin;
an unspecified method is shown as unknown, not inferred.
[src: file: backend/src/api/artifact_portability/import.rs:1]
[src: file: frontend/src/components/ArtifactImportDialog.tsx:1]

Configured connections, their stored credentials, agent-library entries and
local files are not bundled. Definitions retain those references; preview lists missing known
plugin/model connections, skills, profiles and directives, and offers an
explicit import-and-configure-later confirmation. File paths and endpoint
availability are not checked. Copied webhook approval tokens are cleared.
The selected project applies to newly created automation definitions and
Artifact action scopes. A deliberately reused local definition is unchanged.
[src: file: backend/src/api/artifact_portability/import.rs:1]

`GET /api/pages/{id}/export`, `POST /api/pages/import/preview` and
`POST /api/pages/import` retain the established Page API naming. Import requires
the digest from the reviewed preview, recomputes it against current local
definitions and import identities, and refuses stale decisions. All resources,
datasets, points, origin mappings and the library activation commit in one
SQLite transaction; an error rolls everything back. Origin mappings identify
earlier imported copies without overwriting their source or local definitions.
[src: file: backend/src/lib.rs:1]
[src: file: backend/src/db/sql/188_artifact_import_origins.sql:1]

## Status

Shipped in the first v0.10.0 vertical. Later phases may extend the rendering
catalogue, but they must preserve the storage and isolation boundaries below.

## Product boundary

A Live Page is a readable HTML report. It may be standalone HTML, use persisted
mock datasets during design, or receive production data from Kronn workflows.
It is not a monitoring backend and it does not call third-party APIs from the
browser. `ApiCall` and `CollectApiData` are responsible for collection,
`TransformData` can shape their typed JSON, and `PublishPageData` is responsible
for the durable hand-off to a Page.

A Page is a shared destination, not a child owned by one Workflow. Each
`PublishPageData` step stores the target Page ID (or legacy slug), so several
Workflows may publish different datasets into the same Page. The UI exposes
that configured relationship in both directions: a selected Page can be opened
from the step, and the Page viewer lists every saved Workflow step that targets
it. This is a configuration link; the publication ledger remains the source of
truth for actual run provenance. A compact status control in the viewer header
opens the three newest successful ledger entries as a vertical timeline without
permanently reducing the report viewport. Each entry distinguishes datasets
whose JSON actually changed from datasets that were checked but stayed
identical, includes append/retention point counts, and links to the exact
Workflow run when that provenance still exists. Operators can therefore tell a
successful check from a meaningful data update without opening the complete
Workflow history. Every enabled linked Workflow also exposes a compact run
action in this control; disabled Workflows remain visible but cannot be launched
until the operator enables them from the Workflow screen.

The header also reports the compact JSON payload size retained by all datasets.
The dropdown breaks that total down per dataset so operators can spot an
unbounded collection or time series before it becomes expensive. The measure
includes current snapshot/collection JSON and retained time-series payloads;
it excludes schemas, SQLite row metadata and indexes.

The first vertical targets small operational reports such as an Adobe analytics
follow-up: current indicators, a bounded time series and a table that changes
after each manual or cron run.

Pages form a library with persistent `pinned` and `archived` state. Deletion is
explicit and cascades the Page's revisions, datasets, points, publications and
links. The library deliberately reuses the Discussion sidebar interaction
model: search, favorite shortcuts, a canonical active section, multi-selection
actions and a collapsed archive section.

Multi-selection can also open two or more Pages in one external mosaic route.
Two-Page presets support columns or rows; three-Page presets place the first
selected Page above, below, left or right of the other two; four or more Pages
use a responsive automatic grid. Every tile independently reuses the same
opaque `sandbox="allow-scripts"` rendering and parent-fed dataset bridge as the
single standalone Page, so combining reports does not merge their HTML, CSS or
JavaScript contexts.

A Page may also be linked to one or more Discussions. `created_from` records
the room where an agent authored the Page; `attached` is an explicit later
association. These links are independent from Workflow publisher links and are
deleted automatically if either side is removed.

## Domain model

```text
Page
├── immutable HTML revisions
├── named datasets
│   ├── snapshot     (replace)
│   ├── time_series  (append observations)
│   └── collection   (upsert by a stable key)
└── publication ledger (workflow/run provenance)
```

The HTML revision and data revision are independent. A cron run normally
changes data only. Editing the presentation creates an immutable HTML revision
and never rewrites the historical document used by a prior publication. The
Page editor exposes that revision list, an HTML-highlighted line-numbered
editor, and a code/preview comparison against any earlier revision. Preview
renders the selected revision and the current draft side by side on wide screens
using the same static isolated HTML policy as Project Code; it has no Live Page
action bridge. An explicit restore-to-draft action remains separate from the
comparison. Restoring does not mutate history: saving the restored draft creates
another immutable revision. `[src: file: frontend/src/components/HtmlCodeEditor.tsx:70-131]`

JSON is the dataset payload format. Snapshot and collection values are stored
as one JSON value. Time-series observations are stored as individual rows so a
new point does not rewrite the full history.

CSV export normalizes that retained JSON into tabular rows: top-level arrays
become rows, a single array inside an object envelope is expanded while scalar
metadata is repeated, parallel nested arrays are zipped by index, and matrix
arrays receive stable `column_N` headers. Time-series exports apply the same
normalization per observation and retain `observed_at` plus
`workflow_run_id`. `[src: file: frontend/src/lib/live-page-csv.ts:1-106]` The
export uses a semicolon for French and Spanish UI locales so spreadsheet tools
configured with those regional separators open columns directly; other locales
keep the standard comma. `[src: file: frontend/src/pages/PagesPage.tsx:300-329]`

## Theme and refresh behavior

The sandbox receives Kronn's `data-theme` before the Page markup is parsed.
Subsequent theme changes arrive through `kronn:page-theme` messages, so they
do not reload the iframe. Pages can style explicit `light` and `dark` values
on their root element. A `prefers-color-scheme` fallback should exclude an
explicit light value, for example with `:root:not([data-theme="light"])`.
Custom theme names can use the Page's own fallback rules.
`[src: file: frontend/src/lib/live-page-sandbox.ts:1]`

The embedded viewer, standalone Page and mosaic skip automatic data
publication when the Page id, slug, title, data revision and dataset count
are unchanged. This preserves local state during quiet polling cycles.
A newly loaded iframe always receives the current data. A real publication
increments the data revision and still reaches the Page; preserving expanded
rows or scroll position across that render is the Page's responsibility.
`[src: file: frontend/src/hooks/useLivePageActions.ts:1]`

## Publication contract

One `PublishPageData` execution contains one or more writes. The database
applies all writes and the publication-ledger insert in one SQLite transaction.
The visible page therefore never combines datasets from two partial publishes.

Supported operations:

- `replace`: replace the complete snapshot value;
- `append`: add one value or every value of an input array as observations;
- `upsert`: insert or replace collection entries using a declared key field.

Every successful publication increments the Page data revision once and stores
its workflow run id when available. Dedupe keys make replayed append writes
idempotent. Retention is enforced in the same transaction with `max_points`
and optional `max_age_days`; the initial implementation deletes the oldest raw
observations and deliberately does not aggregate them.

The same transaction compares each write against the stored dataset value.
The publication ledger stores only the names of changed and unchanged datasets,
not duplicate JSON snapshots. `replace` and `upsert` use structural JSON
equality; `append` is changed only when a point was inserted or retention
removed one. Pre-delta ledger rows are conservatively backfilled as changed
because their historical before-value cannot be reconstructed.

## Rendering and trust boundary

The frontend renders a Page in an iframe with `sandbox="allow-scripts"` and no
`allow-same-origin`. The generated document receives a restrictive CSP and no
credentials. It cannot fetch APIs: the authenticated parent loads datasets from
Kronn, then posts a versioned, validated snapshot into the frame.

PDF and DOCX export starts from the materialized iframe DOM, not from the stored
HTML template. A request/response `postMessage` bridge keeps the opaque-origin
boundary intact while capturing the current dataset-driven document. The same
browser engine that displays the preview paginates that document into local PNG
images (canvas charts are rasterized first), so WebView-only CSS does not have
to be reinterpreted by WeasyPrint. Export scripts are removed from the captured
copy. The Docs sidecar wraps the browser-rendered pages in PDF or fixed-layout
DOCX; static Discussion documents without these images keep the original HTML
rendering path and selectable PDF text.

The browser's local storage is never a source of truth for Page content or
datasets. This keeps refresh, export/import and transfer to another Kronn
instance deterministic.

### Inline Kronn actions

A Page revision may pair a visible CTA with one inert action data island:

```html
<button data-kronn-action="frame-ticket"
        data-kronn-bindings='{"ticket":"KT-538"}'>Frame ticket</button>
<script type="application/kronn-action" data-action-id="frame-ticket">
{"kind":"quick_prompt","target_id":"<existing QP id>","values":[{"name":"ticket","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).id>"}]}
</script>
```

The action block is parsed and validated in the same transaction as its HTML
revision. The sandbox cannot launch it: a user-activated click sends only the
stable action reference, row selectors and anchor geometry through the private
parent bridge. The authenticated parent then opens the shared native action
card. A second explicit human click performs the server-side preflight and
atomic launch claim. Action references use at most 256 URL-safe unreserved
characters (`A-Z`, `a-z`, `0-9`, `.`, `_`, `~`, `-`); malformed script types,
prefixed lookalike attributes and stale proposals removed from the current
revision fail closed.

`dynamic_binding` references may resolve `page.id`, `page.slug`, `page.title`,
a snapshot field, or a collection row selected with `find(<field>)`. The click
supplies only the selector; Kronn rereads the current dataset value server-side.
Project environment and Kronn context references use the common execution
variable resolver and are never copied into Page HTML or the action row.

An action block is a template, not a single proposal: a Page instantiates it
once per dataset row, so one `action_ref` can draw forty buttons. The model
keeps the offer and the act apart. The declaration, identified by
`(page_id, action_ref)`, holds the target and the value contract, follows every
republished definition and never carries execution state; a launch is a row of
its own in `live_page_action_launches`, identified by the binding it was
clicked on (the sorted row selectors, empty for an unbound CTA). The
idempotency guard applies to that binding: clicking a row whose launch is still
running shows that run, clicking any other row launches it, and a finished
launch never disarms the button. A launch freezes the revision, target and
values it ran against, so it stays true history and becomes explicitly stale
when the Page moves on. The API keeps speaking one `LivePageAction`: the
declaration before a click, the declaration joined to its launch afterwards,
with `id` naming the launch so a card follows its own run.
`[src: file: backend/src/db/live_page_actions.rs]`

Action launch history retains at most 1,000 terminal rows per
`(page_id, action_ref)`, shared across bindings and revisions. The launch involved
in the current write is retained, then the most recently finished rows; pending,
launching and running rows are exempt. A new launch, decline or completion
reconciles stored active states against their real runs/first agent turns and
prunes terminal overflow in the same write transaction. Existing excess history
is therefore cleaned on the next such write, not during a read or at startup.
Reconciliation reads lifecycle metadata rather than result/step-output/stderr
payloads. A cleanup failure rolls back the associated mutation.
[src: file: backend/src/db/live_page_actions.rs:625]
[src: file: backend/src/db/shared_runs.rs:147]

Pruning removes the Page launch snapshot only: shared results, result discussions,
Page-to-discussion links and declarations remain. A row whose last snapshot aged
out no longer displays that historical result in the Page; retained runs and
discussions remain accessible through their own history. An old pruned launch
handle returns not-found and never falls back to a new launch. Only an explicit
click on the still-current declaration can start another execution.
[src: file: backend/src/db/live_page_actions.rs:625]
[src: file: backend/src/db/live_page_action_retention_tests.rs:1]

The Page reads back how each row went. `GET /api/pages/{id}/action-launches`
returns the latest launch per row (declines excluded), polled every few seconds
while any row runs. The parent posts those states to the iframe as
`kronn:page-action-states`, and the bridge marks every matching
`[data-kronn-action]` with `data-kronn-action-state` and `aria-busy`, including
rows a Page script renders later. A zero-specificity default indicator is
injected; the author's own CSS wins. Clicking a row that is still running
reopens its run instead of a blank offer, clicking the button of the open card
closes it, and the card also closes with its × or Escape. A row that has run
reopens on its latest launch, finished or not, and the card offers to go back
to the offer and launch the same row again; a Discussion card never does,
since a fence carries one intention.

Once launched, a card tells what the run produced, the same way on a Page and
in a Discussion. A workflow's steps are listed (name, type, status, duration)
instead of its raw result, and `GET /api/runs/{id}/outcome` gathers the
discussions of the run's whole tree, because a `BatchQuickPrompt` step opens
its discussions under a child batch run. Each one carries its agent's state
and the head of its latest answer, where agents put their verdict. A launch
whose result is a discussion reads `GET /api/discussions/{id}/outcome`.
`[src: file: backend/src/db/run_outcome.rs]`
`[src: file: frontend/src/components/RunOutcomePanel.tsx]`

Agents learn this contract where they write Page HTML: the MCP manuals of
`page_create` and `page_update_html` share one text (one block per row,
`data-kronn-action-state`, what the card shows), `page_update_html` adds that a
block's reference must stay stable across revisions since it ties a button to
its past launches, and the `workflow-architect` skill carries the same rules.
`[src: file: backend/scripts/disc-introspection-mcp.py]`
`[src: file: frontend/src/lib/live-page-sandbox.ts]`
Active and terminal execution rendering delegates to the common
`RunStatusCard` contract used by Discussions. A Quick Prompt result records
both the action's result-discussion anchor and the existing Page-to-discussion
relationship in one transaction, so either side remains traceable after a
reload or backend restart.

The embedded Page viewer, the standalone tab and every mosaic tile share one
`useLivePageActions` hook and the same `LivePageActionCard` rendering, so the
load → validate → activate → mutate lifecycle is identical everywhere: each
surface loads its own action list, fails closed on an `action_ref` absent from
that list, keeps a launch on the card of the click that started it (never on
the shared offer, and never on a later click's card if the answer arrives
late), and (for mosaic) keeps one tile's action state fully isolated from
its siblings — a valid or fail-closed click in one tile never affects another,
even when two tiles share the same `action_ref` string for different Pages.
`[src: file: frontend/src/hooks/useLivePageActions.ts]`
`[src: file: frontend/src/components/LivePageActionOverlay.tsx]`
The standalone tab and mosaic tiles have no Dashboard shell to navigate
within, so a terminal action's "open discussion" jump seeds the same
session-storage reload checkpoint Dashboard already reads on mount
(`dashboard-navigation.ts`) and opens it in a fresh same-origin tab instead of
switching in place. `[src: file: frontend/src/lib/live-page-navigation.ts]`

Workflow export bundle v2 includes each statically referenced Page's current
HTML template and dataset contract, then remaps every `PublishPageData.page_id`
on import. Retained values and publication/run history are excluded to avoid
silently leaking production observations through a Workflow definition file;
the imported Page is populated by its next run.

A Workflow import commits its entire bundle in one SQLite transaction: Pages,
revisions, datasets, capability activation, Quick Prompts and their versions,
Quick APIs, Quick Execs, root and child workflows. A late validation or SQL
failure rolls everything back and preserves existing resources. The Page
creation helper accepts the caller's transaction; starting and committing a
separate Page transaction inside an import would break this guarantee.
[src: file: backend/src/api/workflows.rs:2277]
[src: file: backend/src/db/live_pages.rs:126]

Kronn-bundled declarative charts are the default. Custom JavaScript and D3 are
an advanced escape hatch and remain subject to the same iframe, CSP, payload
and runtime limits.

## Progressive disclosure

The Pages navigation entry is hidden until the capability has been activated
by the first successful Page creation or import. Activation is durable and is
not reversed when the last Page is deleted: users who already know the feature
must retain access to templates and creation affordances.

The natural first entry point is a Workflow step. A minimal source can publish
directly, while a multi-source report uses the deterministic pipeline:

```text
ApiCall -> Update a Page -> existing Page | create visualization

CollectApiData -> TransformData -> Update a Page
 Quick APIs / CLI   JSON recipe       durable datasets
```

Creating a visualization opens a draft studio with mock data, a reused API test
response, or an explicitly requested real API call. Agent-generated components
are local to the Page until the user deliberately promotes them.

## Deterministic data pipeline

`CollectApiData` fans out to 1–50 saved Quick APIs or saved shell-free Quick Execs
with a bounded concurrency of 1–20 (5 by default). Each source has a stable
alias, optional workflow variable overrides and a required/optional policy.
Quick APIs remain the preferred source when a REST spec exists. A reusable
`quick_exec_id` covers CLI-only integrations; an inline `quick_exec` is kept for
one-offs. The resolved bare binary must be present in the
workflow `exec_allowlist`, shell binaries are rejected, arguments remain
separate literals, timeout is bounded to 1–1800 seconds, and stdout is decoded
as `json`, `csv` (an array of objects keyed by the header row), `text`, or
`lines` with a 1 MiB per-stream ceiling. The complete, unmodified typed values are
returned under `sources.<alias>`; execution metadata and
per-source failures are returned under `meta`. An optional failure produces a
successful `PARTIAL` envelope only when another source produced data; a
required failure or a collection where every source failed makes the step
fail. Quick Exec failures prefer the process stderr in the visible summary,
including an actionable login command for an expired AWS SSO session.

Saved sources also expose their `source_id` beside the alias and kind in
`meta.sources`; failure summaries include that identity before the cause.
A deleted required Quick API reports that it does not exist, while HTTP and
JSON parsing failures retain their respective diagnostics. The workflow history
list deliberately omits outputs; expanding a run fetches its full detail and
reports a failed detail read with a retry action. A focused navigation keeps
the full run even when the compact list already contains its id.
[src: file: backend/src/workflows/collect_api_data_step.rs:60-67]
[src: file: backend/src/workflows/collect_api_data_step.rs:508-529]
[src: file: frontend/src/components/workflows/LoadedRunDetail.tsx:1]
[src: file: frontend/src/pages/WorkflowsPage.tsx:804-807]

Rolling windows use the common run-anchored time grammar in source variables,
for example
`{{time.now|shift:-24h|tz:Europe/Paris|floor:hour|fmt:local_iso_ms}}`.
Every parallel source receives a clone of the same durable `started_at` anchor,
so one collection cannot straddle an hour boundary.

`TransformData` consumes one typed context value such as
`steps.collect.data`. Its recipe maps JSONPath sources to dotted output keys
with deterministic operations (`copy`, `count`, `sum`, `average`, `min`,
`max`, `first`, `last`) and optional scalar conversion. It executes no user
code and consumes no model tokens. The Workflow wizard previews the recipe on
mock or copied real JSON through the same backend function used at runtime.

The wizard provides a guided design loop for this pipeline. **Test all
sources** executes the collector once, displays the aggregate JSON and each
source's status, then keeps that result as an in-memory sample for every linked
`TransformData` step. The sample is deliberately not persisted with the
workflow because it may contain production data. In the transform editor, the
operator selects a previous step, clicks fields in the JSON tree to create
JSONPath mappings, and sees the deterministic output preview update. **Test
pipeline to here** refreshes the upstream collector and sample in one action.
Manual JSON remains available under an advanced disclosure for offline/mock
design.

Directly below a `CollectApiData` editor, the wizard also presents the two
normal continuations. **Add Transform** inserts a `TransformData` step directly
after the collector and binds `input_from` to its typed output. **Add Update a
Page** inserts a `PublishPageData` step with a replace write bound to that same
output. Inline help explains when the stable business contract of a transform
is preferable to publishing the lossless aggregate directly, and points users
to the Automation catalog for creating reusable Quick APIs and Quick Prompts.

The visual mapper exposes `sources` as business input and keeps collector
`meta` as diagnostic information in the collector preview. Its output panel is
not a mirror of all available data: it is the exact JSON produced by the active
mappings. The active mapping list makes that distinction explicit. **Use
complete sources** replaces the current recipe with one copy mapping per source,
which provides a clean reset when an operator wants the lossless aggregate.

This split keeps collection lossless and debuggable while allowing a compact,
stable Page contract. A Page can therefore change its presentation without
coupling the template to every upstream provider response.

## Agent and MCP authoring contract

The built-in Workflow Architect and the `kronn-internal` MCP expose the same
twelve-step taxonomy. For a Page pipeline, an agent must discover dependencies
before composing the workflow:

1. `qa_list` resolves every saved Quick API used by `CollectApiData`; `qe_list`
   resolves saved Quick Execs. For a missing CLI collector, the agent calls
   `qe_create_draft`, validates it with `qe_run`, references `quick_exec_id`,
   and adds its bare command to workflow `exec_allowlist`.
2. `page_list` resolves a shared Page destination. If none matches and the
   user authorized creation, `page_create` creates the HTML revision and named
   datasets first.
3. `workflow_step_schema` supplies the canonical `CollectApiData`,
   `TransformData`, and `PublishPageData` shapes.
4. `workflow_create_draft` persists the workflow disabled for human review.

`page_get` returns the current HTML, datasets, retained values, saved workflow
links and discussion links. `page_create` records the current Discussion as
`created_from` when the MCP session has one, accepts an explicit optional
Discussion id, and otherwise creates an unlinked Page from a host CLI. An empty
`datasets` array creates a standalone HTML Page and `initial` values create a
mock-backed design. `page_update_html`
replaces the complete document by creating an immutable revision; it never
changes dataset history. Pages are not a
`KRONN:BUNDLE_READY` category in this vertical, so an agent must never emit a
placeholder Page id or claim that a workflow bundle will create one.

## Scope sequence

1. Persistence, publication operations and provenance.
2. Sandboxed viewer and progressively revealed Pages navigation.
3. `PublishPageData` workflow step and Adobe end-to-end fixture.
4. Agent-assisted draft studio inside the Workflow wizard.
5. Reusable templates/components, email rendition and portable sharing.

Email reuses data contracts and logical components, not the same DOM: email
rendering is static and script-free, while the Page rendition may be interactive.
