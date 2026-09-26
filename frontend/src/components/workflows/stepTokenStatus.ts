import type { StepResult } from '../../types/generated';

/** A step whose runtime reported no usage shows "unknown", never a zero. */
export function stepTokensUnknown(sr: StepResult): boolean {
  return sr.tokens_used === null && sr.status !== 'Running' && sr.status !== 'Pending';
}
