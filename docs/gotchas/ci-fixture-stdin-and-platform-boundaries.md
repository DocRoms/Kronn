# CI fixtures must consume stdin and compile across platforms

## ACP subprocess completion

The September 8 coverage job failed in the Claude adapter's two-turn test
with `write claude prompt: Broken pipe (os error 32)`. The adapter writes the
prompt to its child's stdin, but the success fixture emitted its result and
exited without reading that pipe. Whether a small prompt fit before the child
exited was a scheduling accident, not a coverage-threshold failure.
[src: file: backend/src/acp/claude_adapter.rs:369-397#write_all]
[src: file: backend/src/acp/claude_adapter.rs:535-556#FIXTURE_BODY]
[src: url: https://github.com/DocRoms/Kronn/actions/runs/34250607216/job/102143783105]

The fixture now drains stdin before emitting its response. The existing
create/resume test sends a large Unicode prompt on both turns and retains its
session and response assertions. A prompt larger than pipe capacity exposed
the old fixture deterministically: the regression failed with the same broken
pipe before the drain was added and passed afterwards. No production write
error is swallowed, and no retry, sleep or timeout increase was added.
[src: file: backend/src/acp/claude_adapter.rs:535-556#FIXTURE_BODY]
[src: file: backend/src/acp/claude_adapter.rs:566-620#create_then_prompt_uses_session_id_then_resume_uses_resume_flag]
[src: file: backend/src/acp/claude_adapter.rs:379-396#prompt_write_failure]

## Windows compilation is not POSIX execution

The portability job compiles the complete Rust test library even when it
executes only the worktree, maintenance, orchestration and Windows-to-WSL
launch test groups. An
unconditional Unix import/chmod in the ACP fixture helper and an unconditional
Unix symlink in the source-explorer test therefore blocked that Windows job.
[src: file: .github/workflows/ci-build.yml:205-258#test-backend-portability]
[src: url: https://github.com/DocRoms/Kronn/actions/runs/34250607216/job/102143783376]

Only the Unix permissions operation (and the warm-up exec that follows it) is
platform-conditional; the helper still exists on Windows. The directory-link test uses the platform's actual symlink
operation and retains every assertion, with setup failure remaining a failure.
This does not make the POSIX shell fixtures executable on Windows or broaden
the portability job into a full Windows adapter test run.
[src: file: backend/src/acp.rs:36-40#PermissionsExt]
[src: file: backend/src/acp.rs:129-153#write_fixture_script]
[src: file: backend/src/api/ai_docs.rs:2150-2186#a_symlinked_directory_is_refused_however_it_is_named]

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
