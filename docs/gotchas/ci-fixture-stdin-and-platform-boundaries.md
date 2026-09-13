# CI fixtures must consume stdin and compile across platforms

## ACP subprocess completion

The September 8 coverage job failed in the Claude adapter's two-turn test
with `write claude prompt: Broken pipe (os error 32)`. The adapter writes the
prompt to its child's stdin, but the success fixture emitted its result and
exited without reading that pipe. Whether a small prompt fit before the child
exited was a scheduling accident, not a coverage-threshold failure.
[src: file: backend/src/acp/claude_adapter.rs:234-252]
[src: url: https://github.com/DocRoms/Kronn/actions/runs/34250607216/job/102143783105]

The fixture now drains stdin before emitting its response. The existing
create/resume test sends a large Unicode prompt on both turns and retains its
session and response assertions. A prompt larger than pipe capacity exposed
the old fixture deterministically: the regression failed with the same broken
pipe before the drain was added and passed afterwards. No production write
error is swallowed, and no retry, sleep or timeout increase was added.
[src: file: backend/src/acp/claude_adapter.rs:366-441]

## Windows compilation is not POSIX execution

The portability job compiles the complete Rust test library even when it
executes only the worktree, maintenance and orchestration test groups. An
unconditional Unix import/chmod in the ACP fixture helper and an unconditional
Unix symlink in the source-explorer test therefore blocked that Windows job.
[src: file: .github/workflows/ci-test.yml:705-740]
[src: url: https://github.com/DocRoms/Kronn/actions/runs/34250607216/job/102143783376]

Only the Unix permissions operation is platform-conditional; the helper still
exists on Windows. The directory-link test uses the platform's actual symlink
operation and retains every assertion, with setup failure remaining a failure.
This does not make the POSIX shell fixtures executable on Windows or broaden
the portability job into a full Windows adapter test run.
[src: file: backend/src/acp.rs:31-61]
[src: file: backend/src/api/ai_docs.rs:2125-2159]

## September 13 bounded evidence

On parent `a30e9f237d5388d288424c46823c877fa97effa6` plus these three test-file
changes: 67 ACP tests and 36 source-explorer tests pass with no failures or
skips; formatting and strict all-target Clippy pass. The exact extracted helper
and symlink statements were also compiled to metadata for the already-installed
`x86_64-pc-windows-msvc` target: three compiler errors before the platform
branches, zero afterwards. This is a compile-only fragment check, not a full
backend cross-build or Windows filesystem execution.

Local immutable logs, source snapshots and receipts are under
`/private/tmp/release-013-takeover-proof.XMDShb/` (the `acp-pipe`,
`acp-adapters`, `ai-docs`, `fmt`, `clippy` and `windows-fixture` records).
Fresh full backend/frontend, coverage and Windows CI on the final integrated
release candidate remain separate requirements. The existing macOS linker
compact-unwind warning and ts-rs attribute notices are not resolved here.
