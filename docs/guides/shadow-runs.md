# Test a new version without risk: step shadows

You changed a skill or a Quick Prompt and you think the new version is better.
Or you want a second opinion from another model, or a local one that costs no
tokens. Or you want to know how much the same agent changes its mind on the same
input. A **shadow** is a second execution of one step, started beside its main
execution, on exactly the same input, with the agent, model, prompt and skills
you chose. The run view then sets the main execution and its shadows side by
side. When a new version wins, one click hands it to the step.

Measured on a pull-request review (99 known problems on 16 pull requests): one
reviewer found 84 % of the problems at its first pass, the same reviewer run
twice 85 %, two different reviewers merged 90 %. Diversity beats repetition, and
a shadow on a local model makes it cheap.

## What a step shadow is

A step carries `shadows`, 0 to 8 of them. Every time the step runs in a project
a shadow covers, the shadow re-runs **only that step**, with the step's rendered
prompt context, previous outputs and variables as the main step received them.
**Only the main output continues the workflow**: a shadow's output never feeds a
later step (a later step can read a summary of what ran, never an output: see
[Show the shadows on a Page](#show-the-shadows-on-a-page)).
[src: file: backend/src/workflows/shadow.rs:506-512#start_for_step]

Only an Agent step can carry shadows, and not one marked **Acts outside Kronn**.
[src: file: backend/src/workflows/shadow.rs:467-469#can_run_shadows]

Each shadow has:

- **an id and a label**: the id is a short slug (`qwen-local`, `v3.10.1`),
  unique in the step; the label is what the comparison shows; `enabled: false`
  keeps a shadow saved without running it;
  [src: file: backend/src/models/workflows.rs:235#StepShadow]
- **what it changes**, all optional, one level or several:
  - **an agent and a model** (`agent`): a local Ollama, a LiteLLM proxy or
    another hosted model, with its `model`, its `reasoning_effort` and, for a
    custom HTTP agent, its `connection_id`;
    [src: file: backend/src/models/workflows.rs:170#ShadowAgent]
  - **a prompt**: another Quick Prompt (optionally a version of it) or a prompt
    written in the shadow;
    [src: file: backend/src/models/workflows.rs:189#ShadowPrompt]
  - **skills**: `skills` swaps a skill by its base id (`custom-x` →
    `custom-x@3`), or drops it with an empty replacement, to measure what a
    skill brings; `quick_prompt_versions` loads a version of a Quick Prompt;
    [src: file: backend/src/workflows/shadow.rs:315-317#apply_versions]
- **its scope**: `project_ids` (empty means every project) and `sample_every`
  (one run in N gets the shadow, to bound the cost);
  [src: file: backend/src/workflows/shadow.rs:460-462#is_sampled]
- **a budget**: `max_tokens`, `max_duration_secs`, `max_cost_usd`.
  [src: file: backend/src/models/workflows.rs:205#ShadowBudget]

A shadow that changes nothing is **identical** to its step. It re-runs the same
agent on the same input, to show how many different outputs that agent gives.

What a shadow changes is applied to a copy of the step, never to the step
itself.
[src: file: backend/src/workflows/shadow.rs:297-298#apply_shadow]

## Pin the versions your steps load

A comparison means something only if the step loads a known version.

- **Skills.** On an Agent step, each custom skill loads its latest version, or
  a pinned one written `custom-x@N`. Only custom skills have versions; built-in
  and repository skills have none.
  [src: file: backend/src/core/skill_versions.rs:151#split_pin]
- **Quick Prompts.** An Agent step linked to a Quick Prompt has a version
  picker next to it (`quick_prompt_version`); unset, the step follows the
  latest version.
  [src: file: backend/src/models/workflows.rs:901#quick_prompt_version]

## Set up the shadows of a step

In the workflow editor, open the step and its **Shadows** section (a simple
workflow has it under its one task):

1. **Add a shadow**, then give it an id and a label;
2. leave it as it is to measure determinism, or choose its agent and model;
3. choose its prompt, and under **Versions compared** the version or the
   dropped skill it tests for each skill and Quick Prompt the step loads;
4. limit it to some projects, sample it one run in N, set its budget.

Only a person sets shadows: they start extra executions, so an agent's save that
sets or changes them is refused, and a save that sends the steps back keeps the
stored ones.
[src: file: backend/src/api/workflows.rs:2906#AGENT_SHADOW_REFUSAL]
[src: file: backend/src/workflows/shadow.rs:1115-1117#keep_stored_shadows]
A save is checked: the ids, the sampling, the overrides, the budgets, and that
the step is an Agent step that stays inside Kronn.
[src: file: backend/src/workflows/shadow.rs:960-962#validate]
A Page's **Quick Prompts and skills** panel also adds a shadow that tests a
version on a step, changes the version a shadow tests, and removes a shadow.

A shadow costs as much as its step: its agent is paid again, unless it runs on a
local model. The budget caps it.

## What a shadow never does

Mark every step that posts, sends or writes outside Kronn (a message, a ticket,
an API call) with **Acts outside Kronn**, under the step's advanced settings.
That is the step's `external_effect` flag.
[src: file: backend/src/models/workflows.rs:808#external_effect]

A shadow never runs such a step, and never a Gate: saving refuses shadows on
them, and one saved by hand is recorded as *skipped* instead of run. A later step
marked `external_effect` runs once, in the main path: no GitHub write, no message,
no publication happens because of a shadow.
[src: file: backend/src/workflows/shadow.rs:753-755#run_one]

Two cautions:

- a shadow runs in the main run's working directory, with no room and no Kronn
  tool: put shadows on steps that analyse or review, not on steps that edit
  files;
- a shadow of an Agent step that writes elsewhere through a tool would write
  too: keep that work in a step marked **Acts outside Kronn**, after the step
  that carries the shadows.

## A shadow never hurts the main step

A shadow that fails, stalls or spends its budget settles its own result and
nothing else: the main step keeps its status, its duration and its output, and
does not wait for the shadow. Each shadow runs in the background, one task per
shadow, and a crash in it is recorded on its own row.
[src: file: backend/src/workflows/shadow.rs:562#run_step_shadows]

- the **time budget** is enforced: the shadow is stopped (status *time limit
  reached*) and `max_duration_secs` replaces the workflow's timeout for it, so a
  slow local model may outlast the main step;
- the **token and cost budgets** are checked when the shadow ends: one that
  spent more is marked *over budget* with the limit, its output kept;
  `max_tokens` is also handed to an agent that takes a generation cap;
  [src: file: backend/src/workflows/shadow.rs:894-895#budget_exceeded]
- a shadow left running by a restart is marked *interrupted*.
  [src: file: backend/src/db/step_shadows.rs:154-156#interrupt_running]

What each execution loaded and produced is stored in `step_shadow_runs`, with its
tokens, duration and cost, and removed with its run; run retention blanks the
outputs with the run's.

## Read the comparison

Open a run: for each step that ran shadows, the run view shows the main
execution beside its shadows.
[src: file: backend/src/workflows/shadow.rs:1354-1356#comparisons]
`GET /api/workflows/{id}/runs/{run_id}/step-shadows` returns the same data.

- **a header per execution** says what differs from the others (agent, model,
  prompt version, skill versions) and groups what is identical;
  [src: file: frontend/src/lib/shadowRuns.ts:96-97#compareIdentities]
- **tokens, cost and duration** of each, and how far a shadow is from the main
  execution;
- **the output of each shadow against the main one**, with a line diff where
  they differ, and any two executions side by side;
  Each execution is named with its agent and model. JSON fields are compared
  in alphabetical order at every depth, so two models that write the same
  fields in another order are not shown as different; lists keep their order.
  [src: file: frontend/src/lib/shadowRuns.ts:120-129#withSortedKeys]
- **a determinism summary** when some shadows are identical to the step: how many
  distinct outputs there are out of N+1.
  [src: file: frontend/src/lib/shadowRuns.ts:184-186#determinism]

While a shadow is still going, the comparison refreshes and shows it as running.

## See the renders side by side (KT-1990)

When a step writes a page (an HTML report, a mini game), a line diff says
nothing useful. The run view then turns each execution's output into an
**Artifact**, the same way for the main execution and for every shadow. A
shadow has no Kronn tool and never makes one itself: Kronn makes it from the
stored output, on a person's click, so the comparison measures the answer and
not tool calling.

- **Per execution, « Make an Artifact »** appears when the output holds an HTML
  document: an `html` or `kronn-doc-preview` fence, or, without such a fence, a
  bare document from `<!DOCTYPE html>` or `<html>` to `</html>`. Kronn takes the
  **last** HTML block, makes one Artifact titled
  `<workflow> · <step> · <execution label>` (the main execution's label is
  `main`) in the run's project, and opens it in its standalone tab. The button
  then reads **Open the Artifact**. A closed fence holding a fragment (no
  `<html>` tag) is taken whole.
  [src: file: backend/src/workflows/shadow_artifact.rs:295-324#find]
  [src: file: backend/src/api/step_artifacts.rs:130-133#artifact_title]
- **Where the document starts and ends (KT-1999).** A page's script may hold
  `<html>` or `</html>` in a string (a template, a `document.write`); that
  string never cuts the page.
  - In a closed fence, the fence is the unit: the document runs from the first
    `<!DOCTYPE html>` or `<html>` of the fence found outside a script, a style
    or a comment, to the fence's **last** `</html>`, so a document template in
    a fragment's script is not taken for the page. A fence with an opening and
    no `</html>` after it is refused as cut.
    [src: file: backend/src/workflows/shadow_artifact.rs:210-228#fenced_document]
  - A bare document is read as a browser reads it: a `<!DOCTYPE html>` or an
    `<html>` opens a document, the first `</html>` closes it, and nothing inside
    a `<script>`, a `<style>` or an HTML comment counts as either. Another
    doctype or `<html>` tag before the close starts the document again there
    (the earlier one never closed). The last document closed wins; an output
    that ends inside a document, or inside a script or comment that never
    closes, is refused as cut.
    [src: file: backend/src/workflows/shadow_artifact.rs:246-288#bare_document]
  - In a bare output, a doctype or an `<html>` tag is an opening only when
    markup follows it (after its `>` and any whitespace, the next character is
    `<`, or the output ends there): prose naming the tag before, inside or after
    a page neither opens nor restarts a document. Known limit: a bare
    `<html>Hello</html>` with no doctype before it is therefore not seen as a
    document.
    [src: file: backend/src/workflows/shadow_artifact.rs:172-180#markup_follows]
- **No HTML, or a cut one, makes nothing.** A document that is cut is refused
  with the reason: its fence never closes, it opens `<html>` with no `</html>`
  after it, or the stored output ends with the cut marker
  (`[output cut at N bytes]`). An output with no HTML document says so, but
  only in a step where another execution wrote one, complete or cut: in a step
  that writes a review or an analysis, no execution shows anything about
  Artifacts.
  [src: file: frontend/src/lib/stepArtifacts.ts:66-74#stepHasHtml]
  [src: file: frontend/src/components/workflows/StepArtifactControls.tsx:103-107#stepHasHtml]
  [src: file: backend/src/workflows/shadow_artifact.rs:28-39#ExecutionHtml]
  [src: file: backend/src/workflows/shadow.rs:34#CUT_MARKER]
- **Per step, « Open the renders in a mosaic »** appears when two executions or
  more have a render. It makes the missing Artifacts and opens the Artifact
  mosaic: the main execution first, then the shadows in the order they started.
  Each tile names its Artifact above the page.
  [src: file: backend/src/api/step_artifacts.rs:292-321#mosaic]
  [src: file: frontend/src/pages/StandaloneLivePageMosaic.tsx:133#standalone-live-page-mosaic-caption]
- **One Artifact per execution.** The link Artifact ↔ (run, step result,
  execution) is stored in `step_execution_artifacts` (migration 302), unique per
  execution: after a reload the button opens the Artifact already made, never a
  duplicate. Only this route sets the link, and it is a person's: a caller
  marked as an agent is refused, and the routes are absent from the bridge
  tokens.
  [src: file: backend/src/db/sql/302_step_execution_artifacts.sql:12-24#step_execution_artifacts]
  [src: file: backend/src/api/step_artifacts.rs:327-342#make_route]

The HTML is copied from the output with no dataset, through the redaction pass an
agent's message gets when a discussion stores it (`redact_agent_output`), not the
audit pass: only a real credential is masked, so game code that names a secret
word (`ball.spin = 0.4;`, `const secret = false;`) and a link's query string
stay as written, and an Artifact made from an execution holds what one made from
a discussion preview holds. The Artifact shows what the execution wrote at that
time.
[src: file: backend/src/api/step_artifacts.rs:199-206#redact_agent_output]
[src: file: backend/src/core/redact.rs:790-806#redact_agent_output]
[src: file: backend/src/db/discussions.rs:2901-2907#mask_agent_output]
It lives in the
Artifact library like any other, in a folder of its run (see
[the Artifacts architecture](../architecture/live-pages.md#run-folders-kt-1990)).

**Deleting the run deletes its Artifacts.** Deleting a run, all the runs of a
workflow, or the workflow deletes the Artifacts made from those runs, in the same
transaction. Each confirmation says how many, and tells the run's working files
(its worktree, the files it produced) from the Artifacts of the library. When
the runs have no such Artifact, it adds no sentence about Artifacts. The
workflow delete, shown on one line of the workflow card, counts them in the
same short sentence as its runs.
Deleting one Artifact only removes its link: the run view offers the button
again. Run retention never trims nor deletes a run that has such an Artifact.
[src: file: backend/src/db/workflows.rs:2678-2685#delete_run]
[src: file: backend/src/db/run_retention.rs:55#step_execution_artifacts]
[src: file: frontend/src/lib/stepArtifacts.ts:92-101#artifactsSentence]
[src: file: frontend/src/lib/stepArtifacts.ts:113-119#workflowDeleteImpact]

What each output keeps, so you know what a render can hold: a shadow keeps at
most 1 000 000 bytes of output and marks a longer one cut, which is then
refused; the main execution's output is the step result of the run, stored
whole with no cap of its own (the run view grades a heavy one, see
[heavy step outputs](../operations/workflow-heavy-outputs.md)).
[src: file: backend/src/workflows/shadow.rs:32#MAX_STORED_OUTPUT_BYTES]
[src: file: backend/src/workflows/steps.rs:970#with_runtime_notices]

## Merge the shadows into the step's output

Off by default, a step with shadows only compares them. Under the step's
**Shadows**, **Merge with the main execution** condenses the main execution and
the shadows you pick into one result, which becomes the step's output for the
rest of the workflow. Measured on the pull-request review above, two different
first passes merged find more than either alone; but a shadow on a small local
model may invent a finding, so the merge **verifies every point before it keeps
it**.
[src: file: backend/src/workflows/shadow_merge.rs:926-931#merge_step]

- **Which shadows merge**: one, several or all of the step's shadows
  (`shadows`, empty means all). The others still run and are compared.
- **The wait**: the merge waits for the selected shadows up to `timeout_secs`
  (10 minutes by default, 60 at most), goes on with those that finished and
  lists the missing ones with the reason (failed, timed out, still running, not
  sampled). A dead shadow never hangs the run.
- **The condense pass** is an Agent step on the step's own agent, or the one you
  name (usually the strongest model). It dedups the points of the main execution
  and of each shadow, verifies each point a shadow raised against the code,
  calibrates severity, writes the single result in the step's own output format
  and records the provenance of every point: which executions found it, and
  whether it was kept, was a duplicate or was dropped as unverified.
- **A point it cannot verify is never published.** The engine refuses a result
  where a shadow-only point is kept without a verification and its evidence, or
  where a dropped point's text still appears; the main output then stays the
  step's output and the run view says why. The same goes when no selected
  shadow finished or the condense pass fails.
- **A verdict has to prove itself** (KT-1731). Every kept point cites an
  evidence the engine can open: a `path:line` that exists in the working
  directory (the line inside the file) or a step output (`steps.<step>.output`).
  When an Exec step of the run printed, as its whole output, the path of a
  linked git worktree of the project's repository (a `setup` step that checks
  a pull request out beside the checkout), a `path:line` is also looked up
  there (KT-1960). The repository's own worktree registry decides what a
  worktree is, and the worktree has to point back at its record; one no step of the run named is never read, whatever branch it
  holds. A path that leaves the tree, through `..` or a link, never resolves,
  and the answer is the same whether its target exists. A line of a file that
  may hold secrets (`.env`, a key) is not an evidence, and a file over 2 MiB
  is not counted.
  A point marked `verified` whose citation does not resolve is refused, whatever
  its verdict. A point marked `verified` on an execution's own word alone is
  refused when only a shadow raised it; when the main execution raised it, the
  point stood before the merge, so it stays and loses the mark (KT-1947): the
  merge is published and the point reads « not verified », with the reason. A verdict that keeps
  no point (an empty list, `{}`) is refused unless the condense pass states in
  `agreement` that the executions agree there is nothing to report; the
  statement is then shown as the note of the merge. The statement is the pass's
  word: the engine does not check that the executions really agree.
- **The merged result becomes the step's output**; the main and shadow outputs
  stay visible for comparison.

The run view shows, under the step, how the merge ended, the missing shadows,
what each execution brought (points raised, of which in the result, of which
found alone, and left out as unverified: a point of the result counts for every
execution that found it, so a row always adds up), the provenance of every
point, what the merge spent and the merged result against each execution. The
main execution's row keeps its own tokens and cost, whether the merge was
published, refused or failed; the pass is counted in the step. Only a person
sets a merge, like a shadow.

The header of the run gives what the whole run spent (KT-1948): the tokens of
its steps (a merge pass is counted in its step) plus those of its shadows, and
the summed cost of the steps, the shadows and the merges. The cost is preceded
by « ≥ » when an execution spent tokens without reporting a cost. The run list,
the workflow card and the token statistics show the same tokens, steps plus
shadows (KT-1957): each run stores what its shadows spent, recounted whenever
one of them finishes. The MCP status and wait answers keep the steps' tokens,
next to the steps they list.
[src: file: frontend/src/lib/shadowRuns.ts:336-341#mergeTally]

## Show the shadows on a Page

A Page only reads the datasets a workflow published, and a shadow's output
never reaches a later step. What a later step can read is a **summary** of the
step's shadows and merge, `{{steps.<name>.shadows}}` (also
`{{previous_step.shadows}}`): who ran with which agent, model and transport,
how each ended and what it cost, what the merge kept, and how deterministic the
step is. A `PublishPageData` step writes it into a dataset with
`value_from: "steps.advise.shadows"`, and the Page then shows the merged shadow
and the identical one beside the step's own agent.
[src: file: backend/src/workflows/shadow_summary.rs:297#summarize]
[src: file: backend/src/workflows/template.rs:204#set_step_shadows]

```json
{ "step": "advise",
  "executions": [
    { "key": "main", "label": "main", "identical": false, "agent": "ClaudeCode", "model": "claude-opus-4-1",
      "transport": "cli", "status": "success", "tokens": 8200, "cost_usd": 0.31, "duration_ms": 41000 },
    { "key": "argon", "label": "Argon", "identical": false, "agent": "LiteLlm", "model": "argon",
      "transport": "http", "status": "success", "tokens": 7400, "cost_usd": null, "duration_ms": 52000 },
    { "key": "qwen", "label": "Qwen", "identical": true, "agent": "ClaudeCode", "model": "claude-opus-4-1",
      "transport": "cli", "status": "running", "tokens": null, "cost_usd": null, "duration_ms": null } ],
  "merge": { "status": "merged", "included": ["argon"], "missing": [{ "key": "qwen", "reason": "still running when the merge stopped waiting (600 s)" }],
    "points": { "kept": 5, "unique": 2, "duplicate": 3, "dropped": 1 },
    "per_execution": { "main": { "kept": 3, "duplicate": 0, "dropped": 0 }, "argon": { "kept": 2, "duplicate": 3, "dropped": 1 } } },
  "determinism": { "identical_runs": 1, "distinct_outputs": 1 } }
```

- **`executions`** lists the main execution first, then the shadows that
  started, in start order. `status` is `success`, `failed`, `skipped`,
  `over_budget`, `timed_out`, `interrupted` (Kronn stopped while it ran) or
  `running`. `transport` is `cli` (an agent CLI), `http` (a remote endpoint)
  or `local` (Ollama). A measure not known yet, or never reported (a cost, for
  a provider that returns none), is `null`, never `0`.
- **`merge`** is `null` when the step merges nothing. `included` are the
  shadows whose output went into the condense pass, `missing` those that did
  not, with the reason. `points` counts the points by what became of them
  (`unique` is the kept points only one execution found), `per_execution` the
  share of each execution.
  [src: file: backend/src/workflows/shadow_summary.rs:259#merge_summary]
- **`determinism`** is `null` without an identical shadow; otherwise how many
  executions produced an output (the main one and the identical shadows) and how
  many distinct outputs they gave, timings left out, as the run view counts them.
  [src: file: backend/src/workflows/shadow_summary.rs:221#determinism]
- **It is final when the next step renders.** The merge runs inside the
  shadowed step, so the step after it waits for the merge, up to its timeout. A
  shadow still going (not selected by the merge, or past its timeout) is
  reported `running` and the summary is not rewritten afterwards. A run resumed
  after a Gate rebuilds it from what was stored.
  [src: file: backend/src/workflows/shadow_summary.rs:366#restore]
- **No output, prompt or secret is in it**: the identity (agent, model,
  transport), the status, the measures and the merge's counts only. The outputs
  and the merge's points stay in the run view.
- **A step without shadows** has the same summary with `executions: []`, so a
  template never needs to know whether the step carries any. The save-time check
  of `{{steps.X.…}}` references does not cover `.shadows`: a misspelt step name
  fails when the run renders it.

## Promote the better version

A Page shows, in its **Quick Prompts and skills** panel, every Quick Prompt and
skill used by the workflows behind it, step by step, with the version each step
loads and the one each of its shadows tests.
[src: file: backend/src/db/live_pages.rs:2244-2266#list_live_page_resources]
**Promote** hands the version a shadow tested to its step in one click:

- a skill: the step now loads the tested version, pinned (a skill the shadow
  drops is dropped from the step);
- a Quick Prompt: the step pins the tested version; the Quick Prompt itself is
  unchanged, so other workflows keep theirs.

Either way that shadow stops testing it; the other shadows keep theirs. Promotion
is a person's decision too: the route has no agent access.
[src: file: backend/src/workflows/shadow.rs:1177#promote]
[src: file: backend/src/api/workflows.rs:2885#promote_shadow]

## From the workflow-level shadow

Before step shadows, a workflow had one `shadow` with lanes that replayed the
whole workflow. A workflow that had one now carries one shadow on each of its
Agent steps (those that stay inside Kronn), with the same overrides: the shadow
of a flat shadow is named `shadow`, a lane keeps its id, label, projects,
sampling, skills, prompt and budget, and an agent set for a named step goes to
that step. The role (`witness`, `contributor`) is gone. The migration moves only
the stored shadow: the steps are left as they were.
[src: file: backend/src/workflows/shadow.rs:1401#legacy_shadow_to_steps]
[src: file: backend/src/workflows/shadow.rs:1401#migrate_workflow_shadows]
[src: file: backend/src/db/workflows.rs:3010-3013#split_shadows]

Runs the former shadow recorded stay in the run list with a **Shadow** badge.
An export written before this change carries a `shadow` key that an import now
ignores.

## Current limits

- **A shadow that runs a different workflow** is not supported: a shadow
  re-runs its own step.
- **The merge costs one more agent call**, and reads the outputs, not the whole
  input again, unless a point needs verification. Its tokens are reported, and
  counted in the step's tokens.
- **Tokens and cost** are checked when a shadow ends, not while it runs: a single
  long shadow can spend more than its budget before it is marked.
- A step carries at most eight shadows.
- Only Agent steps carry shadows.
- **An Artifact of an execution is made by a person**, one execution or one
  step at a time: no option publishes every execution of a step on its own.

See also: [the Kronn way](the-kronn-way.md) and the
[glossary](../glossary.md) entries *Step shadow*, *Shadow run* and *Page
resources*.
