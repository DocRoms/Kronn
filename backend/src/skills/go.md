---
name: go
description: Use when writing or reviewing Go code, CLI tools, or microservices. Covers error handling, concurrency patterns, interface design, and common Go traps.
license: AGPL-3.0
category: language
icon: 🔵
builtin: true
---

## Procedure

1. **Error handling**: Always check `if err != nil`. Wrap with `fmt.Errorf("context: %w", err)` to preserve the error chain. Never use `_` to discard errors.
2. **Interfaces**: Define at the consumer, not the provider. Keep them small (1-3 methods). Accept interfaces, return structs.
3. **Concurrency**: Pass `context.Context` as the first parameter. Use channels for communication, `sync.WaitGroup` for fan-out. Always select on `ctx.Done()` in goroutines.
4. **Packages**: One concept per package. Avoid package-level `var` state (it breaks testing). Keep `main()` thin — just wiring.
5. **Naming**: Short receivers (`s` for `Server`). Exported = capitalized. No getters — use `Name()` not `GetName()`.
6. **Testing**: Table-driven tests with `t.Run()`. Prefer stdlib `testing` over testify. Use `t.Parallel()` when safe.

## Gotchas

- Goroutine leaks: every goroutine must have a termination path. Always select on `ctx.Done()` or a done channel.
- Loop variable capture: in Go < 1.22, `go func() { use(v) }()` inside a `for` loop captures the loop variable by reference. Use `go func(v T) { use(v) }(v)`.
- `defer` in loops: deferred calls stack until the function returns, not the loop iteration. Extract to a helper function.
- `nil` interface vs `nil` pointer: an interface holding a `nil` pointer is NOT `== nil`. Check the concrete value.

## Validation

Run `go vet ./... && go test ./...` before considering work done. Use `golangci-lint run` for deeper checks.

`✓ if err != nil { return fmt.Errorf("load config: %w", err) }`
`✗ result, _ := doSomething() // silently ignoring error`
