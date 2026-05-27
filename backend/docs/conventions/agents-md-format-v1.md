<!-- kronn:doc-version="1.0" -->
# Kronn `AGENTS.md` convention — v1

**Status:** v1, shipped in Kronn 0.8.7. Open convention — anyone may emit or consume it, with or without Kronn.

The whole idea in one loop:

> An agent states a claim → cites a source → the source is **mechanically verifiable** → future agents inherit grounded information instead of inheriting a hallucination as "established truth".

This document specifies the machine-readable markers and the inline `[src: …]` citations that make that loop work. It is **asymmetrically beneficial**: with Kronn the citations are validated automatically; without Kronn it is still plain Markdown that any human or agent can read and check by hand.

> **Scope discipline.** This core spec describes the present convention. Advanced and
> future semantics — status lifecycles, claim invalidation, source-map aliases, runtime
> freshness automation — live in the provenance RFCs
> (https://github.com/DocRoms/Kronn/blob/main/docs/research/provenance-rfcs.md),
> deliberately kept OUT of here so the convention stays simple to adopt and explain in
> 30 seconds.

---

## 1. Section markers

Sections are delimited by **HTML comments** — invisible in every Markdown renderer
(GitHub, mkdocs, Obsidian, the Kronn viewer), so there is zero cost to human readability.

```markdown
<!-- kronn:doc-version="1.0" -->

<!-- kronn:section name="stack" curated="ai" audit="2026-05-25" -->
## Stack
- Rust 1.78 [src: file: Cargo.toml:5]
- Frontend uses pnpm strict mode [src: file: frontend/.npmrc:2]
<!-- kronn:section:end -->

<!-- kronn:section name="conventions" curated="human" -->
## Conventions
Free-form prose owned by humans. Kronn never validates this section.
<!-- kronn:section:end -->
```

### Attributes

| Attribute | Required | Meaning |
|---|---|---|
| `name` | yes | Stable, unique-within-file section id. Survives reordering/renames. |
| `curated` | yes | `"ai"` (provenance required) or `"human"` (free-form, never validated). |
| `audit` | for `curated="ai"` | ISO-8601 date the section was last **verified against reality**. Distinct from git's "last edited". |

The file SHOULD open with `<!-- kronn:doc-version="1.0" -->`. A file with **no**
`kronn:doc-version` marker is treated as legacy and is **not** validated — the convention
is strictly opt-in by presence.

### `curated="ai"` vs `curated="human"`

- **`curated="ai"`** — provenance is required: every non-trivial assertion needs a
  `[src: …]` citation.
- **`curated="human"`** — owned by a person, free-form, never validated.
- **Asymmetric conversion** — a human may convert a `curated="ai"` section to
  `curated="human"`. The reverse never happens automatically: human authority outranks AI,
  never the other way around.

### `audit="<date>"` is not git blame

Kept deliberately, *in addition* to git: git records *when a line was edited*; `audit=`
records *when the content was last verified to match reality*. A cosmetic edit does not bump
`audit`; a pure re-verification (no edit) does. Per-section, survives refactors/moves, works
offline (HTTP fetch, zip export, no `.git`).

---

## 2. Source citations (`[src: …]`)

Attach a citation to any non-trivial technical assertion (file paths, function/API/config
names, versions, behaviour). The grammar:

```
[src: <type>[:<date>]: <ref>]
```

`<type>` is optional but recommended; when omitted the ref is treated as a file path.
`<date>` is optional ISO-8601 (defaults to the section's `audit` date).

> **Grammatical exception — `user`.** Every type follows the 2-part shape
> `[src: <type>: <ref>]` EXCEPT `user`, which carries an identity segment:
> `[src: user:<identifier>:<date>: <ref>]`. A parser MUST special-case it. The
> `<identifier>` is a stable handle (pseudo preferred over raw email — privacy by default).

### Types & confidence gradient

| `<type>` | Trust | What the validator does |
|---|---|---|
| `file` | **HIGH** | Resolves the path under the project root, checks it exists, and (if `:line`/`:start-end`) that the line(s) are in bounds. |
| `url` | HIGH if recent | Not network-checked (`unchecked`) — SSRF-safe. |
| `user` | **HIGH** | A traceable human declaration, e.g. `[src: user:romuald:2026-05-25: disc-123]`. Not machine-verifiable → `unchecked`, but a legitimate trust tier. |
| `commit` | HIGH if recent | A commit hash. `unchecked`. |
| `api` | HIGH if recent | A captured API response (`api_call_log#…`). `unchecked`. |
| `code-comment` | **LOW** ⚠️ | A code comment is **not authoritative** — a hint to verify, never a fact. |
| `inferred` | **SOFT** | A derived guess, not a stated fact. |
| `hypothesis` | **VERY SOFT** | Unverified, confirm before acting. |
| `training-data` | **ZERO** | A model's prior knowledge. **Rejected** — this is exactly the hallucination case. |

### File ref forms

- `[src: file: backend/src/api/mod.rs:14]` — file + single line.
- `[src: file: backend/src/api/mod.rs:14-22]` — file + line range.
- `[src: backend/src/api/mod.rs]` — file only (type defaulted).

Mechanical verification is **path-jailed to the project root**: a citation that escapes the
root (`../../etc/passwd`) is rejected as `outside_project` without ever touching the
filesystem. This is the ungameable core — an agent cannot fabricate a file or line that
actually exists.

> **What verification proves — and what it does not.** A `verified` status means the cited
> source *exists and is in bounds*. It does **not** prove the source *supports the claim*.
> Never assume `verified` == `true` — that is the job of human review (and a future LLM-judge).

### Claim granularity — when to cite

Three obligation levels (RFC 2119). Unlabelled assertions default to MAY.

| Level | Rule | Examples |
|---|---|---|
| **MUST cite** | Precise/verifiable value, or decision-level assertion | "port is `3140`", "timeout default is 30s" |
| **SHOULD cite** | Specific technical assertion, lower immediate impact | "uses Redis cluster mode" |
| **MAY cite** | General context, common knowledge, broad pattern | "uses Redis", "Rust backend" |

**Counter-examples:** `"Uses Redis"` → MAY · `"Uses Redis cluster mode"` → SHOULD ·
`"Redis runs on port 6380"` → MUST · `"The timeout is configurable"` → MAY ·
`"The timeout default is 30s"` → MUST.

---

## 3. What Kronn enforces today (0.8.7)

This is the honest, present-tense answer to *"what does Kronn actually do now?"* — one global
mode (`Settings → Anti-hallucination`, or `config.toml anti_hallucination_mode`):

| Mode | Behaviour |
|---|---|
| `off` | Nothing. |
| `warn` (default) | **P1** sourcing directive injected into every agent prompt + **P2** post-output lint, surfaced as a non-blocking pill. |
| `enforce` | Same as `warn` today; reserved for future write-time refusal. |

**P2 lint, concretely** — what is mechanically checked on each agent reply:
1. A prose heuristic flags confident technical claims left **unsourced** (one confidence tier).
2. Every `[src: …]` marker present is **mechanically verified**: file exists, path is jailed to
   the project root, line/range is in bounds. A citation pointing at a non-existent or
   out-of-project path is flagged **fabricated**; a `training-data` source is **rejected**.

**Convention vs enforcement.** The markers and citation grammar above are a *writing
convention* — real and usable today, with or without Kronn. What 0.8.7 mechanically gates is
exactly the two P2 steps. Section-level validation, MUST/SHOULD/MAY level-aware linting, and
write-time refusal are part of the convention's design but are **not** machine-enforced yet —
see the RFC for the roadmap. (No scattered "coming soon" notes elsewhere: this section is the
single source of truth for what's live.)

---

## 4. Vendor-neutral parsing

The marker prefix is `kronn:` in v1 (clear ownership, zero collision). Consumers SHOULD also
accept the neutral aliases `doc:`, `agents:`, `aidoc:` for the same markers, so a non-Kronn
tool can emit a Kronn-compatible document without using the brand.

---

## 5. Self-documenting + portable

- A Kronn-managed `AGENTS.md` SHOULD open with a `kronn:spec` pointer (URL + local path) so
  any agent learns the convention from the file itself.
- Kronn bootstraps copy this spec into the project at `docs/conventions/agents-md-format-v1.md`
  so it travels with the repo (offline, post-uninstall, post-move) and is git-versionable.

---

## 6. Annotated example

```markdown
<!-- kronn:doc-version="1.0" -->

<!-- kronn:section name="stack" curated="ai" audit="2026-05-25" -->
## Stack
- Backend: Rust, axum 0.8 [src: file: backend/Cargo.toml:79]
- API base path is `/api` [src: file: backend/src/lib.rs:440]
- Event-driven internals [src: inferred: backend/src/api/]
- Trunk-based release model [src: user:romuald:2026-05-25: disc-187]
<!-- kronn:section:end -->
```

---

## 7. Versioning

This is **v1.0**. Backwards-incompatible changes bump the major (`kronn:doc-version`). Kronn
never force-migrates a project: a v1 document keeps working under a later Kronn that also
understands v2.

Advanced semantics and proposed extensions (status lifecycle, `claim_id` references, source-map
aliases, runtime freshness, claim propagation) are tracked in the provenance RFCs
(https://github.com/DocRoms/Kronn/blob/main/docs/research/provenance-rfcs.md) — intentionally
separate so this convention stays small, stable, and easy to adopt.
