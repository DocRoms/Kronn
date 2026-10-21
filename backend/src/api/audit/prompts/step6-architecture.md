Read docs/AGENTS.md and docs/repo-map.md for context. Analyze the high-level architecture.

Fill docs/architecture/overview.md — replace ALL {{PLACEHOLDERS}}:
- Apps/services: replace the {{SERVICES}} block with a table of service, port, tech, and role for each service
- Key patterns: replace the {{PATTERNS}} block with 3-5 architectural patterns (API pattern, state management, auth, data flow, caching, etc.) — 2-3 sentences each
- {{SEPARATION_DESCRIPTION}}: how the codebase is organized (by feature, by layer, etc.)
- {{DATA_FLOW_DESCRIPTION}}: 2-3 sentences on how data moves through the system
- Legacy: replace the {{MIGRATIONS}} block with a table of area, current state, and target for any legacy patterns or planned migrations

**Mermaid diagrams** — mandatory, REPLACES the old ASCII placeholder:
1. **Architecture overview** — replace `{{ARCHITECTURE_MERMAID}}` with a `flowchart TD` (or `LR` for wide projects) inside a ```mermaid fence. Show every service from the table above, the main data flow direction (HTTP, DB, message bus, etc.), and external systems (APIs, third-party providers). Use `subgraph` blocks to group related components. If the project warrants it (multi-tier app, hexagonal arch), simulate **C4-style layers** with named subgraphs (`Person`, `System`, `Container`, `Component`) — still in Mermaid syntax, no PlantUML/Structurizr.
2. **Sequence diagrams** — for the 2-3 MOST CRITICAL flows you can identify in the code (auth, primary request lifecycle, deploy/CI pipeline, payment, etc.), write ONE file per flow under `docs/architecture/sequences/<flow-name>.md` using `sequenceDiagram` Mermaid syntax. Hard cap: **3 files maximum**. Quality > quantity — if you only identify ONE clear critical flow, write only one. Each file must include a 2-3 sentence intro before the diagram ("This sequence describes how a user authenticates against the API. It starts when the client POSTs to /auth/login and ends with a JWT in the response cookie.").

**Mermaid sequenceDiagram safety rules** — Mermaid 11.x parser is unforgiving on message strings; respect these or the diagram won't render and the user sees a parse error instead of a flow:
- **Message text is ASCII-only**. Replace `…` with `...`, `→` with `->`, em-dash with `-`. Unicode punctuation routinely trips the parser.
- **Avoid `:` and `;` inside message text**. The first `:` after the arrow is the separator (`A->>B: msg`), but additional `:` / `;` combined with parens or Unicode can confuse the lexer. Rephrase: write `Cache-Control maxage=604800` instead of `Cache-Control: maxage=604800`, `Link rel=preload` instead of `Link: ...; rel=preload`.
- **No literal `(`/`)`/`[`/`]`/`{`/`}` chains** inside a message. Short, declarative prose only: `301 redirect to /a-propos` not `301 Location: /a-propos (set by LocaleRedirectSubscriber)`. If you need the detail, add a `Note over X` block.
- **Keep each line ≤ 100 chars**. Long lines hide parser-state issues. Break into multiple messages or a `Note over` block.
- **Participant/actor aliases must NOT be Mermaid reserved words** (case-insensitive): `alt`, `else`, `end`, `opt`, `loop`, `par`, `and`, `rect`, `note`, `critical`, `break`, `activate`, `deactivate`, `box`. Declaring `participant Alt as AlternateLocaleSubscriber` then writing `Caddy->>Alt: msg` makes the lexer read `Alt` as the start of an `alt` block → parse error. Pick a non-keyword alias (`AltLoc`, `AltSub`, `Alternate`). Same for `flowchart` node ids — never name a node `end`/`alt`/etc.
- Test mentally: would `mermaid.parse` accept this verbatim? If unsure, simplify.

**Why Mermaid + file separation**: every viewer (GitHub, GitLab, Obsidian, VS Code) renders Mermaid natively — no external tools. Sequence diagrams live in separate files so `docs/AGENTS.md` T0 stays small; an agent only loads them when working on the related flow.