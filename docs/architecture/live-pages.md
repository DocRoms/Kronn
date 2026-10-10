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
A literal credential typed into an embedded Workflow, Quick API or Quick Exec
(authorization header, secret query/body value, `--token`-style argument) is
replaced by a marker and listed in `redacted_fields`, never its value; the
import preview shows that list. `{{…}}` references are kept.
[src: file: backend/src/core/export_secrets.rs:1]
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

A Page's slug can be renamed (`PATCH /api/pages/{id}` with `slug`, the Page
header, or `page_update_html` with `slug`) under the creation rules: same ASCII
format, unique across all projects. The former slug is kept in
`live_page_slug_aliases`, so links, workflow steps and API calls that still
name it reach the Page. Resolution tries the id, then a live slug, then a
former slug, so an alias never shadows another Page. A former slug stays
reserved for its Page (only that Page may take it back) until the Page is
deleted, so no other Page, import included, can capture its old links.

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

An HTML preview in a persisted discussion message can be promoted with
**Make an Artifact**. The title is editable; the preview's HTML, CSS and JavaScript
are copied unchanged to a new Artifact, with no workflow or command execution.
The creation request includes `discussion_id` and `source_message_id`; the
server verifies their relationship, inherits the discussion's project when no
project was supplied, and resolves title/slug collisions with a fresh suffix.
The Artifact, initial revision and source link are created in one transaction.
The library's source link opens the discussion at that exact message. Removing
the message clears only its source anchor and preserves the Artifact and its
discussion link. Streamed previews without a persisted message id do not offer
this action.
[src: file: backend/src/api/live_pages.rs:365]
[src: file: frontend/src/components/DocPreviewArtifact.tsx:10]

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

A publish that changes a Page (API `POST /api/pages/{id}/publish` or a
`PublishPageData` step) broadcasts the local WebSocket event
`live_page_data_changed { page_id, data_revision }`. The Pages view and the
standalone tab re-read that Page at once and post `kronn:page-data` to the
open frame without rebuilding it; their 30 s poll stays as a fallback.
`[src: file: backend/src/api/live_pages.rs:647]`
`[src: file: frontend/src/hooks/useLivePageDataPush.ts:1]`

## « Ma Todo », the default board (KT-1030)

Kronn installs one board Page and six workflows at boot, once
(`core::default_todo::install_on_boot`, from both the standalone and the
desktop mains). The install goes through the Artifact import machinery
(fresh copies, ids remapped) and is recorded in `default_contents`
(migration 240). Rules:

- **Once.** A `default_contents` row means "handled"; a restart, an upgrade or
  a deleted board never installs it again. `GET /api/defaults/todo` reports
  `installed`, `removed`, `kept_existing` or `not_installed`;
  `POST /api/defaults/todo/install` (human only, absent from the bridge-token
  list) installs a fresh copy, refused while Kronn's board page exists. The
  Pages sidebar offers it when the board is absent.
- **An own todo is never touched.** A Page titled « Ma Todo » / « My Todo » /
  « Mi Todo » / « 我的待办 » or slugged `ma-todo`, `my-todo`, `mi-todo`, `todo`
  makes boot record `kept_existing` and create nothing; the sidebar proposes
  to install Kronn's board next to it.
- **Portable.** Every workflow is `TaskBoard` + `PublishPageData`: no Exec, no
  HTTP, no URL or id in the shipped text. A task's discussion link is the
  instance-relative `#discussion-<id>`.
- **Enabled, not trusted.** The workflows arrive enabled (first-party,
  agentless, needed for the board to work without configuration); no trust is
  approved. The page asks the human to approve `todo-move` once in its details
  (KT-1029) to drop without a card; `todo-toggle` is eligible too, `todo-add`
  and `todo-edit` take typed values and keep their card.
- **No polling.** No workflow has a schedule: every action publishes the board
  it changed, which reaches open views through `live_page_data_changed`.
  `todo-refresh` is a manual button for changes made elsewhere.
- **Drag and drop.** The drop moves the card at once and clicks the move CTA
  inside the gesture. The Page watches that CTA's `data-kronn-action-state`:
  a failed launch puts the card back with the reason, a published board
  confirms it, a refused trusted launch leaves the card proposed with
  « Confirm / Cancel » while Kronn's card is open. States marked before the
  launch (an older identical move) are ignored by launch id.
- **First-view notice.** One dismissible notice says the page runs on
  Kronn's Tasks and Workflows and can be edited (by hand or by an agent), with
  the trust hint. Closing it is display-only and remembered by the host (see
  Page preferences below).
- **Editing never cuts.** Rows carry the whole description (the card derives
  its one-line preview), and `edit` writes only the fields that differ from
  the stored task, compared trimmed, so an untouched description keeps its
  exact bytes. A description over 20 000 characters is refused, never cut.
- **Cards.** Title, priority badge (not for `normal`), relative update date,
  linked-discussion link, first-line description preview, tag chips (the
  task's tags minus the board tag), actions. An empty column says so.
- **Search.** A display-only filter over title, description, reference and
  tags, case- and accent-insensitive; each column shows `matches of total`;
  `/` focuses it, × or Escape clears it, a tag chip fills it.
- **Details.** A chevron expands a card's description rendered as Markdown
  (headings, lists, emphasis, code, http(s) links only). The text is escaped
  before any tag is built; opening it launches nothing.

### Page preferences

The opaque sandbox (`allow-scripts`, no `allow-same-origin`) has no storage.
The bridge exposes `window.KronnPagePref(key, value)`: after a live click it
posts `kronn:page-pref { page_id, key, value }` on the private port. The host
relay (`createLivePageOpenLinkRelay`, option `pageId`) accepts it only for an
allow-listed key (`LIVE_PAGE_PREF_KEYS`, today `notice-dismissed`), a boolean
value and the Page the frame shows; it stores it in the host's `localStorage`
(`kronn:page-pref:<page id>:<key>`, errors swallowed) and never acts on it.
`runtimeData` hands the stored flags back as `KronnPageData.prefs`.
`[src: file: frontend/src/lib/live-page-sandbox.ts:1]`

`TaskBoard` (zero tokens) reads or changes the planning tasks carrying the
board tag; it refuses any task without it. The order of open cards lives in
`task_board_orders` per tag, because the planning `rank` is renumbered across
a priority band. Its `data.rows` ends with one `__col_<column>__` marker per
column, which the Page binds to for "end of column" moves.
`[src: file: backend/src/core/default_todo.rs:1]`
`[src: file: backend/src/workflows/task_board_step.rs:1]`
`[src: file: backend/src/core/default_todo/board.html:1]`

## View parameters

The standalone tab accepts display hints after the Page id:
`#page/<id>?tv=1&scene=standup`. The id stays percent-encoded by
`standaloneLivePageUrl`, so a literal `?` can only start the parameters.
They reach the Page as `KronnPageData.page.params` (and in every
`kronn:page-data` event), for example to open a wall-screen layout. Only plain
tokens pass: keys `^[a-z][a-z0-9_]{0,31}$`, values `^[A-Za-z0-9_.-]{0,64}$`,
at most 8, first occurrence wins; anything else is dropped. The embedded viewer
and mosaic tiles have no URL of their own and send no `params` key. Parameters
are hints for the Page's own rendering, never an authorization input.
`[src: file: frontend/src/lib/live-page-navigation.ts]`
`[src: file: frontend/src/lib/live-page-sandbox.ts]`

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

A write whose `value_from` reads an `Exec` stdout (`steps.<exec>.data.stdout`
or `previous_step.data.stdout`) gets the whole document, never a cut one. The
checks below follow what the run recorded: the type of the step that really
produced the value (for `previous_step`, the step that ran last, also after a
jump or a resume). When that type is unknown, a value with the Exec envelope
shape is checked anyway. Any other source is published as is, whatever its
field names. The 2 MiB raise is decided before the run from list order, so it
is a best effort: after a jump, a cut output is refused, never published.
`{{previous_step.data…}}` and `{{steps.<name>.data…}}` no longer read an older
step's data after a step that produced no envelope. To read an earlier
producer, name it with `steps.<producer>.data`, which works as long as that
producer has not been re-run without an envelope. An `Exec` step that a
`PublishPageData` write reads keeps up to 2 MiB of output (the size
`POST /api/pages/{id}/publish` accepts) instead of 100 KB. A stdout cut at that
limit (flagged `stdout_truncated` in the step envelope, or carrying the
truncation marker) fails the publish step with an explicit message and the Page
keeps its previous data. A stdout that starts with `{` or `[` is parsed and
stored as the JSON value; if it does not parse, the step fails the same way.
Other text is published as a string. The Exec output is scrubbed of the
project's GitHub token before it reaches the run record or a Page.
`[src: file: backend/src/workflows/publish_page_step.rs:1]`

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

Images load only from `data:` and `blob:` URIs. An image behind authentication,
such as a Jira attachment thumbnail, is fetched by an `ApiCall` with
`api_response: Binary` and published as a data URI; see
[binary responses](../operations/deagent-apicall.md#binary-responses-images-and-other-files).
[src: file: frontend/src/lib/live-page-sandbox.ts:5]

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

A CTA may add `data-kronn-binding-labels`, a JSON map from the same binding
names to display text (`{"ticket":"Frame the login bug"}`). The card shows it
in place of the selector, which stays in the tooltip and the resolved values.
A label is a string of at most 200 characters (UTF-16 code units); a longer,
blank or non-string label is rejected and the card shows the raw selector
instead. Labels are display only: the host keeps only those of bindings the
click carries, never sends them to the server, and they do not enter the trust
fingerprint.

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
`[data-kronn-action]` with `data-kronn-action-state`, `aria-busy` and
`data-kronn-action-launch` (the launch id, so a Page tells a new attempt of a
row from the previous one even when both succeeded), including rows a Page
script renders later. A zero-specificity default indicator is injected; the
author's own CSS wins. Clicking a row that is still running reopens its run
instead of a blank offer, clicking the button of the open card closes it, and
the card also closes with its × or Escape. A row whose last launch is finished
opens a fresh offer, which launches a new attempt of that row, with the
previous launch one click away; a Discussion card never relaunches, since a
fence carries one intention.
[src: file: frontend/src/hooks/useLivePageActions.ts]

A `user_input` value may also carry a `<page.…>` `source_ref`: when the card
opens, `POST /api/live-page-actions/{id}/prefill` resolves it server-side for
the clicked row (the selector keyed by the field's name, or the click's only
selector) and the field starts from that value, still editable; a missing row
or a null field leaves it empty, and a value the reader already typed is never
overwritten. The launch runs what the reader sends. `GET /api/pages/{id}/actions`
lists only the blocks of the current revision: a block removed from the
published HTML keeps its row for the launches that reference it, but is no
longer offered, and its old id refuses a launch.
[src: file: backend/src/db/live_page_actions.rs]

The periodic refresh of a Page (every 30 s, in the Pages view and in its own
tab) keeps an open card, and what was typed in it, while the Page still offers
its action; switching Page closes it. When new data makes the Page redraw its
rows, the bridge reopens the collapse under the row that now carries the same
binding and the card follows it. A placeholder is shown as an example
(`e.g. …`), since an empty field sends nothing, unless its author already
phrased it as an example or an instruction.
[src: file: frontend/src/lib/live-page-sandbox.ts]

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

#### Trusted actions (KT-1029)

A human may approve, from the Page's details, that one action runs on a click
without its card. The approval lives in `live_page_action_trusts`, never in the
HTML, and is human only: its routes (`GET /api/pages/{id}/action-trusts`,
`POST|DELETE /api/live-page-actions/{id}/trust`) are absent from the
bridge-token list and the handlers refuse a bridge caller.
[src: file: backend/src/api/live_page_actions.rs:1]

- **Scope.** One declaration (`page + action_ref`), bound to a SHA-256 over the
  block (kind, target, project, values), the Page's project and the workflow's
  shared revision identity (`run_pins::revision_fingerprint`, KT-1096). The UI
  sends the fingerprint it showed; a different current one is refused. Each
  approval gets a new `approval_id`.
- **Eligibility.** Workflow targets only; every step must be of a type known
  to run no agent (ApiCall, Notify, Gate, Exec, BatchApiCall, JsonData,
  CollectApiData, TransformData, PublishPageData): any other type, including
  a future one, is refused by default, as is any Quick Prompt or sub-workflow
  reference; no skill, profile or directive; no
  CollectApiData Quick Exec source (a run does not pin it); no `user_input`,
  `project_env` or overridable value; target, block and Page in one project;
  workflow enabled.
- **Invalidation.** Any fingerprint difference or lost eligibility marks the
  approval invalidated with a reason, for good. SQLite triggers on `workflows`
  and `quick_apis` invalidate at write time through
  `live_page_action_trust_deps`: every column but `pinned` and `updated_at`
  is compared, so a pin change in the same write as a content change still
  counts, and a change undone before anything reads it still counts. A test
  keeps the compared columns equal to the tables' columns.
- **Launch.** The host relay forwards an action only with positive
  `navigator.userActivation.isActive`; the Kronn UI sends `trusted: true` with
  no typed value, and ignores an answer that arrives after the reader changed
  Page or opened another card. The claim records the approval id and
  fingerprint. `create_manual_run_admitted` then runs `admit_run`, inserts the
  run and pins it (`run_pins::pin_within`) in one transaction, against the
  definition it read: a claim from an older approval, an edit or a revocation
  before that point is refused, and nothing after it changes what runs. A row
  in flight is never relaunched; one row waits 2 s between trusted launches
  and one action allows 30 per minute.
- **Revocation.** Deletes the row; the next click opens the card.
[src: file: backend/src/db/live_page_action_trusts.rs:1]

Embed permission (third-party embeds) is a separate mechanism and is not
affected by action trust.

### Third-party embeds

A player nested inside a Page (for example `https://suno.com/embed/<id>`)
inherits the Page's opaque-origin sandbox: it renders but never plays, and the
CSP has no `frame-src` anyway. Relaxing either is not an option, since
`allow-scripts` plus `allow-same-origin` on a `srcdoc` frame lets the Page
remove its own sandbox. Embeds therefore follow the action-card pattern: the
host draws the real content over the Page, and only for sites the destination
Kronn allows.

Authoring contract. The Page sizes a placeholder with CSS and gives the full
URL of what it wants to show:

```html
<div data-kronn-embed="https://suno.com/embed/08fca036-317a-4cd5-9860-166c62c0180f"
     style="height:152px;border-radius:12px"></div>
```

Any http(s) URL is accepted by the contract; whether it is drawn depends only
on its origin (scheme, host and port, compared exactly, so
`https://player.example.com` does not cover `https://cdn.player.example.com`
nor `:8443`). URLs with credentials or another scheme are ignored. Adding a new
service is a configuration change plus HTML, never an engine change.
[src: file: frontend/src/lib/live-page-embeds.ts:36]
A third-party site can still refuse to be framed (`X-Frame-Options` or a CSP
`frame-ancestors`); allowing it in Kronn does not override that, and the
player then shows the site's own error.

Allowed sites. One global list per Kronn, `embed_allowed_origins` in
`config.toml`, served by `GET /api/config/embed-origins` and changed by
`POST /api/config/embed-origins` with `{add, remove}`; each origin is
normalized (lowercase host, default port dropped, IDN as punycode) and a
malformed one rejects the whole change.
[src: file: backend/src/models/setup.rs:73]
[src: file: backend/src/core/embed_origins.rs:55]
[src: file: backend/src/api/live_pages.rs:578]
The UI is Configuration → Artifacts → External content (Allowed sites): type an
origin, see the exact value that will be saved, add it, or remove a
permission. Every Page view, the import dialog and that section share one
in-tab store. Every change of the list (Configuration, an import that allowed
sites, a configuration reset) broadcasts `{type:'embed_origins_changed'}` on
the local WebSocket, with no origin in it (never relayed to a P2P peer); every
open tab, a visible wall screen included, re-reads the list at once and a
revoked site's players are unmounted. A tab also re-reads on WebSocket
reconnect (for an event sent while it was disconnected) and on focus.
[src: file: backend/src/api/live_pages.rs:643]
Changes are sent one after the
other, and a read that started before a change landed (or while one is in
flight) is discarded, so a slow read can never bring back a revoked site or
drop a confirmed one. An import that allowed sites invalidates the store the
same way and reads the list again at once.
[src: file: frontend/src/components/settings/ExternalContentSection.tsx:24]
[src: file: frontend/src/hooks/useEmbedAllowedOrigins.ts:95]

Browser enforcement. The host, not only the overlay, limits what may be
framed: every app document carries `frame-src` (and `child-src`) set to
`'self'` plus exactly the allowed sites, built once by `frame_src_sources`
(each origin re-checked as a plain CSP host source: no `;`, quote, space,
wildcard, `_` or IPv6 literal; an `http://` site is listed only when its
`https://` upgrade is allowed too, since CSP lets the former match the latter;
bounded at 16 KiB, and an addition that would exceed it is refused). The
browser applies it to every navigation of a player frame, so an allowed site
that redirects, or navigates its frame, to another origin is blocked before
any request reaches that origin (`e2e/specs/live-page-embed-frame-policy.spec.ts`).
Each mode serves it on the real document response:
- Docker: the gateway asks `GET /api/embed-origins/frame-src` through
  `auth_request` on every document request (open like `/api/health`: it
  carries no credentials and returns only what the document's CSP shows
  anyway) and copies `X-Kronn-Frame-Src` into its single CSP header. If the
  backend does not answer, the document gets `frame-src 'self'`. Assets and
  `/api` keep `frame-src 'self'`.
- Desktop: the Tauri CSP is `null`, but the webview loads the app over HTTP
  from the embedded backend, which serves the built frontend through
  `serve_app_documents`; its middleware puts
  `frame-src S; child-src S; worker-src 'self' blob:` on each response, which
  the webview enforces. The desktop sends no `Cross-Origin-Embedder-Policy`
  or `Cross-Origin-Opener-Policy` (KT-1123): under `require-corp` the webview
  refuses any player that does not itself send COEP + CORP (YouTube, Vimeo,
  Suno do not), whatever the allow-list says, and `credentialless` is not
  supported by WebKit, the macOS webview. They were set only to expose
  `SharedArrayBuffer` to the TTS/STT workers; onnxruntime-web falls back to a
  single WASM thread without it, as in Docker, which never sent them.
  Proven in a native macOS WKWebView: a synthetic player without COEP/CORP is
  blocked under the old headers and loads without them; TTS produces audio
  without isolation. Open: the packaged app, a real player, Windows WebView2,
  and STT, which fails there with or without the headers (KT-1143).
- Native (`./kronn start-dev`, and the desktop dev URL): Vite serves the
  documents; the `kronn-frame-policy` plugin reads the same backend route per
  document request and sets the same policy, re-validating the sources and
  falling back to `'self'` when the backend is down or the value is malformed.
  It renders the app document itself (`transformIndexHtml` included) and
  writes the header and the marker in one response, before Vite's later
  layers. `vite preview` is not used by Kronn: the plugin does nothing there,
  so its documents carry no marker and draw no player.
Documents never answer with a 304 (validators dropped), so a reload carries
the current list. Each document also carries the exact sources its CSP was served
with, as `<meta name="kronn-served-frame-src" content="…">` right after
`<head>` (HTML-escaped; the desktop middleware and the Vite plugin write it
with the header, the Docker gateway with `sub_filter` from the same value).
The page compares every later list against that marker, never against a list
it read afterwards, so a site revoked between serving and the first read is
still seen as revoked. A missing or malformed marker means an unknown policy:
no player is drawn until a reload. A policy is fixed for the life of a
document: a site allowed after the page was opened is shown as a "reload to
show this content" notice, never as a frame the browser would refuse. An
allowed `http://` site whose `https://` address is not allowed is never in
`frame-src` (see above): it gets a notice to allow it over https, not a reload
notice. A revoked site stays in that policy until a reload, and an allowed player may have been redirected to it, or could be
again: so once any site the document's policy admits is revoked, every player
of the tab is unmounted and replaced by the reload notice, whatever its own
site; only a reload (a document with the new policy) brings players back, or
the site being allowed again. A change event also suspends every player until
the list is read again successfully, so a failed re-read never leaves players
running under a list that may have shrunk. Additions alone keep the players.
Not enforceable: a frame that an allowed site creates inside its own page is
governed by that site's document, not by Kronn's CSP, so an allowed site can
itself embed other origins; Kronn controls only what the Page frames directly,
redirects included.
[src: file: backend/src/core/embed_origins.rs:254]
[src: file: backend/src/api/live_pages.rs:660]
[src: file: frontend/vite-frame-policy.ts:72]
[src: file: frontend/src/lib/served-frame-policy.ts:9]
[src: file: .docker/nginx.conf:31]

Bridge. The injected script finds every `[data-kronn-embed]` (including ones
page scripts render or change later, via a MutationObserver), and posts one
`{type:'kronn:page-embeds', version:1, channel_id, embeds:[{key, url, rect,
clip?, visible, radius}]}` message over the private link port. It re-sends on
scroll (capture, so a scrolling container inside the Page counts), resize, DOM
mutations and element resize, throttled to one animation frame and only when
the list changed. `key` is `e<hash of url>:<rank>` (rank among identical
placeholders in document order), so a redrawn placeholder keeps its player.
`rect` is in the Page iframe's viewport coordinates. `clip` is what the
placeholder's clipping ancestors (any `overflow` other than `visible`, up to a
`position: fixed` one) leave visible of it, absent when none clips it;
`visible` is false when that visible part is empty or outside the viewport.
The report is capped at 64 placeholders, a bound on work only: the players'
own cap (8) is applied by the host after its check, so refused placeholders
never crowd out allowed ones. A fresh port always receives the full list,
even empty. The message needs no user activation: it only positions
host-owned content.
[src: file: frontend/src/lib/live-page-sandbox.ts:548]
[src: file: frontend/src/lib/live-page-sandbox.ts:567]

Host. The relay checks the list structurally (bounded, finite rectangles, key
grammar, URL length; a malformed clip becomes an empty one, never none)
[src: file: frontend/src/lib/live-page-sandbox.ts:134], then
`planLivePageEmbeds` decides: nothing before the allowed sites are known; an
allowed origin gets a player (at most 8); a valid URL from another site gets a
warning drawn by Kronn (at most 8), naming the origin, with a "Configure
allowed domains" button that opens the settings with that origin typed in,
never added. Inside the app the current tab navigates
(`#settings/artifacts?origin=…`); a standalone Page or mosaic keeps running and
opens the settings in a new tab.
[src: file: frontend/src/lib/live-page-embeds.ts:71]
[src: file: frontend/src/lib/live-page-navigation.ts:185]
[src: file: frontend/src/pages/Dashboard.tsx:243]
`LivePageEmbedOverlay` renders a layer sized to the iframe's content box with
`overflow: hidden`, so content is clipped to the Page and can never cover host
UI; each item is also cut with `clip-path: inset(…)` to the reported `clip`,
which clips hit-testing too, so a Page control next to a clipped placeholder
still gets its clicks (`e2e/specs/live-page-embed-clipping.spec.ts`).
[src: file: frontend/src/lib/live-page-embeds.ts:114]
The layer sits at z-index 3, under the action card (4), and lets clicks
through except on its items. Players are keyed and kept in first-seen DOM
order, because moving an iframe reloads it: scrolling, re-clipping or
reordering only changes its style, and playback continues. Each player is
`<iframe loading="lazy" allow="autoplay; encrypted-media; fullscreen; picture-in-picture"
referrerpolicy="strict-origin-when-cross-origin"
sandbox="allow-scripts allow-same-origin allow-popups allow-presentation">`;
`allow-same-origin` keeps the site's own origin, cross-origin to Kronn.
[src: file: frontend/src/components/LivePageEmbedOverlay.tsx:42]
[src: file: frontend/src/components/LivePageEmbedOverlay.tsx:105]

Export and import. An exported Artifact carries `embed_origins` (omitted when
empty, so other Artifacts keep their exact bytes): the origins of the
`data-kronn-embed` attributes of its markup, read by html5ever's WHATWG
tokenizer exactly as a browser reads them (quoted or not, every character
reference decoded, comments and raw-text elements skipped, linear cost), plus
quoted ones spelled inside its scripts (decoded the same way), plus the origins
it was itself imported with. That last part is stored with the Page
(`live_pages.declared_embed_origins`), so content built by script keeps its
declaration through a chain of imports. It is information for the importer,
never an authorization, and the creator's allowed sites are not exported.
[src: file: backend/src/core/embed_origins.rs:154]
[src: file: backend/src/api/artifact_portability.rs:123]
The import preview lists one row per distinct origin (from the HTML and the
declaration): already allowed ("in the Artifact and already in your
configuration"), or Add / Refuse. Answers are not part of the reviewed digest
because they change no imported resource. Only declared origins can be sent
in `allow_embed_origins`, and they join the global list only after the import
commits; a cancelled or failed import changes no permission, and a refused (or
unanswered) site does not block the import: its content shows the warning.
Choices that would exceed the 256-site limit refuse the import before anything
is written; the configuration is not locked while the import is planned, only
for the final update. If that update fails once the import committed (the list
changed meanwhile, the configuration cannot be saved), the result lists those
sites in `not_allowed_embed_origins` with `embed_origins_error`, and the dialog
says so before opening the Artifact.
[src: file: frontend/src/components/ArtifactImportDialog.tsx:164]
[src: file: backend/src/api/artifact_portability/import.rs:90]
[src: file: backend/src/api/artifact_portability/import.rs:975]

Security reasoning, in short: the destination Kronn alone decides which sites
may be framed, and checks every URL at render time, including ones added by
script or by a later revision; the content runs outside the Page sandbox in
its own origin; its box is clipped to the Page frame and to the Page's own
containers. The Page's CSP is unchanged: allowing a site lets Kronn frame it,
never lets the Page's own JavaScript reach that site or the network.

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
thirteen-step taxonomy. HTTP agents reach the same five Page tools through their
native catalogue ([Native Page tools](../operations/native-page-tools.md)). For a Page pipeline, an agent must discover dependencies
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
