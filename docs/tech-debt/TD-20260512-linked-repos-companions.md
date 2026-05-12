# TD-20260512 — Linked repos / companion projects

## Context

Real projects don't live alone. A typical Symfony/Next frontend ships
with a backend API repo, an IaC/Terraform repo, a shared design system
repo, etc. Today Kronn audits each in isolation — the agent working on
the frontend has no idea the backend's endpoints live at
`../my-api/src/Controller/`, and ends up either inventing routes or
asking the user to paste them.

Workaround used on `front_euronews`: the user manually documents the
companion repos in `~/.kronn/user-context/` or stuffs them into
`docs/AGENTS.md`. Works but:
- Not discoverable (no UI surface).
- Not reusable across projects.
- Audit doesn't know about them, so the generated docs miss the
  cross-repo context.

User reported (2026-05-12) while auditing DOCROMS_WEB: "would be nice
to declare 'this project depends on the API repo at /path', and have
agents follow that link when they need more context."

## Why we can't fix now (constraint)

Non-trivial scope (~1-2 days). Surfaced at the very end of 0.8.1.
Needs a model + 2 CRUD endpoints + a settings UI section + the audit
prompt integration + i18n + tests. Better as a focused 0.8.2 feature.

## Impact

- **correctness** : agents working on the frontend invent backend
  routes / payload shapes when the API repo isn't loaded into context.
- **dev friction** : user repeats the same "the API lives at..."
  preamble in every discussion.
- **audit quality** : the generated `docs/architecture/overview.md`
  has a "data flow" section that stops at the network boundary because
  the audit can't see the backend.

## Where (pointers)

- `backend/src/models/projects.rs:19` — `Project` struct, where the
  new `linked_repos: Vec<LinkedRepo>` field would live (defaults to
  empty, `#[serde(default)]`).
- `backend/src/api/projects/` — new `LinkedRepo` CRUD endpoints
  (mirror `mcps.rs` linkage pattern).
- `backend/src/api/audit/mod.rs:62` — `ANALYSIS_STEPS[0]` prompt
  could include the linked-repos list so the agent knows where to
  look for upstream context.
- `frontend/src/components/ProjectCard.tsx` — new collapsible
  section "Dépôts liés" after "Documentation projet".
- `~/.kronn/user-context/` — keep this as the fallback for users
  who prefer free-form context, but stop advertising it as the
  "right place" for companion repos.

## Suggested direction (non-binding)

Phase 1 — model + storage (~3h):
- New `LinkedRepo { id, name, kind: 'api'|'iac'|'design'|'other',
  location: String, description: String }`.
- `Project.linked_repos: Vec<LinkedRepo>` (in-row JSON, no separate
  table for now — small data, projects rarely have more than 5
  links).

Phase 2 — Settings UI (~4h):
- New section in ProjectCard between "Documentation projet" and
  "MCPs" : list + Add/Remove inline editor.
- Validation : location must be either a file path that exists OR
  a URL that pings.

Phase 3 — Audit + agent integration (~3h):
- Step 1 prompt picks up `linked_repos` and instructs the agent
  to read each one's `docs/AGENTS.md` (or top-level `README.md`)
  for cross-project context.
- Every discussion / QP / workflow on this project gets the
  linked-repos list in its system prompt prelude.

Phase 4 — UX polish (~2h):
- Bootstrap wizard adds a question "Ce projet a-t-il des dépôts
  liés ?" between Vision and Architecture.
- Migration banner for existing audited projects: "Add your
  linked repos to enrich agent context".

## Next step

create ticket — schedule a quick UX call on the Settings layout
before Phase 2 (where the list lives, how Add/Remove looks, kind
icons).

## Status

Draft

## Effort

L (1+ day, ~1.5d with tests + i18n)

## Blast radius

module — touches `Project` model + 2 new endpoints + 1 new section
in ProjectCard + audit Step 1 prompt + ~20 i18n keys × 3 langs.
Backwards-compat is free (`linked_repos` defaults to empty).
