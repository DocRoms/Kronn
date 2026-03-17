# Tech debt (index)

Track-only list. Prevents AI from doing large refactors without context.
Details in `ai/tech-debt/TD-YYYYMMDD-slug.md` (use today's date for YYYYMMDD).

**To add:** create detail file, add one-line entry below.

**Detail file example** (`ai/tech-debt/TD-20260315-hardcoded-secret.md`):
- **ID**: TD-20260315-hardcoded-secret
- **Area**: Backend
- **Severity**: Critical
- **Problem**: API key hardcoded in `src/config.rs:42`
- **Impact**: security — key exposed in version control
- **Where**: `src/config.rs:42`, `src/payments/client.rs:15`
- **Suggested fix**: Move to environment variable
- **Next step**: create ticket

**Severity scale:**
- **Critical** — security risk or data loss
- **High** — blocks production readiness
- **Medium** — developer friction or performance degradation
- **Low** — cosmetic or minor improvement

## Outdated dependencies

| Component | Current | Status | Risk |
|-----------|---------|--------|------|
| {{COMPONENT}} | {{VERSION}} | {{STATUS}} | {{RISK}} |

## Current list

| ID | Problem | Area | Severity |
|----|---------|------|----------|
| {{ID}} | {{PROBLEM}} | {{AREA}} | {{SEVERITY}} |
