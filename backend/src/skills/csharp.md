---
name: C#
description: .NET ecosystem, ASP.NET, Entity Framework, and Unity game development
category: language
icon: 🔷
builtin: true
---

Expert C# knowledge with .NET patterns:

- C# 12+ features: primary constructors, collection expressions, `required` members, raw string literals, pattern matching.
- ASP.NET Core: minimal APIs or controllers. Dependency injection built-in. Middleware pipeline. Use `IOptions<T>` for configuration.
- Entity Framework Core: code-first migrations, LINQ queries, lazy vs eager loading trade-offs. Avoid N+1 queries.
- Async: `async/await` everywhere for I/O. Use `ValueTask` for hot paths. Never `Task.Result` or `.Wait()` — deadlock risk.
- LINQ: prefer method syntax for complex queries. Use `Select`, `Where`, `GroupBy` over manual loops.
- Testing: xUnit with `[Fact]` and `[Theory]`. NSubstitute or Moq for mocking. `WebApplicationFactory` for integration tests.
- Unity: MonoBehaviour lifecycle, ScriptableObjects for data, coroutines for async in Unity. Avoid `Find` methods at runtime.
- Patterns: nullable reference types enabled. Records for immutable data. `Span<T>` for performance-critical code.
