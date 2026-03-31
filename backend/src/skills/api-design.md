---
name: api-design
description: Use when designing, reviewing, or modifying REST/GraphQL/gRPC endpoints, API contracts, or OpenAPI specs. Covers versioning, pagination, error envelopes, and rate limiting.
license: AGPL-3.0
category: domain
icon: 🔌
builtin: true
---

## Procedures

1. **Define the contract first** — write OpenAPI/proto spec before implementation. Generate stubs from spec.
2. **Pick the right protocol** — REST for public CRUD, gRPC for internal service-to-service, GraphQL for flexible client queries.
3. **Version from day one** — URL path (`/v1/`) for public, header for internal. Never ship unversioned.
4. **Paginate all collections** — cursor-based for large/mutable sets, offset for small/stable. Always return `has_next` or `next_cursor`.
5. **Wrap errors consistently** — `{ "error": { "code": "...", "message": "...", "details": [...] } }`. Machine codes + human messages.
6. **Rate limit and document it** — return `429` with `Retry-After`. Document limits in spec.

## Gotchas

- `PUT` is idempotent, `POST` is not — mixing them up causes duplicate-creation bugs.
- GraphQL N+1: every resolver that touches DB needs DataLoader or batching. No exceptions.
- `GET` query params for filtering are fine, but never pass credentials in query strings (they leak into logs/referers).
- Cursor pagination breaks if you expose raw DB IDs — encode cursors opaquely.
- OpenAPI codegen drifts silently — CI must validate spec matches implementation.

## Validation

- Every collection endpoint has pagination params and envelope.
- Every mutation returns the created/updated resource or a standard error envelope.
- Breaking changes only land behind a new version prefix.

✓ `GET /v1/users?cursor=abc` → `{ "data": [...], "next_cursor": "def" }`
✗ `GET /users` → raw array, no pagination, no version, no envelope.
