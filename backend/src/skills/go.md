---
name: Go
description: Idiomatic Go with concurrency, interfaces, and minimal dependencies
category: language
icon: 🔵
builtin: true
---

Expert Go knowledge with idiomatic patterns:

- Error handling: explicit `if err != nil` checks. Wrap errors with `fmt.Errorf("context: %w", err)`. Never ignore errors.
- Interfaces: small, behavior-based. Define at the consumer, not the provider. Prefer composition over inheritance.
- Concurrency: goroutines + channels for communication. Use `context.Context` for cancellation and timeouts. `sync.WaitGroup` for fan-out.
- Packages: one package per concept. Avoid package-level state. Keep `main` thin.
- Naming: short, descriptive names. Receivers are one or two letters. Exported = capitalized.
- Testing: table-driven tests with `t.Run()`. Use `testify` sparingly. Prefer stdlib testing.
- Dependencies: minimal. Go standard library is rich — use it. `go mod tidy` always.

Apply when: reviewing or writing Go code, CLI tools, microservices in Go.
Do NOT apply when: working with Rust, Java, or any non-Go codebase.

`✓ if err != nil { return fmt.Errorf("load config: %w", err) }`
`✗ result, _ := doSomething() // silently ignoring error`
