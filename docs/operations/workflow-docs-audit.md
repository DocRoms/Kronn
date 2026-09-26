# Workflow document audit

Before launching an Agent step, Kronn records content fingerprints of regular
files in the project's detected documentation directory (`docs/`, `doc/` or
legacy `ai/`). The scope is Git-tracked files and non-ignored untracked files,
enumerated with `git ls-files --cached --others --exclude-standard`; tracked
files remain included even if an ignore pattern matches them. Non-Git projects
remain outside this audit, as in earlier releases.
On Unix, Git's NUL-delimited filename bytes are kept intact for filesystem
access, including non-UTF-8 names. Diagnostics preserve ordinary Unicode names;
invalid names, control characters and literal backslashes use an explicit
`[escaped path bytes]` label with byte escapes instead of replacement characters.
The label is never used to open a file. Platforms without Unix byte paths keep
the UTF-8 conversion and fail explicitly if a Git path cannot be represented.
After the step, it checks new or changed text for credential
patterns, high-entropy tokens and overlaps with the run's sensitive-file scan.
Unchanged content that was already modified, staged or untracked before the
step is outside this audit. Changes committed during the step are still checked.
A document's size alone is not a leak signal. Fingerprinting uses a streaming
hash; the post-step check retains at most 16 MiB per file and inspects the same
bytes that were hashed. An unchanged larger file is allowed; a changed larger
file fails with an explicit incomplete-audit diagnostic and remains intact.

The audit never restores, deletes, stages or commits files. A change observed
during the step may have been written concurrently by a person or another
process; a fingerprint does not establish authorship. Rejected changes fail
the step, clear its condition action and stop normal workflow continuation.
The persisted step output names the paths and detection reasons without
copying suspected credentials. Files and index entries remain available for
inspection. An incomplete snapshot prevents the agent launch; an incomplete
post-step audit also fails the step.

Explicit failure compensation and workspace hooks keep their existing
behavior. This audit is a post-step check, not a sandbox: it cannot undo an
agent's earlier external effects or commits, and it does not prevent a later
manual commit of the preserved file. Inspect flagged content before committing
or publishing it. Unchanged preexisting credentials are not reclassified as
new agent writes. Non-UTF-8 content and symbolic links are not inspected; links
are not followed. The existing curated `AGENTS.md` / `index.md` exemptions are
preserved.

## Earlier releases and recovery

The former audit inspected all dirty documentation paths after an Agent step,
including a launch that failed before generation. It could restore a tracked
file and its index entry from HEAD, or delete an untracked file, even when the
step had not touched it. It also applied an 8 KiB memory-entry limit to ordinary
project documents.

For an affected file, preserve its current contents before investigating
editor local history, backups or Git objects. A previous staged version may
have left a Git object; unstaged or untracked contents may have no Git copy.
An unsuccessful object search does not prove that recovery is impossible.
The new audit cannot reconstruct contents removed by an older release.

[src: file: backend/src/core/docs_write_filter.rs]
[src: file: backend/src/workflows/runner.rs]
