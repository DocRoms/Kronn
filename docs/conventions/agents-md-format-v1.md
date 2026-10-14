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

<!-- kronn:section name="stack" curated="ai" owner="audit" audit="2026-05-25" -->
## Stack
- Rust 1.78 [src: file: Cargo.toml:5]
- Frontend uses pnpm strict mode [src: file: frontend/.npmrc:2]
<!-- kronn:section:end -->

<!-- kronn:section name="conventions" curated="human" owner="human" -->
## Conventions
Free-form prose owned by humans. Kronn never validates this section.
<!-- kronn:section:end -->
```

### Attributes

| Attribute | Required | Meaning |
|---|---|---|
| `name` | yes | Stable, unique-within-file section id. Survives reordering/renames. |
| `curated` | yes | `"ai"` (provenance required) or `"human"` (free-form, never validated). |
| `owner` | template v2 | `"audit"` (the audit may refresh it) or `"human"` (a person wrote or validated it: Kronn may still correct it, and then asks for a reconfirmation). If absent on a legacy marker, `curated="human"` implies human ownership; otherwise audit ownership is assumed. |
| `audit` | for `curated="ai"` | ISO-8601 date the section was last **verified against reality**. Distinct from git's "last edited". |

The file SHOULD open with `<!-- kronn:doc-version="1.0" -->`. A file with **no**
`kronn:doc-version` marker is treated as legacy and is **not** validated — the convention
is strictly opt-in by presence.

### `curated="ai"` vs `curated="human"`

- **`curated="ai"`** — provenance is required: every non-trivial assertion needs a
  `[src: …]` citation.
- **`curated="human"`** — owned by a person, free-form, never validated.
- **`owner="human"`** — written or validated by a person. No Kronn write is
  refused or reverted because of it: an audit, a revalidation, a template
  reinstall or a documentary repair applies its change, and the section is
  then marked "to reconfirm" with the diff until its owner confirms it. The
  legacy `curated="human"` spelling is read the same way. STEP 0 never restamps
  a human opener, and a `kronn:section` marker inside a fenced code block
  (``` or `~~~`) is an example, never a boundary.
- **Human-validation provenance** — a validation (`validate-audit`, a TD
  validation card, « Vérifié, inchangé », « Confirmer ») records the digest of
  what the person approved in `docs/provenance.json`, next to `citations.json`.
  No marker is added to the prose. A doc whose validated text changed reads as
  AI-generated, for the learning gate too, until it is reconfirmed.
- **Text added between runs** — audits snapshot every Markdown document in the
  resolved documentation directory, excluding its `legacy/` and `var/`
  subdirectories. Symbolic links are refused. A change since the last Kronn write or
  validation becomes the next diff's baseline, including unmarked human text.
  Older unreviewed baselines remain available, and whole-audit validation does
  not clear pending reconfirmations. Generated files are tracked separately
  until a human changes or validates them.
  `backend/src/core/doc_provenance.rs` (`record_all`)
  `backend/src/core/doc_provenance.rs` (`snapshot_tracked`)
  `backend/src/core/doc_provenance.rs` (`note_snapshot_writes`)
- **Briefing protection** — each audit step snapshots the briefing and restores
  a rewrite, deletion or unauthorized creation with a visible warning. Review
  completion requires a nonempty file and a successful save; a failure remains
  visible in the open discussion. Reviews restore saved project facts, rules,
  documentation choices and Kronn options before storing the result.
  `backend/src/api/audit/helpers.rs` (`restore_step_briefing`)
  `backend/src/api/audit/briefing.rs` (`complete_reviewed_briefing`)
  `backend/src/api/audit/project_doc.rs` (`reattach_facts`)
- **Keep accurate prose** — finding and review steps also reverify existing
  documentation and must keep true statements as written.
  `backend/src/api/audit/mod.rs` (`REVERIFY_BLOCK`)
  `backend/src/api/audit/mod.rs` (`reverifies_target`)
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
| `deps` | HIGH, dated | A dependency-watch row: `deps: <manager>/<package>@<YYYY-MM-DD>`. An audit checks it against the snapshot it injected (absent = fabricated); elsewhere `unchecked`. |
| `code-comment` | **LOW** ⚠️ | A code comment is **not authoritative** — a hint to verify, never a fact. |
| `inferred` | **SOFT** | A derived guess, not a stated fact. |
| `hypothesis` | **VERY SOFT** | Unverified, confirm before acting. |
| `training-data` | **ZERO** | A model's prior knowledge. **Rejected** — this is exactly the hallucination case. |

### File ref forms

- `[src: file: backend/src/api/mod.rs:14]` — file + single line.
- `[src: file: backend/src/api/mod.rs:14-22]` — file + line range.
- `[src: <path>:<line>]` — the `file:` type is the default.
- `[src: file: backend/src/api/mod.rs:14-22#context_audit]` — line or range + **symbol anchor**.
  The anchor (an identifier; `.` and `-` allowed inside, e.g. `Type.method`, `check-version`)
  must appear as a whole word inside the cited lines. It is only read after a line spec. Use it
  to say *what* the lines hold: when code moves, a stale range fails instead of silently
  pointing beside the claim.
- `[src: file: <path>.docx]` — file only. Required for a binary file (.docx,
  .pdf, images), which has no lines; detected by content, not extension. In audited `docs/`
  a text file always takes a line, and a binary file never does.
- Percent-encoded paths (`My%20File.docx`) are decoded before resolving.

Mechanical verification is **path-jailed to the project root**: a citation that escapes the
root (`../../etc/passwd`, an absolute path such as `/etc/passwd`, or a symlink leading out) is
rejected as `outside_project` without ever touching a file outside the root. An absolute path
under the root is verified like a relative one. This is the ungameable core — an agent cannot
fabricate a file or line that actually exists.

An anchor absent from the cited lines makes the citation `out_of_bounds` (the range is wrong
for what it names); the detail says where the symbol does appear, if anywhere. The check is
textual and deterministic: no code is parsed.

**One anchor rule, no tolerance window.** The runtime verifier, the `verified` badge it feeds
and Kronn's docs check apply this same rule: the symbol is inside the cited lines, or the
citation fails; one line past the range fails everywhere. A tool that re-anchors a moved
citation first rewrites the range when the symbol occurs once in the cited file. If the symbol
occurs several times, the tool may instead map the old range's endpoints through a zero-context
Git diff from the chosen base to `HEAD`; a line inside a changed hunk has no mapping, and the
rewrite is allowed only when the mapped range still contains the symbol as a whole word. The
path and anchor stay unchanged, and a single-line citation stays single-line while a range stays
a range. Anything still ambiguous is listed with its candidate lines for a human to choose; a
missing file or line is never re-anchored.

A marker quoted in inline backticks is verified too: it is `verified` when it resolves, but a
failure there counts as *unverified*, not fabricated, because backticks also quote the grammar
as an example. Markers inside fenced code blocks are not read.

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
   the project root, line/range is in bounds, a `#symbol` anchor appears in the range. A citation pointing at a non-existent or
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

<!-- kronn:section name="stack" curated="ai" owner="audit" audit="2026-05-25" -->
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
