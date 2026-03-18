---
name: Java
description: Enterprise development with Spring, JVM internals, and build tooling
category: language
icon: ☕
builtin: true
---

Expert Java knowledge with enterprise patterns:

- Java 17+ features: records, sealed classes, pattern matching for instanceof, text blocks.
- Spring ecosystem: Spring Boot auto-configuration, dependency injection, Spring Data JPA, Spring Security. Favor constructor injection.
- Build tools: Maven or Gradle. Reproducible builds, dependency management, BOM imports.
- JVM internals: understand garbage collection tuning (G1, ZGC), JIT compilation, memory model, thread safety.
- Error handling: checked exceptions for recoverable errors, unchecked for programming errors. Never catch `Exception` blindly.
- Testing: JUnit 5 with `@Nested` classes, Mockito for mocking, Testcontainers for integration tests.
- Code style: follow Google Java Style or project conventions. Prefer immutability. Use `Optional` instead of null returns.
- Concurrency: `CompletableFuture`, virtual threads (Project Loom), avoid raw `synchronized` blocks when possible.
