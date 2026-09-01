# Live Page ↔ Workflow interaction — design & implementation plan

> Status (2026-08-29): **Phases 1, 2 and 3 shipped & verified end-to-end** on the
> reference page. Phase 1: standalone auto-refresh. Phase 2: server binding
> (`9b320e72`, `64ede0d9`→`cc0a696c`) + client-side run mirror; real run renders
> across the 4 phases, fastly/cloudwatch live after the ctx() fix (page revision
> 12). Phase 3: in-page gate approval — a live SIMULATION run paused at
> `gate_confirm_pr` was approved from the page's broker endpoint and resumed to
> the next gate. `allowed_gate_steps` now lists the 4 gates.
>
> Note: the actual in-iframe button click can't be driven by CDP (sandboxed
> opaque-origin iframe), so that one hop is covered by unit tests
> (`live-page-sandbox.test.ts` action relay, `StandaloneLivePage.test.tsx`
> handler); the server contract was verified live via `POST /api/pages/{id}/gate-decision`.
> Scope: make a Live Page a two-way surface for a workflow — display live step
> results, refresh without a manual reload, and approve/reject gates **from the
> page**. Reference target: page `64ede0d9-2982-4eb2-a798-3c75dda256f3`
> (`mep-pipeline-maquette`).

## 1. Problem

A Live Page today is a **read-only, one-way** surface:

```
workflow step → PublishPageData / publish → dataset → LivePageDetail
             → postMessage('kronn:page-data') → window.KronnPageData
```

The page HTML runs in a sandboxed iframe with `sandbox="allow-scripts"` **without**
`allow-same-origin`, under a strict CSP including `connect-src 'none'` and
`form-action 'none'` (`frontend/src/lib/live-page-sandbox.ts`, const
`LIVE_PAGE_CSP` + `buildSandboxDocument`). Consequences:

- Page JS **cannot** `fetch`/XHR/WebSocket to any backend or external host.
- Page JS **cannot** submit forms, read the parent DOM, cookies, or the Kronn
  session (opaque origin).
- The only outbound bridge today is `kronn:page-open-link` — a gated
  MessagePort relay that opens **one** validated external tab, requiring a real
  user gesture (`createLivePageOpenLinkRelay` in `live-page-sandbox.ts`).

So every capability we want must be brokered by the **trusted parent** frame
(`StandaloneLivePage` / `PagesPage`), which holds the session/auth. The page
never gets network access or credentials.

## 2. What "interact" decomposes into

| User goal | Mechanism | Status today |
|-----------|-----------|--------------|
| See workflow step results on the page | push `StepResult` into a dataset | ✅ mechanism exists (`PublishPageData`) |
| See steps validate "one by one" without reloading | auto-refresh of the **published** page | ❌ missing on standalone view |
| Approve/reject gates **from the page** | upward action channel page→server | ❌ net-new (broker bridge) |

Verified state of the reference page `64ede0d9`:

- 3 `snapshot` datasets: `fastly` (fed), `cloudwatch` (fed), **`pipeline` = null**
  (HTML falls back to a `NOMINAL`/`FAILSCEN` mock when absent).
- HTML already reads `window.KronnPageData` and re-renders on
  `window.addEventListener('kronn:page-data', render)` → **nothing to change in
  the HTML for live data to appear**, we only need the parent to re-push.
- Only workflow feeding it is `1be9bb80` ("Erreurs live — page MEP"), steps
  `publish_fastly` / `publish_cw`. **No step feeds `pipeline`.**
- Gate UI already exists but is **outbound-only**: an `<a href="${runUrl}"
  target="_blank">Approuver dans Kronn ↗</a>`. The run URL already flows into
  the page, so the page **already knows the run id**.

## 3. Decisions

### D1 — Broker via the parent, never loosen the sandbox

The page emits an **intent** by `postMessage`; the trusted parent validates it
against an allowlist and performs the authenticated API call. We do **not** add
`allow-same-origin` / relax `connect-src` — that would let authored page JS hit
any endpoint with the user's session. This reuses the existing
`kronn:page-open-link` relay pattern.

### D2 — Reads autonomous, writes on user gesture

- **Read/observe** (run status, step results): allowed **without** a user
  gesture → the page can live-refresh on a timer or via a parent WS relay.
- **Write** (trigger a workflow, decide a gate): **require a real user gesture**
  (user-activation), exactly like the open-link relay. A page can never
  auto-approve a gate or auto-trigger a workflow.

### D3 — Action bindings gate what a page may act on

A page references an action by an **opaque `bindingKey`**, never a raw
`workflow_id` / `run_id`. The parent resolves `bindingKey → (workflow_id,
constraints)`. Gate decisions are further restricted to runs **belonging to the
bound workflow** and in `WaitingApproval`. This prevents a hostile/authored page
from triggering or deciding arbitrary workflows.

### D4 — the run mirror runs **client-side in the parent** (Voie B)

A config-only mirror inside the workflow engine was proven infeasible: the
JSON-fetching steps (`CollectApiData`/`ApiCall`) run `SecurityPolicy::production()`
which rejects loopback, so no step can read Kronn's own run API
(`backend/src/workflows/api_call_security.rs`, `assert_public_ip`); and
`TransformData` has no array→array map, while `JsonData` can't template — so no
step can fold `step_results[]` into `phases[]`. Adding a bespoke backend step was
also rejected: a Cron mirror is capped at the ~30–60s engine tick (too slow for
"one by one"), and interleaving publishes bloats the workflow.

Instead the **trusted parent** (`StandaloneLivePage`) reads the bound run over the
normal authenticated frontend API and reshapes it client-side with the pure
`runToPipeline()` (`frontend/src/lib/live-page-pipeline.ts`), injecting the result
as a snapshot dataset. This is ~4s latency, no engine change, page HTML and the
MEP workflow untouched, and it reuses the same parent→API channel Phase 3 needs.

A **server-side binding** (`live_page_workflow_bindings`, migration 154) declares
which workflow a Page mirrors, the `phase_map`/`meta_map` (interpreted
client-side), the `run_selector`, and `allowed_gate_steps` (the Phase-3
authorization boundary). See `backend/src/api/live_pages.rs`
(`list_bindings`/`upsert_binding`/`delete_binding`).

## 4. Threat model

| Threat | Mitigation |
|--------|-----------|
| Authored page JS hits arbitrary endpoints | No network from iframe (`connect-src 'none'`); all calls brokered by parent |
| Page triggers/decides a workflow it shouldn't | Action bindings (D3); `bindingKey` opaque; gate `run_id` must belong to bound workflow + be `WaitingApproval` |
| Page auto-approves a gate silently | Writes require user-activation (D2) in the iframe path |
| **Broker endpoint called directly** (skipping the iframe): the page slug is a shareable URL and `run_id` is surfaced into the page, so a viewer knows both | The HTTP endpoint — not the iframe — is the real boundary: `gate-decision` (and `decide`) are classified **destructive** (`is_destructive` in `lib.rs`), so under the auth-off default a remote/no-token caller is refused; the binding's `allowed_gate_steps` still bounds what a *trusted* caller can decide |
| Double-decide race (page + Kronn UI) | Shared atomic `claim_waiting_run` (`... WHERE status='WaitingApproval'`) in `resume_with_decision` |
| Runaway autonomous polling | Prefer WS relay; if polling, rate-limit + only while a run is active |
| Page grabs the private MessagePort | `stopImmediatePropagation` on the port-init event, as the open-link relay already does |

## 5. Gate flow caveat (important)

A workflow with a `Gate` **pauses** the moment it reaches it (`RunStatus::WaitingApproval`,
`execute_gate_step` in `backend/src/workflows/gate_step.rs`), and **no later step
runs** until a decision arrives. So "watch steps scroll by" has two regimes:

- **Before the gate**: steps run consecutively → visible one-by-one via auto-refresh.
- **At the gate**: execution stops. The page shows "waiting for your approval",
  the user clicks **Approve on the page**, the run resumes, and later steps start
  scrolling again.

For the page's Approve button to target the right run, the `pipeline` payload
**must carry** `run_id` (and `dataset`) for the waiting gate (produced by the D4
mirror). The decision is sent to the page-scoped broker
`POST /api/pages/{id}/gate-decision` (`decide_gate` in
`backend/src/api/live_pages.rs`) with `{dataset, run_id, decision, comment}`.
The server derives the waiting gate from the run's trailing step — no
`gate_step` is threaded from the client — and authorizes it against the binding
before resuming through the shared `resume_with_decision` path.

## 6. Token cost

**Zero** for the interaction machinery: the bridge is front JS; action bindings
are DB reads; gate steps already spawn no LLM (`execute_gate_step`); observe
re-reads stored run data. Triggering a workflow costs whatever the workflow
already costs. The D4 mirror uses deterministic API/status reads (no agent), so
it is 0-token too.

## 7. Implementation plan (phased)

### Phase 1 — Auto-refresh the published page (unblocks ② immediately)

`StandaloneLivePage` fetches **once** on mount and never refreshes
(`frontend/src/pages/StandaloneLivePage.tsx`, the `[pageId]` effect →
`pagesApi.get`, then `publishToFrame` posts `kronn:page-data`). The Studio view
already polls every 30 s (`REFRESH_MS = 30_000` + `setInterval` in
`frontend/src/pages/PagesPage.tsx`).

Work:
1. Add a refresh loop to `StandaloneLivePage`: re-`pagesApi.get(pageId)` →
   `setDetail` → the existing `publishToFrame` re-pushes → HTML re-renders on its
   `kronn:page-data` listener (no HTML change).
2. Cadence: short interval (3–5 s) while a run is active, back off to ~30 s when
   idle. Pause when the tab is hidden (`document.visibilityState`).
3. Preferred evolution: relay the existing `WsMessage::WorkflowRunUpdated`
   (emitted in `backend/src/workflows/runner.rs`) into the iframe instead of
   polling — event-driven, no backend hammering. There is currently **no** WS
   wiring for Live Pages, so this is net-new but small.

**Shipped.** `StandaloneLivePage` now runs an adaptive loop (`ACTIVE_REFRESH_MS`
4s / `IDLE_REFRESH_MS` 30s, backing off after `QUIET_POLLS_BEFORE_IDLE` quiet
polls), pausing on `visibilitychange` and keeping the last good render on a
transient poll failure. Covered by `StandaloneLivePage.test.tsx`. The WS-relay
evolution remains a future optimisation.

### Phase 2 — Mirror the run into `pipeline`, client-side (Voie B, D4)

**Contract discovered.** A single `pipeline` payload is
`{ meta, phases: [{ name, emoji?, steps: [{ n, tag?, s, d, at?, dur?, link? }] }] }`,
`s ∈ done|wait|current|pending|failed`. Phase/run status is *derived* by the HTML;
a gate = a step with `s: "wait"`; the approval button uses `meta.runUrl`.

**Latent page bug (must fix on the reference page).** The maquette HTML reads
`window.KronnPageData.fastly` / `.cloudwatch` / `.pipeline` at top level, but the
runtime shape is `KronnPageData.datasets.<name>.current`
(`frontend/src/lib/live-page-sandbox.ts`, `runtimeData`). So it always fell back
to its mocks and never showed live data. Fix `ctx()` to read
`k.datasets.<name>.current`; this also unblocks the already-published
fastly/cloudwatch datasets.

**Shipped (mechanism):**
- Backend binding `live_page_workflow_bindings` (migration 154): model
  `LivePageWorkflowBinding` + `LivePageRunSelector` (`backend/src/models/live_pages.rs`),
  db CRUD (`backend/src/db/live_pages.rs`: `list/upsert/delete_live_page_binding`),
  endpoints `GET/POST /api/pages/{id}/bindings` + `DELETE …/{dataset}`
  (`backend/src/api/live_pages.rs`, routes in `backend/src/lib.rs`). Tests: db unit
  + `api_tests.rs::live_page_workflow_binding_crud_round_trip`.
- Pure reshaper `runToPipeline(run, phase_map, meta_map)`
  (`frontend/src/lib/live-page-pipeline.ts`) + `live-page-pipeline.test.ts`.
- `StandaloneLivePage` resolves each binding (`resolveBindingPipelines`): pick the
  run per `run_selector`, `workflows.getRun` for full `step_results`, reshape,
  overlay as `datasets[dataset].current`. A non-terminal mirrored run keeps the
  fast cadence. Covered in `StandaloneLivePage.test.tsx`.

**Remaining (config, needs the running stack + the user's page):**
1. Fix the reference page's `ctx()` (the latent bug above) via `page_update_html`.
2. Create the binding: `POST /api/pages/64ede0d9…/bindings` with
   `workflow_id = cc0a696c…`, `dataset = "pipeline"`, `run_selector = latest`,
   and a `phase_map` of the 4 phases (Préparation / Tests CI / Merge & release /
   Déploiement) over the 17 steps, `meta_map = { ticket: "trigger.jira_ticket_key" }`.

### Phase 3 — In-page gate approval (net-new, ③)

**Shipped.**
- Backend: `parse_gate_decision` + `resume_with_decision` extracted from
  `decide_run` so the audited TOCTOU-safe claim/spawn lives once
  (`backend/src/api/workflows.rs`); page-scoped `decide_gate`
  (`POST /api/pages/{id}/gate-decision`, `backend/src/api/live_pages.rs`)
  authorizes against the binding — run must belong to the bound workflow, be
  `WaitingApproval`, and its current gate must be in `allowed_gate_steps`. Model
  `PageGateDecisionRequest`, db `get_live_page_binding`. Tests: db unit +
  `api_tests.rs::live_page_gate_decision_enforces_the_binding` (the audit
  rejections) + the untouched `decide_run` tests.
- Frontend: `createLivePageActionRelay` + bridge `window.KronnPageActions`
  (private MessagePort, `stopImmediatePropagation`, **user-activation required**)
  in `frontend/src/lib/live-page-sandbox.ts`; `StandaloneLivePage` connects the
  relay once per loaded document and brokers `gate.decide` → `pages.decideGate`;
  `runToPipeline` surfaces `meta.run_id` + `meta.dataset`. Tests in
  `live-page-sandbox.test.ts` + `StandaloneLivePage.test.tsx`.
- Page HTML (reference page): the gate box renders Approve / Request-changes /
  Reject `<button>`s calling `KronnPageActions.decideGate` when the pipeline is
  real; deep-link fallback otherwise.

The original design sketch below is kept for reference.

**Backend — action bindings**
1. Model `LivePageActionBinding` in `backend/src/models/live_pages.rs`
   (`{ id, page_id, key, kind: trigger|gate, workflow_id, allowed_gate_steps?,
   variable_allowlist? }`), `make typegen`.
2. Migration (new file under `backend/src/db/sql/`, register in
   `backend/src/db/migrations.rs`): `live_page_action_bindings` table.
3. Endpoints in `backend/src/api/live_pages.rs`: create/list/delete bindings +
   a resolve endpoint `bindingKey → workflow_id + constraints`. Route in
   `backend/src/lib.rs` (`build_router`). Optional MCP tool `page_bind_action`.
4. Broker endpoints reuse existing handlers: trigger →
   `POST /api/workflows/{id}/trigger`; decide → `decide_run`. Enforce: gate
   `run_id` belongs to `binding.workflow_id` **and** is `WaitingApproval`;
   `gate_step` ∈ `allowed_gate_steps`; trigger variables ∈ `variable_allowlist`.

**Frontend — the action channel**
5. Extend `frontend/src/lib/live-page-sandbox.ts` with a request/response
   channel `kronn:page-action` `{ id, action, payload }` →
   `kronn:page-action-result` `{ id, ok, data|error }`, modeled on
   `createLivePageOpenLinkRelay`: private MessagePort, `stopImmediatePropagation`
   on init, **user-activation required** for writes.
   Actions: `workflow.trigger` `{bindingKey, variables}`,
   `gate.decide` `{bindingKey, runId, decision, comment, gateStep}`,
   `run.status` `{runId}` (read, no gesture).
6. Parent relay in `StandaloneLivePage` (and `PagesPage` preview): receive
   intent → resolve binding → authenticated call via `frontend/src/lib/api.ts`
   → echo result back into the iframe.
7. Optional authored-page ergonomics: expose a tiny documented helper
   (`window.KronnPageActions.trigger(...)` / `.decideGate(...)`) so page authors
   don't hand-roll `postMessage`.

**HTML (page `64ede0d9`)**
8. Replace the outbound `<a href="${runUrl}">Approuver dans Kronn ↗</a>` in the
   gate block with a `<button>` calling `KronnPageActions.decideGate(...)`; reuse
   existing CSS (`.gate`, `#action-focus`, `.btn.pri/.danger`). On success the
   Phase-1 auto-refresh shows the run resuming.

Tests: backend integration (binding allow/deny, foreign run refused,
double-decide blocked); Vitest (bridge request/response, user-activation gate);
e2e (page button approves a paused gate → run resumes).

## 8. CORS / auth notes

Status/observe endpoints are non-destructive under
`auth_allows`/`auth_middleware` (`backend/src/lib.rs`): open on localhost / when
no token configured, else `Authorization: Bearer <token>`. **Run-resume
endpoints are the exception**: `…/runs/{id}/decide` and `…/pages/{id}/gate-decision`
are classified **destructive** (`is_destructive`), so they require local trust or
a valid token *even when app-wide auth is off* — approving a gate can trigger a
prod deploy and must not be reachable by an anonymous remote peer. CORS origins are a
fixed allowlist (localhost / gateway / configured domain) via `build_cors` in
`backend/src/lib.rs` — a standalone page served from an allowlisted origin is
fine; arbitrary third-party origins are blocked. The broker always runs in the
parent, which already carries the session, so the page never handles auth.

## 9. Recommended order

1. **Phase 1** (auto-refresh) — immediate live effect on `fastly`/`cloudwatch`,
   ~15 lines, 0 token.
2. **Phase 2** (pipeline mirror) — steps start scrolling one-by-one.
3. **Phase 3** (in-page gate approval) — heaviest, but isolated.
