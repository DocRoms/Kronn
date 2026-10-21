API DESIGN AUDIT (0.8.4) — focused re-scan on the public-facing contract. Read existing `docs/inconsistencies-api.md` if present (anti-repetition rules apply, same as Full audit Step 8).

# A. CONTRACT CONSISTENCY
- **Naming**: REST endpoints must follow ONE convention (kebab-case, snake_case, camelCase) — flag inconsistencies like `/api/users-list` next to `/api/orderItems`. Same for query params, JSON keys.
- **HTTP semantics**: GET that mutates state (anti-pattern); POST that returns 200 for create (should be 201); DELETE that returns the deleted resource (should be 204 unless explicitly part of the contract); PATCH vs PUT confusion.
- **Error envelope**: ONE shape, used by EVERY endpoint. Flag endpoints returning `{ "error": "..." }` next to ones returning `{ "errors": [...] }`, mixed status codes (400 vs 422 for validation).
- **Status codes**: judge against the API's OWN documented contract first (OpenAPI/docs); flag INTERNAL inconsistency (same operation returning different codes, 200 bodies carrying `{ "error": ... }` for failures, 401/403 confusion between authn and authz). 200-on-write is a style choice, not a defect, when the contract says so — flag it as `Low`/`Inferred` only if no contract exists.

# B. VERSIONING + EVOLUTION
- **Versioning strategy**: header (`Accept: application/vnd.api+json; version=2`) vs path (`/api/v2/`) — pick one, don't mix.
- **Breaking changes**: removing a field, narrowing a type, renaming — without a deprecation window or version bump.
- **Additive changes safety**: new required field on an existing endpoint breaks old clients.

# C. PAGINATION + LIST RESPONSES
- Unbounded list endpoints (no `?limit=` enforcement) — DoS surface + cursor unsafe at scale.
- Inconsistent pagination shape (cursor on some endpoints, offset on others, or no metadata at all).
- Missing `total` / `has_more` on responses that consumers need to render pagination UI.

# D. AUTHN + AUTHZ + RATE LIMITING
- Endpoints that should require auth but don't (read the routing config — the audit's `repo-map.md` is a primer).
- IDOR risk: endpoints scoped by URL param (`/api/orders/:id`) without ownership check.
- Rate limiting absent on public auth endpoints (`/login`, `/register`, `/password-reset`).
- CSRF tokens absent on state-mutating endpoints when cookies are used for session (SPA + cookie auth).

# E. DOC DRIFT
- OpenAPI / GraphQL schema in the repo (if any) vs what the endpoints actually accept / return. Flag fields documented but unused, or used but undocumented.
- Examples in the docs that no longer parse (wrong field names, stale auth headers).

# F. ANTI-REPETITION + OUTPUT FORMAT

Same rules as the Full audit Step 8: anti-repetition pass + detail-file schema + severity calibration. Index file = `docs/inconsistencies-api.md`. If no findings, the index reads `'None identified during this API design audit pass'`.

Apply the marker discipline (`TODO: verify` only if you couldn't check; `TODO: ask user` for human decisions).