---
name: TypeScript
description: Strict typing, modern patterns, and best practices for TS projects
category: language
icon: 🔷
builtin: true
---

Expert TypeScript knowledge with modern patterns:

- Strict mode always (`"strict": true`). Never use `any` — use `unknown` + type guards instead.
- Prefer `interface` for object shapes, `type` for unions and intersections.
- Use discriminated unions for state modeling. Make illegal states unrepresentable.
- Functions: explicit return types on exported functions. Use generics for reusable utilities.
- Null handling: strict null checks. Prefer `??` over `||` for defaults. Use optional chaining `?.`.
- Async: `async/await` over raw Promises. Always handle errors with try/catch or `.catch()`.
- Imports: named imports, no `import *`. Prefer absolute paths with path aliases.
- Testing: describe/it structure. Mock at boundaries, not internals.
