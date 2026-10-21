# A NUL byte in a spawned command, and why the obvious detection is blind

`Command::spawn` refuses the whole invocation with `nul byte found in provided
data` and names nothing. Four carriers produce that identical error — the
program name, any argument, any environment entry, and the working directory —
and Kronn's pre-spawn check examines all four.
[src: file: backend/src/agents/runner.rs:13414-13462#nul_byte_offender]

A `\0` is valid UTF-8, so `read_to_string` carries one out of a file without a
word. Any value Kronn reads from disk or decrypts can therefore reach a command
line intact.

## The trap

Scanning the bytes of `get_program()`, `get_args()` and `get_current_dir()`
finds nothing: the standard library does not hand back a value it could not turn
into a C string, it substitutes the literal `<string-with-nul>`. Only
`get_envs()` returns the offending value untouched.

A byte scan alone — the obvious implementation — therefore detects the
environment case and stays blind to the other three, with nothing to signal it.
Both checks are needed. [src: file: backend/src/agents/runner.rs:13390-13399#NUL_BEARING_PLACEHOLDER]

A test pins that library behaviour, so a future Rust release that stops
substituting the placeholder fails there instead of quietly blinding the
detection. [src: file: backend/src/agents/runner_test.rs:10508-10542#the_standard_library_still_masks_nul_bearing_values_it_cannot_encode]

## Report the carrier, never the value

These fields hold API keys and tokens. The refusal names the environment
variable by key, or the argument by the flag it follows — positions shift
between agents — and never the content, matching the invocation receipt's own
rule that no argument content is logged.
[src: file: backend/src/agents/runner_test.rs:10440-10506#a_nul_byte_is_named_by_its_carrier_never_by_its_value]

## It is settled before anything runs

Such a failure is deterministic, so deferring it only repeats it. It was
classified as a runtime outage and replayed every 30 seconds — 282 identical
attempts in one report, diagnosing nothing. It is now a hard preflight failure
that surfaces in the discussion.
[src: file: backend/src/api/discussions/streaming.rs:1277-1316#agent_start_failure_outcome]
[src: url: https://github.com/DocRoms/Kronn/issues/201]

## Attachment previews and legacy history (KT-633 / issue 208)

The earlier hard-stop fix does not make an attachment preview safe. Lossy UTF-8
decoding preserves NUL, so a UTF-16 or binary upload could still poison a later
command. Ordinary disk-backed attachments now keep their original bytes while
their preview uses strict UTF-8 or explicit UTF-16 LE/BE BOM decoding. A preview
reads at most 8 KiB of input and returns at most 8 KiB of decoded UTF-8, without
inventing replacement characters at a truncated code point or surrogate pair.
Invalid text, UTF-32 BOMs and NUL-bearing previews (including ambiguous BOMless
UTF-16) produce an explicit unavailable-preview message; no encoding is guessed.
This is preview validation, not validation of the complete file. Office parsing
and image storage retain their existing behavior.
[src: file: backend/src/core/context_files.rs:12-16#PREVIEW_BYTES]
[src: file: backend/src/core/context_files.rs:463-475#DiskFile]
[src: file: backend/src/core/context_files.rs:478-531#attachment_preview]

Old/imported `extracted_text` is protected again at context assembly. When an
original file path exists, the agent is told to read it; an inline-only legacy
entry instead asks for a new attachment. Full-history, session-delta, debate and
synthesis prompt builders replace remaining NUL with one ASCII space only in
the execution projection. This separates neighboring words and preserves the
byte budget and all other Unicode. It does not rewrite stored messages, titles,
summaries, archives or file bytes. There is no bulk DB repair or new message
rejection rule: send/revise/append/import content reaches these common builders.
Other command carriers, such as environment or MCP configuration, still use the
existing hard preflight refusal rather than being silently repaired.
[src: file: backend/src/core/context_files.rs:765-807#build_context_prompt]
[src: file: backend/src/core/context_files.rs:533-541#nul_safe_prompt]
[src: file: backend/src/api/disc_prompts.rs:43-185#build_orchestration_prompt]
[src: file: backend/src/api/disc_prompts.rs:187-260#build_synthesis_prompt]
[src: file: backend/src/api/disc_prompts.rs:387-398#build_agent_prompt]
[src: file: backend/src/api/disc_prompts.rs:400-420#build_agent_delta_prompt]
[src: file: backend/src/api/discussions/streaming.rs:2700-2700#build_agent_delta_prompt]
[src: file: backend/src/api/discussions/streaming.rs:4007-4007#build_agent_prompt]

The new HTTP regressions use the real Router and SQLite with temporary config
and files: upload preserves exact disk bytes, and export/import/re-export keeps
legacy NUL-bearing history and previews intact while their execution context is
NUL-free. A separate production-function composition verifies classification,
persisted System error/job linkage, terminal Failed state, one attempt, cleared
awaiting state and refusal to defer a settled job. It does not spawn a real
provider or claim a browser/full-runtime end-to-end demonstration. Truly absent
runtimes still use the existing deferral policy; this patch does not cap it.
[src: file: backend/tests/context_file_safety.rs:82-137#upload_utf16_and_binary_keeps_original_bytes_and_safe_durable_previews]
[src: file: backend/tests/context_file_safety.rs:139-247#imported_legacy_preview_and_history_are_safe_without_rewriting_the_archive]
[src: file: backend/src/api/discussions/streaming.rs:7322-7352#a_nul_byte_in_the_command_line_is_a_hard_stop_not_a_deferral]
[src: file: backend/src/api/discussions/streaming.rs:7354-7446#kt633_nul_failure_persists_a_visible_error_and_settles_the_dispatch]

Initial production-path regression replay: **9 failed / 0 passed** before the
preview/prompt corrections. Expanded focused replay: **12 passed**; two HTTP
integration tests also passed. Intermediate fixture-only compile errors and a
wrong test message role were corrected; those are not product regression proof.
Full-backend and strict lint qualification is recorded in the
[release ledger](../releases/0.13.0-checklist.md), separately from these focused
results. The original issue's connected session has not been replayed.
[src: url: https://github.com/DocRoms/Kronn/issues/208]
