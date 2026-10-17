/** KT-1131 — what a recorded audit step came to, as the recap shows it. */
export type AuditStepOutcome = 'succeeded' | 'unchanged' | 'accepted' | 'failed';

export interface AuditStepOutcomeInput {
  cli_success: boolean;
  step_warning?: string | null;
  unchanged?: boolean;
  accepted_at?: string | null;
}

/** An accepted step keeps its failure on record but counts as done. */
export function auditStepOutcome(step: AuditStepOutcomeInput): AuditStepOutcome {
  if (step.accepted_at) return 'accepted';
  if (step.unchanged) return 'unchanged';
  if (!step.cli_success || step.step_warning) return 'failed';
  return 'succeeded';
}
