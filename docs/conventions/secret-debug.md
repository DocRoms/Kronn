# Secrets never print through `Debug` (KT-2039)

A failing test prints `{:?}` / `{:#?}` of what it compared. Quick Exec keeps
those failure blocks and stores and posts them to rooms (see
[quick-exec summary](#quick-exec-summary)). A struct that holds a key, token,
password or seed must therefore not `#[derive(Debug)]`.

## Rule

Drop `Debug` from the derive and invoke the macro right after the struct
[src: file: backend/src/core/redact.rs:1413]:

```rust
crate::debug_without_secrets!(ApiKey { id, name, provider, active; masked: value });
// Large struct: only the named fields print, the rest is elided (`..`).
crate::debug_without_secrets!(ServerConfig { host, port; masked: auth_token; .. });
```

- The exhaustive form destructures every field. If you add a field and leave
  it out of the macro, the build fails.
- A masked `String` prints `"***REDACTED***"` (an empty one stays `""`).
  `Option` keeps `None` / `Some(…)`, and a map keeps its keys.
- In a crate above `kronn-l0`, call it as `crate::debug_without_secrets!`. Each
  layer root re-exports it.

## Quick Exec summary

`summarise` redacts the whole summary once, before its final cut
[src: file: backend/src/core/quick_exec.rs:1003]. Any part cut earlier (the
generic tail, failure blocks, error lines) is redacted before its own cut, so
a value cut off from its name is never left behind. A stream capped mid-line
drops its partial last line before extraction
[src: file: backend/src/core/quick_exec.rs:537].
