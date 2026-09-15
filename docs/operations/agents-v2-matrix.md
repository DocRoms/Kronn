# Agents v2 — compatibility matrix

What Kronn knows about each agent, and what it can only learn by talking to it.
Written for the operator deciding which agent to point at a job, and for
whoever has to explain why one of them behaves unlike the others.

Valid for 0.13.0.

## Read this first: two kinds of fact

A row below mixes two things that must not be confused.

**Decided by Kronn, true before anything runs.** Which route an agent takes,
what command starts it, whether a toggle is needed, whether task workers may
use it. These are compiled in; the table states them.

**Negotiated with the agent, unknown until a session opens.** Session resume,
permission callbacks, the model list. Kronn asks at `initialize` and believes
the answer. `runtime_profile` deliberately returns an EMPTY capability set: a
static table claiming "OpenCode supports resume" would be a guess, and the
first CLI update would make it a lie. The table says *negotiated* and means it.

## Routes

| Agent | Route | How it starts | Needs a toggle |
|---|---|---|---|
| OpenCode | native ACP | `opencode acp` | no |
| Gemini CLI | native ACP | `gemini --acp` | no |
| Copilot CLI | native ACP | `copilot --acp` | no |
| Kiro | native ACP | `kiro-cli acp` | no |
| Vibe | native ACP | `vibe-acp` | no |
| Claude Code | adapted ACP | `claude --print …` | no; `KRONN_ACP_ADAPTER_CLAUDE=0` selects direct compatibility |
| Codex | adapted ACP | `codex exec …` | no; `KRONN_ACP_ADAPTER_CODEX=0` selects direct compatibility |
| Ollama, LiteLLM, NVIDIA, Custom | HTTP provider | no process | no |

Claude and Codex have no ACP mode of their own. Their adapter is Kronn wrapping
the CLI in the ACP contract — one process per turn either way. Turning the
override changes the plumbing, not the process model.

### Bounded host check — September 8, 2026

The native commands above match the compiled dispatch table.
[src: file: backend/src/acp.rs:152-169]
These checks inspect help/version metadata, not authentication, negotiated
capabilities, provider availability or successful prompts.

| Native agent | Observed on the macOS qualification host | Official reference |
|---|---|---|
| OpenCode | 1.18.27; `opencode acp --help` exited 0. The latest release checked separately was 1.18.29; no update performed. | [ACP mode](https://opencode.ai/docs/acp/) |
| Gemini CLI | No `gemini` found in the audit shell's PATH; no local execution proof. This is not a claim about a separate container or configured runtime. | [ACP mode documents `--acp`](https://geminicli.com/docs/cli/acp-mode/) |
| Copilot CLI | 1.0.80; installed help exposes `--acp`. | [ACP server](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server) |
| Kiro | 2.21.1; `kiro-cli acp --help` exited 0. | [ACP command](https://kiro.dev/docs/cli/acp/) |
| Vibe | `mistral-vibe` 2.24.5 in `uv tool list`; `vibe-acp --help` exited 0 after explicit local-file permission. | [Installed entrypoints](https://docs.mistral.ai/vibe/code/cli/install-setup) |

Vibe's installed entrypoint initializes logging and runtime files before parsing
help arguments. Its initial sandbox denial was not a provider or ACP failure;
the authorized retry completed without an ACP session or provider prompt.
No login, installation or update was performed. These observations do not
assert that every installed version is the newest available.

## Capabilities

| Capability | Native ACP | Claude/Codex adapter | HTTP provider |
|---|---|---|---|
| Sessions | assumed at initialize | yes | n/a |
| Streaming | assumed at initialize | yes | yes |
| Cancellation | assumed at initialize | yes | yes |
| MCP injection | assumed at initialize | yes, via CLI config | no |
| Session resume | **negotiated**, then bounded by a proven production checkpoint and unseen-message delta (see below) | negotiated ACP resume with the same checkpoint safeguards; direct CLI uses its own resume route | n/a |
| Live permissions | **negotiated** — only if it advertises `permissionCapabilities` | no, computed once per session | n/a |
| Model list | **negotiated** — read from the `session/new` response | from the CLI's own catalogue | from the connection's slots |

Four capabilities are assumed rather than asked, because ACP defines no
negative answer for them: an agent that speaks the protocol at all has them.
Resume and permissions ARE answerable, so they are asked, and an explicit
`false` is honoured rather than read as absence.

## What Kronn hands an agent

| | Native ACP | Claude/Codex adapter | HTTP provider |
|---|---|---|---|
| Kronn's own MCP bridge | yes, in `mcpServers` | yes, in the CLI config | no |
| Project MCP servers | yes, minus any entry carrying a secret | Claude: all-or-nothing; Codex: safe entries only | no |
| Credentials over the wire | never — inherited from the spawned process | never — placeholders in the config | server-side only |
| File and terminal callbacks | refused with a JSON-RPC error | n/a | n/a |

OpenCode's native ACP session receives the same `kronn-internal` bridge as the
other MCP-enabled CLIs. Its discussion prompt therefore exposes Planning,
human-gated Automation proposals and the task-delegation lifecycle from the
first turn; history-tool discovery remains deferred until the third user
message. This instruction contract does not imply a live provider validation
or change session-resume and pricing limitations.
[src: file: backend/src/api/disc_prompts.rs:391]
[src: file: backend/src/agents/runner.rs:3764]

The orchestration database must also read back every native provider name it
writes. OpenCode launch, reload and idempotent replay are covered as one native
execution, with no fallback to Custom; unknown provider strings remain errors.
[src: file: backend/src/db/orchestration.rs:244]
[src: file: backend/src/db/orchestration_tests.rs:3029]

## Known asymmetries

These are real, deliberate, and the reason two agents can behave differently on
the same job.

- **ACP continuity is checkpointed, not a long-lived CLI process.** The
  production OpenCode and enabled adapter branches accept a proven resume ID
  together with its unseen-message delta and bounded full-prompt fallback.
  The completed checkpoint distinguishes the last input from the exact native
  response, so an interleaved peer is retained without repeating that response.
  Missing/incomplete proof or no size saving means a fresh full-prompt turn;
  ambiguous errors never authorize an automatic replay. The deterministic
  two-turn regression exercises the production start branch and atomic reply
  writer across a database reopen, with only the transport replaced. It does
  not qualify live model behavior or KT-577's long-lived Claude process.
  See [native ACP continuity](../gotchas/native-acp-resume-continuity.md) for the
  negotiated `session/resume` contract and exact proof boundaries.
  [src: file: backend/src/agents/runner.rs:3150-3224]
  [src: file: backend/src/api/discussions/streaming.rs:1401-1502]
  [src: file: backend/src/api/discussions/streaming.rs:5791-6085]
- **Kronn does not yet normalize ACP's optional session cost.** The current
  upstream v1 schema supports `usage_update.cost` as a cumulative amount with
  an explicit currency; this is not a per-turn USD price. Kronn's normalized
  ACP event currently retains token counts only, while its global spend report
  reads Claude, Codex and Gemini logs. Missing cost therefore remains unknown,
  never free. This is an implementation limit, not a protocol prohibition.
  [ACP v1 UsageUpdate, checked 2026-09-08](https://agentclientprotocol.com/protocol/v1/schema#usageupdate)
  [src: file: backend/src/acp.rs:352-365]
  [src: file: backend/src/acp.rs:790-813]
- **Statistics distinguish missing cost from a recorded zero.** Token-usage
  aggregates retain recorded amounts, fresh pricing estimates and unpriced
  tokens separately. A mixed total is partial; an existing recorded amount is
  not retroactively claimed to be a measured provider charge. This bounded
  correction does not add cumulative/multicurrency ACP accounting or OpenCode
  collection to the separate global spend report. See
  [cost provenance](../gotchas/stats-cost-usd-not-always-measured.md).
  [src: file: backend/src/models/stats.rs:19-95]
  [src: file: backend/src/api/stats.rs:46-56]
- **MCP servers holding a credential are dropped**, whole. A project mixing
  safe and credentialed entries loses the credentialed ones — silently from the
  agent's point of view, since it simply never sees them.
- **Task workers use the same default adapter route**, retaining the direct
  builder's restrictive worktree/settings/tool policy and a fresh session.
  An explicit false override selects direct compatibility for that agent.
  [src: file: backend/tests/adapter_worker_policy.rs:1]
  The direct Copilot worker preflight keeps a four-second deadline and awaits
  process collection on timeout. Its [timeout regression](../gotchas/copilot-preflight-timeout.md)
  uses controlled time and an owned child, not a startup PID-file race.
- **File and terminal requests are refused**, so an ACP agent reads and writes
  through its own tools, outside Kronn's audit trail.
- **Kronn spawns one process per turn on its local CLI/ACP routes.** HTTP
  provider calls do not spawn a CLI. Nothing here keeps a CLI warm between
  turns; that is 0.14 work (KT-577).

## ACP runtime diagnostics (KT-600)

After an ACP session has started, prompt and session-persistence failures are
recorded in the existing `AgentProcess` diagnostic capture. Before publishing,
Kronn masks vendor tokens and secret-like assignments, then limits the excerpt
by characters; cancellation remains unsuccessful rather than becoming a
successful run. The runner tests exercise this path with a fake ACP transport
for creation, resume and its existing fallback, streaming, usage, cancellation,
and prompt/persistence failures. They do not invoke an agent CLI, and ACP token
usage still has no implied price.
[src: file: backend/src/agents/runner.rs:3523-3758]
[src: file: backend/src/agents/runner.rs:10020-10133]
[src: file: backend/src/core/redact.rs:232-240]

## Settings catalogue correction (KT-531 / KT-597)

The agent-card tier editor now consumes the shared `/api/model-catalogs`
snapshot through `SearchableSelect`, including OpenCode, Kiro and Vibe. It
matches the exact `runtime_target_id`, never another connection's agent-family
projection. A catalogue refresh or manual edit reloads the tier options.
Unavailable models stay visible but disabled; an existing setting absent from
the snapshot is retained explicitly rather than erased or substituted. Option
details include provenance, last check, reasoning modes and known cost metadata;
a stale live result is labelled as cached. The snapshot GET itself remains a
read; opening a selector additionally refreshes stale CLI targets in the
background, preserving visible options and configuration. Claude's
initialization-only SDK catalogue is independent of the ACP execution toggle.
See [Claude discovery and freshness](../gotchas/claude-catalogue-discovery.md).
[src: file: frontend/src/components/settings/AgentsSection.tsx:149-168]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1215-1262]
[src: file: frontend/src/lib/modelCatalogSelection.ts:33-70]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:67-72]
[src: file: backend/src/core/model_catalog/codex_discovery.rs:75-158]

Saving a tier rereads the current settings and replaces only the selected
field, preserving other agents' values and showing the new value only after
the write succeeds. See [the preservation regression](../gotchas/agent-tier-catalogue-settings.md).
The shared picker prioritizes explicit identities, isolates named HTTP targets,
disables known unavailable models and retains a failed reload as cached with
an error. `modelForAgentTier` no longer invents embedded fallback model names.
The later [consumer and migration inventory](../gotchas/model-catalogue-consumer-inventory.md)
covers the remaining custom selector/display paths and removal of the legacy
HTTP runtime fallback. That source/contract inventory does not claim a live
provider run for every selector.
[src: file: frontend/src/lib/constants.ts:60-75]
[src: file: frontend/src/components/AgentSwitchPicker.tsx:113-142]

Quick Prompt and workflow model editors also use a shared catalogue-backed
searchable picker. Model IDs outside the snapshot remain an explicit operator
choice; known unavailable rows are disabled. Reasoning modes come from the
effective model's metadata instead of a fixed list, and existing unadvertised
values remain visible. QP saves preserve the existing token limit and chosen
reasoning mode. See [form preservation](../gotchas/agent-tier-catalogue-settings.md).
[src: file: frontend/src/components/ModelCatalogPicker.tsx:22-68]
[src: file: frontend/src/components/workflows/QuickPromptForm.tsx:187-202]

Explicit QP/workflow target changes now clear old model/reasoning overrides,
preserve the token limit and persist the exact selected connection. Named
connections are available in the creation/full/inline editors and pipeline;
changing only the connection is not treated as an unchanged agent/tier pair.
Ordinary edits and catalogue refreshes do not clear saved settings. The
discussion PATCH correction is backend-owned: it clears the persisted model
only on an actual target/tier change, distinguishes an absent connection from
explicit null, and preserves historical message models. Isolated API tests
cover this path, not provider inference or a new browser qualification.
[src: file: frontend/src/lib/agentSelection.ts:1-16]
[src: file: frontend/src/components/workflows/WorkflowDetail.tsx:1300-1330]
[src: file: frontend/src/pages/WorkflowsPage.tsx:1223-1244]
[src: file: backend/tests/discussion_target_model.rs:94-211]
