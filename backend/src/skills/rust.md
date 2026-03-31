---
name: rust
description: Use when writing or reviewing Rust code. Covers ownership gotchas, error handling patterns, async pitfalls, and idiomatic conventions for production Rust.
license: AGPL-3.0
category: language
icon: 🦀
builtin: true
---

## Procedure

1. **Error handling**: Always propagate with `?`. Use `thiserror` in libraries, `anyhow` in binaries. Never `unwrap()` in production — use `expect("reason")` only for proven invariants.
2. **Ownership**: Pass `&T` or `&mut T` by default. Clone only when you've measured and it's justified. Prefer `Cow<'_, str>` over `String` for read-mostly paths.
3. **Async**: Use `tokio`. Mark shared state `Arc<Mutex<T>>` — but prefer channels over mutexes. Watch for `Send + Sync` bound errors when holding a `MutexGuard` across `.await`.
4. **Iterators**: Prefer `.iter().map().collect()` over manual loops. Use `itertools` only when stdlib combinators fall short.
5. **Testing**: `#[cfg(test)] mod tests` in same file. Integration tests in `tests/`. Use `assert_eq!` with context messages.

## Gotchas

- Holding a `MutexGuard` across an `.await` point makes the future `!Send` — extract the value before awaiting.
- `impl Trait` in return position is opaque — it hides the concrete type, breaks dynamic dispatch, and prevents naming the future for storage.
- `cargo clippy -- -D warnings` catches issues `rustc` misses. Run it before every commit.
- `#[derive(Clone)]` on large structs silently adds expensive copies — audit derive usage on hot paths.

## Validation

Run `cargo clippy -- -D warnings && cargo test` before considering work done.

`✓ let config = std::fs::read_to_string(path)?;`
`✗ let config = std::fs::read_to_string(path).unwrap();`
