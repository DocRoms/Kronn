# TD-20260627-configurable-docs-dir

- **ID**: TD-20260627-configurable-docs-dir
- **Area**: Backend / Config / Frontend
- **Problem (fact)**: The project documentation root is **hardcoded-detected** as
  `docs/` → `doc/` → `ai/` (legacy) in `scanner::detect_docs_dir` and friends.
  There is no way to configure it. Projects that keep their docs elsewhere
  (`documentation/`, `wiki/`, `.kronn/`, …) aren't recognised, and the
  `ai/` → `docs/` pivot (commit `2ff8a17`) left **each layer carrying its own copy
  of the path** (detection, writers, injection, frontend) rather than a single
  source of truth.
- **Why we can't fix now (constraint)**: It's genuinely cross-cutting — the path
  is assumed in detection, in the audit/bootstrap **writers** that create the
  tree, in the `AGENTS.md` wiring + anti-hallucination / continual-learning STEP
  machinery, in the MCP injection, and in the **frontend** viewer (`AiDocViewer`
  looks up `docs/AGENTS.md` literally). A correct fix needs one configurable
  value threaded through all of them plus a back-compat story — too broad to
  bundle with unrelated work.
- **Impact**: dev friction · correctness (wrong/empty docs detection) · flexibility
- **Where (pointers)**:
  - `backend/src/core/scanner.rs` — `detect_docs_dir`, `detect_audit_status`,
    `detect_docs_entry`; the literal `docs/`/`doc/`/`ai/` checks.
  - `backend/src/api/ai_docs.rs` — `list_ai_files`, `is_under_docs_root`
    (hardcoded `docs/`/`doc/`/`ai/` prefixes + traversal guard).
  - Audit/bootstrap **writers** (the template + audit steps that generate the
    tiered, AI-optimised docs tree).
  - `AGENTS.md` wiring + `core::learning_doc` / anti-hallu STEP injection (where
    the `<!-- kronn:section -->` pointers + seeded files are written).
  - `frontend/src/components/AiDocViewer.tsx` — literal `docs/AGENTS.md` /
    `doc/AGENTS.md` / `ai/index.md` entry lookups; `ProjectCard.tsx`.
- **Suggested direction (non-binding)**:
  - Add **`docs_dir` to config** — global default `"docs"` — with an **optional
    per-project override** (`Project.docs_dir: Option<String>`).
  - Make it the **single source of truth**: `detect_docs_dir` reads the
    configured dir first; **writers + injection + AGENTS wiring** all read the
    same value so generated files land in (and reference) the right folder —
    exactly what lets Kronn "adapt AGENTS.md and all Kronn files to the chosen
    folder".
  - Surface the **resolved docs dir in the API** (e.g. on the project payload)
    so the frontend stops hardcoding `docs/AGENTS.md`.
  - Keep `ai/` / `doc/` as **legacy read fallbacks**; no forced migration of
    existing trees.
- **Decision (2026-08-29)**: keep `docs/` as the default, expose the resolved
  documentation root in the project Docs tab, and allow a safe per-project
  override. The configured value must be a repository-relative directory
  (no absolute path or `..`) and must drive every reader and writer: audits,
  templates, validation, drift/checksums, cancellation cleanup, agent wiring,
  MCP injection, and the frontend viewer. When no override exists, retain the
  current `docs/` → `doc/` → `ai/` detection for backwards compatibility.
- **Human documentation follow-up**: improve Kronn's built-in reader first
  (human-oriented `index.md`, navigation, page outline, search, previous/next)
  without requiring a documentation framework in every managed repository.
  Existing Docusaurus, VitePress, or MkDocs configurations may later be
  detected and linked or previewed explicitly; audits must not install, run,
  or overwrite those toolchains automatically.
- **Delivery split**:
  1. Configurable documentation root and one backend source of truth.
  2. Human reading mode backed by the resolved root.
  3. Optional adapters for existing documentation-site generators.
- **Next step**: create or update the tracker issue from this agreed scope when
  the feature is scheduled. No implementation is planned in the current UI
  iteration beyond improving the existing viewer's layout.

## Notes

- Surfaced 2026-06-27 alongside a related fix: `AiDocViewer` used to render the
  identical "no project documentation" message for both a genuinely-empty tree
  AND a failed `list_ai_files` fetch (the `.catch` swallowed the error). That
  silent-catch is now fixed (distinct error + retry state) — but it highlighted
  how much logic keys off the hardcoded docs path, motivating this TD.
