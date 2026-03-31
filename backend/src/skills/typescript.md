---
name: typescript
description: Use when writing or reviewing TypeScript or JavaScript code (frontend or Node.js). Covers strict typing, null safety, async patterns, and common TS pitfalls.
license: AGPL-3.0
category: language
icon: 🔷
builtin: true
---

## Procedure

1. **Strict mode always**: `"strict": true` in tsconfig. Never use `any` — use `unknown` + type guards or `satisfies` instead.
2. **Type design**: Use discriminated unions for state (`type Result = { ok: true; data: T } | { ok: false; error: E }`). Use `interface` for object shapes, `type` for unions/intersections.
3. **Null safety**: Prefer `??` over `||` for defaults (`||` coerces `0` and `""` to falsy). Use optional chaining `?.` but don't chain more than 3 levels deep.
4. **Async**: `async/await` over `.then()` chains. Always wrap in try/catch or add `.catch()`. Never fire-and-forget a Promise without `void` annotation.
5. **Exports**: Explicit return types on all exported functions — inference breaks across module boundaries.
6. **Imports**: Named imports only, no `import *`. Use path aliases over deep relative paths.

## Gotchas

- `||` vs `??`: `value || fallback` treats `0`, `""`, `false` as falsy. Use `??` for nullish-only coalescing.
- `as` casts bypass type checking entirely — prefer type guards (`if ("key" in obj)`) or `satisfies`.
- `Promise<void>` functions that throw are silently swallowed if not awaited. Lint for floating promises.
- Barrel files (`index.ts` re-exports) kill tree-shaking and slow bundlers. Prefer direct imports.

## Validation

Run `tsc --noEmit` and your linter before considering work done.

`✓ const val: unknown = input(); if (typeof val === "string") { use(val); }`
`✗ const val: any = input(); use(val);`
