# MCP surface baseline (KT-192)

Measured 2026-08-04 on the working tree, before any reduction. Same rule as
KT-188: bytes are exact, token figures are estimates at 3.7 B/token and labelled
as such. Reproduce with the snippets below rather than trusting the numbers.

## The headline

After KT-191 took the documentation bootstrap from 89 760 B to 19 009 B, the
MCP surface is **five times** what an agent reads in documents:

| Injected at | Bytes | ~tokens | Frequency |
|---|---|---|---|
| MCP tool catalogue (`tools/list`) | **96 972** | ~26 200 | once per session |
| Documentation bootstrap (post-KT-191) | 19 009 | ~5 137 | once per session |
| `next_steps` protocol block | 4 346 | ~1 174 | per `disc_join` |
| wait hint | 802 | ~216 | **per `disc_wait_for_peer`** |

The catalogue alone is larger than the whole 84 KiB AGENTS.md problem that
KT-191 solved. Whatever else 0.9.4 does, this is where the tokens are.

## Tool catalogue — 84 tools, 96 972 B

Descriptions are 54 804 B (56%), input schemas 36 989 B (38%).

Heaviest declarations:

| Tool | description | schema | total |
|---|---|---|---|
| `workflow_create_draft` | 4 856 | 2 581 | **7 437** |
| `disc_append` | 2 895 | 3 437 | **6 332** |
| `qa_create_draft` | 3 159 | 2 805 | **5 964** |
| `api_call` | 2 886 | 2 062 | **4 948** |
| `disc_wait_for_peer` | 2 686 | 782 | **3 468** |

The top five are 28 149 B — 29% of the catalogue for 6% of the tools. Their
descriptions read as manuals, which is the same failure AGENTS.md had: reference
material living in a slot that is paid for unconditionally.

```
python3 - <<'PY'
import ast, json, pathlib
tree = ast.parse(pathlib.Path('backend/scripts/disc-introspection-mcp.py').read_text())
tools = next(ast.literal_eval(n.value) for n in ast.walk(tree)
             if isinstance(n, ast.Assign)
             and any(getattr(t, 'id', None) == 'TOOLS' for t in n.targets))
print(len(tools), len(json.dumps({"tools": tools}, ensure_ascii=False).encode()))
PY
```

## Repeated per-call text

**The wait hint is emitted twice per response** — once at the top level and once
inside `waited` — with identical content. 401 B each, so 802 B per wait, for text
that does not change between calls. Observed directly in this session's tool
results, not inferred.

| Waits in a session | Identical text shipped |
|---|---|
| 10 | 8 020 B (~2 167 tokens) |
| 30 | 24 060 B (~6 502 tokens) |
| 60 | 48 120 B (~13 005 tokens) |

A long multi-agent session pays more for repeated protocol reminders than for the
entire documentation bootstrap. The duplication is free to remove; the repetition
itself is a design question — after the first delivery the agent has the rule, and
resending it every time treats each call as if the agent had amnesia.

## Directions, not decisions

Recorded so the next pass argues from measurements rather than instinct. None of
these is chosen yet.

1. **Deduplicate the wait hint.** Two identical copies per response is a bug, not
   a trade-off. Smallest possible change, immediate effect on long sessions.
2. **Move tool manuals out of descriptions.** The pattern KT-191 validated: keep a
   tight description, point to a document loaded on demand. Applies to the five
   heavy tools first.
3. **Tier the catalogue.** Expose a core set and let the rest be fetched on
   request. Precedent exists — this very session runs with most tools deferred
   behind a search, so the mechanism is proven outside Kronn. Biggest win,
   biggest behavioural risk: a tool an agent cannot see is a tool it will not use,
   so it needs measuring against real tasks, not assumed.
4. **Trim schema prose.** 36 989 B of schemas carry per-field descriptions that
   partly restate the tool description.

## What this baseline does not claim

- No reduction has been made. These are starting figures.
- Token counts are estimates. A model-specific tokenizer would shift them; the
  ratios between lines would not.
- The catalogue cost is per *session*, the hint cost per *call*. Comparing them
  needs a session profile, which KT-198's benchmark owns — not this file.
