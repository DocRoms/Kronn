# Tech debt (index)

> **TEMPLATE FILE.** Do not refactor items listed here without reading their detail file first.

Track-only list. Prevents AI from doing large refactors without context.
Details in `tech-debt/TD-YYYYMMDD-slug.md` (use today's date for YYYYMMDD).

**To add:** create detail file, add one-line entry below.

**Detail file example** (`tech-debt/TD-20260315-hardcoded-secret.md`):
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

## Dimension coverage

> Filled by the audit (Step 8 § B). **Every dimension MUST have an outcome** — `findings` (listed above), `scanned — nothing substantiable`, or `N/A: <verifiable reason>`. A blank row, or an unverifiable reason, means the audit is **incomplete**. This matrix is the breadth contract; the TDs above are the depth.

| Dimension | Outcome | Evidence / reason |
|-----------|---------|-------------------|
| Dependencies | {{DEP_OUTCOME}} | {{DEP_EVIDENCE}} |
| Security | {{SEC_OUTCOME}} | {{SEC_EVIDENCE}} |
| Code quality | {{CQ_OUTCOME}} | {{CQ_EVIDENCE}} |
| Scalability | {{SCAL_OUTCOME}} | {{SCAL_EVIDENCE}} |
| Maintainability | {{MAINT_OUTCOME}} | {{MAINT_EVIDENCE}} |
| Accessibility | {{A11Y_OUTCOME}} | {{A11Y_EVIDENCE}} |
| Observability | {{OBS_OUTCOME}} | {{OBS_EVIDENCE}} |
| Compliance | {{COMP_OUTCOME}} | {{COMP_EVIDENCE}} |
| Performance | {{PERF_OUTCOME}} | {{PERF_EVIDENCE}} |
| Documentation drift | {{DRIFT_OUTCOME}} | {{DRIFT_EVIDENCE}} |
