---
name: "td-{{DATE}}-{{SLUG}}"
description: "{{ONE_LINE_SUMMARY}}"
metadata:
  type: tech-debt
  audit_history:
    - date: "{{DATE}}"
      status: "Inferred"
      reviewer: "{{AUDIT_KIND}}"
      note: "Initial source review"
---

# TD-{{DATE}}-{{SLUG}}

- **Area**: {{AREA}}
- **Severity**: {{SEVERITY}}
- **Status**: Inferred
- **Effort**: {{EFFORT}}
- **Blast radius**: {{BLAST_RADIUS}}

## Problem (fact)

{{OBSERVED_DEFECT_AND_SOURCE_CITATION}}

## Impact

{{OBSERVED_IMPACT_OR_EXPLICITLY_CONDITIONAL_CONSEQUENCE}}

## Where (pointers)

{{SOURCE_PATHS_WITH_LINE_CITATIONS}}

## Applicability and counter-check

{{TRIGGER_CONDITIONS_AND_RELEVANT_COUNTER_EVIDENCE_CHECKED}}

## Verification

{{OBSERVABLE_PASS_FAIL_CRITERION_AND_EXECUTED_VS_PROPOSED_CHECKS}}

## Suggested direction

{{NON_BINDING_FIX_HINT_OR_EXPLICIT_UNKNOWN}}

## Next step

{{CREATE_TICKET_OR_LINK_EXISTING_TICKET}}

<!-- Copy to a TD-YYYYMMDD-slug.md file and fill every field using named
fields, not a positional shell function. Keep this reusable template unchanged.
Severity: Critical/High/Medium/Low. Effort: S/M/L/XL (never a status).
New audit status: Inferred, or Verified in source only after the evidence and
counter-check establish the stated defect. Mirror it in audit_history.
Quote YAML strings correctly; append history on updates. Human-owned statuses
(Confirmed by user, Rejected, Accepted decision, Deferred) require that decision.
Remove this instruction comment from the resulting TD, then parse/check the
written YAML, required sections, field values and the index Markdown link. -->
