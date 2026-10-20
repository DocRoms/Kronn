Read docs/AGENTS.md for context. Find ALL linter, formatter, and type-checker configs in the repo (e.g. .eslintrc, eslint.config.js, prettier, rustfmt.toml, tsconfig.json, phpcs.xml, etc.).

Fill docs/coding-rules.md — replace ALL {{PLACEHOLDERS}}:
- Replace {{LANGUAGE_1}}, {{LANGUAGE_2}} with one section per language/framework used in the project (add more sections if the project uses more than two)
- For each language, fill its {{TOOLS_1}}/{{TOOLS_2}} block with the linter, formatter, and type checker — config file and run command for each
- Replace {{CONVENTIONS_1}}, {{CONVENTIONS_2}} with coding conventions OBSERVED in existing code (naming, error handling, imports). Write fewer rather than inventing.
- Replace {{MISTAKES_1}}, {{MISTAKES_2}} with common mistakes to avoid (from linter configs, framework gotchas observed in code)
- If no linter/formatter is configured, write 'Not configured' in its place
- Add or remove language sections as needed to match the actual project stack

**ANTI-HALLU FOR THIS STEP** — {{CONVENTIONS_*}} and {{MISTAKES_*}} are the #1 vector for training-data leakage into AGENTS.md. Apply this rule strictly:
- Every CONVENTION row MUST cite the rule's source: either a linter config (`[src: file: .eslintrc.json:12]`, `[src: file: rustfmt.toml:3]`) OR at least 3 observed call sites (`[src: file: src/foo.rs:120]`, `[src: file: src/bar.rs:45]`, …).
- Every MISTAKE row MUST cite either a linter rule that enforces the prohibition OR a real bug found in the repo's git history (`[src: commit: <sha>]`).
- A rule you 'know from the language ecosystem' but cannot anchor in this repo is NOT a project convention — do NOT add it. Write fewer rules with proof, not more rules from memory.

**COMMIT CONVENTIONS** — this step also fills `docs/workflow/commits.md` (the message convention, the attribution policy and the sign-off policy: replace every placeholder of that file). Kronn computed the facts from the git history and the commit tooling and gives them in the `Commit facts` block at the end of this prompt: write the OBSERVED subject shape (colon or no colon, language, ticket keys, the closed type and scope lists when a tool file defines them), and call a convention enforced ONLY when that block lists a hook or CI step enforcing it. A tool that exists but that nothing runs (a `make commit` target, a configured Commitizen) is documented as opt-in. Remove the template banner and the `<!-- Fill: -->` hints. A `commits.md` that still holds a placeholder fails this step.
