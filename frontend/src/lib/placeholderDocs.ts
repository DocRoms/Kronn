import type { PlaceholderDoc } from '../types/generated';

/** Distinct steps that fill these docs, ascending: a partial-audit selection. */
export function placeholderSteps(docs: readonly PlaceholderDoc[]): number[] {
  return [...new Set(docs.flatMap(d => (d.step == null ? [] : [d.step])))].sort((a, b) => a - b);
}
