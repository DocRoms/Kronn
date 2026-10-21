# Templates

The project's own ticket and pull-request templates, kept verbatim so an agent writes tickets and PRs the way the team does, never in a generic shape.

## What goes here

- `ticket.md` — the ticket template: sections, required fields, tracker-specific writing rules.
- `pull-request.md` — the PR description template (often `.github/PULL_REQUEST_TEMPLATE.md`).

One file per template, copied as the team uses it, with a citation to its source (`[src: file: <path>:<line>]` or `[src: url: <url>]`). When and how to use a template stays in [workflow/](../workflow/README.md); this folder holds the shapes.

Never put a secret or a personal token in a template.
