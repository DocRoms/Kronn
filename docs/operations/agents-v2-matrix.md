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

## Capabilities

| Capability | Native ACP | Claude/Codex adapter | HTTP provider |
|---|---|---|---|
| Sessions | assumed at initialize | yes | n/a |
| Streaming | assumed at initialize | yes | yes |
| Cancellation | assumed at initialize | yes | yes |
| MCP injection | assumed at initialize | yes, via CLI config | no |
| Session resume | **negotiated** — only if the agent answers `loadSession: true` | yes, via `--resume` | n/a |
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

## Known asymmetries

These are real, deliberate, and the reason two agents can behave differently on
the same job.

- **An ACP agent reports tokens but no spend.** The protocol carries no price,
  the catalogue records a qualitative hint rather than a rate, and the spend
  report reads Claude, Codex and Gemini logs only. Absence there means unknown,
  never free.
- **MCP servers holding a credential are dropped**, whole. A project mixing
  safe and credentialed entries loses the credentialed ones — silently from the
  agent's point of view, since it simply never sees them.
- **Task workers never take the adapter route**, whatever the toggle says.
- **File and terminal requests are refused**, so an ACP agent reads and writes
  through its own tools, outside Kronn's audit trail.
- **Kronn spawns one process per turn** on every route. Nothing here keeps a
  CLI warm between turns; that is 0.14 work (KT-577).

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
