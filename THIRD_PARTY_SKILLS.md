# Third-Party Skills

Kronn ships with **vendored** skills from third-party open-source projects. These skills cover
**methodologies** (TDD, debugging, code review, etc.) — orthogonal to the domain skills written
in-house (Rust, React, Python, …).

This file is the **bill of materials** for those vendored skills: every skill listed here is a
verbatim copy of an upstream `SKILL.md` (with the Kronn frontmatter extended for in-app metadata).
The project's name, license, source URL, and commit SHA are recorded so the attribution stays clear
and updates are reproducible.

## Why we vendor instead of fetch at runtime

- **Reproducibility** — the exact content shipped with Kronn `vX.Y.Z` is identical across every
  install. No surprise upgrade if upstream re-writes a skill.
- **Offline-friendly** — Kronn runs without internet access (self-hosted, desktop app, behind air-gapped
  networks). External skills shouldn't be a network dependency.
- **Sécu** — we don't need to trust an HTTPS endpoint not to MITM us at install time. Skills are
  hashed (commit SHA pinned), reviewed in PRs, signed by Kronn release tags.

## What we don't do (yet)

- **No marketplace** — Kronn 0.7 doesn't auto-install skills from a remote registry. A future
  version may add this for **user-installed** skills (alongside `~/.config/kronn/skills/`), but the
  vendored set in this repo is opinionated and curated by Kronn maintainers.
- **No automatic upstream sync** — when an upstream skill ships a useful update, a Kronn
  contributor manually re-vendors it (re-copy, bump SHA + `imported_at`, PR for review). See
  "Update process" below.

## Vendored skills

| Skill ID | Source | License | Upstream commit | Imported |
|----------|--------|---------|-----------------|----------|
| `test-driven-development` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `systematic-debugging` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `writing-plans` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `brainstorming` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `verification-before-completion` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `requesting-code-review` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `receiving-code-review` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |
| `finishing-a-development-branch` | [obra/superpowers](https://github.com/obra/superpowers) | MIT | `e7a2d164` | 2026-05-04 |

Files live in `backend/src/skills/external/` and are bundled at compile-time via `include_str!()`
from `backend/src/core/skills.rs::BUILTIN_SKILLS`.

Each skill's frontmatter (visible in-app and in API responses) carries:
- `external: true` — the UI renders a "🔗 External" badge.
- `source_url` — the upstream project (clickable in the UI).
- `source_path` — the file path inside the upstream repo.
- `source_commit` — the commit SHA pinned at vendoring time.
- `imported_at` — date of the import.

## Modifications vs upstream

To keep skills compatible with Kronn (no broken references inside the agent's prompt), a small
number of cosmetic edits were made:

- **`@filename.md` references** — Kronn's `BuiltinSkill` is a single `&'static str`, so attached
  companion files (e.g. `testing-anti-patterns.md` in `test-driven-development/`) are not bundled.
  References to those files were rewritten as **plain prose** with a pointer to the upstream URL.
- **No content was rewritten** beyond the frontmatter and these `@filename.md` mentions. The
  body of every skill is byte-identical to upstream.

If you want the full companion files, install the upstream skill directly via Anthropic's plugin
marketplace: `/plugin install superpowers@claude-plugins-official`. Kronn's vendored copy is
intended for **automatic, opt-in injection inside Kronn workflows** — not as a replacement of the
full upstream package.

## Update process

To re-vendor a skill (e.g. upstream shipped an improvement worth bringing in):

```bash
# 1. Clone upstream at the new commit
git clone --depth 1 https://github.com/obra/superpowers.git /tmp/sp
cd /tmp/sp && git rev-parse HEAD   # note the new SHA

# 2. Copy the SKILL.md
cp /tmp/sp/skills/<name>/SKILL.md \
   $KRONN_REPO/backend/src/skills/external/<name>.md

# 3. Edit the frontmatter (keep the Kronn extension fields, update source_commit + imported_at)

# 4. If the skill references @companion-file.md, rewrite the section to plain prose
#    (or inline the companion content if small enough)

# 5. Update this file with the new commit + date

# 6. PR for review.
```

A future Kronn release may ship a `make update-external-skills` script that automates step 1-3.

## Licenses (verbatim)

All upstream sources used so far are MIT-licensed. The full license text is available at:
- obra/superpowers: <https://github.com/obra/superpowers/blob/main/LICENSE>

Kronn's own code is AGPL-3.0. Vendored skills retain their original MIT license — Kronn does
**not** relicense them. Both licenses coexist in the codebase: AGPL for Kronn's runtime + UI, MIT
for the vendored skill content.
