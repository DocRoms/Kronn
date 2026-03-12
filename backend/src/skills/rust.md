---
name: Rust
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
