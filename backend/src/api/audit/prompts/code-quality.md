You are running a FOCUSED CODE-QUALITY AUDIT. Output ONLY code quality / maintainability findings — ignore security, docs drift, infra config (other audits own those).

If `docs/briefing.md` exists, read it FIRST: the user's stated quality concerns take priority over the generic checklist below.

# A. MANDATORY QUALITY CHECKLIST (adapt to the project's actual stack)

Walk through every applicable item. If satisfied, write "verified clean". If NOT, emit a TD finding. Sample REAL files — never assert from file names alone.

**Templates / markup (Twig, Blade, JSX, ERB, plain HTML…):**
- Duplication: same block/markup repeated across ≥3 templates instead of a partial/macro/component (diff 3 similar templates).
- Dead templates: files never referenced by any route/controller/include (grep template names against source).
- Inline `style=` / inline `<script>` blocks in templates (grep, count, sample) — belongs in the asset pipeline.
- Oversized templates (> ~300 lines): candidates for decomposition; name the worst 3.
- Data/logic in templates: heavy computation, raw SQL-ish calls or business rules inside template files.

**Styles (CSS/SCSS):**
- Architecture: is there a discernible convention (BEM, utility, tokens)? Mixed conventions in the same codebase = finding.
- Duplication: same rule blocks repeated (sample the 2 largest files); hardcoded colors/sizes repeated ≥5 times instead of variables.
- Dead selectors: classes defined but absent from every template (sample 20 selectors).
- Single global stylesheet > ~1500 lines with no imports/layers = finding.

**Backend code (PHP, TS/JS, Rust, Python… — whatever the project uses):**
- God files: source files > ~500 lines mixing unrelated responsibilities; name the worst 3 with line counts.
- Dead code: unreferenced functions/classes/exports (sample; use grep on symbol names).
- Swallowed errors: empty catch blocks, `catch (e) {}`, `@`-suppression (PHP), `.unwrap()` chains on fallible paths in handlers.
- Copy-paste logic: same function body duplicated across files (sample the controllers/services layer).
- Typing hygiene where the language offers it: untyped public signatures, `any`/`mixed` density.
- TODO/FIXME/HACK density: count them; > 20 in source (not docs) = finding listing the 5 oldest.

**Perf & eco hygiene (code-level only — infra belongs to other audits):**
- Assets: images > 300 KB served without modern format (webp/avif); JS/CSS bundles not minified in the build.
- Requests in loops: DB/HTTP calls inside iteration (N+1 shape) — sample controllers/services.
- Caching affordances in code: repeated identical computations/fetches with no memoization where an obvious hot path exists.

# B. ANTI-REPETITION (priors)

Before writing findings: read existing `docs/tech-debt/TD-*.md`. If a finding matches an existing one, REUSE its ID (append an `audit_history` entry — never overwrite, never re-slug).

# C. OUTPUT FORMAT

1. Write `docs/inconsistencies-code-quality.md` with the standard table header:
`| ID | Problem | Area | Severity |`
2. For each finding, also write `docs/tech-debt/<ID>.md` using the existing TD detail template — add a `**Category**: code-quality` line.
3. Status taxonomy: `Verified in source` (you read the file) vs `Inferred` (pattern only).
4. Cap: 20 findings. High severity exempt from the cap.
5. Each finding: file path + line, one-sentence maintainability impact, one-sentence remediation.