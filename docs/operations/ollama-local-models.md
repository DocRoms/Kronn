# Ollama local models — reliable subtask workers (0.11.0)

Run bounded subtasks and deterministic workflow steps on **local** models via
Ollama to reduce API cost and keep work moving during cloud rate limits. Keep a
strong principal agent for decomposition and review; use the local worker for a
small scope with an explicit Definition of Done and mechanically checkable
evidence.

## Capability boundary

Ollama is an HTTP agent, so it has **no host shell and no arbitrary MCP
bridge**. It is not text-only, however. When Kronn can bind a project/worktree,
it declares a bounded native toolbox on the request:

- workspace discovery, sliced reads/searches and exact receipt-bound edits;
- local Git status/diff/log and commit (never push or merge);
- configured REST/Quick APIs, safe web fetches and the planning reads relevant
  to the current scope.

A task worker gets a narrower catalogue than a principal: it cannot reshape the
backlog, launch other executions or approve its own result. That restriction is
part of the correctness contract, not a model-specific workaround. A workflow
without a project has no workspace to expose; inject its inputs in the prompt or
through an earlier Exec/API step.

Worker delivery is intentionally `task_exec_deliver({manifest})`, with no
execution id or caller identity in the model-authored schema. Kronn resolves the
target from the executor's durable child room, typed provider, source message
and exact dispatch job. A supplied `task_execution_id` is ignored even when it
names another valid concurrent execution. Principals keep explicit execution
references for the lifecycle actions they steer; workers are not offered
`task_exec_status`, because asking a local model to reconstruct an opaque UUID
from a branch or worktree name is both unreliable and an authorization smell.
`[src: file: backend/src/api/agent_tools.rs]`

The discussion SSE path resolves this scope from the durable
`task_executions.sub_discussion_id` lineage before it constructs the HTTP tool
executor. If that lookup fails, the run is refused before provider inference;
Kronn must never silently broaden a worker to the principal catalogue.
`[src: file: backend/src/api/discussions/streaming.rs:45-89]`
`[src: file: backend/src/db/orchestration.rs:525-535]`

## Recommended subtask loop

### Positive eligibility and bounded fallback

The principal, not the local worker, decides whether a task is a good local
unit. Start with `agent_list()`, copy the Ollama entry's typed `worker` object
unchanged, then pass it to `task_exec_prepare`. The catalogue distinguishes
`configured`, `reachable` and `available`; unavailable workers remain visible
with stable, secret-free reasons. `available` proves transport readiness only.
It does not prove model entitlement, model quality or task fit, and
it does not probe whether Ollama has already pulled the exact resolved tag. A
missing tag fails explicitly at `/api/chat` and must be treated as a model
presence failure, not as evidence that the transport probe lied. Likewise,
`task_exec_prepare` still proves that the repository, task state and selected
worker are launchable. An Ollama subtask is eligible only when all of these are
positively true:

- the objective is one atomic change or analysis; for repository work the
  principal names the exact file **and** prelocalizes the symbol or inclusive
  line range (a file name alone still delegates discovery), while data work
  names the exact input payload;
- the worker can collect the required evidence inside its bounded catalogue,
  without discovering an architecture or reconstructing an implicit protocol;
- success is mechanically distinguishable from plausible prose through exact
  DoD evidence and principal-owned validation commands;
- the principal will review the delivered SHA and remains the only integration
  authority; the worker's allowed `files_touched` set is closed in advance and
  includes only the target plus any explicitly named colocated test.

Use a stronger worker from the start for trust or protocol boundaries,
authentication/security, concurrency, migrations/data integrity, architectural
decisions, or parity changes spanning several layers. A small file count does
not make such a task local-safe: a schema + handler + persistence + test parity
change is still transversal even when it happens to touch only two files.

Fallback is deliberately bounded. A principal may request one local rework
when the review identifies a narrow, evidence-backed correction. If the worker
does not produce a durable manifest, or the review reveals a structural
misunderstanding, preserve the attempt and its tool trace, then reassign once
to a stronger worker. Replaying the same local model indefinitely is neither a
cost optimization nor recovery. Transport failures and model-quality failures
must remain distinguishable in the audit trail.

A Rust parser refusal is that one local correction, not permission to explore
again. Kronn writes no bytes, keeps the previous receipt authoritative, freezes
the same edit primitive/path/anchor or line range, and accepts exactly one new
replacement proposal based on the parser's line, column and message. A second
invalid proposal ends the local attempt so the principal can reassign the
preserved worktree to a stronger worker. Builds and cross-file tests remain the
principal/integrator gate; running `cargo` after every local edit would replace
a cheap structural check with an expensive, noisy one.

1. The principal creates one cohesive task with paths/scope, acceptance checks
   and stable DoD identifiers. It calls `agent_list`, copies an available
   Ollama `worker`, preflights that exact identity, and supplies deterministic
   validation commands at launch; these belong to the run policy, never to the
   worker manifest.
2. Kronn launches a fresh attempt in its worker room/worktree. The brief says
   which tools exist and that there is no shell.
3. The worker reads before editing. Existing-file writes require 32 to 64
   leading hexadecimal characters of the SHA-256 receipt returned by a
   read/search; fewer than 32 characters and stale or ambiguous edits refuse
   without mutation. The 128-bit minimum preserves a strong whole-file CAS
   guard while tolerating a local model dropping the last token(s) of a long
   hex value. It commits locally, then delivers the commit and evidence for
   every DoD item.
4. The principal reviews the **delivered attempt and exact commit SHA** and
   records a met/unmet result plus non-empty evidence for every DoD. Worker prose
   and planning checkboxes cannot forge approval.
5. After approval, Kronn runs the principal-owned commands against the ephemeral
   merge candidate. Only a green candidate can advance the target branch. A
   rejected or mechanically red result gets a new attempt; an unknown
capability becomes a visible blocker.

### Prelocalized mutation contracts

For a tiny, line-bounded replacement, the principal passes the same optional
`worker_scope` to `task_exec_prepare` and `task_exec_launch`:

```json
{
  "mode": "prelocalized_edit",
  "path": "backend/src/example.rs",
  "start_line": 40,
  "end_line": 44
}
```

This is an execution boundary, not prompt advice. Kronn accepts it only for a
native HTTP `discussion_agent`, persists it on the execution, validates the
relative path and inclusive range against the SHA-pinned worktree before
dispatch, and refuses an idempotent replay that tries to change it. The range
is limited to 200 lines.

The first model turn exposes only one `read_file`, whose path and padded context
window are fixed by Kronn. After that real read succeeds, every read/search tool
is withdrawn and only one `edit_lines` remains; its path, inclusive range and
fresh whole-file `content_sha256` receipt are fixed server-side. Only
`new_string` is model-authored. Kronn rechecks those arguments before execution,
so remembered or hallucinated tools cannot widen the target. Read and edit each
allow one response plus one correction. Exhaustion fails visibly with a stable
`prelocalized_*_exhausted` reason and leaves the worktree unmutated for
reassignment. After a successful edit, Kronn exposes only `git_commit` (one
response plus one correction), then only `task_exec_deliver`: status/diff and
all read/edit tools stay withdrawn.

Before any write, the edit is refused and nothing reaches disk when it breaks
the file's structure. Rust is parsed. PHP, Twig, SCSS/CSS/LESS, TS/JS and JSON
get a language-aware delimiter check (strings and comments ignored; Twig tags
and `if`/`for`/`block`… with their `end…`), applied only to a file whose
delimiters were balanced before the edit. An `edit_lines` replacement must
also keep the indentation of the first and last non-blank lines it replaces;
inner lines may move. The diagnostic starts with `Structure validation refused`
and, like a Rust parser error, puts the worker into its one strict correction
of the same frozen range. The prelocalized brief carries no human-arbitration
or parent-milestone section: a bounded edit has neither.
`[src: file: backend/src/api/agent_workspace_structure.rs]`

For a pure insertion, do not make the worker reproduce a paragraph inside a
replacement range. Use one verified anchor line:

```json
{
  "mode": "prelocalized_insert_after",
  "path": "docs/operations/example.md",
  "anchor_line": 58
}
```

Both `task_exec_prepare` and `task_exec_launch` must pair that object with
`worker_scope_intent: "scoped"`. A deliberately unrestricted worker instead
uses `worker_scope_intent: "generic"` and omits `worker_scope`; omitting the
intent itself is refused with an MCP reconnect diagnostic, because it means a
stale host tool schema may have stripped the scope before transport.
This is an intentional fail-closed contract break: MCP sessions connected
before the contract was introduced must reconnect before their next prepare or
launch; Kronn never treats their missing intent as an implicit generic launch.
The check must cover the contract actually declared and transported, not only the Python process fingerprint: a fresh bridge process can still carry a stale host tool declaration that omits `worker_scope`, so the bridge/backend must verify the schema version/capability of the transported declaration and refuse before provisioning with a reconnect diagnostic.

Kronn then exposes `insert_after_line` after the authoritative read. The
worker can author only `new_string`; path, anchor and receipt are frozen, and
the executor preserves the anchor bytes mechanically. Empty insertions, stale
receipts, missing anchors and source-syntax failures write nothing. For an
ordinary `prelocalized_edit`, choose the narrowest verified range possible —
`start_line == end_line` when one line really is the whole replacement — but
never use replacement semantics for an insertion when this structural mode is
available.
A pure `insert_after_line` never asks the worker to retype the anchor: the executor preserves the anchor bytes mechanically, so the worker authors only the new text.

The scope is written against the pinned base, but a delivery can change the
file's line count. Before `request_changes` hands the work back, and before a
resumed or reassigned worker runs, Kronn re-anchors it: every line before the
range (and after it, for `prelocalized_edit`) must still match the base byte for
byte, and the new range is what lies between them. The persisted scope is
updated, journaled as `worker_scope_reanchored`, and the hand-off names the new
range. When anything outside the range changed, the range was deleted
entirely, or it grew past 200 lines, the rework is refused with
`stale range: …; relaunch with a new worker_scope`, naming the launch range
and the observed change.
`[src: file: backend/src/api/orchestration.rs]`
`task_exec_status` now exposes `usage.http` — cumulative HTTP traffic, context peak, and per-phase detail — without prompts or arguments.

Native HTTP delivery — and delivery from a spawned host CLI worker that uses
the same runner capability — is also a projection, not a transcription
exercise. The model authors only tests, ordered `{met, evidence}` DoD assertions, docs,
migrations, risks, limitations and a summary. After authenticating the exact
worker capability, Kronn injects the contract version, task reference, current
clean committed HEAD, committed file inventory and opaque DoD ids from the
task/worktree. The ordered DoD ids are snapshotted when the execution is
created; a reorder/replacement under an active brief is refused instead of
silently attaching evidence to another item. A wrong assertion count or a
model-authored mechanical field is refused before persistence. Only joined CLI
sessions and public/principal callers keep the full DeliveryManifest v1
contract.

Use this mode only when the principal has already verified the exact replacement
range and that the automatically padded window contains enough evidence. If the
worker needs to discover a symbol, compare several files or broaden the range,
split/pre-analyse the task first or use the normal bounded worker path instead
of weakening this contract.
`[src: file: backend/src/models/orchestration.rs]`
`[src: file: backend/src/agents/runner.rs]`

When changing this lifecycle itself, first run the same commit → manifest →
review → integration gate with capable Claude/Codex workers. That separates a
transport or authorization defect from a small-model convergence defect. Only
after that invariant is green should an Ollama replay be treated as a model
quality test. The Ollama acceptance gate remains identical: a plausible answer
without a persisted manifest and principal-reviewed SHA is not success.

Do not delegate an open-ended architectural decision, an unbounded repository
tour or the final quality gate to a small local model. First split it into a
read/transform/edit unit whose failure is observable.

On reassignment, the principal's `reason` is part of the new worker message,
not audit metadata only. Use it for the shortest recovery instruction (for
example, “the commit already exists; deliver only the manifest”). The generic
handoff and the principal's reason are additive; a recovery must not make the
worker infer why it was restarted from branch names or old prose.

The selected provider, tier, explicit model and profile are also written to the
durable child discussion in the same transaction as the execution assignment.
This room state is the runtime source of truth: persisting a model only on the
execution would make the audit display the new model while silently launching
the previous one. A reassignment must therefore fail atomically if its child
discussion has vanished; it must never fall back to the old model.

Task child rooms pin their first message because it is a protocol brief, not
ordinary chat history: it contains the objective, ordered DoD and delivery
contract needed after every resume. Tool traces may fill the rolling context,
but truncation must never remove that brief. The `task_exec_deliver` tool
publishes the complete semantic projection schema; `manifest: {}` is not an
adequate contract for a local model, while opaque ids and Git facts never enter
its schema.

## Model resolution (precedence)

`runner::effective_model_flag` resolves the requested model without inventing
one. Empty or whitespace-only values are unset; a nonblank explicit ID is
preserved exactly.
[src: file: backend/src/agents/runner.rs:2650-2737]

1. **Explicit model** (`AgentStartConfig.model_override`) — wins outright.
   Fed from a workflow step's `agent_settings.model` (`steps.rs`) or a
   discussion's `model` (`streaming.rs`, e.g. inherited from the launching QP).
2. **Configured tier** → `resolve_model_flag(agent, tier, model_tiers)` uses
   that agent's exact per-tier value. An empty HTTP-chat tier may use that
   agent's configured Default; another provider's slot is never substituted.
3. **Durable catalogue** → the available assigned tier projected at bootstrap
   or after catalogue writes. HTTP-chat agents may use their assigned Default
   if their selected tier has no assignment. No assignment means no model.
[src: file: backend/src/agents/runner.rs:2650-2720]
[src: file: backend/src/core/model_catalog/mod.rs:54-97]

An unresolved Ollama launch is refused before any provider request, with an
actionable Settings/override message. Historical seeds are written once by the
migration; no runner fallback revives them after deletion. Unit tests use the
same resolver as production; the historical expectation helper itself is
test-only and is not called by the resolver.
[src: file: backend/src/agents/runner.rs:2739-2741]
[src: file: backend/src/agents/runner.rs:3036-3090]
[src: file: backend/src/core/model_catalog/mod.rs:62-77]
[src: file: backend/src/core/model_catalog/mod.rs:98-219]

Ollama is always routed through HTTP before the production command-builder
call. Its obsolete `ollama run` branch is a non-inference diagnostic sentinel,
like the other HTTP families; it cannot choose or invoke an embedded model.
This is based on the caller's control flow, not an assumption from its comment.
[src: file: backend/src/agents/runner.rs:2608-2613]
[src: file: backend/src/agents/runner.rs:3114-3134]
[src: file: backend/src/agents/runner.rs:3232-3248]
[src: file: backend/src/agents/runner.rs:8748-8759]

Preflight checks the exact durable target/tier assignment, including unavailable
rows, before using any HTTP Default assignment. Otherwise the launch cache's
intentional exclusion of unavailable rows could hide a disappeared model behind
another model or a CLI default. Explicit operator settings still take priority;
failed assignment reads refuse launch. A target with no selected or assigned
identity keeps the existing no-model preflight behavior; this change does not
introduce new discovery for unconfigured runtimes.
[src: file: backend/src/core/model_catalog/mod.rs:493-549]

The production integration fixture links the library without `cfg(test)` and
uses isolated SQLite plus explicit loopback endpoints. It actually creates,
dispatches and deletes a catalogue entry, refreshes the runtime projection,
asserts refusal without a further request, and preserves a later explicit ID.
Other cases cover empty/whitespace models and each HTTP family's own configured
Default and exact override, unavailable tier versus available Default, exact
connection namespace, and failed catalogue reads. No real provider inference
is qualified.
[src: file: backend/tests/http_model_resolution.rs:1-404]

Per-step model: workflow Agent step → Advanced → *Modèle* (WorkflowStep
`agent_settings.model`). Per-QP model: QuickPrompt form → *Modèle* field
(persisted in `quick_prompts.agent_settings_json`, migration 070). A QP model
reaches execution via three paths: workflow hydration
(`quick_prompt_hydrate`), batch launch (`create_batch_run`), and standalone
launch (`crud.rs` stamps `discussions.model` from the QP).

## "Stable output", NOT bit-exact determinism

`build_ollama_chat_body` [src: file: backend/src/agents/runner.rs] sends
`temperature:0, top_k:1, seed:42` as INTERNAL constants (no per-step knob).
On Apple Metal the float reduction order isn't guaranteed, so two logits within
epsilon can flip the argmax (more so under Q4 quant) → output is *greedy-stable*,
**not bit-exact reproducible**. **Never** build logic (output hash-caching,
strict text-equality tests) that presumes reproducibility — especially not
cross-machine (Mac/Metal vs a CPU/CUDA peer). Ordered pillars: fixed num_ctx >
temp=0/top_k=1 > same model+quant > seed (near-inert under greedy).

Tests assert on the constructed request BODY, never on generated text.

## Context-window resolution and observability

Kronn resolves a ceiling independently for each model, in this order:

1. `KRONN_OLLAMA_NUM_CTX_CAP` — process-global break-glass override;
2. the persistent override for the exact model tag, configured through Kronn;
3. the model's trained window from Ollama `/api/show`, capped by the local
   machine's RAM tier;
4. a conservative portable fallback when the model window cannot be learned.

The model list exposes the trained window, the resolved **ceiling** and its
origin. A ceiling is not necessarily the `num_ctx` sent for one short,
tool-free prompt: Kronn may request less. A run that declares tools requests the
ceiling up front because tool results grow the conversation after model load.
The dedicated `kronn::ollama` event records the requested `num_ctx`, trained
window and resolution origin. The portable fallback is also announced in the
run output; it must never look like a fact about the model.

An override above the advertised model window or RAM-derived ceiling is
accepted with all applicable warnings, but impossible/fat-finger values are
bounded. The saved value is per model and survives restart.

### What `qwen3.8:27b-mlx` gets on a 64 GB Mac (KT-929)

The question came from a discussion where an Ollama agent on this model lost part
of a large API answer and the brief suspected a 32K window. Which window a run
gets depends on who runs the model, not only on the machine:

| Rule | Window | Where |
|---|---|---|
| Memory band for 64 GiB, weights and cache cost not yet known | 65,536 | `ram_derived_ceiling` [src: file: backend/src/agents/runner.rs:4940] |
| Trained window (262,144 in the test fixtures), held to the band | 65,536, origin `machine_ceiling` | `resolve_ctx_cap_within` |
| Discussion principal (`ToolRunMode::General`) | 65,536, requested up front because tools are declared | `worker_effective_ctx_cap` [src: file: backend/src/agents/runner.rs:216] |
| Task worker (`ToolRunMode::Worker`) on native MLX | 32,768 | `MLX_WORKER_EFFECTIVE_CTX_CAP` [src: file: backend/src/agents/runner.rs:93] |
| Per-model override (Settings), any value from 2,048 to 1,048,576 | the override, for a discussion; a worker is still held at 32,768 or below | `resolve_ctx_cap_for_model` |
| `KRONN_OLLAMA_NUM_CTX_CAP` | that number, for both | `resolve_ctx_cap_for_model` |

These rows are computed by the functions a run calls, and the test pins them, the
override cases included
[src: file: backend/src/agents/runner_test.rs:2520]. They are not an observation of
a running Ollama: the band, the trained window and the 64 GiB are inputs, and
`GET /api/ollama/models` (`context_ceiling`, `context_origin`) and the
`kronn::ollama` log line give what a given machine actually resolved. Once the
model has been loaded at two different windows, the exact figure the machine can
hold (70 % of memory minus the weights, over the measured cache cost per token)
replaces the band
[src: file: backend/src/agents/runner.rs:5208].

So the 32K ceiling is a worker's and not a discussion's, and a discussion on this
Mac is at 65,536 unless a per-model override or the environment variable says
otherwise. That is the one thing this section could not read: the override map
of the machine that produced the discussion. `ollama_context_overrides` in the
Kronn configuration, or `context_origin: "model_override"` in the model list, says
whether the discussion was at 65,536 or at a number somebody chose.

No cap was changed. The worker ceiling is there because a nominal 65K slot cost
31 GB and produced no first tool call in seventeen minutes (comment at
`MLX_WORKER_EFFECTIVE_CTX_CAP`), which is evidence against raising it. The
discussion's 65,536 is already above what the failing case needed once `api_call`
can select: a 198 KB answer, the full catalogue and a 32,768-token window ended at
19,319 estimated tokens (see
[HTTP-agent capabilities](../architecture/http-agent-capabilities.md#asking-for-part-of-an-api-response-kt-929)).

## Runtime gotchas (handled and empirically verified)

1. **num_ctx** — Ollama's default context window is huge (up to 256K for some
   qwen3 tags). An oversized KV cache balloons memory and spills onto the CPU:
   `llama3.3:70b` measured **0.2 tok/s** at 128K ctx vs **12.5 tok/s** at 8K
   (100% GPU). `ollama_num_ctx` sizes tool-free prompts within the resolved
   per-model ceiling. Diagnose with `ollama ps` (PROCESSOR column → want 100%
   GPU) and `kronn logs | grep ollama` (the requested `num_ctx` is logged on
   target `kronn::ollama`).
2. **qwen3 reasoning** — qwen3 are hybrid-reasoning. The Ollama `think:false`
   API flag is **NOT honored** on `/api/chat` (verified: `message.content`
   still carries reasoning, untagged). The only reliable switch is the qwen
   `/no_think` control token in a dedicated system message
   (`ollama_disables_thinking`, applied to any `qwen3*` tag), which routes
   reasoning into a separate `thinking` field and keeps `content` clean. The
   `strip_thinking_leaks` regex is only a secondary net (widened to catch the
   short `<think>` tag; it can't catch untagged reasoning).
3. **Long MLX tool loops** — Ollama's MLX engine currently re-prefills the
   growing conversation on every `/api/chat` turn and can retain KV memory
   across requests. On `qwen3.8:27b-mlx`, a real worker stayed responsive for
   roughly 32 tool rounds, then individual turns grew to several minutes.
   Kronn detects the actual `safetensors` storage format from the cached
   `/api/show` profile; an explicit `mlx` tag is only the fallback for older
   metadata or aliases that preserve the engine name. MLX-risk workers enter
   delivery finalization at the first of 32 rounds or 75% estimated context
   pressure. They also enter finalization after 12 successful repository
   observations without a workspace mutation: this is an explicit pre-analysis
   budget for small local subtasks, not a claim that every distinct read was
   semantically useless. Other Ollama workers use 50 rounds or 75% and have no
   observation budget. Finalization retains only the CAS read/edit/Git/delivery
   tools for 12 bounded rounds. A successful
   `git_commit` then starts a separate three-attempt delivery phase whose
   catalogue contains only `task_exec_deliver`; a commit on the last
   finalization round therefore still has a bounded path to a durable manifest.
   The delivery tool accepts only the manifest. This was pinned after a real
   `qwen3.6:35b-mlx` worker made the requested repair and commit, then guessed a
   shortened execution id from its branch name because the old HTTP schema
   demanded an opaque id it could not possess.
   Finalization also bounds repository inspection per mutation epoch: after at
   most three combined `git_status`/`git_diff` calls since the last successful
   edit, both inspection tools are removed and the worker must commit, make a
   justified edit from the evidence it already has, or name the exact blocker.
   A later successful edit starts one fresh inspection epoch; it does not
   restore a tool disabled by an error circuit or another global budget.
   If an actual write/edit is refused after the finalization reads are spent,
   Kronn arms one non-renewable repair sequence outside that 12-round budget:
   up to two response attempts to execute exactly one `read_file`, up to three
   edit responses (one stale remembered call, one invalid edit and its bounded
   correction), then up to three Git status/diff/commit rounds. A model that
   remembers `read_file` after Kronn withdraws it enters that same sequence on
   the first refusal, before generic convergence can discard the still-available
   edit tools. Each stage exposes only its own tools; no search, tree walk or
   second repair sequence can re-enter the conversation.
   Do not reset the live conversation to the initial brief at those phase
   boundaries. A bounded `qwen3.6:35b-mlx` dogfood run reached the correct
   repair read, but an experimental reset then made all three edit responses
   request withdrawn exploration tools and produced no mutation. The
   experiment was reverted: a shorter prompt is not automatically a better
   prompt when it removes the model's accumulated implementation intent.
   `[src: commit: 0019cdf]` `[src: commit: 430cb4e9]`
   A native MLX worker also uses a 32K effective exploration ceiling (or the
   operator's smaller explicit cap) instead of eagerly asking Ollama for the
   model's full trained window. The slot cannot grow while that model instance
   stays loaded, so every exploration clamp/resize uses the same ceiling. Its
   75% pressure boundary leaves 8K of the effective slot for the next response
   and tool result; retaining the earlier 50% boundary after imposing the 32K
   cap would have compounded both mitigations and forced finalization near 16K.
   If
   the initial brief plus tool catalogue and reply headroom do not honestly fit,
   Kronn refuses the run with an actionable error instead of silently trimming
   the task. GGUF and OpenAI-wire providers keep their existing context policy.
   [src: file: backend/src/agents/runner.rs]
   Finalization instead uses a deterministic checkpoint after the already
   authorized boundary call has executed and its result has been recorded:
   immutable system/user policy plus the three most recent complete
   assistant-tool/result rounds whose tools still exist in the narrowed
   catalogue. This keeps the in-flight result auditable without teaching the
   next turn to request a broad exploration tool that Kronn just withdrew.
   Large retained results become valid JSON envelopes which preserve scalar
   facts — including CAS receipts, paths and offsets — and bounded excerpts of
   large fields. Kronn adds only executor-observed mutation paths and the exact
   narrowed tool catalogue; it never asks the model to invent a summary of its
   own state. The phase is trimmed against a 16K context target with explicit
   reply headroom, and diagnostics expose the seed/tail/result and token counts.
   A commit/delivery dogfood remains the acceptance gate: unit compaction alone
   cannot prove that a local model preserved its implementation trajectory.
   Kronn stops immediately after the manifest is accepted instead of paying for
   another full model turn. Prefer a GGUF/non-MLX model for an unusually long
   repository investigation until the upstream prefix-cache and retained-memory
   defects are fixed.
   [src: file: backend/src/agents/runner.rs]
   [src: url: https://github.com/ollama/ollama/issues/17829]
   [src: url: https://github.com/ollama/ollama/issues/17875]
4. **Quoted scalar arguments** — local models sometimes serialize a numeric or
   boolean tool argument as a JSON string. Kronn normalizes only unambiguous
   scalar spellings at the tool boundary: unsigned decimal counts for sliced
   reads and line edits, and explicit `true`/`false` values for flags. Prose,
   signed numbers and fractional values remain refusals; path, range and CAS
   validation are unchanged.
   Missing required line fields are reported separately from present-but-invalid
   values so the model can correct the shape instead of guessing a new number.
   [src: file: backend/src/api/agent_tools.rs]
5. **Rust edits are structurally atomic** — `write_file`, `edit_file` and
   `edit_lines` parse a proposed `.rs` file in memory before the first durable
   byte. A refusal reports the exact parser position and leaves the prior bytes
   and SHA receipt unchanged. In a worker run it immediately ends exploration,
   narrows the catalogue to the same preconstructed edit target for one
   correction, then proceeds to bounded commit/delivery or returns control for
   strong fallback. Non-Rust files and syntactically valid Rust edits keep the
   existing CAS behavior.
   [src: file: backend/src/api/agent_workspace_tools.rs]
   [src: file: backend/src/agents/runner.rs]
6. **Failed workspace edits remain diagnosable** — persisted tool traces include
   bounded arguments and a bounded, single-line executor error for workspace
   tools such as `edit_lines`. This keeps an interrupted local-worker attempt
   auditable after its stream has ended. API calls remain route-only (never
   query/body/error data), and commit arguments remain hidden.
   [src: file: backend/src/agents/tools.rs]

## TypedSchema → constrained JSON + quality escalation

For an Agent step with `output_format: TypedSchema`, `steps.rs::ollama_envelope_format`
wraps the author's `data` schema in the canonical envelope shape
`{data, status, summary}` and requests constrained decoding through Ollama's
`format` parameter. OpenAI-compatible HTTP agents send the corresponding
`response_format: json_schema`. Both use `stream:false`; CLI agents receive the
schema through the prompt. Provider acceptance alone does not prove schema
enforcement: Kronn still extracts and validates the returned envelope.

If the initial HTTP response explicitly rejects structured output support
(400, 422 or 501), Kronn retries once without that format field. The model,
prompt, tools and other settings stay unchanged. This occurs before any tool
execution, and a notice is retained in the run output and diagnostics. A
generic 501, invalid schema, authentication or quota error does not authorize
this fallback; 501 is never treated as a transient capacity failure. There is
no permanent MLX/model blacklist: a future provider that accepts the format
uses it normally.

Post-extract schema validation and repair still run. The step's `on_invalid`
policy is unchanged: `Fail` rejects output that remains invalid, while
`Continue` may keep invalid raw output. Prompt-based output is not a guarantee
of schema compliance. Repeated launches, including a repair launch, negotiate
the capability independently; no persistent capability cache is written.
`[src: file: backend/src/agents/runner.rs]`
`[src: file: backend/src/workflows/steps.rs]`

**Quality escalation** (`steps.rs::escalation_step`): if a LOCAL (Ollama)
TypedSchema step still fails validation after the repair attempt, it retries
ONCE on the paid reasoning tier (Claude) before falling through to `on_invalid`.
This is a loop POLICY (derived from the step having run on Ollama), not a knob.
The escalation RATE — logged on `kronn::ollama::escalation` — is the health
metric that reveals which steps are too hard for the chosen local model.

## Downloading, updating and Mac MLX builds from Settings (KT-930)

Config › Agents › Local models › Ollama.

### The download block folds

The "Download a model" block opens by itself while nothing is installed (a first
use) and is folded as soon as one model is. It is the way to ADD a model, not
something to scroll past forever. Once the user clicks it, that choice is kept in
this browser under `kronn:ollamaDownloadOpen` and wins over the default; the
default itself is never written, so someone who never touched it keeps the
automatic behaviour. Folded, its summary line counts the suggestions and the
downloads in progress. A download or an update in flight is drawn above the
block, outside the fold, with its progress and its Cancel, so folding the block
back does not hide it. [src: file: frontend/src/components/settings/OllamaCard.tsx:157]
[src: file: frontend/src/hooks/usePersistentFold.ts:19]

### The suggestions, and what "verified" means here

`frontend/src/components/settings/ollamaModels.ts` holds one portable suggestion
per hardware tier and a short list of Mac MLX builds. Every entry is an exact tag
with the repository file it was taken from, and a tag goes in only when something
in this repository shows it exists AND ran. Sizes are not stored: a figure typed
here goes stale, so the card shows the one the backend reads off the library's own
manifest (see [Reading the official library](#reading-the-official-library)).
[src: file: frontend/src/components/settings/ollamaModels.ts:31]

| Tag | Tier | Seen to exist and run in |
|---|---|---|
| `qwen3.5:4b` | CPU, no GPU | `docs/research/native-tool-catalogue-2026-09-22.md` (2026-09-22 run) |
| `qwen3:8b` | ≥ 16 GB RAM | the Ollama Default seed of the model catalogue [src: file: backend/src/core/model_catalog/mod.rs:100]; the bench table below |
| `qwen3:30b-a3b` | GPU or ≥ 32 GB | the Ollama Reasoning seed [src: file: backend/src/core/model_catalog/mod.rs:101]; the bench table below |
| `gemma4:12b-mlx` | ≥ 16 GB RAM, Mac only | `docs/research/native-tool-catalogue-2026-09-22.md` |
| `qwen3.8:27b-mlx` | GPU or ≥ 32 GB, Mac only | `docs/research/native-qp-litellm-ollama-2026-09-22.md` |

**Checked against the live library on 2026-10-01.** All five tags answer HTTP 200
on `https://registry.ollama.ai/v2/library/<family>/manifests/<tag>` with
`Accept: application/vnd.docker.distribution.manifest.v2+json` (`qwen3.5:4b`,
`qwen3:8b`, `qwen3:30b-a3b`, `gemma4:12b-mlx`, `qwen3.8:27b-mlx`). The check was
run from the user's own machine by the task's reviewer, because the sandbox the
list was written in cannot reach the registry; the column above says where each
tag was first seen to run.

Re-check before a release, one tag at a time:

```sh
curl -s -o /dev/null -w '%{http_code}\n' \
  -H 'Accept: application/vnd.docker.distribution.manifest.v2+json' \
  https://registry.ollama.ai/v2/library/qwen3.8/manifests/27b-mlx
```

A plain sibling of an MLX tag (`gemma4:12b`, `qwen3.8:27b`) is deliberately NOT
suggested: nothing here shows it exists, and a name that does not exist is a
download that fails in front of the user.

### Updating an installed model

The installed models are listed inside the download block with an **Update**
button, hidden for a model the registry confirmed up to date (a **Not checked**
model keeps it, since nothing was confirmed). It is the download flow pointed at a tag Ollama already holds:
`POST /api/ollama/pull` with that exact tag, the same progress, the same Cancel,
the same error messages, then a refresh of the installed list. There is no
separate update endpoint to drift from the download one. [src: file: backend/src/api/ollama.rs:502]

Update re-asks Ollama for the tag; the files that did not change stay as they are.
Next to each model's Update button a badge says whether the tag has moved on:
**Update available**, **Up to date**, or **Not checked**. When the block is
folded, its summary counts the updates waiting. Updating a model withholds its
badge until the library has been asked again, so a verdict that described the
replaced copy is never shown. [src: file: frontend/src/components/settings/OllamaCard.tsx:449]

### Reading the official library

Both the "update available" badge and the size shown on each suggestion come
from the registry's manifest of a tag, `GET
https://registry.ollama.ai/v2/library/<family>/manifests/<tag>` with
`Accept: application/vnd.docker.distribution.manifest.v2+json`. Nothing is
downloaded. [src: file: backend/src/core/ollama_registry.rs:43]

- **Freshness.** The registry does not send a `Docker-Content-Digest` header, but
  the SHA-256 of the manifest body, hashed exactly as received, is the `digest`
  that the local Ollama reports for the same tag in `/api/tags`. Measured on
  2026-10-01 on the user's machine: `qwen3.8:27b-mlx` (`5642e97495e1a088…`), `qwen3.5:4b`
  and `qwen3.5:2b` identical; `gemma4:12b-mlx` different (registry `ded7a27350032202…`,
  local `117d0d84cf2ab865…`), a real pending update. Equal is "up to date",
  different is "update available". [src: file: backend/src/core/ollama_registry.rs:184]
  [src: file: backend/src/core/ollama_registry.rs:211]
- **Size.** The manifest's `config.size` plus every `layers[].size`, formatted
  like the installed list ("4.1 GB"). A manifest with a missing or overflowing
  size gives no size rather than a partial sum.
- **Never a guess.** Each of these reads "not checked" (`unknown`), and never "up
  to date": the registry did not answer or was too slow, the body was not a
  manifest (an HTML error page served with a 200), the model is not in the
  official library (`user/model`, `hf.co/…`, a second colon, an uppercase family),
  or either digest is unreadable (a local Ollama too old to report one). A model
  that is not a plain library name is never even sent to the registry. A model
  made locally under a library name reads as an update, and updating it would
  replace it: that is the one case this comparison cannot tell apart.

How it stays light: `GET /api/ollama/registry?suggested=<tag>,<tag>` is its own
call, separate from `/api/ollama/models`, which the card never waits on: it is
drawn from the local answers and the badges and sizes fill in when the library
answers, or never. In the backend each lookup has a 4 s timeout, a 256 KiB body
cap, at most 4 run at once, at most 48 tags are looked up per request, and a whole
answer is bounded to 8 s; whatever finished by then is used and cached. Answers are
cached for 6 hours, a failure for 2 minutes (long enough that a blocked network is
not asked again on every visit, short enough that it never sticks). The only thing
that leaves the machine is a library tag name, sent by the Kronn backend, not the
browser. [src: file: backend/src/api/ollama.rs:316]

The cache can be bypassed: `&fresh=true` asks the registry again and replaces what
was cached. The card does it on the Refresh button and after an update finishes,
and only then. Without it, a model updated right after its tag moved on again would
be compared with a manifest cached before the move and called outdated; with it,
the verdict follows what is installed now. If the registry does not answer a
recheck, the model reads "not checked": a stale answer is not passed off as current.
The card marks a recheck done only once its answer has landed, so a request that a
newer render supersedes is followed by another fresh one, never by a cached one
that raced it. [src: file: backend/src/core/ollama_registry.rs:55]
[src: file: frontend/src/components/settings/OllamaCard.tsx:424]
[src: file: backend/src/core/ollama_registry.rs:47]
[src: file: backend/src/core/ollama_registry.rs:81]

To check one model by hand:

```sh
curl -s -H 'Accept: application/vnd.docker.distribution.manifest.v2+json' \
  https://registry.ollama.ai/v2/library/gemma4/manifests/12b-mlx | shasum -a 256
curl -s localhost:11434/api/tags   # "digest" of the same tag
```

The tests never reach the registry: the manifest source is a trait, and the
fixture is a hand-built manifest in the same shape (its fake blob digests aside)
whose SHA-256 was computed independently with `shasum -a 256`. They cover identical
and different digests, a dead registry, a model outside the library, a body that is
not a manifest, the cache's expiry, the concurrency bound, the time budget and the
size sum. [src: file: backend/src/core/ollama_registry.rs:459]

### Mac MLX builds

`GET /api/ollama/health` carries `version` (the running server's, from
`/api/version`, read fresh on each probe so a Refresh sees an update to Ollama)
and `mlx_capable`. The browser never decides: a user agent says nothing about
where Ollama runs. [src: file: backend/src/api/ollama.rs:102]

`mlx_capable` is true when both hold:

1. **The host is a Mac on Apple Silicon.** The OS is `host_os_label()`, so a
   Kronn in Docker on a Mac answers for the Mac; the chip is the build's own
   architecture (`aarch64`). An amd64 image emulated on Apple Silicon reads as
   Intel, which errs towards not offering MLX. [src: file: backend/src/core/env.rs:81]
2. **Ollama is at least 0.34.** That is the version the runner already scopes
   its MLX worker mitigations to, and the one MLX was measured on (0.34.2 reused
   9428 of 9500 prompt tokens on a second turn). Older MLX servers lack
   prompt-prefix reuse (ollama/ollama#17829), which is not what a suggestion
   should steer a user to.
   An unreadable version is not capable. [src: file: backend/src/agents/runner.rs:5448]

When it is, the `-mlx` builds are listed first with an "Optimized for Mac" mark,
then the portable list. Everywhere else the list is exactly the portable one.

## Bench snapshot (M5 Max 64 GB, Q4_K_M, informational)

| Model | tok/s (warm) | Note |
|---|---|---|
| qwen3:4b | ~114 | needs `/no_think`; economy |
| qwen3:8b | ~80 | clean, portable default |
| qwen3:30b-a3b | ~110 | MoE, best speed/quality |
| qwen3:32b | ~23 | dense, no advantage over 30b-a3b |
| llama3.3:70b | ~12.5 | excellent, heavy; cap num_ctx |

Numbers are machine-specific; the mechanisms above are hardware-agnostic.
