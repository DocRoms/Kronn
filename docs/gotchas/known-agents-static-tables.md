# A static agent table makes a new agent kind invisible, not neutral

- **Area**: Frontend (`lib/constants.ts`) — with symptoms reaching the backend
- **Seen**: 0.13.0, seven separate bug reports, one cause

## The shape of it

Several unrelated frontend surfaces answer questions about agents from
hand-written tables: `KNOWN_AGENTS`, `AGENT_LABELS`, `AGENT_COLORS`,
`AGENT_MENTIONS`. Each is a literal map from `AgentType` to something.

`AgentType::Custom` — the wire type every external API connection carries — was
in none of them.

The failure mode is what makes this worth a page: **absence does not read as
"unknown", it reads as "not an agent"**, and each surface then draws its own
wrong conclusion from that:

| Table consulted | Question it answers | Answer for an absent kind |
|---|---|---|
| `KNOWN_AGENTS` | is this a real agent? | no → muted in the room, stream declared lost |
| `AGENT_MENTIONS` | what can be `@`-mentioned? | nothing → never in the composer list |
| `AGENT_LABELS` | what is it called? | fallback → generic label instead of the alias |
| `AGENT_COLORS` | what colour is it? | fallback → no identity of its own |

Seven reported symptoms. No two looked alike; the composer bug and the "lost
connection" toast were filed as unrelated. Chasing them one at a time would have
produced seven local patches and left the eighth surface waiting.

## What to do instead

- **When a symptom is "this agent behaves as if it were disabled", check the
  static tables before reading any logic.** Absence is the cheapest hypothesis and
  the fastest to confirm.
- **Adding an `AgentType` variant means auditing every table keyed by it.** The
  compiler does not help: these are literal maps, not exhaustive `match`es. A map
  with a fallback compiles perfectly while being wrong.
- **Prefer a total mapping over a literal map** where the shape allows it —
  `externalAgentColor(alias)` derives a colour from the alias instead of looking one
  up, so a connection that did not exist when the table was written still gets a
  distinct, stable colour.

## The diagnostic lesson, which cost more than the fix

Three hypotheses were formed and pursued before the database was queried —
launch mode, the supersede rule, the disabled flag. Each was consistent with the
symptom and each was wrong. One query against `agent_dispatch_jobs` returned the
cause immediately.

The pattern to recognise: **concluding from a weak signal while a strong one sits
unread.** The weak signal is a plausible mechanism; the strong one is the row the
system already wrote down. Query first.

## Related

- [`operations/external-api-connections.md`](../operations/external-api-connections.md)
  — the operator-facing account of the same seven symptoms.
