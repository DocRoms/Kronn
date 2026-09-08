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
| Claude Code | direct CLI | `claude --print …` | `KRONN_ACP_ADAPTER_CLAUDE=1` for the adapter |
| Codex | direct CLI | `codex exec …` | `KRONN_ACP_ADAPTER_CODEX=1` for the adapter |
| Ollama, LiteLLM, NVIDIA, Custom | HTTP provider | no process | no |

Claude and Codex have no ACP mode of their own. Their adapter is Kronn wrapping
the CLI in the ACP contract — one process per turn either way. Turning the
toggle on changes the plumbing, not the process model.

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
| Session resume | **host capability negotiated**; production currently starts fresh (see below) | yes, via `--resume` | n/a |
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

- **Native ACP does not currently resume production discussion turns.** The
  shared host implements negotiated loading, but the production `NativeAcp`
  branch passes both `resume_id: None` and `session_store: None`. The OpenCode
  runtime key recognized by `AcpSessionStore` does not make that branch persist
  or reload a conversation. A host/fake-transport resume test is therefore not
  evidence of OpenCode production continuity. Enabling it also needs the unseen
  message delta and full-history fallback; merely passing the old ID would
  repeat history into a resumed conversation. This remains an open KT-543
  qualification boundary, distinct from KT-577's long-lived Claude process.
  [src: file: backend/src/agents/runner.rs:3121-3156]
  [src: file: backend/src/agents/runner.rs:2068-2086]
- **Kronn does not yet normalize ACP's optional session cost.** The current
  upstream v1 schema supports `usage_update.cost` as a cumulative amount with
  an explicit currency; this is not a per-turn USD price. Kronn's normalized
  ACP event currently retains token counts only, while its global spend report
  reads Claude, Codex and Gemini logs. Missing cost therefore remains unknown,
  never free. This is an implementation limit, not a protocol prohibition.
  [ACP v1 UsageUpdate, checked 2026-09-08](https://agentclientprotocol.com/protocol/v1/schema#usageupdate)
  [src: file: backend/src/acp.rs:341-356]
  [src: file: backend/src/acp.rs:794-806]
- **MCP servers holding a credential are dropped**, whole. A project mixing
  safe and credentialed entries loses the credentialed ones — silently from the
  agent's point of view, since it simply never sees them.
- **Task workers never take the adapter route**, whatever the toggle says.
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

## Settings catalogue diagnostic (KT-597)

The dynamic catalogue and the model-tier editor on an agent card are currently
different consumers. `ModelCatalogSection` reads `/api/model-catalogs`, and
Codex discovery calls its own `codex app-server` / `model/list`. However, the
Codex card in `AgentsSection` still supplies `SearchableSelect` from the static
`AGENT_TIER_MODELS.codex.options` array. A newly available model absent from that
array will not appear there even after a catalogue refresh. Updating OpenCode
or an OpenRouter connection cannot change this array. This is a remaining UI
integration gap, not evidence that the Codex account lacks access to the model.
[src: file: frontend/src/components/settings/AgentsSection.tsx:106-123]
[src: file: frontend/src/components/settings/AgentsSection.tsx:1233-1253]
[src: file: frontend/src/components/settings/ModelCatalogSection.tsx:67]
[src: file: backend/src/core/model_catalog/codex_discovery.rs:75-158]
