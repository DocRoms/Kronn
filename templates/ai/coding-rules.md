# Coding rules (AI contract)

> **TEMPLATE FILE.** Sections marked `{{...}}` must be filled by the AI audit.
> If you see unfilled `{{...}}`, say `NOT_FOUND` — **never invent tool names, conventions, or commands**.

> Glossary: `ai/glossary.md`.

## Global

- Prefer smallest diffs. Avoid drive-by refactors.
- Follow existing naming in adjacent code. Avoid generic names (`Helper`, `Utils`).
- **Every change must include tests** — see `ai/testing-quality.md`.

## {{LANGUAGE_1}} [ex: "Rust", "TypeScript", "Python"]

### Tools

<!-- Fill: linter, formatter, type checker with config file and run command -->
{{TOOLS_1}}

### Conventions

<!-- Fill: 3-5 project-specific conventions (naming, patterns, imports, etc.) -->
{{CONVENTIONS_1}}

### Common mistakes to avoid

<!-- Fill: 2-3 project-specific pitfalls that cause bugs or build failures -->
{{MISTAKES_1}}

## {{LANGUAGE_2}} [ex: "TypeScript", "Go", "Shell"]

### Tools

<!-- Fill: linter, formatter, type checker -->
{{TOOLS_2}}

### Conventions

<!-- Fill: 3-5 project-specific conventions -->
{{CONVENTIONS_2}}

### Common mistakes to avoid

<!-- Fill: 2-3 project-specific pitfalls -->
{{MISTAKES_2}}
