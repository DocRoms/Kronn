export interface DocStateCounts {
  broken: number;
  reanchor: number;
  stale: number;
  reconfirm: number;
}

export type DocStateKey = keyof DocStateCounts;

export function docStateNeedsAttention(counts: DocStateCounts | null): boolean {
  return counts !== null
    && (counts.broken > 0 || counts.reanchor > 0 || counts.stale > 0 || counts.reconfirm > 0);
}
