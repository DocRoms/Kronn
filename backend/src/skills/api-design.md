---
name: API Design
description: REST, GraphQL, gRPC, versioning, OpenAPI, and API best practices
category: domain
icon: 🔌
builtin: true
---

API design expertise covering protocols, conventions, and best practices:

- REST: use nouns for resources, HTTP verbs for actions. GET is safe and idempotent. PUT is idempotent. POST is not.
- Status codes: 200 OK, 201 Created, 204 No Content, 400 Bad Request, 401 Unauthorized, 403 Forbidden, 404 Not Found, 409 Conflict, 422 Unprocessable Entity, 429 Too Many Requests, 500 Internal Server Error.
- Versioning: prefer URL path versioning (`/v1/`) for public APIs. Header versioning for internal. Never break existing clients.
- Pagination: use cursor-based pagination for large datasets. Offset-based is fine for small, stable collections. Always return `total_count` or `has_next`.
- Error handling: consistent error envelope `{ "error": { "code": "...", "message": "...", "details": [...] } }`. Machine-readable codes, human-readable messages.
- Rate limiting: return `429` with `Retry-After` header. Use token bucket or sliding window. Document limits in API docs.
- OpenAPI/Swagger: spec-first design. Generate server stubs and client SDKs from the spec. Keep spec in sync with implementation.
- GraphQL: use for flexible client queries. Define clear types and resolvers. Watch for N+1 queries — use DataLoader.
- gRPC: use for internal service-to-service. Define `.proto` files first. Use streaming for large payloads.
- Authentication: use Bearer tokens (OAuth2/JWT). API keys for server-to-server. Never pass credentials in query strings.

When reviewing APIs, flag: inconsistent naming, missing pagination, no error envelope, breaking changes without versioning, missing rate limiting, and undocumented endpoints.

Apply when: designing, reviewing, or modifying REST/GraphQL/gRPC endpoints or API contracts.
Do NOT apply when: working on internal function signatures, CLI tools, or frontend-only components.

✓ Scenario: `GET /v1/users?cursor=abc123` returns `{ "data": [...], "next_cursor": "def456" }`
✗ Scenario: `GET /users` returns raw array with no pagination, no versioning, no error envelope.
