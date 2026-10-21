

## Legacy docs (PRIMARY SOURCE for this audit)
This project had a hand-curated `docs/` folder BEFORE Kronn was bootstrapped. We moved that content under `docs/legacy/` so the freshly-installed Kronn templates don't collide with it. **READ every `*.md` under `docs/legacy/` BEFORE filling the Kronn templates.** That content is the human-curated knowledge — the README and source code alone would lose 6 months of accumulated context.

When filling each Kronn template, cite the legacy source inline (`see docs/legacy/installation.md`, `cf docs/legacy/architecture/overview.md`) so the user can verify the mapping and decide what to keep / discard after the audit. After the audit, the user reviews `docs/legacy/` and either deletes it or migrates remaining pieces into the Kronn structure manually.

**Navigation requirement for `docs/AGENTS.md` ONLY (Step 1):** when filling `docs/AGENTS.md`, add ONE line in the appropriate section (or a small dedicated `## Legacy docs (pre-Kronn snapshot)` section if none fits) that points future agents to `docs/legacy/` — wording like `> Hand-curated docs from before Kronn — see [docs/legacy/README.md](legacy/README.md) for context preserved from the previous structure.` Without this pointer the folder is invisible to anyone re-reading `AGENTS.md` next week. Do NOT add this line to other Kronn templates (`glossary.md`, `repo-map.md`, etc.) — the entry point is enough.
