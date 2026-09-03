# A NUL byte in a spawned command, and why the obvious detection is blind

`Command::spawn` refuses the whole invocation with `nul byte found in provided
data` and names nothing. Four carriers produce that identical error — the
program name, any argument, any environment entry, and the working directory —
each verified by a real spawn rather than assumed.
[src: file: backend/src/agents/runner.rs:8629]

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
Both checks are needed. [src: file: backend/src/agents/runner.rs:8593-8595]

A test pins that library behaviour, so a future Rust release that stops
substituting the placeholder fails there instead of quietly blinding the
detection. [src: file: backend/src/agents/runner_test.rs:5947]

## Report the carrier, never the value

These fields hold API keys and tokens. The refusal names the environment
variable by key, or the argument by the flag it follows — positions shift
between agents — and never the content, matching the invocation receipt's own
rule that no argument content is logged.
[src: file: backend/src/agents/runner_test.rs:5875]

## It is settled before anything runs

Such a failure is deterministic, so deferring it only repeats it. It was
classified as a runtime outage and replayed every 30 seconds — 282 identical
attempts in one report, diagnosing nothing. It is now a hard preflight failure
that surfaces in the discussion.
[src: file: backend/src/api/discussions/streaming.rs:694]
[src: url: https://github.com/DocRoms/Kronn/issues/201]
