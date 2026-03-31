---
name: security
description: Use when handling user input, authentication, secrets, network requests, or data storage. Covers OWASP Top 10 prevention, auth patterns, and secure coding.
license: AGPL-3.0
category: domain
icon: 🔒
builtin: true
---

## Procedures

1. **Parameterize all queries** — never concatenate user input into SQL/shell/LDAP. Use `$1` placeholders or ORM bindings.
2. **Validate input on the server** — whitelist allowed chars/ranges. Client-side validation is UX, not security.
3. **Auth flow** — OAuth2/OIDC for users, API keys for service-to-service. Check `exp`, `iss`, `aud` on every JWT.
4. **Authorize server-side** — RBAC/ABAC checks in the handler, never trust client claims. Test with unprivileged user.
5. **Manage secrets via vault** — HashiCorp Vault, AWS SSM, or similar. Never in code, never in env files committed to git.
6. **Set security headers** — CSP, HSTS, X-Frame-Options, X-Content-Type-Options. CORS: whitelist specific origins, never `*` in prod.

## Gotchas

- **`401` vs `403`**: 401 = "who are you?" (missing/invalid auth), 403 = "you can't do that" (valid auth, insufficient perms). Mixing them leaks info.
- **JWT in localStorage** is XSS-vulnerable. Use httpOnly cookies with SameSite=Strict for browser clients.
- **Logging secrets** — structured logging frameworks auto-serialize objects. A `user` object with a `password` field WILL end up in logs. Redact explicitly.
- **Transitive dependencies** are the real attack surface — `npm audit` / `cargo audit` in CI, not just at dev time.
- **CORS preflight caching** — `Access-Control-Max-Age` too high means you can't revoke access quickly. Keep under 1h.
- **bcrypt/argon2 only** for password hashing. SHA-256 is not a password hash. scrypt is acceptable but argon2 is preferred.

## Validation

- No hardcoded secrets in codebase (grep for API keys, tokens, passwords).
- All user-facing inputs have server-side validation.
- Auth middleware covers every non-public route.

✓ `db.query("SELECT * FROM users WHERE id = $1", [id])`
✗ `db.query("SELECT * FROM users WHERE id = " + id)`
