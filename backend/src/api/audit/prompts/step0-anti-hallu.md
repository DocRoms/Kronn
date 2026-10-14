This is the FIRST step of the audit. Your job is to ensure `docs/AGENTS.md` carries the canonical anti-hallucination protocol section that every subsequent step (and every future agent reading this file) will rely on.

## Algorithm

1. Read `docs/AGENTS.md`.
2. Search for the opening marker `<!-- kronn:section name="anti-hallu"`.
3. **If found** : refresh ONLY the `audit="<date>"` attribute on the opening marker (set it to today's date, ISO YYYY-MM-DD). Do NOT touch the body — `owner="audit"` means Kronn owns the content. Leave the closing `<!-- kronn:section:end -->` marker untouched.
4. **If NOT found** : insert the canonical block (exactly as below) immediately after the file's H1 (`# AI agent context — Entry point`) and BEFORE the next `---` separator. Preserve everything else.

## Canonical block to write (EXACT, no edits)

```markdown
<!-- kronn:section name="anti-hallu" curated="ai" owner="audit" audit="<TODAY YYYY-MM-DD>" -->
## 0. Anti-Hallucination Protocol

Never state a non-trivial technical fact (paths, APIs, config, versions, behaviour) without proof — read the code, then `docs/`, then official docs, then ask; never guess. Cite every assertion as `[src: file: <path>:<line>]` or `[src: url: <url>]`; a citation that doesn't resolve is rejected as fabricated.

Full grammar and cascade: [`docs/conventions/agents-md-format-v1.md`](conventions/agents-md-format-v1.md).
<!-- kronn:section:end -->
```

## DO NOT

- Do NOT touch any OTHER section of `docs/AGENTS.md` in this step (placeholders, tables, etc. are handled by subsequent steps).
- Do NOT add commentary, summary, or step-numbering. Output the file silently after the write.
- Do NOT escalate (no `<!-- TODO: ask user -->`). This step is purely mechanical.