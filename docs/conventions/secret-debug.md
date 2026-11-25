# Secrets never print through `Debug` (KT-2039)

A failing test prints `{:?}` / `{:#?}` of what it compared. Quick Exec keeps
those failure blocks and stores and posts them to rooms (see
[quick-exec summary](#quick-exec-summary)). A struct that holds a key, token,
password or seed must therefore not `#[derive(Debug)]`.

## Rule

Drop `Debug` from the derive and invoke the macro right after the struct
[src: file: backend/src/core/redact.rs:1506]:

```rust
crate::debug_without_secrets!(ApiKey { id, name, provider, active; masked: value });
// Large struct: only the named fields print, the rest is elided (`..`).
crate::debug_without_secrets!(ServerConfig { host, port; masked: auth_token; .. });
```

- The exhaustive form destructures every field. If you add a field and leave
  it out of the macro, the build fails.
- A masked `String` prints `"***REDACTED***"` (an empty one stays `""`).
  `Option` keeps `None` / `Some(…)`, a map (`HashMap`, `BTreeMap`) keeps its
  keys, and a `Vec` keeps its length.
- In a crate above `kronn-l0`, call it as `crate::debug_without_secrets!`. Each
  layer root re-exports it.
- The macro covers structs only. For an enum, write `Debug` by hand and wrap
  each secret in `crate::core::redact::Masked`, as `RepoSourceAuth` does
  [src: file: backend/src/api/discover.rs:19].
- When an enum holds a secret in one variant of many, a masked newtype for
  that field is smaller. Make it `#[serde(transparent)]` so the wire format
  does not change, keep the field `#[ts(skip)]` (or `#[ts(as = "String")]`)
  so the TS type does not change, and expose it through `Deref<Target = str>`.
  `PairingSecret` in `WsMessage::Presence` is an example
  [src: file: backend/src/models/multiuser.rs:58].
- Mask every field that carries the secret, including text built around it
  (`InviteResponse`'s instruction texts embed the token).
- Judge a field by what it carries, not by its name (KT-2043 review). Mask the
  values of:
  - an MCP server's `env` map;
  - header maps (`headers`, `api_headers`);
  - run-variable maps (`variables`, `vars`, `launch_variables`,
    `mock_variables`, `values`): a variable can be the manual override of a
    `ProjectEnv` credential;
  - a custom API field's `value`.

  Keys stay visible. Query, path-parameter and body maps are left visible:
  plugin auth reaches the request server-side, not through them.
- Add the type to the table test in
  [src: file: backend/src/core/redact_debug_test.rs:34] (KT-2043). It runs under
  `cargo test --lib core::redact`. A private type gets a test in its own module
  whose name contains `debug_without_secrets`.

## Quick Exec summary

`summarise` redacts each stream it reads whole, once, before any line window
or per-line pass [src: file: backend/src/core/quick_exec.rs:857] (KT-2044).
`{:#?}` prints a field's name and its value on two lines, so a window that
starts between them (the generic 20-line tail, a first error line) would
otherwise keep the value without its name. `Collected` never shows its stdout,
only its size, so only its stderr is redacted. The parts cut afterwards (the
generic tail, failure blocks, error lines) are redacted again before their own
cut, and the whole summary once more before its final cut
[src: file: backend/src/core/quick_exec.rs:1026]. A stream capped mid-line
drops its partial last line before extraction
[src: file: backend/src/core/quick_exec.rs:537].

The redaction keeps a value's call readable: `api_key: Some("***REDACTED***")`,
also when calls nest (`Some(Token("***REDACTED***"))`) and when the value is cut
before its closing quote (`password: Some("***REDACTED***` at the end of a
capped stream or block). A quoted key is matched with either quote on its value
(`{ 'x-api-key': "***REDACTED***" }`, as `util.inspect` and Python's `repr`
print a value holding a `'`) [src: file: backend/src/core/redact.rs:706].
