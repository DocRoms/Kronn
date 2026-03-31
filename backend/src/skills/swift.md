---
name: swift
description: "Use when writing or reviewing Swift code, iOS/macOS apps, or SwiftUI interfaces. Covers structured concurrency, ARC, protocols, and Apple-platform gotchas."
license: AGPL-3.0
category: language
icon: 🍎
builtin: true
---

## Procedures

1. **Optionals** — `guard let` for early exit, `if let` for scoped unwrap. Never force-unwrap in production.
2. **Concurrency** — `async/await` with `Task` and `TaskGroup`. Use actors for shared mutable state.
3. **SwiftUI** — `@State` for local, `@Binding` for child, `@Observable` (Swift 5.9+) replaces `ObservableObject`.
4. **Testing** — XCTest with async support. Inject dependencies via protocols for mockability.

## Gotchas

- **Retain cycles**: closures capturing `self` strongly inside a class = memory leak. Use `[weak self]` then `guard let self`.
- `@StateObject` vs `@ObservedObject`: `@StateObject` owns the lifecycle, `@ObservedObject` does not — wrong choice = object recreated on every view update.
- `@Published` fires `willSet`, not `didSet` — Combine subscribers see the OLD value if you read the property in the sink.
- `Task { }` inherits actor context in SwiftUI views but NOT in plain classes — data races if you assume isolation.
- `Sendable` enforcement in Swift 6 is strict — closures crossing actor boundaries must capture only sendable types.
- Combine is legacy for new code — prefer `AsyncSequence` / `AsyncStream`.

## Validation

- Zero force-unwraps (`!`) outside tests.
- Closures in classes: verify `[weak self]` where needed.
- Build with strict concurrency checking enabled.

## Do/Don't

✓ `guard let user = fetchUser() else { return }`
✗ `let user = fetchUser()! // crash on nil`
✓ `@StateObject var vm = ViewModel()` — view owns it
✗ `@ObservedObject var vm = ViewModel()` — recreated each render
