// Line diff for the « À reconfirmer » panel (KT-1079).

export type DiffLine = { kind: 'same' | 'add' | 'del'; text: string };

/** Above this many cells the LCS is skipped: both sides are shown whole. */
const MAX_DIFF_CELLS = 400_000;
const CONTEXT = 2;

/** Line diff of the changed middle (common head and tail trimmed). */
export function lineDiff(before: string, after: string): DiffLine[] | null {
  const a = before.split('\n');
  const b = after.split('\n');
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head
    && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const midA = a.slice(head, a.length - tail);
  const midB = b.slice(head, b.length - tail);
  if (midA.length * midB.length > MAX_DIFF_CELLS) return null;
  const n = midA.length;
  const m = midB.length;
  const lcs = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = midA[i] === midB[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const mid: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && midA[i] === midB[j]) { mid.push({ kind: 'same', text: midA[i] }); i++; j++; }
    else if (j < m && (i === n || lcs[i][j + 1] >= lcs[i + 1][j])) { mid.push({ kind: 'add', text: midB[j] }); j++; }
    else { mid.push({ kind: 'del', text: midA[i] }); i++; }
  }
  const before_ = a.slice(Math.max(0, head - CONTEXT), head).map(text => ({ kind: 'same' as const, text }));
  const after_ = b.slice(b.length - tail, Math.min(b.length, b.length - tail + CONTEXT)).map(text => ({ kind: 'same' as const, text }));
  return [...before_, ...mid, ...after_];
}
