---
name: Kronn Doc Author
description: Concise authoring cheat-sheet for Kronn's `docs/AGENTS.md` convention v1 — the `<!-- kronn:section -->` markers and the `[src: …]` provenance grammar. Attach this skill when you'll edit or generate AGENTS.md content (audit output, bootstrap docs, manual edits) so the agent writes in the convention Kronn's lint will accept. The full spec lives at `/api/conventions/agents-md-format-v1` and is one `convention_get` MCP call away when a detail is missing.
icon: 📐
category: domain
auto_triggers:
  common:
    - "kronn:section"
    - "\\[src:"
    - "AGENTS\\.md"
  fr:
    - "\\b(édit|écri|rédig|génér|met[s]?\\s+à\\s+jour|complét).{0,40}(AGENTS\\.md|doc\\s+ia|section\\s+curated)"
  en:
    - "(edit|write|generate|update|fill).{0,40}(AGENTS\\.md|ai\\s+section|curated\\s+section)"
  es:
    - "(edit|escrib|gener|actuali|complet).{0,40}(AGENTS\\.md|sección\\s+ia|sección\\s+curated)"
---

# Kronn AGENTS.md convention — author's cheat sheet

Kronn ships a single open convention for agent-context docs (v1, embedded
in this Kronn install). Follow it whenever you write into `docs/AGENTS.md`
or any file that uses `<!-- kronn:section -->` markers. Kronn's lint
(`core::anti_halluc::analyze`) mechanically verifies what you cite at
message finalize — fabricated citations surface as a red pill on the
message.

## 1. Section markers

```
<!-- kronn:section name="stack" curated="ai" audit="2026-05-25" -->
- … your assertions, each with [src: …] …
<!-- kronn:section:end -->
```

- `name` — stable, unique within the file. Don't rename casually.
- `curated="ai"` — provenance is **required** on every non-trivial
  assertion (= every `[src: …]` must resolve). Lint runs here.
- `curated="human"` — owned by a person, never validated. Free-form.
  You may convert ai → human, never the reverse automatically.
- `audit="YYYY-MM-DD"` — required on `curated="ai"`. The date you (or
  the audit) last *verified the section against reality*. NOT git's
  last-edit date.

## 2. The `[src: …]` grammar

```
[src: <kind>[:<date>]: <ref>]
```
EXCEPT `user`, which carries an identity:
```
[src: user:<identifier>:<date>: <ref>]
```

## 3. The 9 kinds — pick the one that matches the evidence you actually have

| kind | trust | when to use |
|---|---|---|
| `file` | **HIGH** | A path under the project root. Optional `:line` or `:start-end`. **Mechanically verified by Kronn.** |
| `url` | HIGH if recent | External docs / spec. Not network-checked (SSRF-safe). |
| `user` | **HIGH** | A traceable human declaration (`user:<id>:<date>: <ref>`). |
| `commit` | HIGH if recent | A commit hash. `unchecked`. |
| `api` | HIGH if recent | A captured API response (`api_call_log#…`). |
| `code-comment` | **LOW** ⚠ | A comment isn't authoritative — a hint, never a fact. |
| `inferred` | **SOFT** | A derived guess, mark it honestly. |
| `hypothesis` | **VERY SOFT** | Unverified, confirm before acting on it. |
| `training-data` | **ZERO** ❌ | A model's prior knowledge — **forbidden**, this is the hallucination case Kronn refuses. |

## 4. Hard rules

1. **No assertion without `[src: …]`** in a `curated="ai"` section. Either
   you cite, or you mark `[src: inferred: …]` honestly. Don't omit.
2. **Prefer the highest available trust tier** that fits — `file` over
   `inferred` whenever the evidence is in the repo.
3. **`training-data` is never acceptable.** If your only source is what
   you "know", you don't know — verify or mark `hypothesis`.
4. **A `code-comment` is a hint, not a fact.** Cite it only to point at
   something to verify, never as the assertion itself.
5. **`audit="<date>"` is mandatory on `curated="ai"`.** Stamp the date
   you actually verified, not today's reflex.

## 5. Examples

```
- Backend: Rust, axum 0.8 [src: file: backend/Cargo.toml:79]
- API base path is `/api` [src: file: backend/src/lib.rs:440]
- Event-driven internals [src: inferred: backend/src/api/]
- Trunk-based release model [src: user:romuald:2026-05-25: disc-187]
- 1.5 Tbps capacity claim [src: url: https://www.cloudflare.com/network/]
```

## 6. When in doubt — fetch the full spec

The cheat sheet above is the 80% case. For edge cases (line-range
parsing, parser aliases EN/FR, the exact behaviour Kronn's lint applies
per kind), call the `convention_get` MCP tool:

```
convention_get(name="agents-md-format", version="v1")
```

It returns the canonical spec this Kronn installation actually
implements. Don't paraphrase from training-data — fetch.
