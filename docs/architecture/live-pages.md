# Live Pages architecture (v0.10.0)

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
editor, a side-by-side line diff against any earlier revision and an explicit
restore-to-draft action. Restoring does not mutate history: saving the restored
draft creates another immutable revision.

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

Workflow export bundle v2 includes each statically referenced Page's current
HTML template and dataset contract, then remaps every `PublishPageData.page_id`
on import. Retained values and publication/run history are excluded to avoid
silently leaking production observations through a Workflow definition file;
the imported Page is populated by its next run.

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
