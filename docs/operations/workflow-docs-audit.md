# Workflow document audit

Before launching an Agent step, Kronn records content fingerprints of regular
files in the project's detected documentation directory (`docs/`, `doc/` or
legacy `ai/`). [src: file: backend/src/core/docs_write_filter.rs:735-757#snapshot_docs] [src: file: backend/src/workflows/runner.rs:2763-2774#docs_before] The scope is Git-tracked files and non-ignored untracked files,
enumerated with `git ls-files --cached --others --exclude-standard`; tracked
files remain included even if an ignore pattern matches them. Non-Git projects
remain outside this audit, as in earlier releases. [src: file: backend/src/core/docs_write_filter.rs:675-750#document_paths]
On Unix, Git's NUL-delimited filename bytes are kept intact for filesystem
access, including non-UTF-8 names. Diagnostics preserve ordinary Unicode names;
invalid names, control characters and literal backslashes use an explicit
`[escaped path bytes]` label with byte escapes instead of replacement characters.
The label is never used to open a file. Platforms without Unix byte paths keep
the UTF-8 conversion and fail explicitly if a Git path cannot be represented. [src: file: backend/src/core/docs_write_filter.rs:616-657#native_document_path]
After the step, it checks new or changed text for credential
patterns and overlaps with the run's sensitive-file scan: either one fails the
step, and the rejection names the file and the line, never the content. A
rejected audit also drops the step's quota wait, which would run it again over
the rejected content. [src: file: backend/src/core/docs_write_filter.rs:104-120#check_docs_content]

A long run (32 characters or more among letters, digits and `_ - / + =`, at
least 15 % digits, as a whole or in a part between `=` or `/`, so that a name
glued to a value does not hide it) is a possible secret, not a known one: its form cannot tell
a token from a dated identifier or a hash the document cites. It is a known
secret, and fails the step, when a sensitive file of the worktree holds it:
a value or a line of the file found in the document, or a stretch the
document shares with the text of the files (also read without NUL bytes, for
UTF-16, and with the escapes of a JSON string undone, `\uXXXX` included)
that reads as a value: every character covered by 32 in a row
that a sensitive file holds, with six different characters and either a
digit with a letter or letters whose case keeps changing (a value whole,
glued to a name, cut short or extended; not a rule, a variable name or a
plain path). A document in any encoding is read (byte-order mark and NUL
bytes dropped, bytes that are no UTF-8 replaced), an image or a PDF like any
other: nothing is set aside as binary. A
changed document that cannot be read whole (over 16 MiB, unreadable) fails
the step once what the others hold is recorded. The
sensitive files are scanned at the start of the run and again at each audit.
Any other run fails nothing: the step stands, and the run records, per step,
the documents and the lines that hold one (50 documents and 5 lines each at
most, then a count; never the content). A rejected document still lists its
lines, the rejected one included. A step can run several
times (a loop, a resume, a quota wake) and each audit sees only what that
execution changed: the record keeps what it said of the documents this
execution left alone, and replaces or drops what it said of those it changed
or removed. After a sub-workflow step, the records of its child runs are
copied to the parent run under the step and the child run, in keys apart
from its own, replacing the ones copied before from the runs read again (a
child run deleted since keeps its record); child runs that could not be read
leave a record saying so. The run list marks such a run on its row, and the run
view lists the documents above the step list, saying that they did not block
the run, so the reader checks them before committing or publishing. Accepted
limit: a credential in a format the pattern list does not know, and that no
sensitive file of the worktree holds (or holds only transformed: encoded,
shorter than 32 characters and reworded, a part of it without a digit and
in one case, or in a sensitive file over 4 MiB, which is read by lines only),
is reported and not blocked. A step
cancelled or stopped by a guard while it runs is not audited at all
(KT-1796). [src: file: backend/src/core/docs_write_filter.rs:127-163#possible_secret_lines] [src: file: backend/src/workflows/runner.rs:32-60#possible_secrets_report] [src: file: frontend/src/lib/docsPossibleSecrets.ts:49-54#runPossibleSecrets]
Unchanged content that was already modified, staged or untracked before the
step is outside this audit. Changes committed during the step are still checked.
A document's size alone is not a leak signal. Fingerprinting uses a streaming
hash; the post-step check retains at most 16 MiB per file and inspects the same
bytes that were hashed. An unchanged larger file is allowed; a changed larger
file fails with an explicit incomplete-audit diagnostic and remains intact. [src: file: backend/src/core/docs_write_filter.rs:590-630#MAX_AUDIT_BUFFER_BYTES] [src: file: backend/src/core/docs_write_filter.rs:776-864#audit_docs_writes]

The audit never restores, deletes, stages or commits files. A change observed
during the step may have been written concurrently by a person or another
process; a fingerprint does not establish authorship. Rejected changes fail
the step, clear its condition action and stop normal workflow continuation.
The persisted step output names the paths and detection reasons without
copying suspected credentials. Files and index entries remain available for
inspection. An incomplete snapshot prevents the agent launch; an incomplete
post-step audit also fails the step. [src: file: backend/src/workflows/runner.rs:17-26#fail_document_audit] [src: file: backend/src/workflows/runner.rs:2789-2796#preparation_error] [src: file: backend/src/workflows/runner.rs:3004-3070#audit_docs_writes]

Explicit failure compensation and workspace hooks keep their existing
behavior. This audit is a post-step check, not a sandbox: it cannot undo an
agent's earlier external effects or commits, and it does not prevent a later
manual commit of the preserved file. Inspect flagged content before committing
or publishing it. Unchanged preexisting credentials are not reclassified as
new agent writes. Symbolic links are not inspected and not followed. The existing curated `AGENTS.md` / `index.md` exemptions are
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
