---
name: kotlin
description: "Use when writing or reviewing Kotlin code, Android apps, or Kotlin Spring Boot services. Covers coroutines, null safety, Compose, and Kotlin-specific gotchas."
license: AGPL-3.0
category: language
icon: 🟣
builtin: true
---

## Procedures

1. **Null safety** — use `?.`, `?:`, `let`, `requireNotNull`. Reserve `!!` for genuinely impossible nulls with a comment.
2. **Coroutines** — launch from a `CoroutineScope` (viewModelScope, lifecycleScope). Use `Flow` for streams, `suspend` for one-shot.
3. **Android UI** — Jetpack Compose with `StateFlow` in ViewModel. Hilt for DI, Room for persistence.
4. **Testing** — JUnit 5 with backtick names, MockK for mocking, Turbine for Flow assertions.

## Gotchas

- `data class copy()` is shallow — nested mutable objects share references.
- `Flow` is cold: nothing executes until collected. Forgetting `.collect()` = silent no-op.
- `GlobalScope.launch` leaks coroutines — always use structured concurrency with a bounded scope.
- Kotlin `Int` boxes to `java.lang.Integer` in generics — causes `===` identity failures in collections.
- Spring `@Transactional` on Kotlin requires `open` class/methods (or `allopen` plugin) — final by default breaks proxies.
- `when` without `else` on non-sealed types: compiles but crashes at runtime if new enum values are added.

## Validation

- Zero `!!` usages without justifying comment.
- Coroutine launches tied to a lifecycle scope (never `GlobalScope`).

## Do/Don't

✓ `val name = user?.name ?: "Anonymous"`
✗ `val name = user!!.name // NPE risk`
✓ `viewModelScope.launch { repo.fetch() }`
✗ `GlobalScope.launch { repo.fetch() } // leaked coroutine`
