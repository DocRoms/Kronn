# The anti-hallucination guard

AI agents confidently assert file paths that do not exist, functions they
invented and versions that are not yours. Once one of those lands in
`docs/AGENTS.md`, every later agent inherits it as established truth. Kronn
therefore runs a two-tier guard on every agent reply: a **sourcing directive**
before the agent writes, and a **mechanical check** of its citations after.

The citation grammar itself is an open convention:
[`agents-md-format-v1.md`](agents-md-format-v1.md), also served on
`/api/conventions/agents-md-format-v1` and linked from Settings. Anyone can emit
Kronn-compatible documentation, with or without Kronn.

## Three modes

One global mode, chosen in **Settings → Anti-hallucination** or with
`anti_hallucination_mode` in `config.toml`. The default is `warn`.
[src: file: backend/src/models/setup.rs:429#anti_hallucination_mode]

- **`off`**: no directive, no checking.
- **`warn`** (default): each agent receives a sourcing directive ("cite a
  `file:line`, a URL or a user confirmation for every non-trivial claim; if you
  cannot, say so plainly and verify before asserting"). Kronn then inspects each
  `[src: …]` citation the agent emits: the path is jailed to the project root
  (no escape from it), the file must exist and the line range must be in bounds.
  A non-blocking pill on the message reports problems: **red** for fabricated
  citations (the cited file or line does not exist, or the source is
  `training-data`, the model's prior knowledge, rejected automatically),
  **amber** for confident claims left without any anchor. Clicking the pill
  lists the bad citations.
- **`enforce`**: `warn`, plus a refusal where it matters. An audit step that
  writes a `docs/` file with a fabricated citation is re-run with a corrective
  note, up to three attempts, and then fails, so the audit ends interrupted
  rather than committing an invented fact. A discussion reply with a fabricated
  citation stays visible but gets a refusal note telling you not to rely on it.
  On a project with a `docs/AGENTS.md`, the `kronn-doc-author` skill is attached
  to the agent.
  [src: file: backend/src/api/audit/anti_hallu_enforce.rs:24#MAX_ATTEMPTS]
  [src: file: backend/src/core/anti_halluc.rs:175-177#enforce_refusal_needed]

## Honest by design

`verified` means the citation **exists**, not that the claim is **true**:
catching a real but irrelevant citation is the job of a future judge layer. The
`warn` pill is a signal you decide to act on, not a hard gate. The check does not
depend on the language and survives output compression, and it cannot be gamed:
an agent cannot fabricate a file or a line number that actually exists in your
repository.
