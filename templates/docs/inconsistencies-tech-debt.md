# Tech debt (index)

> **TEMPLATE FILE.** Do not refactor items listed here without reading their detail file first.

Track-only list. Prevents AI from doing large refactors without context.
Details in `tech-debt/TD-YYYYMMDD-slug.md` (use today's date for YYYYMMDD).

**To add:** copy [the detail template](tech-debt/TEMPLATE.md), fill and check
its named fields, then add an actual Markdown link in the relevant index.
Keep one ID per root cause. Do not turn a checklist suggestion into a finding
without source evidence and a relevant counter-check.

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
