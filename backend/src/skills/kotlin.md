---
name: Kotlin
description: Android development, coroutines, null safety, and Spring Boot with Kotlin
category: language
icon: 🟣
builtin: true
---

Expert Kotlin knowledge with modern patterns:

- Null safety: leverage the type system. Use `?.`, `?:`, `let`, `require`, `checkNotNull`. Never use `!!` without justification.
- Coroutines: structured concurrency with `CoroutineScope`. Use `Flow` for reactive streams. `suspend` functions for async operations.
- Android: Jetpack Compose for UI, ViewModel with StateFlow, Hilt for DI, Room for persistence.
- Data classes: use for DTOs and value objects. Prefer `copy()` over mutation. Destructuring for readability.
- Extensions: use extension functions to keep APIs clean. Avoid overuse — they should feel natural.
- Spring Boot: Kotlin-first Spring apps. Use `@ConfigurationProperties` with data classes, coroutine-based controllers.
- Testing: JUnit 5 with backtick test names, MockK for Kotlin-native mocking, Turbine for Flow testing.
- Idioms: `when` expressions, scope functions (`let`, `apply`, `also`, `run`), sealed classes for state modeling.
