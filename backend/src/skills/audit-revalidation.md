---
name: Audit Revalidation
description: Re-verifies an audited documentation file against the current code, driven by the cited lines Kronn found changed or gone since validation. Kronn attaches it to the bounded agent pass of “Verify docs”; it corrects what is false or stale, fixes broken citations, and leaves every true statement byte-identical.
icon: 🔁
category: domain
license: AGPL-3.0
builtin: true
---

# Audit revalidation

You re-verify one documentation file that a previous audit wrote and a human
validated. Since then the code moved. Kronn compared every `[src: file: …]`
citation of the file with a fingerprint of the cited lines taken at
validation, deterministically and without a model, and lists below the
citations that no longer match. Your job is to make the file true again with
the smallest possible edit.

## Inputs

- **Kronn findings**: the citations whose cited lines changed (`changed`),
  fall past the end of the file (`removed`), or whose file is gone
  (`file missing`). They are facts, not guesses: start there.
- **Relevance findings**: Kronn also judged each section of the file against
  the current code: `outdated` (a cited line changed or vanished, or some of
  the files it names are gone) or `no longer relevant` (everything it points
  at is gone), each with a proposed correction. A section Kronn did not list
  as outdated is not contradicted by anything it can check: that does not
  prove it true.
- **Unverifiable sections**: sections with no citation and no repository
  path. Kronn cannot measure them. Confront each with the code, then anchor
  it with a citation, or remove what is no longer true.
- **Moved citations** were already re-anchored by Kronn (same content, new
  line). Do not touch them.
- **Human-owned citations** sit in `owner="human"` sections: correct them
  like the others. Kronn asks their owner to reconfirm the change.

## Procedure

1. For each finding, open the cited file at the cited line and read what it
   says now.
2. Decide for the claim the citation supports:
   - still true, the line merely changed around it: cite the line that
     supports it now;
   - false or stale: correct the claim and cite the line that proves the
     correction;
   - the code it described is gone: remove the claim.
3. For each relevance finding, apply or refine the proposed correction:
   - outdated: re-read the code, then correct the claim or cite what supports
     it now;
   - no longer relevant: remove the section, unless it still describes
     something that exists (then cite where).
   - unverifiable: read the code the section talks about, correct or remove
     what is false, and add the citation that supports what stays;
4. Check the other facts and citations of the file the same way, without
   rewriting what is still true.
5. Use the citation grammar `[src: file: <path>:<line>]` or
   `[src: file: <path>:<start>-<end>]`, one reference per marker. Never cite a
   line you did not open.

## Rules

- Keep every statement that is still true exactly as written: no rephrasing,
  reordering or restyling. A file that is still accurate stays byte-identical.
- In a section marked `owner="human"`, correct only what the code
  contradicts and keep the rest of the human wording.
- Never invent a fact to fill a gap. If you cannot prove a replacement, remove
  the claim.
- Edit only the target file named by the step.
