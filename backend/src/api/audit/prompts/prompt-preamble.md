Rules: Write new content in the documentation language declared by `docs/AGENTS.md` § Project parameters. While Step 1 is still filling that value, preserve the language of existing docs and use English only for a fresh skeleton with no established language. Be factual and concise — this is AI context for coding agents, NOT human documentation.
- Do NOT invent — read `docs/AGENTS.md` § 0 Anti-Hallucination Protocol (STEP 0 of this audit just wrote/refreshed it). The cascade is `read code → existing docs/ → external doc → escalate via TODO marker → never assert without proof`. Every non-trivial fact written into `docs/` MUST carry `[src: file: <path>:<line>]` or `[src: url: …]` ; a binary file (.docx, .pdf, image) has no lines and is cited by bare path `[src: file: <path>]`, a form refused for a text file ; fabricated citations (path doesn't exist / outside project root) are mechanically checked — `enforce` mode rejects them with a retry, the default `warn` logs them; treat any warning as a defect.
- Replace ALL `{{PLACEHOLDERS}}` and `<!-- ... -->` comment placeholders with real content. {{PLACEHOLDERS}} are literal text markers — replace by editing file content directly.
- Keep the existing file structure and section headings — fill in the blanks, do NOT rewrite the file from scratch.
- If a section does not apply to this project, replace placeholders with 'N/A — not used in this project.' Do not delete the section.
- A section marked `owner="human"` (legacy `curated="human"`) was written or validated by a person. In a full OR partial audit, change it only to correct what the code contradicts and keep its wording otherwise; never reorder or delete it. Kronn keeps your change and asks its owner to reconfirm it, with the diff.
- Never force-translate an existing `docs/` file into another language — a doc already written in a different language stays as-is; the docs/tickets language parameter only governs NEW content.
- Plain facts in docs/ files — no opinions, debate or trade-off analysis. One exception: `Suggested direction` in TD detail files (and index remediation lines) may carry a one-line, non-binding fix hint.
- Each section should be self-contained: another AI agent reading just that section should get the full picture.
- Add or remove table rows as needed to match the project. Write fewer entries rather than inventing content to fill slots.

## MARKER DISCIPLINE (critical — avoid marker overuse)

Three marker types exist. Each has a STRICT semantic — using the wrong one creates noise the user has to triage later.

1. **`<!-- TODO: verify -->`** — RESERVED for facts you literally could not check.
Examples: file lives outside the repo (linked_repos), tool requires credentials not provided, sandbox blocks the read.
**DO NOT use** when you DID verify (via Glob/Read/ls) — write the conclusion directly:
- WRONG: `phpstan.neon.dist <!-- TODO: verify — file not present at project root -->`
- RIGHT: `phpstan.neon.dist (not present at project root)`
If you Globed for a config and found nothing, that IS the verified answer — don't add a TODO marker.

2. **`<!-- TODO: ask user -->`** — for facts that require a HUMAN DECISION, not a verification you could do yourself.
Examples: "is this rule aspirational or enforced?", "which domain is canonical?", "is this port intentional or vestigial?".
The Phase 2 validation discussion will ask the user these specific questions.

3. **`<!-- TODO: unknown -->`** — placeholder when a previous validation pass left a question unanswered. Rare. Don't introduce new ones; only preserve existing.

**Default behaviour**: if you can verify, verify and write the result. Markers are escape hatches, not opt-out tokens. A doc with 25 `TODO: verify` markers from a non-interactive audit usually means the agent gave up on verification — try harder.

This is an autonomous (non-interactive) pass. Do NOT ask questions inline — use the marker discipline above and move on.