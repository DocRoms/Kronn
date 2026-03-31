---
name: java
description: Use when writing or reviewing Java code or Spring Boot services. Covers modern Java 17+ features, Spring patterns, concurrency, and common JVM pitfalls.
license: AGPL-3.0
category: language
icon: ☕
builtin: true
---

## Procedure

1. **Modern Java**: Use records for DTOs, sealed classes for type hierarchies, pattern matching for `instanceof`, text blocks for multi-line strings. Target Java 17+.
2. **Spring Boot**: Constructor injection only (no `@Autowired` on fields). Use `@ConfigurationProperties` over `@Value`. Favor Spring Data JPA repositories over manual JDBC.
3. **Null safety**: Return `Optional<T>` from query methods, never return `null`. Do not use `Optional` as a field or parameter type — it's for return values only.
4. **Error handling**: Checked exceptions for recoverable errors, unchecked for bugs. Never catch `Exception` or `Throwable` broadly — catch the specific type.
5. **Immutability**: Records and `List.of()` / `Map.of()` by default. Use `final` on local variables when practical.
6. **Testing**: JUnit 5 with `@Nested` for grouping. Mockito for mocking. Testcontainers for integration tests with real databases.

## Gotchas

- `Optional.get()` without `isPresent()` throws `NoSuchElementException` — use `orElseThrow()` with a message or `map()`/`flatMap()` chains.
- `@Transactional` on private methods is silently ignored (Spring uses proxies). Must be on public methods.
- Virtual threads (Loom): don't use `synchronized` blocks with virtual threads — they pin the carrier thread. Use `ReentrantLock` instead.
- `equals()`/`hashCode()`: if you override one, override both. Records generate these automatically.

## Validation

Run `mvn verify` or `gradle check` before considering work done.

`✓ Optional<User> user = repository.findById(id);`
`✗ User user = repository.findById(id); // returns null, NPE risk`
