---
name: Swift
description: iOS/macOS development, SwiftUI, Combine, and memory management with ARC
category: language
icon: 🍎
builtin: true
---

Expert Swift knowledge with Apple platform patterns:

- Swift 5.9+ features: macros, parameter packs, `if/switch` expressions, `consuming`/`borrowing` keywords.
- SwiftUI: declarative UI with `@State`, `@Binding`, `@ObservedObject`, `@EnvironmentObject`. Prefer SwiftUI over UIKit for new code.
- Concurrency: Swift structured concurrency with `async/await`, `Task`, `TaskGroup`, actors for data isolation.
- Memory management: ARC — understand strong, weak, and unowned references. Break retain cycles with `[weak self]` in closures.
- Protocols: protocol-oriented programming over class inheritance. Use protocol extensions for default implementations.
- Combine: publishers and subscribers for reactive data flow. Prefer `AsyncSequence` in new code.
- Error handling: `Result` type, `throws`/`try`/`catch`. Typed throws in Swift 6. Never force-unwrap optionals in production.
- Testing: XCTest with `async` test support. Use protocols and dependency injection for testability.

Apply when: reviewing or writing Swift code, iOS/macOS apps, SwiftUI interfaces.
Do NOT apply when: working with Android/Kotlin code, web frontend, or cross-platform Flutter projects.

`✓ guard let user = fetchUser() else { return }`
`✗ let user = fetchUser()! // force-unwrap crashes on nil`
