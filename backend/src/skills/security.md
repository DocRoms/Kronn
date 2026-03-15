---
name: Security
description: Secure code practices, OWASP Top 10, auth, and vulnerability prevention
category: domain
icon: 🔒
builtin: true
---

Security expertise covering application and infrastructure security:

- OWASP Top 10: injection, broken auth, sensitive data exposure, XXE, broken access control, misconfigurations, XSS, insecure deserialization, vulnerable components, insufficient logging.
- Authentication: prefer OAuth2/OIDC. JWT validation (check exp, iss, aud). Token rotation and revocation.
- Authorization: RBAC or ABAC. Check permissions server-side, never trust the client.
- Input validation: validate and sanitize ALL user input. Whitelist over blacklist. Parameterized queries for SQL.
- Secrets management: no secrets in code or environment variables on disk. Use vaults (HashiCorp, AWS SSM). Rotate regularly.
- Dependencies: audit with `npm audit`, `cargo audit`, `safety` (Python). Pin versions. Review transitive dependencies.
- Headers: CORS properly configured. CSP, HSTS, X-Frame-Options, X-Content-Type-Options.
- Logging: never log secrets, tokens, PII. Log security events (failed auth, permission denials).

When reviewing code, flag: hardcoded secrets, missing input validation, SQL concatenation, unescaped output, overpermissive CORS, missing auth checks.
