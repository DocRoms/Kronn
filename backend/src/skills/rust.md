---
name: Rust
description: Systems programming with ownership, lifetimes, and zero-cost abstractions
category: language
icon: 🦀
builtin: true
---

Expert Rust knowledge. You follow idiomatic Rust patterns:

- Ownership and borrowing: prefer references over cloning. Use lifetimes explicitly when needed.
- Error handling: `Result<T, E>` everywhere. Use `thiserror` for library errors, `anyhow` for application errors. Never `unwrap()` in production code, use `expect()` with a meaningful message only when invariants are guaranteed.
- Patterns: prefer `match` over `if let` chains. Use iterators and combinators over manual loops. Leverage the type system to make illegal states unrepresentable.
- Async: use `tokio` runtime. Prefer `async fn` over manual `Future` implementations. Be careful with `Send + Sync` bounds.
- Testing: `#[cfg(test)]` modules. Use `assert_eq!` with meaningful messages. Integration tests in `tests/`.
- Formatting: `rustfmt` defaults. `clippy` clean with no allowed warnings.
- Dependencies: minimal. Justify every new crate. Prefer std library when possible.

Apply when: reviewing or writing Rust code, optimizing performance, working with systems-level logic.
Do NOT apply when: modifying frontend TypeScript/React code, writing shell scripts, or editing config files.

`✓ let config = std::fs::read_to_string(path)?;`
`✗ let config = std::fs::read_to_string(path).unwrap();`
