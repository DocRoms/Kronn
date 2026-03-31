---
name: csharp
description: "Use when writing or reviewing C#/.NET code, ASP.NET APIs, Entity Framework, or Unity projects. Covers async patterns, LINQ, DI, and .NET-specific gotchas."
license: AGPL-3.0
category: language
icon: 🔷
builtin: true
---

## Procedures

1. **Async I/O** — always `async/await`. Use `ValueTask` on hot paths.
2. **EF Core queries** — eager-load with `.Include()`, check for N+1 via logging. Prefer `AsNoTracking()` for read-only.
3. **DI** — register services in `Program.cs`. Use `IOptions<T>` for config. Prefer constructor injection.
4. **Testing** — xUnit `[Fact]`/`[Theory]`, `WebApplicationFactory` for integration, NSubstitute for mocks.

## Gotchas

- `Task.Result` / `.Wait()` deadlocks in ASP.NET — synchronization context blocks. Always await.
- `IQueryable` vs `IEnumerable` — calling `.ToList()` too early pulls entire table into memory.
- Nullable reference types: enable `<Nullable>enable</Nullable>` — compiler warns but does NOT enforce at runtime.
- Unity: `MonoBehaviour` cannot use constructors for DI — use `[SerializeField]` or `Zenject`.
- `record` equality is structural, `class` equality is referential — mixing them in collections causes subtle bugs.

## Validation

- Run `dotnet build --warnaserror` — zero warnings.
- Check async chains: no `.Result`, no `.Wait()`, no `Task.Run` wrapping async calls.

## Do/Don't

✓ `var result = await httpClient.GetAsync(url);`
✗ `var result = httpClient.GetAsync(url).Result; // deadlock`
✓ `users.Where(u => u.Active).Select(u => u.Name)` — server-side filter
✗ `users.ToList().Where(u => u.Active)` — loads all rows first
