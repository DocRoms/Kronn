# Workflow — process conventions

Four short, focused files — one process each. Split so an agent loads only
the one it needs (see the T2 table in [`../AGENTS.md`](../AGENTS.md)),
instead of one long "how we work" document nobody fully reads.

- [commits.md](commits.md) — commit message convention.
- [pull-requests.md](pull-requests.md) — PR conventions (title, description, review).
- [tickets.md](tickets.md) — ticket/tracker conventions, including whether a
  code comment may cite a ticket/issue ID.
- [ci-cd.md](ci-cd.md) — CI/CD pipeline: what runs, where, and what blocks a merge.

Each file states the actual project convention as a checkable fact, not
generic advice. If a process isn't formalized yet, say so explicitly
(`Not formalized — ask before assuming.`) rather than inventing one.
