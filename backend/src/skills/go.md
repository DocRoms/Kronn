---
name: Go
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
