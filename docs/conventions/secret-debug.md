# Secrets never print through `Debug` (KT-2039)

A failing test prints `{:?}` / `{:#?}` of what it compared. Quick Exec keeps
those failure blocks and stores and posts them to rooms (see
[quick-exec summary](#quick-exec-summary)). A struct that holds a key, token,
password or seed must therefore not `#[derive(Debug)]`.

## Rule

Drop `Debug` from the derive and invoke the macro right after the struct
[src: file: backend/src/core/redact.rs:3477]:

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
  - a custom API field's `value`;
  - an API plugin's default header `value` and an OAuth2 extra header's
    `value_template` (KT-2053): a literal or a `{ENV_KEY}` template, and
    `OAuth2ExtraHeader` prints nested in an API spec's
    `ApiAuthKind::OAuth2ClientCredentials`
    [src: file: backend/src/models/mcp.rs:330].

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
print a value holding a `'`) [src: file: backend/src/core/redact.rs:745].

KT-2053 reads a secret's whole value expression, not a list of call shapes.
After a secret-ish name and its `:` / `=`, every quoted literal of the value
expression is masked [src: file: backend/src/core/redact.rs:2623]:

- A literal is `"…"`, `'…'`, a backtick one, or `\"…\"` inside a string printed
  with `{:?}`, with an optional prefix (`b"…"`, `r"…"`, `r#"…"#`, `f"…"`,
  `u'…'`), or Lua's long bracket `[[…]]` / `[==[…]==]` (closed by the same
  number of `=`s, across lines; not closed, it runs to the end of the text)
  where a value starts: first, or after an operator, an opener or the word
  `or` / `and` (`x or [[…]]`), never after a name or a closer, where `a[[1]]`
  is an index [src: file: backend/src/core/redact.rs:1352]. The literals
  inside a `[[…]]` are read too, as it may be a nested array
  (`new Map([[k, "…"]])`), so agent output judges each on its own
  [src: file: backend/src/core/redact.rs:1511]. Its position does
  not matter: after a type (`let token: &str = "…"`, `api_key: Option<String> = Some("…".into())`), a
  reference (`&"…"`), a method or dotted call (`os.getenv("X", "…")`,
  `config.get('…')`, `env::var("X").unwrap_or("…")`), or as a later argument
  of a call (`env::var_or("KEY", "…")`).
- The expression ends at a `;` `)` `]` `}` at depth 0, at a `,` at depth 0
  that a new key follows, or at the end of the line
  [src: file: backend/src/core/redact.rs:1208]. Depth counts the
  parentheses, brackets, braces and generics (`<` right after a word) opened
  inside it, so `Cfg { token: Some("…"), retries: 3 }` keeps `retries: 3`.
  A new key after the `,` is a name (after an optional `$`, `@` or Ruby's
  `:`, before an optional `?` / `!`) or a quoted name (`"…"`, `'…'`,
  `\"…\"`), then a `:` (not `::`) or a `=` (not `==` / `=~`; `=>` is one):
  `, name = …`, `, name: …`, `, "name": …`, `, 'name' => …`, blanks, line
  breaks, a diff marker and line comments skipped before it. After a
  `lambda`, a `name:` is a parameter, not a key. Before anything else (a
  value, a call, a comparison, a block comment) the value goes on past the
  `,` (`token = x, "…"`, a Python tuple or Ruby array), and a line that ends
  with that `,` carries it like `||` (KT-2053 round 9)
  [src: file: backend/src/core/redact.rs:1595].
  The `>` of `->` / `=>` closes no generic (`Result<fn() -> String, E>`). A
  line break closes an open generic unless its list goes on: the line ends
  with its `<` or a `,`, or the next line starts with its `>` or a `,`
  (`Option<\n    String,\n> = Some("…")`, `HashMap<\n    String\n    ,
  String\n>`), so a `<` glued to a word at a
  line's end (`a<b`) opens nothing past that line
  [src: file: backend/src/core/redact.rs:1557]. A
  line break, or a printed `\n` / `\r`, ends it only at depth 0, as a `{:#?}`
  dump puts a call's value on the next line. It does not end it in three
  cases. First, nothing but blanks was read since the `:` / `=`, so the value
  is on the next line (`const token =\n  process.env.X ?? "…"`). Second, the
  line ends with a continuation operator: `||`, `&&`, `?` (so `??`), `:`, `+`,
  `=`, `(`, a `,` no new key follows, `^` (cmd), the word `or` / `and` (any case, a whole word, so
  `color` is none), a `..` after a blank (Lua's `"sk_" ..`), or a `.` after a
  blank (`$salt .`). A `.` right after a word or a
  closer goes on only into a chained call, a next line that starts with a name
  right before `(` (Go's `os.Getenv("X").\n\tOr("…")`), so a sentence's `.`
  still ends the expression. Third, the next non-blank line starts with a
  continuation operator: `.` or Lua's `..` (not `...`), `?.`, `??`, a
  ternary's `?` or `:` (not `::`), `||`, `&&`, `+`, a `,` (comma-first:
  `x\n  , "…"`; a new key after it still ends the value), Perl's `//`, or the
  word `or` / `and` (`env::var("X")\n    .unwrap_or("…")`,
  `local t = os.getenv("X")\n  or "…"`). That word is whole (`order`,
  `android` are none); in lower or upper case it always carries, and a
  capitalised `Or` / `And` only before a non-letter (`Or "…"`, `And(…)`), as
  it otherwise starts a sentence (`And then she said "…"`)
  [src: file: backend/src/core/redact.rs:2154]. A leading `//` line is also
  read as a comment, so its literals are masked either way. A line that holds
  only comments (`--`, `#`, `//`, `/* … */` or `--[[ … ]]`, a block across
  lines too) between the value and that operator is skipped like a blank line
  (`x\n  -- note\n  or "…"`), and its literals are still read; before
  anything else the value still ends at the first break
  [src: file: backend/src/core/redact.rs:2074]. This holds after a
  trailing comment and in both versions of a diff. How
  the line ends, and where it starts (past a `\n` or a `\r`), are tracked
  while it is read, never searched again backwards
  [src: file: backend/src/core/redact.rs:2405]. A
  trailing comment does not count: a `//`, `#`, `--`, `/* … */` or Lua
  `--[[ … ]]` (`--[==[ … ]==]`) that starts after a blank or an operator is
  read as blanks, so `x || // default` and `x or -- default` still end with
  their operator, and its brackets, `,` and `;` end nothing. Glued to a `:`
  `(` `[` `{` `,` `^`, a `/*` opens a comment too (`a :/* c; */ "…"`), and a
  `//`, `--` or `#` when a blank or the line's end follows it (`(// c`), so
  a URL's `://x`, `(--i`, `color:#fff`, `(#[x]` and `(#"…"#` are none. A
  literal inside a comment is still masked; one that its comment line does
  not close is masked to that line's end, and the next line is still read
  as code, its line break and code kept. A `//` with nothing after
  it is Perl's defined-or (`$ENV{X} //`) and carries the line; a block comment
  runs across line breaks to its end. A URL (`https://x`) or a decrement
  (`i--`) is no comment, nor, after a word or right after the `:` / `=`, a raw
  string (`#"…"#`), an attribute (`#[x]`), a shebang (`#!`) or `##`; after an
  operator any `#` opens a comment (`x || #[c]`)
  [src: file: backend/src/core/redact.rs:1738]. A bare `#"` (Swift) is a raw
  string: it closes only at `"#`, or runs to the end of the text. In a
  unified diff the next line's `+` / `-` marker is skipped first, and the
  pass also reads the diff's old version (`+` lines blanked) and new version
  (`-` lines blanked) at the same offsets
  [src: file: backend/src/core/redact.rs:2623], so the carry that enters a
  `-` run enters the `+` run after it (` token = a ||\n-  "x";\n+  "…";`), and
  a context line after both goes on from either. A `"` right
  after a word or a closing bracket closes the string the expression is
  written in, so it ends there too. So does a bare `"` once a `\"…\"` literal
  was read (`left: "token: \"…\""`: the `{:?}` string closes there).
- After `.` `!` `?` a quote opens the value (`$salt."…"`, `ok?"…":"x"`,
  `env.X??"…"`, `!!"…"`) unless it closes a sentence: that literal's closing
  quote comes right before a word (`… the key Y.", "b": "c"`), or, after a
  sentence's `.` `!` `?` only, the literal starts with a blank or `,` `;`
  `:` `)` `]` `}`. An operator is no sentence's end: a `.` after a blank, a
  closer, a quote, Lua's `..` or a variable or member (`$prefix.":"."…"`,
  `$this->p.`, `self::P.`); a `?` after `?` (`env.X??":…"`), after another
  operator, or before a ternary's `:` (`empty?" ":"…"`); a `!` after
  anything but a word or a blank [src: file: backend/src/core/redact.rs:2313].
- A `'` right after a word is an apostrophe (`it's`), and after `&` or `<`,
  or a `,` or `<` of a generic's list (across lines too), a lifetime
  (`&'static`, `Foo<\n    'a,\n>`). A `\"…\"` value is read as a string escaped twice, so
  a `"` inside it (`\\\"`) does not end it
  [src: file: backend/src/core/redact.rs:679].
- In stored agent output, a literal is masked when it is a credential for its
  name or for a secret-ish name written inside the expression before it. An
  unmasked literal that runs to the end of the text does not hide the names
  after it: reading resumes inside it.
- The pass is linear in the text on any input: a closing sequence already
  found absent after a position is not searched for again, and a later literal
  running to the end inside such a tail is judged up to the next name only.
  A test counts the bytes read on adversarial inputs, `\r`-only and mixed
  line breaks among them, and runs of `,` (each `,` looks at the segment
  after it once, up to the next one it reads).

The call shapes still keep a call readable around a masked value
(`Some("***REDACTED***")`, `strdup ("***REDACTED***")`,
`Secret::<Vec<Box<str>>>::new("***REDACTED***")`). A space may come before the
parenthesis. A turbofish segment reads generics of any depth and lifetimes as
a flat class. A closure (`|| "…"`, `move |_|`) opens a call. A printed `\n` /
`\r` / `\t` may follow an opener [src: file: backend/src/core/redact.rs:704].
`name_before_value`, `past_call_openers` and `call_path_len` read the same
syntax. Elsewhere the bare-value pattern also masks what comes before the
literal (`token: ***REDACTED*** = "***REDACTED***"`), which fails closed.

A quoted key also matches a value in the other quote inside a printed string
(`{ \"token\": '…' }`, `{ 'token': \"…\" }`)
[src: file: backend/src/core/redact.rs:753].

Accepted as is, because they fail closed: an unclosed quote in a secret's
value masks up to its closing quote or the end of the text, and an unclosed
bracket extends the expression across lines until it closes. A secret with no
secret-like name before it is out of scope: the pass is anchored on the name.
